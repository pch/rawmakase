//! State owned by the document, preview, viewport and preset browser.
use crate::catalog::PhotoId;
use crate::{
    camera_data::{CameraImage, Metadata},
    export_settings::ExportOptions,
    model::recipe::Recipe,
};
use eframe::egui::{self, Vec2};
use std::{path::PathBuf, sync::Arc, time::Instant};

#[derive(Default)]
pub(in crate::app) struct Document {
    /// The photo's load: started when it opens, ended when it closes, and invalidated
    /// alone when the photo stays but what its load would bring no longer counts
    /// (quitting, or the file moving while it loads). Only [`Document::open`],
    /// [`Document::close`] and [`Document::invalidate_load`] change it with the document.
    load: crate::app::task::Task,
    /// The settings, their History and whether they still need saving.
    pub(in crate::app) edit: crate::edit_session::EditSession,
    pub(in crate::app) path: Option<PathBuf>,
    pub(in crate::app) metadata: Option<Metadata>,
    image: Option<Arc<CameraImage>>,
    /// How the decoded photo's colors spread, for Auto black & white; measured on
    /// first use.
    color_spread: std::cell::OnceCell<crate::develop::ColorSpread>,
    /// A conversion to black & white waiting for the photo to decode, for its
    /// Auto mix.
    pub(in crate::app) pending_treatment: Option<PendingTreatment>,
    pub(in crate::app) export: ExportOptions,
    pub(in crate::app) catalog_photo: Option<PhotoId>,
    /// Why opening the photo failed: no image is coming.
    pub(in crate::app) failed: Option<String>,
    pub(in crate::app) lightroom_notice: String,
    /// Lightroom's history for the open catalog photo, oldest first.
    pub(in crate::app) lightroom_history: Vec<crate::catalog::HistoryStep>,
    /// What Before shows when not the photo's starting settings: a History step,
    /// snapshot or the edit copied to it. Not saved, as in Lightroom.
    pub(in crate::app) before: Option<Recipe>,
    /// The open catalog photo's Snapshots.
    pub(in crate::app) snapshots: crate::app::develop::panels::snapshots::Snapshots,
    /// The photo's Lightroom settings, as read with its edit, to apply once its
    /// profiles arrive.
    pub(in crate::app) pending_lightroom: Option<String>,
    /// The photo's file as it was when it was opened: what an export of the edit
    /// shown checks it still has. For a photo opened from its editable preview,
    /// the identity the preview recorded of its original.
    pub(in crate::app) file: Option<crate::storage::Identity>,
    /// What the photo was opened from, once its header arrived: its original, or
    /// its editable preview, which nothing replaces until the photo opens again.
    pub(in crate::app) from: crate::app::worker::OpenedFrom,
    /// Where the edit on screen started from.
    pub(in crate::app) origin: EditOrigin,
    /// The raw defaults for this photo, once its profiles are known: what Reset
    /// returns to and Before shows.
    pub(in crate::app) defaults: Option<crate::raw_defaults::Resolved>,
    pub(in crate::app) profiles: Vec<Arc<crate::camera_profiles::CameraProfile>>,
    pub(in crate::app) profile_errors: Vec<String>,
    /// Where the Profile menu lists each of `profiles`.
    pub(in crate::app) profile_groups: crate::camera_profiles::Groups,
    /// The file as it was before the decoded image was made from it (the loader's
    /// reading, not the header's): what AI Denoise's result and an export of it must be
    /// made from. `None` when not known.
    pub(in crate::app) image_file: Option<crate::storage::Identity>,
    /// AI Denoise's result for the decoded image and what the Detail panel says of it.
    pub(in crate::app) ai: crate::app::develop::ai_denoise::Photo,
    /// The Auto estimate for this photo; dropping it with the document cancels it.
    pub(in crate::app) auto: crate::app::task::Task,
    /// Point Color's dropper sampling the photo, off the UI thread.
    pub(in crate::app) point_color_pick: crate::app::task::Task,
    /// The Targeted Adjustment Tool sampling the photo where a drag started.
    pub(in crate::app) targeted_pick: crate::app::task::Task,
    /// What the running estimate measures: the recipe without the settings Auto sets.
    pub(in crate::app) auto_input: Option<Recipe>,
    /// The recipe as Auto last left it; while it is unchanged, Auto has nothing to do.
    pub(in crate::app) auto_applied: Option<Recipe>,
    /// The recipe [`Editor::auto_in_effect`] last answered for, and its answer, so the
    /// Basic panel does not rebuild what Auto measures on every frame.
    ///
    /// [`Editor::auto_in_effect`]: crate::app::Editor::auto_in_effect
    pub(in crate::app) auto_effect: std::cell::RefCell<Option<(Recipe, bool)>>,
    /// The Transform panel's Upright analysis for this photo.
    pub(in crate::app) upright: crate::app::task::Task,
    /// The Crop panel's Auto straighten analysis for this photo.
    pub(in crate::app) straighten: crate::app::task::Task,
    /// Profile → Fit Camera Look for this photo.
    pub(in crate::app) camera_look: crate::app::task::Task,
    /// The camera thumbnail view's image for this photo, or why there is none, once read.
    pub(in crate::app) camera_thumbnail:
        Option<Result<crate::app::develop::camera_thumbnail::Thumbnail, String>>,
    pub(in crate::app) camera_thumbnail_task: crate::app::task::Task,
}

