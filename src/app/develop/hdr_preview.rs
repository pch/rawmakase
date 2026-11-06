//! Develop's HDR preview (docs/hdr.md#display): the HDR master of an edit with HDR
//! on, shown above SDR white on a display with headroom, in place of its SDR
//! rendition, in the Fit view.
//!
//! It renders on the CPU from a reduced copy of the photo (twice the fit size, as
//! `develop::preview` makes it), on a worker of its own that keeps only the latest
//! request. Its pixels go to a 16-bit float texture in extended sRGB, the window
//! surface's encoding (`platform::display::Layer`), held to the display's headroom
//! by the HDR limit's roll-off. A result is shown only while it still matches the
//! photo, the edit, the fit size and the headroom it was rendered for; a display
//! change is a view change, never an edit.
use crate::app::worker::Latest;
use crate::camera_data::CameraImage;
use crate::model::recipe::Recipe;
use eframe::{egui, egui_wgpu};
use std::sync::{Arc, Mutex};

/// What a preview was rendered for.
#[derive(Clone, Debug)]
pub(in crate::app) struct Key {
    /// The decoded photo, by its allocation: a reopened or replaced photo is another.
    /// Held weakly, so the allocation, and with it this identity, can't be reused by
    /// another photo while the key exists.
    image: std::sync::Weak<CameraImage>,
    recipe: Recipe,
    edge: u32,
    /// The display's headroom, to a tenth of a stop: macOS moves it with the
    /// display's brightness, and each step is a new render.
    headroom: i32,
}
impl Key {
    pub(in crate::app) fn new(
        image: &Arc<CameraImage>,
        recipe: &Recipe,
        edge: u32,
        headroom: f32,
    ) -> Self {
        Self {
            image: Arc::downgrade(image),
            recipe: recipe.clone(),
            edge,
            headroom: (headroom.log2() * 10.).round() as i32,
        }
    }
    fn headroom(&self) -> f32 {
        (self.headroom as f32 / 10.).exp2()
    }
}
impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Weak::ptr_eq(&self.image, &other.image)
            && self.recipe == other.recipe
            && self.edge == other.edge
            && self.headroom == other.headroom
    }
}

struct Job {
    key: Key,
    /// Which request this is: a key asked for again is a new attempt, and only the
    /// latest attempt's result counts.
    attempt: u64,
    image: Arc<CameraImage>,
}

/// What Develop shows of an HDR edit, for the Basic panel's note.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) enum Presentation {
    /// The master, up to this many stops above SDR white.
    Hdr { stops: f32 },
    /// The SDR rendition, while the master for the edit as it is now renders.
    Rendering,
    /// The SDR rendition: the master for the edit as it is now failed to render.
    Failed(String),
    /// The SDR rendition, for this reason.
    Sdr(&'static str),
}

/// What the worker hands back for an attempt: the render, or why it failed.
type Outcome = (u64, Result<Done, String>);

/// A finished render: RGBA half floats, row by row.
struct Done {
    key: Key,
    width: u32,
    height: u32,
    rgba: Vec<u16>,
}

pub(in crate::app) struct HdrPreview {
    worker: Latest<Job>,
    done: Arc<Mutex<Option<Outcome>>>,
    /// The latest request, and its attempt.
    requested: Option<Key>,
    attempt: u64,
    shown: Option<(Key, egui::TextureId, wgpu::Texture)>,
    /// Why the render for this key failed.
    failure: Option<(Key, String)>,
}

