//! Develop's HDR preview (docs/hdr.md#display): the HDR master of an edit with HDR
//! on, shown above SDR white on a display with headroom, in place of its SDR
//! rendition, in every view of the photo Develop draws: Fit, zoomed out, a 100%
//! region, and each side of Before/After and Reference View.
//!
//! Each place the master can show is a [`Slot`]. A whole photo renders on the CPU
//! from a reduced copy of it (twice the size shown, as `develop::preview` makes it),
//! a 100% region from the full photo. One worker renders them, the latest request of
//! each slot only, the edit's region first; asking a slot for something else cancels
//! its render under way. Pixels go to a 16-bit float texture in extended sRGB, the
//! window surface's encoding (`platform::display::Layer`), held to the display's
//! headroom by the HDR limit's roll-off. A result is shown only while it still
//! matches the photo, the edit, the part of it and the headroom it was rendered for;
//! a display change is a view change, never an edit.
//!
//! Visualize HDR (Lightroom's) has slots of its own: the same master, rendered
//! whatever the display, drawn as the colour of the stop above SDR white each pixel
//! reaches ([`overlay`]) and left clear below white.
use crate::app::worker::Latest;
use crate::camera_data::CameraImage;
use crate::develop::AiSource;
use crate::model::recipe::Recipe;
use eframe::{egui, egui_wgpu};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Where a preview shows. Its index is its worker lane: lower lanes render first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Slot {
    /// The edit's 100% region, drawn over its whole photo.
    Region,
    /// The edit's whole photo, in Fit or zoomed out.
    Whole,
    /// Before's (or the reference photo's) 100% region.
    BeforeRegion,
    /// Before's (or the reference photo's) whole photo.
    BeforeWhole,
    /// Visualize HDR's overlay of the shown photo's 100% region.
    OverlayRegion,
    /// Visualize HDR's overlay of the shown photo's whole photo.
    OverlayWhole,
}
const SLOTS: usize = 6;
impl Slot {
    const ALL: [Slot; SLOTS] = [
        Slot::Region,
        Slot::Whole,
        Slot::BeforeRegion,
        Slot::BeforeWhole,
        Slot::OverlayRegion,
        Slot::OverlayWhole,
    ];
    fn lane(self) -> usize {
        self as usize
    }
    /// Whether this slot holds Visualize HDR's overlay rather than the master.
    fn overlay(self) -> bool {
        matches!(self, Slot::OverlayRegion | Slot::OverlayWhole)
    }
}

/// Which part of a photo a preview renders, in the pixels of the photo as its
/// geometry lays it out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Part {
    /// The whole photo within this long edge.
    Whole(u32),
    /// This region `[x, y, w, h]` at 100%.
    Region([u32; 4]),
}

/// What a preview was rendered for.
#[derive(Clone, Debug)]
pub(in crate::app) struct Key {
    /// The decoded photo, by its allocation: a reopened or replaced photo is another.
    /// Held weakly, so the allocation, and with it this identity, can't be reused by
    /// another photo while the key exists.
    image: std::sync::Weak<CameraImage>,
    recipe: Recipe,
    /// The AI Denoise source the render blends in, as the SDR rendition's render job
    /// captures it: each result, partial or final, is a new render.
    ai: Option<AiSource>,
    part: Part,
    /// The display's headroom, to a tenth of a stop: macOS moves it with the
    /// display's brightness, and each step is a new render.
    headroom: i32,
}
impl Key {
    pub(in crate::app) fn new(
        image: &Arc<CameraImage>,
        recipe: &Recipe,
        ai: Option<&AiSource>,
        part: Part,
        headroom: f32,
    ) -> Self {
        Self {
            image: Arc::downgrade(image),
            recipe: recipe.clone(),
            ai: ai.cloned(),
            part,
            headroom: (headroom.log2() * 10.).round() as i32,
        }
    }
    /// The display headroom this preview is for, to a tenth of a stop.
    pub(in crate::app) fn headroom(&self) -> f32 {
        (self.headroom as f32 / 10.).exp2()
    }
    pub(in crate::app) fn part(&self) -> Part {
        self.part
    }
}
impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Weak::ptr_eq(&self.image, &other.image)
            && self.recipe == other.recipe
            && self.ai == other.ai
            && self.part == other.part
            && self.headroom == other.headroom
    }
}