/// Where a photo's edit in Develop started from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum EditOrigin {
    /// No edit: the raw defaults, which follow Preferences until it is edited.
    #[default]
    Defaults,
    /// The catalog's RAWmakase edit.
    Saved,
    /// The photo's Lightroom edit, converted from Adobe Default.
    Lightroom,
}

/// What the latest render showed: the whole photo, or a 1:1 region of it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(in crate::app) enum TextureMode {
    #[default]
    Whole,
    Region([u32; 4]),
}
/// A texture the viewport draws: uploaded through egui, or presented by the GPU
/// renderer into a texture it registered.
#[derive(Clone)]
pub(in crate::app) struct Picture {
    id: egui::TextureId,
    size: [usize; 2],
    /// Keeps an uploaded texture alive; presented ones belong to the renderer.
    handle: Option<egui::TextureHandle>,
}
impl Picture {
    pub(in crate::app) fn presented(id: egui::TextureId, size: [usize; 2]) -> Self {
        Self {
            id,
            size,
            handle: None,
        }
    }
    pub(in crate::app) fn id(&self) -> egui::TextureId {
        self.id
    }
    /// Presented into a texture the renderer registered, rather than uploaded.
    fn is_presented(&self) -> bool {
        self.handle.is_none()
    }
    pub(in crate::app) fn size_vec2(&self) -> Vec2 {
        Vec2::new(self.size[0] as f32, self.size[1] as f32)
    }
    /// Shows `image` in `slot`, reusing its uploaded texture when it has one.
    pub(in crate::app) fn upload(
        slot: &mut Option<Picture>,
        ctx: &egui::Context,
        name: &str,
        image: egui::ColorImage,
    ) {
        let size = image.size;
        match slot.as_mut().and_then(|p| p.handle.clone()) {
            Some(mut handle) => {
                handle.set(image, egui::TextureOptions::LINEAR);
                *slot = Some(handle.into());
            }
            None => {
                *slot = Some(
                    ctx.load_texture(name, image, egui::TextureOptions::LINEAR)
                        .into(),
                )
            }
        }
        debug_assert_eq!(slot.as_ref().map(|p| p.size), Some(size));
    }
}
impl From<egui::TextureHandle> for Picture {
    fn from(handle: egui::TextureHandle) -> Self {
        Self {
            id: handle.id(),
            size: handle.size(),
            handle: Some(handle),
        }
    }
}
/// What a render was asked for, which its result is shown with.
#[derive(Clone)]
pub(in crate::app) struct Pending {
    /// What the pixels are rendered with, Visualize Range included, so a picker
    /// never takes a gray preview for the photo.
    pub(in crate::app) recipe: Option<Recipe>,
    pub(in crate::app) mode: TextureMode,
    pub(in crate::app) crop: [f32; 4],
    /// The AI Denoise result the render was given.
    pub(in crate::app) ai: RenderedAi,
}
impl Default for Pending {
    fn default() -> Self {
        Self {
            recipe: None,
            mode: TextureMode::Whole,
            crop: [0., 0., 1., 1.],
            ai: RenderedAi::None,
        }
    }
}
/// The AI Denoise result a render was given: none, the tiles done so far, or the
/// final one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum RenderedAi {
    None,
    Partial,
    Complete,
}
/// The document, photo, region and size a render shows.
pub(in crate::app) struct RenderView {
    pub(in crate::app) image: std::sync::Weak<CameraImage>,
    pub(in crate::app) path: Option<PathBuf>,
    pub(in crate::app) photo: Option<PhotoId>,
    pub(in crate::app) region: Option<[u32; 4]>,
    pub(in crate::app) max_edge: u32,
}
impl RenderView {
    pub(in crate::app) fn same(&self, other: &Self) -> bool {
        self.same_photo(other) && self.region == other.region
    }
    /// The same view panned at 100% or more: only the region it shows moved.
    pub(in crate::app) fn panned(&self, other: &Self) -> bool {
        self.same_photo(other) && self.region.is_some() && other.region.is_some()
    }
    fn same_photo(&self, other: &Self) -> bool {
        self.image.ptr_eq(&other.image)
            && self.path == other.path
            && self.photo == other.photo
            && self.max_edge == other.max_edge
    }
}
pub(in crate::app) struct PreviewState {
    pub(in crate::app) task: crate::app::task::Task,
    /// The last whole-photo render, always drawn so zooming never shows a gap.
    pub(in crate::app) texture: Option<Picture>,
    /// The last 100% region render, drawn over `texture` while `mode` is a region.
    pub(in crate::app) region: Option<Picture>,
    /// Small copy of the last whole-photo render for the minimap.
    pub(in crate::app) navigator: Option<Picture>,
    pub(in crate::app) histogram: crate::rendered::Histogram,
    /// The shown pixels of `texture` and `region` while the white balance selector
    /// is active, for its loupe.
    pub(in crate::app) samples: Option<image::RgbImage>,
    pub(in crate::app) region_samples: Option<image::RgbImage>,
    /// Whether renders keep their samples: while a loupe or the readout reads them.
    pub(in crate::app) samples_requested: bool,
    /// The recipe the shown samples were rendered with.
    pub(in crate::app) samples_recipe: Option<Recipe>,
    /// What the latest render was asked for, applied to its result when it lands.
    pub(in crate::app) pending: Pending,
    /// Earlier renders an edit let finish (see `Task::supersede`), oldest first: those
    /// the renderer took, as one it replaced before taking it never reports
    /// (`replaced`).
    pub(in crate::app) overtaken: std::collections::VecDeque<(u64, Pending)>,
    /// The render whose result is shown, so an earlier one landing late is not.
    pub(in crate::app) shown: u64,
    /// What the latest render shows: an edit lets the running render finish only
    /// when it is still the same.
    pub(in crate::app) last_view: Option<RenderView>,
    pub(in crate::app) status: String,
    pub(in crate::app) last_fit_edge: u32,
    pub(in crate::app) last_region: Option<[u32; 4]>,
    pub(in crate::app) mode: TextureMode,
    /// The crop `texture` was rendered with, when known: until a render for a new
    /// crop lands, the old one is placed where its crop sits instead of stretched.
    pub(in crate::app) crop: Option<[f32; 4]>,
    /// Before's render, while it shows beside the edit.
    pub(in crate::app) before: crate::app::develop::before_after::BeforePreview,
    /// The magnifier's render, while it is shown.
    pub(in crate::app) magnifier: crate::app::view::magnifier::MagnifierPreview,
    /// The photo's stored Standard preview, shown while it opens until the
    /// first live render; nothing else reads it.
    pub(in crate::app) stand_in: Option<Picture>,
    /// `texture` is the camera's embedded JPEG, not a render.
    pub(in crate::app) embedded: bool,
}
impl Default for PreviewState {
    fn default() -> Self {
        Self {
            task: Default::default(),
            texture: None,
            region: None,
            navigator: None,
            histogram: crate::rendered::Histogram::EMPTY,
            samples: None,
            region_samples: None,
            samples_requested: false,
            samples_recipe: None,
            pending: Pending::default(),
            overtaken: Default::default(),
            shown: 0,
            last_view: None,
            status: String::new(),
            last_fit_edge: 0,
            last_region: None,
            mode: TextureMode::Whole,
            crop: None,
            before: Default::default(),
            magnifier: Default::default(),
            stand_in: None,
            embedded: false,
        }
    }
}