impl HdrPreview {
    pub(in crate::app) fn new(ctx: egui::Context) -> Self {
        let done: Arc<Mutex<Option<Outcome>>> = Arc::default();
        let out = done.clone();
        let worker = Latest::new(move |job: Job| {
            // A panic is this key's failure too, so its note shows and the next edit
            // asks again.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render(&job)))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("the renderer stopped unexpectedly")))
                .map_err(|e| format!("{e:#}"));
            *out.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((job.attempt, result));
            ctx.request_repaint();
        });
        Self {
            worker,
            done,
            requested: None,
            attempt: 0,
            shown: None,
            failure: None,
        }
    }
    /// Stops the worker; a render under way finishes and is dropped.
    pub(in crate::app) fn stop(&mut self) -> crate::app::task::Stopping {
        self.worker.stop()
    }
    /// Asks for the preview `key` describes, unless it was asked for already.
    pub(in crate::app) fn request(&mut self, key: Key, image: Arc<CameraImage>) {
        if self.requested.as_ref() != Some(&key) {
            self.requested = Some(key.clone());
            self.attempt += 1;
            // A key asked for again renders again, and its earlier failure goes.
            self.failure = None;
            self.worker.submit(Job {
                key,
                attempt: self.attempt,
                image,
            });
        }
    }
    /// The texture for `key`, uploading a finished render first; `None` until the
    /// render for exactly `key` is there.
    pub(in crate::app) fn texture(
        &mut self,
        key: &Key,
        state: &egui_wgpu::RenderState,
    ) -> Option<(egui::TextureId, [u32; 2])> {
        if let Some(done) = self.collect() {
            self.upload(done, state);
        }
        let (shown, id, texture) = self.shown.as_ref()?;
        (shown == key).then(|| (*id, [texture.width(), texture.height()]))
    }
    /// Takes the worker's result: a finished render of the latest request, to be
    /// uploaded, or that request's failure, kept for its note. A result of an older
    /// attempt is dropped, even one for the same key: a newer one is on its way.
    fn collect(&mut self) -> Option<Done> {
        let (attempt, result) = self
            .done
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()?;
        if attempt != self.attempt {
            return None;
        }
        match result {
            Ok(done) => {
                self.failure = None;
                Some(done)
            }
            Err(e) => {
                self.failure = self.requested.clone().map(|key| (key, e));
                None
            }
        }
    }
    /// Whether the preview drawn is the one for `key`.
    pub(in crate::app) fn shows(&self, key: &Key) -> bool {
        self.shown.as_ref().is_some_and(|(k, ..)| k == key)
    }
    /// Why the render for `key` failed, if it did.
    pub(in crate::app) fn failure(&self, key: &Key) -> Option<&str> {
        self.failure
            .as_ref()
            .filter(|(k, _)| k == key)
            .map(|(_, e)| e.as_str())
    }
    fn upload(&mut self, done: Done, state: &egui_wgpu::RenderState) {
        let size = wgpu::Extent3d {
            width: done.width,
            height: done.height,
            depth_or_array_layers: 1,
        };
        let texture = state.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("HDR preview"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let bytes: Vec<u8> = done.rgba.iter().flat_map(|v| v.to_ne_bytes()).collect();
        state.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(done.width * 8),
                rows_per_image: Some(done.height),
            },
            size,
        );
        let view = texture.create_view(&Default::default());
        let mut renderer = state.renderer.write();
        let id = renderer.register_native_texture(&state.device, &view, wgpu::FilterMode::Linear);
        if let Some((_, old, _)) = self.shown.replace((done.key, id, texture)) {
            renderer.free_texture(&old);
        }
    }
}

/// The HDR master of `job`'s photo within its fit edge, encoded for the window.
fn render(job: &Job) -> anyhow::Result<Done> {
    let key = &job.key;
    let reduced = crate::develop::preview(&job.image, key.edge.saturating_mul(2));
    let recipe = key.recipe.checked()?;
    let master = crate::develop::render_hdr(
        &reduced,
        &recipe,
        None,
        key.edge,
        rawmakase_cancel::Cancel::NEVER,
    )?;
    Ok(Done {
        key: key.clone(),
        width: master.width,
        height: master.height,
        rgba: encode(&master.pixels, key.headroom()),
    })
}

/// HDR master pixels (linear ProPhoto, 1 = graphics white) as extended sRGB half
/// floats for the window: linear sRGB primaries (negative where outside sRGB, which
/// extended sRGB keeps), each channel held to `headroom` by the HDR limit's roll-off,
/// then the sRGB curve, mirrored below 0.
pub(in crate::app) fn encode(pixels: &[[f32; 3]], headroom: f32) -> Vec<u16> {
    use crate::color::hdr_output::{HdrGamut, roll_off};
    let to_srgb = HdrGamut::Rec709.prophoto_to_rgb();
    pixels
        .iter()
        .flat_map(|p| {
            let rgb = crate::color::mul(to_srgb, *p).map(|v| {
                let v = if v.is_finite() { v } else { 0. };
                let encoded = crate::color::srgb_encode(roll_off(v.abs(), headroom));
                half(v.signum() * encoded)
            });
            [rgb[0], rgb[1], rgb[2], half(1.)]
        })
        .collect()
}

