//! What an experimental AI Remove generator is given, and how its answer becomes a
//! patch ([`crate::model::retouch::RetouchPatch`]).
//!
//! Hosted removers take 8-bit display images, so the region around the painted
//! area is rendered as one, through a fixed, invertible rendition the request
//! captures once ([`Rendition::of`]):
//!
//! - forward: the retouched camera image (the effective denoising, every spot and
//!   red eye correction already applied) × the photo's default white balance ×
//!   the camera stage's matrix (`profile_matrix` with the current recipe, to linear
//!   sRGB primaries, as [`crate::develop::camera_stage`] uses it), then to linear
//!   ProPhoto; a rendered file's pixels as they are, its pseudo-scene. Then × the
//!   default exposure the photo opens with (its baseline), Camera Raw's default
//!   rendering `F` ([`crate::develop::rendered`]) DNG RGBTone fashion, ProPhoto to
//!   linear sRGB with negative and above-1 channels clipped, the sRGB transfer and
//!   rounding to 8 bits. A JPEG or TIFF gives the generator its own pixels back; a
//!   RAW, its default rendering.
//! - inverse: 8 bits to linear sRGB, to linear ProPhoto, `F⁻¹`
//!   ([`crate::develop::rendered::scene_from_rendered`]), ÷ exposure, the inverse
//!   matrix, ÷ white balance, with the values captured: whatever the user changes
//!   meanwhile, the inverse undoes exactly what the forward did.
//!
//! It is lossy on purpose: colours outside sRGB, clipped channels, quantisation
//! in the shadows and the reduction to at most [`MAX_SIDE`] pixels are not
//! recovered. The renderer lays the patch with Heal's membrane, which takes the
//! residual error at the shape's border back to the photo's own values. The
//! rendition is the experiment's default, the matrix and the fixed `F`, without a
//! profile's tables or look: it changes how the region looks to the model, not
//! which pixels it is.
use super::heal::{self, FeatherProfile};
use crate::camera_data::{CameraImage, Metadata};
use crate::model::image_frame::ImageFrame;
use crate::model::recipe::Recipe;
use crate::model::retouch::{RetouchOp, RetouchShape};
use anyhow::{Result, bail, ensure};
use rawmakase_cancel::Cancel;
use rawmakase_model::camera_profiles::{PRO_TO_RGB, RGB_TO_PRO};
use rawmakase_model::color::{inverse, mul, mul_matrix, srgb_decode, srgb_encode};

/// The longest side of a request's region, in pixels.
pub const MAX_SIDE: u32 = 2048;
/// The least margin around the painted area, in the photo's pixels.
const MIN_MARGIN: f32 = 64.;
/// Why an HDR edit isn't removed from yet.
pub const HDR_REFUSAL: &str = "AI Remove doesn't work on HDR edits yet";

/// Where a request's region lies: `rect` in the image frame (`[x0, y0, x1, y1]`),
/// and its size in pixels, upright.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    pub rect: [f32; 4],
    pub width: u32,
    pub height: u32,
    /// Region pixels per pixel of the photo.
    pub scale: f32,
}

/// The region a request for `op`'s shape sends: the shape's bounds, grown by half
/// their size on each side and at least [`MIN_MARGIN`] pixels, within the photo,
/// reduced so the long side is at most [`MAX_SIDE`]. `None` when the shape lies
/// outside the photo.
pub fn region(op: &RetouchOp, frame: &ImageFrame) -> Option<Region> {
    let [w, h] = frame.size();
    let (rx, ry) = crate::model::retouch::radii(op.radius(), frame.aspect());
    let points: Vec<[f32; 2]> = match &op.shape {
        RetouchShape::Spot { center, .. } => vec![*center],
        RetouchShape::Brush { points, .. } => points.to_vec(),
    };
    let mut b = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    for p in &points {
        b = [
            b[0].min(p[0] - rx),
            b[1].min(p[1] - ry),
            b[2].max(p[0] + rx),
            b[3].max(p[1] + ry),
        ];
    }
    let mx = ((b[2] - b[0]) * w / 2.).max(MIN_MARGIN) / w;
    let my = ((b[3] - b[1]) * h / 2.).max(MIN_MARGIN) / h;
    let rect = [
        (b[0] - mx).clamp(0., 1.),
        (b[1] - my).clamp(0., 1.),
        (b[2] + mx).clamp(0., 1.),
        (b[3] + my).clamp(0., 1.),
    ];
    if !(rect.iter().all(|v| v.is_finite()) && rect[2] > rect[0] && rect[3] > rect[1]) {
        return None;
    }
    // The painted area must reach into the photo.
    if b[2] <= 0. || b[3] <= 0. || b[0] >= 1. || b[1] >= 1. {
        return None;
    }
    let (pw, ph) = ((rect[2] - rect[0]) * w, (rect[3] - rect[1]) * h);
    let scale = (MAX_SIDE as f32 / pw.max(ph)).min(1.);
    Some(Region {
        rect,
        width: ((pw * scale).round() as u32).clamp(1, MAX_SIDE),
        height: ((ph * scale).round() as u32).clamp(1, MAX_SIDE),
        scale,
    })
}