/// The tool that owns clicks and drags on the photo, as in Lightroom's tool strip.
/// Only one is active; activating one closes the others.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum Tool {
    #[default]
    None,
    Crop,
    WhiteBalance,
    /// Defringe's Fringe Color Selector.
    Defringe,
    /// Point Color's dropper, which adds a swatch.
    PointColor,
    /// Spot removal: Heal and Clone.
    Remove,
    /// Red Eye Correction.
    RedEye,
    Mask,
    /// The Transform panel's Guided Upright tool.
    Guided,
    /// The Targeted Adjustment Tool of the Tone Curve, the Color Mixer or B&W.
    Targeted(crate::develop::targeted::Target),
}
/// The Color Mixer's tabs, as in Lightroom.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum MixerTab {
    #[default]
    Mixer,
    PointColor,
}
/// Point Color's panel: the selected swatch and the view options.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) struct PointColorView {
    pub(in crate::app) selected: Option<usize>,
    /// Visualize Range: the selected swatch's selection in color, the rest gray.
    pub(in crate::app) visualize: bool,
    /// The Hue, Saturation and Luminance range controls are shown.
    pub(in crate::app) ranges: bool,
}
pub(in crate::app) struct ViewState {
    /// Fit or a zoom level, and where; Develop's and the Library's.
    pub(in crate::app) zoom: crate::app::view::navigator::Zoom,
    /// Aperture's Loupe over the fitted photo in Develop.
    pub(in crate::app) magnifier: crate::app::view::magnifier::Magnifier,
    /// The corner the minimap sits in while zoomed in.
    pub(in crate::app) minimap: crate::app::view::minimap::Minimap,
    /// The size, in pixels, of the area the edit is shown in: the viewport, or its
    /// half beside Before.
    pub(in crate::app) viewport: Vec2,
    /// Before alone or beside the edit.
    pub(in crate::app) compare: crate::app::develop::before_after::Compare,
    /// The histogram's clipping warnings.
    pub(in crate::app) clipping: crate::app::develop::clipping::ClippingView,
    /// The RGB values under the pointer, shown under the histogram.
    pub(in crate::app) readout: crate::app::develop::readout::Readout,
    /// A drag in the histogram in progress.
    pub(in crate::app) tone_drag: Option<crate::app::develop::panels::tone_drag::ToneDrag>,
    pub(in crate::app) tool: Tool,
    /// A drag on the crop, from the press to the release.
    pub(in crate::app) crop_drag: Option<crate::app::develop::tools::crop_drag::CropDrag>,
    /// The aspect this photo's crop is locked to, in the Crop menu's values: the
    /// photo's own ratio (negative), a preset or Custom ratio (positive; its
    /// reciprocal after X), or 0 for unlocked (Free). It is 0 exactly when Free is
    /// the chosen aspect. Gestures ask [`Editor::crop_constraint`], not this.
    ///
    /// [`Editor::crop_constraint`]: crate::app::Editor::crop_constraint
    pub(in crate::app) aspect: f32,
    /// Whether `aspect` was read from this photo's crop since the Crop tool opened.
    pub(in crate::app) aspect_read: bool,
    /// The photo was loaded again with the Crop tool open: its aspect is read from
    /// the edit loaded, which an uncropped photo keeps rather than taking the
    /// chosen aspect.
    pub(in crate::app) aspect_reloaded: bool,
    /// The aspect last chosen from the Crop tool's menu, which an uncropped photo
    /// opens with; saved in the session. Free (0) leaves every photo's aspect
    /// unlocked; any other choice locks it.
    pub(in crate::app) chosen_aspect: f32,
    /// The crop guide overlay; saved in the session.
    pub(in crate::app) crop_guides: crate::app::develop::tools::crop_tool::CropGuides,
    /// When the overlay was last changed; it shows for a moment after, even in Auto.
    pub(in crate::app) crop_guides_changed: Option<std::time::Instant>,
    /// The Crop tool's Straighten ruler.
    pub(in crate::app) ruler: crate::app::develop::tools::crop_tool::Ruler,
    /// Spot removal settings, selection and drag in progress.
    pub(in crate::app) retouch: crate::app::develop::tools::retouch_tool::RetouchTool,
    /// Red Eye Correction's selection, last size and drag in progress.
    pub(in crate::app) red_eye: crate::app::develop::tools::red_eye_tool::RedEyeTool,
    /// A wheel scroll sizing a brush, recorded as one History step when it pauses.
    pub(in crate::app) wheel: crate::app::develop::tools::brush_scroll::WheelGesture,
    /// Masking panel state.
    pub(in crate::app) masking: crate::app::develop::tools::mask_tool::MaskTool,
    /// The Guided Upright tool's selection, drag and view options.
    pub(in crate::app) guided: crate::app::develop::tools::guided_tool::GuidedTool,
    pub(in crate::app) monitor: crate::app::preferences::monitor::MonitorProfile,
    /// The display profile the system assigns to the window's display.
    pub(in crate::app) system_display: crate::app::preferences::monitor::SystemProbe,
    /// What the window's display can show of HDR now: a view fact, never part of an
    /// edit, read each frame.
    pub(in crate::app) hdr_display: crate::app::shell::display::HdrDisplay,
    /// Whether this frame's photo is due as its HDR master, drawn or still
    /// rendering, which asks the compositor for EDR.
    pub(in crate::app) hdr_shown: bool,
    pub(in crate::app) selected_band: usize,
    /// The Color Grading panel's view: 3-Way or one wheel.
    pub(in crate::app) grading: crate::app::develop::panels::color_grading::GradingView,
    pub(in crate::app) selected_curve: usize,
    pub(in crate::app) parametric_curve: bool,
    pub(in crate::app) mixer_color: bool,
    pub(in crate::app) mixer_adjust: usize,
    /// The Color Mixer's tab: the mixer or Point Color.
    pub(in crate::app) mixer_tab: MixerTab,
    pub(in crate::app) point_color: PointColorView,
    /// A Targeted Adjustment Tool drag in progress.
    pub(in crate::app) targeted: Option<crate::app::develop::tools::targeted_tool::TargetDrag>,
    pub(in crate::app) shortcuts: bool,
    /// The Basic slider the `.` and `,` keys selected for `+` and `-`, named as on
    /// a RAW file (see `basic_keys`); kept across photos and modules.
    pub(in crate::app) selected_setting: Option<crate::model::params::ParameterId>,
    pub(in crate::app) zoom_key: (bool, f32),
    pub(in crate::app) zoom_anim: Option<(f64, egui::Rect)>,
    pub(in crate::app) shown_rect: Option<egui::Rect>,
    /// Where a minimap click or drag is moving the view.
    pub(in crate::app) glide: Option<crate::app::view::navigator::Glide>,
}
impl Default for ViewState {
    fn default() -> Self {
        Self {
            zoom: Default::default(),
            magnifier: Default::default(),
            minimap: Default::default(),
            viewport: Vec2::ZERO,
            compare: Default::default(),
            clipping: Default::default(),
            readout: Default::default(),
            tone_drag: None,
            tool: Tool::None,
            crop_drag: None,
            aspect: -1.,
            aspect_read: false,
            aspect_reloaded: false,
            chosen_aspect: -1.,
            crop_guides: Default::default(),
            crop_guides_changed: None,
            ruler: Default::default(),
            retouch: Default::default(),
            red_eye: Default::default(),
            wheel: Default::default(),
            masking: Default::default(),
            guided: Default::default(),
            monitor: Default::default(),
            system_display: Default::default(),
            hdr_display: Default::default(),
            hdr_shown: false,
            selected_band: 0,
            grading: Default::default(),
            selected_curve: 0,
            parametric_curve: false,
            mixer_color: false,
            mixer_adjust: 0,
            mixer_tab: MixerTab::Mixer,
            point_color: Default::default(),
            targeted: None,
            shortcuts: false,
            selected_setting: None,
            zoom_key: (false, 1.),
            zoom_anim: None,
            shown_rect: None,
            glide: None,
        }
    }
}