struct Job {
    slot: Slot,
    key: Key,
    /// Which request of its slot this is: a key asked for again is a new attempt,
    /// and only the latest attempt's result counts.
    attempt: u64,
    image: Arc<CameraImage>,
    /// Set when its slot asks for something else, or the worker stops.
    cancel: Arc<AtomicBool>,
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

/// A finished render: RGBA half floats, row by row, and the master they show.
struct Done {
    key: Key,
    width: u32,
    height: u32,
    rgba: Vec<u16>,
    master: Master,
}

/// A shown preview's master, for the RGB readout: linear ProPhoto (1 = graphics
/// white) as half floats, row by row, at the preview's size. Halves keep a fifth of a
/// percent, a few thousandths of a stop, at half an `f32`'s memory.
#[derive(Default)]
struct Master(Vec<[u16; 3]>);

/// A preview on the screen: its key, texture and master.
struct Shown {
    key: Key,
    id: egui::TextureId,
    /// Kept alive while egui draws it.
    _texture: wgpu::Texture,
    width: u32,
    height: u32,
    master: Master,
}

/// One slot's requests and what it shows.
#[derive(Default)]
struct State {
    /// The latest request, its attempt, and the flag that cancels its render.
    requested: Option<Key>,
    attempt: u64,
    cancel: Option<Arc<AtomicBool>>,
    shown: Option<Shown>,
    /// Why the render for this key failed.
    failure: Option<(Key, String)>,
}

pub(in crate::app) struct HdrPreview {
    worker: Latest<Job>,
    done: Arc<Mutex<[Option<Outcome>; SLOTS]>>,
    slots: [State; SLOTS],
    /// The slots the frame drawn so far asked for or kept ([`Self::sweep`]).
    used: [bool; SLOTS],
}

impl HdrPreview {
    pub(in crate::app) fn new(ctx: egui::Context) -> Self {
        let done: Arc<Mutex<[Option<Outcome>; SLOTS]>> = Arc::default();
        let out = done.clone();
        let worker = Latest::with_lanes(SLOTS, move |job: Job| {
            // A panic is this key's failure too, so its note shows and the next edit
            // asks again.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render(&job)))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("the renderer stopped unexpectedly")))
                .map_err(|e| format!("{e:#}"));
            out.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)[job.slot.lane()] =
                Some((job.attempt, result));
            ctx.request_repaint();
        });
        Self {
            worker,
            done,
            slots: Default::default(),
            used: [false; SLOTS],
        }
    }
    /// Stops the worker; a render under way is cancelled and dropped.
    pub(in crate::app) fn stop(&mut self) -> crate::app::task::Stopping {
        for state in &self.slots {
            if let Some(cancel) = &state.cancel {
                cancel.store(true, Ordering::Relaxed);
            }
        }
        self.worker.stop()
    }
    /// Asks `slot` for the preview `key` describes, unless it was asked for already.
    /// A render of its earlier request is cancelled.
    pub(in crate::app) fn request(&mut self, slot: Slot, key: Key, image: Arc<CameraImage>) {
        self.used[slot.lane()] = true;
        let state = &mut self.slots[slot.lane()];
        if state.requested.as_ref() == Some(&key) {
            return;
        }
        if let Some(old) = state.cancel.take() {
            old.store(true, Ordering::Relaxed);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        state.requested = Some(key.clone());
        state.attempt += 1;
        state.cancel = Some(cancel.clone());
        // A key asked for again renders again, and its earlier failure goes.
        state.failure = None;
        self.worker.submit_to(
            slot.lane(),
            Job {
                slot,
                key,
                attempt: state.attempt,
                image,
                cancel,
            },
        );
    }
    /// Forgets what `slot` was asked for and shows, freeing its texture: for a view
    /// that is gone, e.g. a 100% region after zooming out.
    pub(in crate::app) fn release(&mut self, slot: Slot, state: Option<&egui_wgpu::RenderState>) {
        let lane = slot.lane();
        let slot_state = std::mem::take(&mut self.slots[lane]);
        if let Some(cancel) = &slot_state.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        // Attempts keep counting, so a result still on its way is never taken for
        // a later request's.
        self.slots[lane].attempt = slot_state.attempt;
        if let (Some(shown), Some(state)) = (slot_state.shown, state) {
            state.renderer.write().free_texture(&shown.id);
        }
    }
    /// Whether `slot` was asked for or shows anything.
    fn holds(&self, slot: Slot) -> bool {
        let state = &self.slots[slot.lane()];
        state.requested.is_some() || state.shown.is_some()
    }
    /// Keeps what `slot` holds through this frame's [`Self::sweep`] without asking
    /// it for anything: the whole photo under a region still rendering.
    pub(in crate::app) fn keep(&mut self, slot: Slot) {
        self.used[slot.lane()] = true;
    }
    /// Ends a frame: releases each slot the frame neither asked for nor kept, as
    /// its view is gone, however it went (zoomed out, a comparison closed, a
    /// reference photo removed, another module). The edit's whole photo stays, to
    /// show again when Fit returns to the same edit.
    pub(in crate::app) fn sweep(&mut self, state: Option<&egui_wgpu::RenderState>) {
        for slot in Slot::ALL {
            if !self.used[slot.lane()] && slot != Slot::Whole && self.holds(slot) {
                self.release(slot, state);
            }
        }
        self.used = [false; SLOTS];
    }
    /// The texture `slot` has for `key`, uploading a finished render first; `None`
    /// until the render for exactly `key` is there.
    pub(in crate::app) fn texture(
        &mut self,
        slot: Slot,
        key: &Key,
        state: &egui_wgpu::RenderState,
    ) -> Option<egui::TextureId> {
        if let Some(done) = self.collect(slot) {
            self.upload(slot, done, state);
        }
        let shown = self.slots[slot.lane()].shown.as_ref()?;
        (&shown.key == key).then_some(shown.id)
    }
    /// Takes the worker's result for `slot`: a finished render of its latest
    /// request, to be uploaded, or that request's failure, kept for its note. A
    /// result of an older attempt is dropped, even one for the same key: a newer one
    /// is on its way.
    fn collect(&mut self, slot: Slot) -> Option<Done> {
        let (attempt, result) = self
            .done
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[slot.lane()]
        .take()?;
        let state = &mut self.slots[slot.lane()];
        if attempt != state.attempt {
            return None;
        }
        match result {
            Ok(done) => {
                state.failure = None;
                Some(done)
            }
            Err(e) => {
                state.failure = state.requested.clone().map(|key| (key, e));
                None
            }
        }
    }
    /// Whether `slot` draws the preview for `key`.
    pub(in crate::app) fn shows(&self, slot: Slot, key: &Key) -> bool {
        self.slots[slot.lane()]
            .shown
            .as_ref()
            .is_some_and(|shown| &shown.key == key)
    }
    /// The master's value (linear ProPhoto, 1 = graphics white) under `pos` of the
    /// preview `slot` draws for `key` at `at`; `None` off it, or while it doesn't
    /// draw that preview.
    pub(in crate::app) fn master_at(
        &self,
        slot: Slot,
        key: &Key,
        at: egui::Rect,
        pos: egui::Pos2,
    ) -> Option<[f32; 3]> {
        let shown = self.slots[slot.lane()]
            .shown
            .as_ref()
            .filter(|s| &s.key == key)?;
        let (x, y) = crate::app::develop::readout::index_at(shown.width, shown.height, at, pos)?;
        let p = shown.master.0.get((y * shown.width + x) as usize)?;
        Some(p.map(unhalf))
    }
    /// Why the render for `key` in `slot` failed, if it did.
    pub(in crate::app) fn failure(&self, slot: Slot, key: &Key) -> Option<&str> {
        self.slots[slot.lane()]
            .failure
            .as_ref()
            .filter(|(k, _)| k == key)
            .map(|(_, e)| e.as_str())
    }
    fn upload(&mut self, slot: Slot, done: Done, state: &egui_wgpu::RenderState) {
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
        let shown = Shown {
            key: done.key,
            id,
            _texture: texture,
            width: done.width,
            height: done.height,
            master: done.master,
        };
        if let Some(old) = self.slots[slot.lane()].shown.replace(shown) {
            renderer.free_texture(&old.id);
        }
    }
}

/// The HDR master of the part of `job`'s photo its key names, encoded for the window.
fn render(job: &Job) -> anyhow::Result<Done> {
    let key = &job.key;
    let recipe = key.recipe.checked()?;
    let cancel = rawmakase_cancel::Cancel::new(&job.cancel);
    let ai = key.ai.as_ref();
    let master = match key.part {
        Part::Whole(edge) => {
            crate::develop::render_hdr_preview(&job.image, &recipe, ai, edge, cancel)?
        }
        Part::Region(region) => {
            crate::develop::render_hdr_region(&job.image, &recipe, ai, region, cancel)?
        }
    };
    let rgba = match job.slot.overlay() {
        true => overlay(&master.pixels),
        false => encode(&master.pixels, key.headroom()),
    };
    Ok(Done {
        key: key.clone(),
        width: master.width,
        height: master.height,
        rgba,
        master: Master(master.pixels.iter().map(|p| p.map(half)).collect()),
    })
}

/// Visualize HDR's colours for light 0–1, 1–2, 2–3 and 3 or more stops above SDR
/// white: cyan, blue, purple and magenta, the order Lightroom's overlay uses (its
/// values aren't published), sRGB-encoded.
pub(in crate::app) const STOP_COLORS: [[u8; 3]; 4] = [
    [0, 224, 255],
    [40, 110, 255],
    [150, 70, 255],
    [255, 50, 220],
];

/// The [`STOP_COLORS`] index for light `stops` above SDR white, `None` at or below it.
/// White itself reaches a few millionths above 1 through the primaries' matrix, so
/// the first thousandth of a stop counts as white.
pub(in crate::app) fn stop_color(stops: f32) -> Option<usize> {
    (stops > 1e-3).then(|| (stops.ceil() as usize).clamp(1, 4) - 1)
}

/// Visualize HDR's overlay of HDR master pixels (linear ProPhoto, 1 = graphics
/// white), RGBA half floats, premultiplied as egui draws them: each pixel whose
/// brightest channel in the display's primaries (linear sRGB, as the histogram and
/// readout read them) is above white takes its stop's colour, opaque; the rest are
/// clear, so the photo shows through.
pub(in crate::app) fn overlay(pixels: &[[f32; 3]]) -> Vec<u16> {
    let to_srgb = crate::color::hdr_output::HdrGamut::Rec709.prophoto_to_rgb();
    pixels
        .iter()
        .flat_map(|p| {
            let top = crate::color::mul(to_srgb, *p)
                .into_iter()
                .filter(|v| v.is_finite())
                .fold(0f32, f32::max);
            match stop_color(top.max(f32::MIN_POSITIVE).log2()) {
                Some(k) => {
                    let [r, g, b] = STOP_COLORS[k].map(|c| half(f32::from(c) / 255.));
                    [r, g, b, half(1.)]
                }
                None => [0; 4],
            }
        })
        .collect()
}

/// HDR master pixels (linear ProPhoto, 1 = graphics white) as extended sRGB half
/// floats for the window ([`crate::color::hdr_output::extended_srgb`]), RGBA.
pub(in crate::app) fn encode(pixels: &[[f32; 3]], headroom: f32) -> Vec<u16> {
    pixels
        .iter()
        .flat_map(|p| {
            let [r, g, b] = crate::color::hdr_output::extended_srgb(*p, headroom).map(half);
            [r, g, b, half(1.)]
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

/// Half-precision bits as the value they hold.
fn unhalf(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1. } else { 1. };
    let exponent = ((h >> 10) & 0x1f) as i32;
    let mantissa = (h & 0x3ff) as f32;
    if exponent == 0 {
        return sign * mantissa * 2f32.powi(-24);
    }
    sign * (1. + mantissa / 1024.) * 2f32.powi(exponent - 15)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let key = |headroom| Key::new(&image, &recipe, None, Part::Whole(100), headroom);
        assert_eq!(key(4.), key(4.05));
        assert_ne!(key(4.), key(4.4));
        assert!((key(4.).headroom() - 4.).abs() < 1e-3);
        assert_ne!(
            key(4.),
            Key::new(&image, &recipe, None, Part::Region([0, 0, 100, 100]), 4.)
        );
        let (_, other) =
            crate::app::testing::editor_with_blue_photo(&ctx, Default::default(), false);
        assert_ne!(
            key(4.),
            Key::new(&other, &recipe, None, Part::Whole(100), 4.)
        );
        // A closed photo's key keeps its identity: no photo opened later is it.
        let closed = Key::new(&other, &recipe, None, Part::Whole(100), 4.);
        drop(other);
        for _ in 0..8 {
            let (_, next) =
                crate::app::testing::editor_with_blue_photo(&ctx, Default::default(), false);
            assert_ne!(closed, Key::new(&next, &recipe, None, Part::Whole(100), 4.));
        }
    }

    fn hdr_recipe() -> Recipe {
        let mut recipe = Recipe::default();
        recipe.hdr.enabled = true;
        recipe
    }

    /// Takes `slot`'s result from the worker, waiting at most ten seconds.
    fn finished(preview: &HdrPreview, slot: Slot) -> (u64, Result<Done, String>) {
        let start = std::time::Instant::now();
        loop {
            if let Some(done) = preview.done.lock().unwrap()[slot.lane()].take() {
                return done;
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
        let key = Key::new(&image, &hdr_recipe(), None, Part::Whole(16), 4.);
        preview.request(Slot::Whole, key.clone(), image.clone());
        let done = finished(&preview, Slot::Whole).1.expect("an HDR master");
        assert_eq!(done.key, key);
        assert!(done.width.max(done.height) <= 16 && done.width > 0);
        assert_eq!(done.rgba.len(), (done.width * done.height * 4) as usize);
        preview.request(Slot::Whole, key, image);
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(preview.done.lock().unwrap()[Slot::Whole.lane()].is_none());
        let _ = preview.stop();
    }

    /// An overlay slot renders the same master as Visualize HDR's overlay; the
    /// master's slot keeps its own encoding.
    #[test]
    fn an_overlay_slot_renders_the_overlay_of_the_master() {
        let ctx = egui::Context::default();
        let (_, image) = crate::app::testing::editor_with_blue_photo(
            &ctx,
            crate::app::preferences::session::Session::default(),
            false,
        );
        let mut recipe = hdr_recipe();
        recipe.exposure = 3.;
        let mut preview = HdrPreview::new(ctx);
        let key = Key::new(&image, &recipe, None, Part::Whole(16), 1.);
        preview.request(Slot::OverlayWhole, key.clone(), image.clone());
        preview.request(Slot::Whole, key, image);
        let shown = finished(&preview, Slot::OverlayWhole)
            .1
            .expect("an overlay");
        let master = finished(&preview, Slot::Whole).1.expect("a master");
        let pixels: Vec<[f32; 3]> = (master.master.0.iter()).map(|p| p.map(unhalf)).collect();
        assert_eq!(shown.rgba, overlay(&pixels));
        assert!(
            shown.rgba.chunks(4).any(|p| p[3] != 0),
            "light above white is coloured"
        );
        let _ = preview.stop();
    }

    /// A region renders at 100%: the part of the full photo it names, at its size.
    #[test]
    fn a_region_renders_at_full_size() {
        let ctx = egui::Context::default();
        let (_, image) =
            crate::app::testing::editor_with_blue_photo(&ctx, Default::default(), false);
        let mut preview = HdrPreview::new(ctx);
        let key = Key::new(&image, &hdr_recipe(), None, Part::Region([4, 2, 10, 6]), 4.);
        preview.request(Slot::Region, key.clone(), image);
        let done = finished(&preview, Slot::Region).1.expect("an HDR region");
        assert_eq!((done.key, done.width, done.height), (key, 10, 6));
        let _ = preview.stop();
    }

    /// Asking a slot for something else cancels its render under way, whose result
    /// is then never kept; another slot's render goes on.
    #[test]
    fn a_new_request_cancels_the_slots_render_alone() {
        let ctx = egui::Context::default();
        let (_, image) =
            crate::app::testing::editor_with_blue_photo(&ctx, Default::default(), false);
        let mut preview = HdrPreview::new(ctx);
        // The worker is stopped, so the jobs stay queued and their flags can be read.
        let _ = preview.worker.stop();
        let key = |edge| Key::new(&image, &hdr_recipe(), None, Part::Whole(edge), 4.);
        preview.request(Slot::Whole, key(16), image.clone());
        preview.request(Slot::BeforeWhole, key(16), image.clone());
        let first = preview.slots[Slot::Whole.lane()].cancel.clone().unwrap();
        let other = preview.slots[Slot::BeforeWhole.lane()]
            .cancel
            .clone()
            .unwrap();
        preview.request(Slot::Whole, key(24), image.clone());
        assert!(first.load(Ordering::Relaxed));
        assert!(!other.load(Ordering::Relaxed));
        // The first attempt's late result is not the second's.
        preview.done.lock().unwrap()[Slot::Whole.lane()] = Some((1, Err("cancelled".into())));
        assert!(preview.collect(Slot::Whole).is_none());
        assert_eq!(preview.failure(Slot::Whole, &key(24)), None);
        assert_eq!(preview.failure(Slot::Whole, &key(16)), None);
        // A cancelled render stops early with an error.
        let job = Job {
            slot: Slot::Whole,
            key: key(16),
            attempt: 1,
            image: image.clone(),
            cancel: first,
        };
        assert!(render(&job).is_err());
        // Stopping cancels every slot's render.
        let _ = preview.stop();
        assert!(other.load(Ordering::Relaxed));
    }

    /// A released slot forgets its request, so asking again renders again, and a
    /// result of its earlier request that lands later is dropped.
    #[test]
    fn a_released_slot_drops_its_late_result() {
        let ctx = egui::Context::default();
        let (_, image) =
            crate::app::testing::editor_with_blue_photo(&ctx, Default::default(), false);
        let mut preview = HdrPreview::new(ctx);
        let _ = preview.worker.stop();
        let key = Key::new(&image, &hdr_recipe(), None, Part::Region([0, 0, 8, 8]), 4.);
        preview.request(Slot::Region, key.clone(), image.clone());
        let cancel = preview.slots[Slot::Region.lane()].cancel.clone().unwrap();
        assert!(preview.holds(Slot::Region));
        preview.release(Slot::Region, None);
        assert!(cancel.load(Ordering::Relaxed));
        assert!(!preview.holds(Slot::Region));
        let done = Done {
            key: key.clone(),
            width: 1,
            height: 1,
            rgba: vec![0; 4],
            master: Master::default(),
        };
        preview.done.lock().unwrap()[Slot::Region.lane()] = Some((1, Ok(done)));
        preview.request(Slot::Region, key, image);
        assert_eq!(preview.slots[Slot::Region.lane()].attempt, 2);
        assert!(preview.collect(Slot::Region).is_none());
    }

    /// A frame releases what it didn't ask for or keep, except the edit's whole
    /// photo; a kept slot stays.
    #[test]
    fn a_frame_releases_the_slots_it_did_not_use() {
        let ctx = egui::Context::default();
        let (_, image) =
            crate::app::testing::editor_with_blue_photo(&ctx, Default::default(), false);
        let mut preview = HdrPreview::new(ctx);
        let _ = preview.worker.stop();
        let key = |part| Key::new(&image, &hdr_recipe(), None, part, 4.);
        for slot in Slot::ALL {
            preview.request(slot, key(Part::Whole(16)), image.clone());
        }
        let running = preview.slots[Slot::BeforeRegion.lane()]
            .cancel
            .clone()
            .unwrap();
        preview.sweep(None);
        assert!(Slot::ALL.iter().all(|s| preview.holds(*s)), "all used");
        preview.request(Slot::Region, key(Part::Whole(16)), image.clone());
        preview.keep(Slot::BeforeWhole);
        preview.sweep(None);
        assert!(preview.holds(Slot::Region) && preview.holds(Slot::BeforeWhole));
        assert!(preview.holds(Slot::Whole), "the edit's whole photo stays");
        assert!(!preview.holds(Slot::BeforeRegion));
        assert!(running.load(Ordering::Relaxed), "its render is cancelled");
        preview.sweep(None);
        assert!(!preview.holds(Slot::Region) && !preview.holds(Slot::BeforeWhole));
        assert!(preview.holds(Slot::Whole));
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
        let a = Key::new(&image, &hdr_recipe(), None, Part::Whole(16), 4.);
        let b = Key::new(&image, &hdr_recipe(), None, Part::Whole(24), 4.);
        let done = |key: &Key| Done {
            key: key.clone(),
            width: 1,
            height: 1,
            rgba: vec![0; 4],
            master: Master::default(),
        };
        let slot = Slot::Whole;
        let set = |preview: &HdrPreview, outcome| {
            preview.done.lock().unwrap()[slot.lane()] = Some(outcome);
        };
        // A, B and A again: the first A's late failure is not the second A's.
        preview.slots[slot.lane()].requested = Some(a.clone());
        preview.slots[slot.lane()].attempt = 3;
        set(&preview, (1, Err("first A".into())));
        assert!(preview.collect(slot).is_none());
        assert_eq!(preview.failure(slot, &a), None);
        set(&preview, (2, Ok(done(&b))));
        assert!(preview.collect(slot).is_none());
        // The latest attempt's failure is kept for its key and slot alone...
        set(&preview, (3, Err("second A".into())));
        assert!(preview.collect(slot).is_none());
        assert_eq!(preview.failure(slot, &a), Some("second A"));
        assert_eq!(preview.failure(slot, &b), None);
        assert_eq!(preview.failure(Slot::BeforeWhole, &a), None);
        // ...and goes when the key renders.
        set(&preview, (3, Ok(done(&a))));
        assert!(preview.collect(slot).is_some());
        assert_eq!(preview.failure(slot, &a), None);
        // Asking for another key clears it too.
        set(&preview, (3, Err("again".into())));
        assert!(preview.collect(slot).is_none());
        preview.request(slot, b, image);
        assert_eq!(preview.failure(slot, &a), None);
        let _ = preview.stop();
    }

    /// A request renders the master it names; its result is collected for upload.
    #[test]
    fn a_finished_render_is_collected_for_its_request() {
        let ctx = egui::Context::default();
        let (_, image) =
            crate::app::testing::editor_with_blue_photo(&ctx, Default::default(), false);
        let mut preview = HdrPreview::new(ctx);
        let key = Key::new(&image, &hdr_recipe(), None, Part::Whole(16), 4.);
        preview.request(Slot::Whole, key.clone(), image);
        let start = std::time::Instant::now();
        let done = loop {
            if let Some(done) = preview.collect(Slot::Whole) {
                break done;
            }
            assert!(start.elapsed().as_secs() < 10, "no HDR preview");
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!(done.key, key);
        assert_eq!(preview.failure(Slot::Whole, &key), None);
        let _ = preview.stop();
    }

    /// Visualize HDR colours each pixel by the stop above white its brightest
    /// display channel reaches and leaves light at or below white clear.
    #[test]
    fn the_overlay_colours_light_by_its_stop_above_white() {
        let white = [1.; 3];
        let at = |stops: f32| white.map(|v: f32| v * stops.exp2());
        let shown = overlay(&[at(-1.), white, at(0.5), at(1.5), at(2.5), at(3.5), at(7.)]);
        let pixel = |k: usize| -> [f32; 4] { std::array::from_fn(|c| unhalf(shown[4 * k + c])) };
        assert_eq!(pixel(0), [0.; 4], "below white is clear");
        assert_eq!(pixel(1), [0.; 4], "white is clear");
        for (k, colour) in [(2, 0), (3, 1), (4, 2), (5, 3), (6, 3)] {
            let want = STOP_COLORS[colour].map(|c| f32::from(c) / 255.);
            let got = pixel(k);
            for c in 0..3 {
                assert!((got[c] - want[c]).abs() < 2e-3, "pixel {k}: {got:?}");
            }
            assert_eq!(got[3], 1.);
        }
        // A saturated colour counts by its brightest channel in the display's
        // primaries, not its luminance.
        let to_prophoto =
            crate::color::inverse(crate::color::hdr_output::HdrGamut::Rec709.prophoto_to_rgb());
        let red = crate::color::mul(to_prophoto, [1.5, 0.1, 0.1]);
        assert_eq!(unhalf(overlay(&[red])[3]), 1.);
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
