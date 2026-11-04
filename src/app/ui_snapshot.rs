//! Renders the interface to an image without a window or GPU, for looking
//! at a dialog while designing it: egui's own tessellation, then the
//! triangles filled on the CPU with the fonts and textures egui uploaded.
//! Test-only; nothing is compared.
use eframe::egui;
use image::RgbaImage;
use std::collections::HashMap;

/// The textures egui uploaded, kept from one render to the next as egui
/// sends only what changed.
pub(in crate::app) type Textures = HashMap<egui::TextureId, RgbaImage>;

/// Runs `frames` frames of `draw` at `size` points and `scale` pixels per
/// point, and paints the last.
pub(in crate::app) fn render(
    ctx: &egui::Context,
    textures: &mut Textures,
    size: egui::Vec2,
    scale: f32,
    frames: usize,
    mut draw: impl FnMut(&mut egui::Ui),
) -> RgbaImage {
    ctx.all_styles_mut(|s| s.animation_time = 0.);
    let mut output = None;
    for _ in 0..frames.max(1) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        };
        ctx.set_pixels_per_point(scale);
        let mut out = ctx.run_ui(input, |ui| draw(ui));
        for (id, deltas) in &out.textures_delta.set {
            for delta in deltas {
                apply(textures, *id, delta);
            }
        }
        out.textures_delta.clear();
        output = Some(out);
    }
    let output = output.expect("one frame");
    let primitives = ctx.tessellate(output.shapes, scale);
    let (w, h) = ((size.x * scale) as u32, (size.y * scale) as u32);
    let mut canvas = vec![[0f32; 4]; (w * h) as usize];
    for p in &primitives {
        let egui::epaint::Primitive::Mesh(mesh) = &p.primitive else {
            continue;
        };
        let clip = p.clip_rect;
        let clip = [
            (clip.min.x * scale).max(0.) as i32,
            (clip.min.y * scale).max(0.) as i32,
            (clip.max.x * scale).min(w as f32) as i32,
            (clip.max.y * scale).min(h as f32) as i32,
        ];
        let texture = textures.get(&mesh.texture_id);
        for tri in mesh.indices.as_chunks::<3>().0 {
            let v = [
                mesh.vertices[tri[0] as usize],
                mesh.vertices[tri[1] as usize],
                mesh.vertices[tri[2] as usize],
            ];
            fill(&mut canvas, w, clip, scale, v, texture);
        }
    }
    let mut image = RgbaImage::new(w, h);
    for (i, px) in image.pixels_mut().enumerate() {
        let c = canvas[i];
        *px = image::Rgba([
            (c[0] * 255.) as u8,
            (c[1] * 255.) as u8,
            (c[2] * 255.) as u8,
            255,
        ]);
    }
    image
}

/// Puts an uploaded texture, or a part of one, into `textures`.
fn apply(
    textures: &mut HashMap<egui::TextureId, RgbaImage>,
    id: egui::TextureId,
    delta: &egui::epaint::ImageDelta,
) {
    let egui::ImageData::Color(image) = &delta.image;
    let [dw, dh] = image.size;
    let part = RgbaImage::from_fn(dw as u32, dh as u32, |x, y| {
        let c = image.pixels[y as usize * dw + x as usize];
        image::Rgba([c.r(), c.g(), c.b(), c.a()])
    });
    match delta.pos {
        None => {
            textures.insert(id, part);
        }
        Some([x, y]) => {
            if let Some(whole) = textures.get_mut(&id) {
                image::imageops::replace(whole, &part, x as i64, y as i64);
            }
        }
    }
}

/// Fills one triangle, blending its premultiplied colours over `canvas`.
fn fill(
    canvas: &mut [[f32; 4]],
    width: u32,
    clip: [i32; 4],
    scale: f32,
    v: [egui::epaint::Vertex; 3],
    texture: Option<&RgbaImage>,
) {
    let p: Vec<(f32, f32)> = v
        .iter()
        .map(|v| (v.pos.x * scale, v.pos.y * scale))
        .collect();
    let area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[2].0 - p[0].0) * (p[1].1 - p[0].1);
    if area.abs() < 1e-6 {
        return;
    }
    let min_x = (p.iter().map(|q| q.0).fold(f32::MAX, f32::min).floor() as i32).max(clip[0]);
    let max_x = (p.iter().map(|q| q.0).fold(f32::MIN, f32::max).ceil() as i32).min(clip[2]);
    let min_y = (p.iter().map(|q| q.1).fold(f32::MAX, f32::min).floor() as i32).max(clip[1]);
    let max_y = (p.iter().map(|q| q.1).fold(f32::MIN, f32::max).ceil() as i32).min(clip[3]);
    let color = |c: egui::Color32| {
        [
            c.r() as f32 / 255.,
            c.g() as f32 / 255.,
            c.b() as f32 / 255.,
            c.a() as f32 / 255.,
        ]
    };
    let cs = [color(v[0].color), color(v[1].color), color(v[2].color)];
    for y in min_y..max_y {
        for x in min_x..max_x {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let w0 = ((p[1].0 - px) * (p[2].1 - py) - (p[2].0 - px) * (p[1].1 - py)) / area;
            let w1 = ((p[2].0 - px) * (p[0].1 - py) - (p[0].0 - px) * (p[2].1 - py)) / area;
            let w2 = 1. - w0 - w1;
            if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                continue;
            }
            let mut src = [0f32; 4];
            for (k, s) in src.iter_mut().enumerate() {
                *s = cs[0][k] * w0 + cs[1][k] * w1 + cs[2][k] * w2;
            }
            if let Some(t) = texture {
                let u = v[0].uv.x * w0 + v[1].uv.x * w1 + v[2].uv.x * w2;
                let vv = v[0].uv.y * w0 + v[1].uv.y * w1 + v[2].uv.y * w2;
                let tx = ((u * t.width() as f32) as u32).min(t.width() - 1);
                let ty = ((vv * t.height() as f32) as u32).min(t.height() - 1);
                let s = t.get_pixel(tx, ty).0;
                for k in 0..4 {
                    src[k] *= s[k] as f32 / 255.;
                }
            }
            let dst = &mut canvas[(y as u32 * width + x as u32) as usize];
            for k in 0..3 {
                dst[k] = src[k] + dst[k] * (1. - src[3]);
            }
            dst[3] = src[3] + dst[3] * (1. - src[3]);
        }
    }
}

#[cfg(test)]
#[path = "ui_snapshot_editor_tests.rs"]
mod editor_tests;