#[derive(Default)]
pub(in crate::app) struct PresetBrowser {
    pub(in crate::app) library: Arc<crate::presets::Library>,
    pub(in crate::app) issues: Vec<Option<String>>,
    /// Per preset: the profile it names and the one it renders with instead.
    pub(in crate::app) substitutes: Vec<Option<(String, String)>>,
    pub(in crate::app) filter: String,
    /// The search in the Library's Saved Preset chooser.
    pub(in crate::app) chooser_filter: String,
    pub(in crate::app) favorites: std::collections::BTreeSet<String>,
    pub(in crate::app) compatible_only: bool,
    pub(in crate::app) favorites_only: bool,
    pub(in crate::app) selected: String,
    pub(in crate::app) preview: Option<Recipe>,
    pub(in crate::app) hover: Option<(usize, Instant)>,
    /// The list as last shown, kept until what it is built from changes.
    pub(in crate::app) list: Option<crate::app::presets::PresetList>,
    /// Counts changes to `favorites` and `issues`, which the list depends on.
    pub(in crate::app) revision: u64,
    /// Numbers library scans, so only the latest one is shown.
    pub(in crate::app) scans: u64,
    /// Whether a scan has finished since the app started, so `library` lists
    /// every preset rather than none yet.
    pub(in crate::app) scanned: bool,
    /// The Amount of the preset just applied, while nothing else has changed.
    pub(in crate::app) amount: Option<crate::app::presets::AmountSession>,
}