/// The colour rendition of one request, captured when it starts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rendition {
    /// White-balanced camera RGB to linear ProPhoto, and back.
    to_pro: [[f32; 3]; 3],
    from_pro: [[f32; 3]; 3],
    /// The white balance multipliers.
    wb: [f32; 3],
    /// The default exposure, as a linear gain.
    gain: f32,
}
impl Rendition {
    /// The rendition of `m`'s photo edited with `r`: the camera stage's matrix for
    /// `r`, and the white balance and exposure the photo opens with.
    pub fn of(m: &Metadata, r: &Recipe) -> Result<Self> {
        // What the camera stage renders with: switched-off panels bypassed.
        let r = &*r.resolved(m);
        // The white balance and exposure a photo opens with: as shot (gains of 1
        // over the camera image), and the camera's baseline.
        let mut default = Recipe::default();
        default.use_camera_baseline(m);
        let mut gain = (default.exposure + default.camera_exposure).exp2();
        let (to_pro, wb) = if m.is_rendered() {
            // Its pixels are its pseudo-scene in linear ProPhoto already.
            ([[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]], [1.; 3])
        } else {
            // The linear part of `scene_color`: a profile's matrix maps to linear
            // ProPhoto and its exposure follows; the camera's own maps to linear
            // sRGB primaries.
            let to_pro = match &r.profile {
                Some(p) => {
                    gain *= p.exposure().exp2();
                    p.camera_matrix(r.temperature)
                }
                None => mul_matrix(RGB_TO_PRO, m.matrix),
            };
            (to_pro, [1.; 3])
        };
        let det = {
            let [a, b, c] = to_pro;
            a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0])
        };
        ensure!(
            det.is_finite() && det.abs() > 1e-8,
            "The photo's colour matrix can't be inverted"
        );
        ensure!(
            gain.is_finite() && gain > 0. && wb.iter().all(|w: &f32| w.is_finite() && *w > 0.),
            "The photo's white balance or exposure can't be inverted"
        );
        Ok(Self {
            to_pro,
            from_pro: inverse(to_pro),
            wb,
            gain,
        })
    }
    /// One camera sample as the generator sees it.
    pub fn forward(&self, p: [f32; 3]) -> [u8; 3] {
        let scene = mul(self.to_pro, std::array::from_fn(|c| p[c] * self.wb[c]))
            .map(|v| if v.is_finite() { v * self.gain } else { 0. });
        let rendered = crate::develop::scene_tone::rgb_tone(scene, |s| {
            crate::develop::rendered::default_rendering(s.max(0.))
        });
        mul(PRO_TO_RGB, rendered).map(|v| {
            (srgb_encode(v.clamp(0., 1.)) * 255.)
                .round()
                .clamp(0., 255.) as u8
        })
    }
    /// One 8-bit sRGB pixel of the generator's answer as a camera sample.
    pub fn inverse(&self, p: [u8; 3]) -> [f32; 3] {
        let linear = p.map(|v| srgb_decode(f32::from(v) / 255.));
        let scene = crate::develop::rendered::scene_from_rendered(mul(RGB_TO_PRO, linear));
        let camera = mul(self.from_pro, scene.map(|v| v / self.gain));
        std::array::from_fn(|c| camera[c] / self.wb[c])
    }
}

/// A request's pixels: its region of the photo in 8-bit sRGB, and the painted
/// area's coverage there, both `width` × `height`, rows packed.
#[derive(Clone, Debug, PartialEq)]
pub struct Input {
    pub region: Region,
    pub rgb: Vec<u8>,
    pub coverage: Vec<u8>,
}

/// Whether `im` edited with `r` can't be removed from yet: HDR files and edits.
pub fn refusal(im: &CameraImage, r: &Recipe) -> Option<&'static str> {
    (im.hdr().is_some() || r.uses_hdr()).then_some(HDR_REFUSAL)
}