/// `v` as IEEE 754 half-precision bits, rounded to nearest; finite inputs only.
fn half(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let mantissa = bits & 0x7f_ffff;
    if exponent <= 0 {
        // Subnormal or zero: below 2^-14, far darker than anything shown.
        return sign;
    }
    if exponent >= 31 {
        return sign | 0x7bff;
    }
    // Added, not or-ed: a mantissa that rounds up carries into the exponent.
    let rounded = ((exponent as u32) << 10) + ((mantissa + 0x1000) >> 13);
    sign | rounded.min(0x7bff) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhalf(h: u16) -> f32 {
        let sign = if h & 0x8000 != 0 { -1. } else { 1. };
        let exponent = ((h >> 10) & 0x1f) as i32;
        let mantissa = (h & 0x3ff) as f32;
        if exponent == 0 {
            return sign * mantissa * 2f32.powi(-24);
        }
        sign * (1. + mantissa / 1024.) * 2f32.powi(exponent - 15)
    }

    /// Graphics white is extended sRGB 1, light above it encodes above 1 up to the
    /// display's headroom, and a neutral stays neutral.
    #[test]
    fn the_preview_encodes_extended_srgb_within_the_headroom() {
        let pixels = [[1.; 3], [4.; 3], [100.; 3], [0.18; 3]];
        let out = encode(&pixels, 4.);
        let px = |i: usize| {
            [
                unhalf(out[i * 4]),
                unhalf(out[i * 4 + 1]),
                unhalf(out[i * 4 + 2]),
            ]
        };
        assert!(px(0).iter().all(|v| (v - 1.).abs() < 2e-3), "{:?}", px(0));
        let top = crate::color::srgb_encode(4.);
        assert!(px(2).iter().all(|v| *v <= top + 1e-2), "{:?}", px(2));
        assert!(px(1).iter().all(|v| *v > 1.5), "{:?}", px(1));
        let gray = px(3);
        assert!((gray[0] - gray[1]).abs() < 2e-3 && (gray[1] - gray[2]).abs() < 2e-3);
        assert_eq!(unhalf(out[3]), 1.);
    }

    /// A headroom that moves within a tenth of a stop keeps the shown preview; one
    /// that moves further asks for a new render.
    #[test]
    fn the_key_follows_the_headroom_in_tenths_of_a_stop() {
        let ctx = egui::Context::default();
        let (_, image) =
            crate::app::testing::editor_with_blue_photo(&ctx, Default::default(), false);
        let recipe = Recipe::default();
        let key = |headroom| Key::new(&image, &recipe, 100, headroom);
        assert_eq!(key(4.), key(4.05));
        assert_ne!(key(4.), key(4.4));
        assert!((key(4.).headroom() - 4.).abs() < 1e-3);
        let (_, other) =
            crate::app::testing::editor_with_blue_photo(&ctx, Default::default(), false);
        assert_ne!(key(4.), Key::new(&other, &recipe, 100, 4.));
        // A closed photo's key keeps its identity: no photo opened later is it.
        let closed = Key::new(&other, &recipe, 100, 4.);
        drop(other);
        for _ in 0..8 {
            let (_, next) =
                crate::app::testing::editor_with_blue_photo(&ctx, Default::default(), false);
            assert_ne!(closed, Key::new(&next, &recipe, 100, 4.));
        }
    }

    fn hdr_recipe() -> Recipe {
        let mut recipe = Recipe::default();
        recipe.hdr.enabled = true;
        recipe
    }

    /// Waits for the worker's result, at most ten seconds.
    fn finished(preview: &HdrPreview) -> Result<Done, String> {
        let start = std::time::Instant::now();
        loop {
            if let Some(done) = preview.done.lock().unwrap().take() {
                return done.1;
            }
            assert!(start.elapsed().as_secs() < 10, "no HDR preview");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// A request renders the master of exactly the photo, edit, size and headroom it
    /// names, within the fit size; asking again for the same key renders nothing new.
    #[test]
    fn a_request_renders_the_master_it_names_once() {
        let ctx = egui::Context::default();
        let (_, image) = crate::app::testing::editor_with_blue_photo(
            &ctx,
            crate::app::preferences::session::Session::default(),
            false,
        );
        let mut preview = HdrPreview::new(ctx);
        let key = Key::new(&image, &hdr_recipe(), 16, 4.);
        preview.request(key.clone(), image.clone());
        let done = finished(&preview).expect("an HDR master");
        assert_eq!(done.key, key);
        assert!(done.width.max(done.height) <= 16 && done.width > 0);
        assert_eq!(done.rgba.len(), (done.width * done.height * 4) as usize);
        preview.request(key, image);
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(preview.done.lock().unwrap().is_none());
        let _ = preview.stop();
    }

    /// Only the latest attempt's result counts, even when an older attempt was for
    /// the same key (an edit away and back while it rendered), and a failure goes
    /// when that key is asked for again or renders.
    #[test]
    fn only_the_latest_attempts_result_is_kept() {
        let ctx = egui::Context::default();
        let (_, image) =
            crate::app::testing::editor_with_blue_photo(&ctx, Default::default(), false);
        let mut preview = HdrPreview::new(ctx);
        let a = Key::new(&image, &hdr_recipe(), 16, 4.);
        let b = Key::new(&image, &hdr_recipe(), 24, 4.);
        let done = |key: &Key| Done {
            key: key.clone(),
            width: 1,
            height: 1,
            rgba: vec![0; 4],
        };
        // A, B and A again: the first A's late failure is not the second A's.
        preview.requested = Some(a.clone());
        preview.attempt = 3;
        *preview.done.lock().unwrap() = Some((1, Err("first A".into())));
        assert!(preview.collect().is_none());
        assert_eq!(preview.failure(&a), None);
        *preview.done.lock().unwrap() = Some((2, Ok(done(&b))));
        assert!(preview.collect().is_none());
        // The latest attempt's failure is kept for its key alone...
        *preview.done.lock().unwrap() = Some((3, Err("second A".into())));
        assert!(preview.collect().is_none());
        assert_eq!(preview.failure(&a), Some("second A"));
        assert_eq!(preview.failure(&b), None);
        // ...and goes when the key renders.
        *preview.done.lock().unwrap() = Some((3, Ok(done(&a))));
        assert!(preview.collect().is_some());
        assert_eq!(preview.failure(&a), None);
        // Asking for another key clears it too.
        *preview.done.lock().unwrap() = Some((3, Err("again".into())));
        assert!(preview.collect().is_none());
        preview.request(b, image);
        assert_eq!(preview.failure(&a), None);
        let _ = preview.stop();
    }

    /// A request renders the master it names; its result is collected for upload.
    #[test]
    fn a_finished_render_is_collected_for_its_request() {
        let ctx = egui::Context::default();
        let (_, image) =
            crate::app::testing::editor_with_blue_photo(&ctx, Default::default(), false);
        let mut preview = HdrPreview::new(ctx);
        let key = Key::new(&image, &hdr_recipe(), 16, 4.);
        preview.request(key.clone(), image);
        let start = std::time::Instant::now();
        let done = loop {
            if let Some(done) = preview.collect() {
                break done;
            }
            assert!(start.elapsed().as_secs() < 10, "no HDR preview");
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!(done.key, key);
        assert_eq!(preview.failure(&key), None);
        let _ = preview.stop();
    }

    #[test]
    fn halves_round_trip_the_values_shown() {
        // Including values that round up across a power of two.
        for v in [
            0., 1., -0.25, 0.5, 2.5, 7.9, 1e-3, 0.4999, 1.9999, 7.999, -3.9999,
        ] {
            assert!((unhalf(half(v)) - v).abs() <= v.abs() * 1e-3 + 1e-4, "{v}");
        }
    }
}