impl PresetBrowser {
    /// Starts counting a new library scan.
    pub(in crate::app) fn next_scan(&mut self) -> u64 {
        self.scans += 1;
        self.scans
    }
    /// Whether `scan` is the latest one started.
    pub(in crate::app) fn is_latest(&self, scan: u64) -> bool {
        scan == self.scans
    }
}

impl Document {
    /// Follows the open photo's file to `path`, where a synchronization
    /// found it moved and verified, keeping the edit, its History and
    /// whether it still needs saving; a save that failed on the old path is
    /// tried again on the new one.
    pub(in crate::app) fn relink(&mut self, path: PathBuf) {
        // Opened from its editable preview with its original still not there
        // (its folder moved, say), the recorded identity stays what saves are
        // checked against.
        let recorded = self
            .from
            .editable_preview()
            .filter(|_| crate::edits::Original::is_absent(&path))
            .map(|preview| preview.original.clone());
        self.file = crate::storage::Identity::read(&path).ok().or(recorded);
        self.path = Some(path);
        self.edit.save_state_mut().retry_now();
    }
    /// Stands in for opening the file at `path`: its path and its identity as
    /// read now, which saves are checked against.
    #[cfg(test)]
    pub(in crate::app) fn opened_at(&mut self, path: PathBuf) {
        self.file = crate::storage::Identity::read(&path).ok();
        self.path = Some(path);
    }
    /// Opens the document of catalog photo `photo` (or of a file outside the catalog,
    /// with none) and starts its load: what earlier loads bring no longer counts.
    pub(in crate::app) fn open(
        &mut self,
        photo: Option<PhotoId>,
    ) -> (u64, Arc<std::sync::atomic::AtomicBool>) {
        self.replace(photo);
        self.load.start()
    }
    /// Closes the document: its load no longer counts, and no photo is open.
    pub(in crate::app) fn close(&mut self) {
        self.load.invalidate();
        self.replace(None);
    }
    /// What the load under way brings no longer counts; the document stays, its edit,
    /// History and save state with it (to save it on quitting, or to open it again where
    /// its file moved).
    pub(in crate::app) fn invalidate_load(&mut self) {
        self.load.invalidate();
    }
    /// The latest load's id, which its results carry.
    pub(in crate::app) fn load_id(&self) -> u64 {
        self.load.id()
    }
    /// Whether the latest load is still under way.
    pub(in crate::app) fn loading(&self) -> bool {
        self.load.is_running()
    }
    /// Whether a result of load `id` still counts.
    #[cfg(test)]
    pub(in crate::app) fn load_counts(&self, id: u64) -> bool {
        self.load.counts(id)
    }
    /// Load `id` is done (its first image arrived, or it failed); a later one's results
    /// still count.
    pub(in crate::app) fn finish_load(&mut self, id: u64) {
        self.load.finish(id);
    }
    /// A fresh document for `photo`, its load kept, so the next one's id follows on.
    fn replace(&mut self, catalog_photo: Option<PhotoId>) {
        let load = std::mem::take(&mut self.load);
        *self = Self {
            catalog_photo,
            load,
            ..Default::default()
        };
    }
    /// For tests: a fresh document for `catalog_photo`, with no load started.
    #[cfg(test)]
    pub(crate) fn reset(&mut self, catalog_photo: Option<PhotoId>) {
        self.replace(catalog_photo);
    }
    /// For tests: starts a load for the document as it is.
    #[cfg(test)]
    pub(in crate::app) fn start_load(&mut self) -> (u64, Arc<std::sync::atomic::AtomicBool>) {
        self.load.start()
    }
}
impl PreviewState {
    /// Starts the render of a change, cancelling the running one; with `edit`, an
    /// edit of the same view, lets it finish and show on the way.
    pub(in crate::app) fn start(
        &mut self,
        edit: bool,
        pending: Pending,
    ) -> (u64, Arc<std::sync::atomic::AtomicBool>) {
        if edit && self.task.is_running() {
            let earlier = std::mem::replace(&mut self.pending, pending);
            self.overtaken.push_back((self.task.id(), earlier));
            self.task.supersede()
        } else {
            self.pending = pending;
            self.overtaken.clear();
            self.task.start()
        }
    }
    /// What render `id` was asked for, if its result is still to show: the latest
    /// render's, or an earlier one an edit let finish and not older than the shown.
    pub(in crate::app) fn request(&self, id: u64) -> Option<Pending> {
        if !self.task.counts(id) || id < self.shown {
            None
        } else if id == self.task.id() {
            Some(self.pending.clone())
        } else {
            self.overtaken
                .iter()
                .find(|(i, _)| *i == id)
                .map(|(_, p)| p.clone())
        }
    }
    /// Forgets render `id`, which the renderer replaced before taking it.
    pub(in crate::app) fn replaced(&mut self, id: u64) {
        self.overtaken.retain(|(i, _)| *i != id);
    }
    /// Notes that render `id`'s result is shown: earlier ones no longer are.
    pub(in crate::app) fn showing(&mut self, id: u64) {
        self.shown = id;
        self.overtaken.retain(|(i, _)| *i >= id);
    }
    pub(crate) fn clear_document(&mut self) {
        self.task.invalidate();
        self.texture = None;
        self.region = None;
        self.navigator = None;
        self.histogram = crate::rendered::Histogram::EMPTY;
        self.status.clear();
        self.last_fit_edge = 0;
        self.last_region = None;
        self.mode = TextureMode::Whole;
        self.crop = None;
        self.before.clear();
        self.magnifier.clear();
        // The pixels the readout and loupes read belong to the photo left.
        self.samples = None;
        self.region_samples = None;
        self.samples_recipe = None;
        self.samples_requested = false;
        self.stand_in = None;
        self.embedded = false;
    }
    /// Starts keeping the shown pixels, and whether a render must be asked for
    /// them: not when those kept from an earlier hover still match what is shown,
    /// with no render landed or on its way since.
    pub(in crate::app) fn ask_for_samples(&mut self) -> bool {
        if self.samples_requested {
            return false;
        }
        self.samples_requested = true;
        // A 100% region's render keeps only the region's pixels.
        let kept = match self.mode {
            TextureMode::Whole => self.samples.is_some(),
            TextureMode::Region(_) => self.region_samples.is_some(),
        };
        !kept || self.task.is_running()
    }
    /// Stops keeping the pixels of later renders. Those shown stay until one lands,
    /// so pointing back at an unchanged photo needs no render.
    pub(in crate::app) fn stop_asking_for_samples(&mut self) {
        self.samples_requested = false;
    }
    /// Keeps a landed render's pixels while they are asked for. Otherwise drops
    /// all kept ones, the photo's and its region's, which no longer match it.
    pub(in crate::app) fn keep_samples(&mut self, region: bool, samples: Option<image::RgbImage>) {
        if !self.samples_requested {
            self.samples = None;
            self.region_samples = None;
        } else if region {
            self.region_samples = samples;
        } else {
            self.samples = samples;
        }
    }
    /// Whether a live render of the photo is shown.
    pub(crate) fn live(&self) -> bool {
        self.texture.is_some() && !self.embedded || self.region.is_some()
    }
    /// The stored preview is what shows: there is one, and no live render yet.
    pub(crate) fn standing_in(&self) -> bool {
        self.stand_in.is_some() && !self.live()
    }
    /// Textures the renderer presented into that the viewport draws.
    pub(crate) fn presented(&self) -> Vec<egui::TextureId> {
        [
            &self.texture,
            &self.region,
            &self.navigator,
            &self.before.texture,
            &self.before.region,
            &self.magnifier.texture,
        ]
        .into_iter()
        .flatten()
        .filter(|p| p.is_presented())
        .map(Picture::id)
        .collect()
    }
    /// Stops drawing textures the renderer presented into, once it has freed them.
    /// Not rendering again at once: a render that keeps failing would repeat.
    pub(crate) fn forget_presented(&mut self) {
        let [before, before_region] = self.before.pictures();
        for slot in [
            &mut self.texture,
            &mut self.region,
            &mut self.navigator,
            before,
            before_region,
            &mut self.magnifier.texture,
        ] {
            if slot.as_ref().is_some_and(Picture::is_presented) {
                *slot = None;
            }
        }
    }
}
impl ViewState {
    /// Moves the view on to where the minimap points, once a frame before the
    /// photo is drawn; see [`Glide`](crate::app::view::navigator::Glide).
    pub(in crate::app) fn glide_pan(&mut self, ctx: &egui::Context, photo: Option<PhotoId>) {
        let Some(glide) = &mut self.glide else {
            return;
        };
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        if glide.step(&mut self.zoom, photo, dt) {
            ctx.request_repaint();
        } else {
            self.glide = None;
        }
    }
    /// Sends the view of `photo` to `at` on the minimap, zoomed in.
    pub(in crate::app) fn glide_to(&mut self, at: [f32; 2], photo: Option<PhotoId>) {
        self.zoom.on = true;
        self.glide = Some(crate::app::view::navigator::Glide::to(
            at,
            self.zoom.pan,
            photo,
        ));
    }
    /// The ICC profile renders convert to for the monitor, captured by each job.
    pub(in crate::app) fn display_profile(&self) -> Option<PathBuf> {
        self.monitor
            .transform(self.system_display.display())
            .map(Into::into)
    }
    pub(crate) fn is(&self, tool: Tool) -> bool {
        self.tool == tool
    }
    /// An eyedropper is active: White Balance, the Fringe Color Selector or Point
    /// Color's dropper.
    pub(crate) fn picks_color(&self) -> bool {
        matches!(
            self.tool,
            Tool::WhiteBalance | Tool::Defringe | Tool::PointColor
        )
    }
    /// What the eyedropper's loupe asks for.
    pub(crate) fn loupe_prompt(&self) -> &'static str {
        match self.tool {
            Tool::Defringe => "Pick a purple or green fringe",
            Tool::PointColor => "Pick a color to adjust",
            _ => "Pick a target neutral",
        }
    }
    /// Opens `tool`, or closes it when it is already open.
    pub(crate) fn toggle(&mut self, tool: Tool) {
        self.tool = if self.tool == tool { Tool::None } else { tool };
        if matches!(self.tool, Tool::Crop) {
            self.zoom.on = false;
            self.aspect_read = false;
            self.aspect_reloaded = false;
        }
        self.ruler = Default::default();
        self.guided.drag = None;
    }
    /// Loads a photo in Develop: as `clear_document`, but the Crop tool stays open,
    /// as Lightroom's does from photo to photo. `another` photo's aspect is read as
    /// if the tool were opened on it; the same photo loaded again (after a Sync's
    /// Undo, say) reads the edit it loads, not cropped to the chosen aspect.
    pub(crate) fn next_document(&mut self, another: bool) {
        let crop = self.is(Tool::Crop);
        self.clear_document();
        if crop {
            self.tool = Tool::Crop;
            self.aspect_read = false;
            self.aspect_reloaded = !another;
        }
    }
    pub(crate) fn clear_document(&mut self) {
        self.zoom.on = false;
        self.zoom_anim = None;
        self.glide = None;
        self.shown_rect = None;
        self.tool = Tool::None;
        self.point_color.selected = None;
        self.targeted = None;
        self.crop_drag = None;
        self.ruler = Default::default();
        // Ends a histogram drag: the next photo starts from its own values.
        self.tone_drag = None;
        self.retouch.clear_document();
        self.red_eye.clear_document();
        self.masking.clear_document();
        self.guided.clear_document();
        // The next photo's values come with its first render.
        self.readout.values = None;
        // Before alone is left with the photo; Before beside the edit stays, as
        // Lightroom keeps its Before/After view from photo to photo.
        if self.compare.before_only() {
            self.compare = Default::default();
        }
    }
}
impl PresetBrowser {
    pub(crate) fn clear_document(&mut self) {
        self.revision += 1;
        self.issues.clear();
        self.substitutes.clear();
        self.selected.clear();
        self.preview = None;
        self.hover = None;
        self.amount = None;
    }
}