/// Renders the request for `op`'s shape: the image the retouch layer will lay
/// the patch on (`im` with `r`'s effective denoising, every spot and red eye
/// correction of `r`), through `rendition`, over [`region`].
pub fn render(
    im: &CameraImage,
    r: &Recipe,
    ai: Option<&crate::develop::AiSource>,
    op: &RetouchOp,
    rendition: &Rendition,
    cancel: Cancel<'_>,
) -> Result<Input> {
    if let Some(why) = refusal(im, r) {
        bail!(why);
    }
    // As every render reads it: switched-off panels bypassed (Detail's noise
    // reduction, Spot Removal's and Red Eye's corrections).
    let r = &*r.resolved(&im.metadata);
    let retouched = crate::develop::quality::retouched(im, r, ai, cancel, None)?.image;
    let frame = ImageFrame::new(&retouched);
    let Some(region) = region(op, &frame) else {
        bail!("The painted area is outside the photo");
    };
    let (w, h) = (region.width as usize, region.height as usize);
    let [x0, y0, x1, y1] = region.rect;
    // A box filter over each region pixel's footprint: `taps`² bilinear samples.
    let taps = (1. / region.scale).ceil().clamp(1., 16.) as usize;
    let mut rgb = vec![0u8; w * h * 3];
    for (j, row) in rgb.chunks_mut(w * 3).enumerate() {
        ensure!(!cancel.is_cancelled(), "Cancelled");
        for i in 0..w {
            let mut sum = [0f32; 3];
            for ty in 0..taps {
                for tx in 0..taps {
                    let u =
                        x0 + (i as f32 + (tx as f32 + 0.5) / taps as f32) / w as f32 * (x1 - x0);
                    let v =
                        y0 + (j as f32 + (ty as f32 + 0.5) / taps as f32) / h as f32 * (y1 - y0);
                    let [sx, sy] = frame.to_source([u, v]);
                    let p = bilinear(&retouched, sx, sy);
                    for c in 0..3 {
                        sum[c] += p[c];
                    }
                }
            }
            let n = (taps * taps) as f32;
            let out = rendition.forward(sum.map(|v| v / n));
            row[i * 3..i * 3 + 3].copy_from_slice(&out);
        }
    }
    let coverage = coverage(op, &frame, &region);
    Ok(Input {
        region,
        rgb,
        coverage,
    })
}

/// `op`'s painted area over `region`'s pixels, 0 to 255, with its feather.
pub fn coverage(op: &RetouchOp, frame: &ImageFrame, region: &Region) -> Vec<u8> {
    let [x0, y0, x1, y1] = region.rect;
    let (w, h) = (region.width as f32, region.height as f32);
    // Region pixel `i` is centred at `i`, as the coverage's grid is.
    let to_region = |p: &[f32; 2]| {
        [
            (p[0] - x0) / (x1 - x0) * w - 0.5,
            (p[1] - y0) / (y1 - y0) * h - 0.5,
        ]
    };
    let points: Vec<[f32; 2]> = match &op.shape {
        RetouchShape::Spot { center, .. } => vec![to_region(center)],
        RetouchShape::Brush { points, .. } => points.iter().map(to_region).collect(),
    };
    let radius = (op.radius() * frame.long_edge() * region.scale).max(0.5);
    heal::coverage(
        &points,
        radius,
        op.feather,
        FeatherProfile::Measured,
        [0, 0, region.width as i32, region.height as i32],
    )
    .into_iter()
    .map(|c| (c.clamp(0., 1.) * 255.).round() as u8)
    .collect()
}

/// The generator's answer (rows `stride` bytes apart) with `input`'s own pixels
/// wherever it wasn't painted, rows packed. The protocol leaves what a program
/// writes outside the painted area unspecified, and the patch is the membrane's
/// source on the ring around the shape too: there it must be the photo's own.
pub fn keep_unpainted(input: &Input, result: &[u8], stride: usize) -> Result<Vec<u8>> {
    let (w, h) = (input.region.width as usize, input.region.height as usize);
    ensure!(
        stride >= w * 3 && result.len() >= stride * (h - 1) + w * 3,
        "The result is smaller than the request"
    );
    let mut out = input.rgb.clone();
    for j in 0..h {
        for i in 0..w {
            if input.coverage[j * w + i] > 0 {
                let k = (j * w + i) * 3;
                out[k..k + 3].copy_from_slice(&result[j * stride + i * 3..][..3]);
            }
        }
    }
    Ok(out)
}

/// The generator's answer (`width` × `height` 8-bit sRGB, rows `stride` bytes
/// apart) as a patch's camera RGB samples, through the captured `rendition`.
pub fn patch_samples(
    rendition: &Rendition,
    result: &[u8],
    width: u32,
    height: u32,
    stride: usize,
) -> Result<Vec<f32>> {
    let (w, h) = (width as usize, height as usize);
    ensure!(
        stride >= w * 3 && result.len() >= stride * (h - 1) + w * 3,
        "The result is smaller than the request"
    );
    let mut out = Vec::with_capacity(w * h * 3);
    for j in 0..h {
        for i in 0..w {
            let p = &result[j * stride + i * 3..][..3];
            out.extend(rendition.inverse([p[0], p[1], p[2]]));
        }
    }
    Ok(out)
}

fn bilinear(im: &CameraImage, x: f32, y: f32) -> [f32; 3] {
    let x = x.clamp(0., (im.width - 1) as f32);
    let y = y.clamp(0., (im.height - 1) as f32);
    let (ix, iy) = (x as u32, y as u32);
    let (jx, jy) = ((ix + 1).min(im.width - 1), (iy + 1).min(im.height - 1));
    let (fx, fy) = (x - ix as f32, y - iy as f32);
    let at = |x: u32, y: u32| im.pixels[(y * im.width + x) as usize];
    let (a, b, c, d) = (at(ix, iy), at(jx, iy), at(ix, jy), at(jx, jy));
    std::array::from_fn(|i| {
        (a[i] * (1. - fx) + b[i] * fx) * (1. - fy) + (c[i] * (1. - fx) + d[i] * fx) * fy
    })
}