impl Document {
    /// What the open photo's edit is saved and loaded against: its file at `path`,
    /// or, opened from its editable preview, the identity that recorded, standing
    /// in for the file while it is not there.
    pub(in crate::app) fn original<'a>(
        &'a self,
        path: &'a std::path::Path,
    ) -> crate::edits::Original<'a> {
        match self.from.editable_preview() {
            Some(source) => crate::edits::Original::recorded(path, &source.original),
            None => crate::edits::Original::file(path),
        }
    }
    /// Whether the photo was opened from its editable preview.
    pub(in crate::app) fn opened_from_preview(&self) -> bool {
        self.from.is_final_reduced()
    }
    pub(crate) fn full(&self) -> Option<&Arc<CameraImage>> {
        self.image.as_ref()
    }
    /// What applying XMP or Lightroom settings measures for Auto, on the full-size
    /// image when it is decoded.
    pub(crate) fn measures(&self) -> Option<crate::develop::Measures<'_>> {
        self.full()
            .map(|image| crate::develop::Measures(image.as_ref()))
    }
    pub(crate) fn set_image(&mut self, full: Arc<CameraImage>) {
        self.image = Some(full);
        self.color_spread = Default::default();
        // A result belongs to the image it was made for, as does the file it came from.
        self.ai = Default::default();
        self.image_file = None;
    }
    /// The AI Denoise source a render of `image` with `setting` uses: the latest result,
    /// partial or final, when the setting renders AI Denoise and `image` is this
    /// document's own (never another photo's, such as Reference View's).
    pub(crate) fn ai_source(
        &self,
        image: &Arc<CameraImage>,
        setting: &crate::model::ai_denoise::AiDenoise,
    ) -> Option<crate::develop::AiSource> {
        let own = self.image.as_ref().is_some_and(|i| Arc::ptr_eq(i, image));
        (own && setting.renders())
            .then(|| self.ai.source().cloned())
            .flatten()
    }
    /// How the decoded photo's colors spread, once it is decoded.
    pub(in crate::app) fn color_spread(&self) -> Option<crate::develop::ColorSpread> {
        self.colors().spread()
    }
    /// The recipe to edit, and what the photo's colors are measured from, borrowed
    /// apart so a panel can measure them only when it needs them.
    pub(in crate::app) fn recipe_and_colors(&mut self) -> (&mut Recipe, PhotoColorSource<'_>) {
        (
            self.edit.recipe_mut(),
            PhotoColorSource {
                image: self.image.as_ref(),
                spread: &self.color_spread,
                metadata: self.metadata.as_ref(),
            },
        )
    }
    fn colors(&self) -> PhotoColorSource<'_> {
        PhotoColorSource {
            image: self.image.as_ref(),
            spread: &self.color_spread,
            metadata: self.metadata.as_ref(),
        }
    }
}

/// A Treatment asked for while the photo decodes, and the recipe it was asked for:
/// once the recipe changes otherwise (Reset, Undo, a preset), the request lapses.
#[derive(Clone)]
pub(in crate::app) struct PendingTreatment {
    pub(in crate::app) treatment: crate::model::recipe::Treatment,
    pub(in crate::app) recipe: Recipe,
}

/// The decoded photo and its measured colors, for Auto black & white.
#[derive(Clone, Copy)]
pub(in crate::app) struct PhotoColorSource<'a> {
    image: Option<&'a Arc<CameraImage>>,
    spread: &'a std::cell::OnceCell<crate::develop::ColorSpread>,
    pub(in crate::app) metadata: Option<&'a Metadata>,
}
impl PhotoColorSource<'_> {
    /// How the photo's colors spread, measured on first use once it is decoded.
    pub(in crate::app) fn spread(&self) -> Option<crate::develop::ColorSpread> {
        let im = self.image?;
        Some(
            *self
                .spread
                .get_or_init(|| crate::develop::ColorSpread::measure(im)),
        )
    }
}
