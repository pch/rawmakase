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
pub(super) struct Document {
    /// The settings, their History and whether they still need saving.
    pub(super) edit: crate::edit_session::EditSession,
    pub(super) path: Option<PathBuf>,
    pub(super) metadata: Option<Metadata>,
    image: Option<Arc<CameraImage>>,
    /// How the decoded photo's colors spread, for Auto black & white; measured on
    /// first use.
    color_spread: std::cell::OnceCell<crate::develop::ColorSpread>,
    /// A conversion to black & white waiting for the photo to decode, for its
    /// Auto mix.
    pub(super) pending_treatment: Option<PendingTreatment>,
    pub(super) export: ExportOptions,
    pub(super) catalog_photo: Option<PhotoId>,
    pub(super) lightroom_notice: String,
    /// Lightroom's history for the open catalog photo, oldest first.
    pub(super) lightroom_history: Vec<crate::catalog::HistoryStep>,
    /// What Before shows when not the photo's starting settings: a History step,
    /// snapshot or the edit copied to it. Not saved, as in Lightroom.
    pub(super) before: Option<Recipe>,
    /// The open catalog photo's Snapshots.
    pub(super) snapshots: super::snapshots::Snapshots,
    /// The photo's Lightroom settings, as read with its edit, to apply once its
    /// profiles arrive.
    pub(super) pending_lightroom: Option<String>,
    /// The photo's file as it was when it was opened: what an export of the edit
    /// shown checks it still has.
    pub(super) file: Option<crate::storage::Identity>,
    /// Where the edit on screen started from.
    pub(super) origin: EditOrigin,
    /// The raw defaults for this photo, once its profiles are known: what Reset
    /// returns to and Before shows.
    pub(super) defaults: Option<crate::raw_defaults::Resolved>,
    pub(super) profiles: Vec<Arc<crate::camera_profiles::CameraProfile>>,
    pub(super) profile_errors: Vec<String>,
    /// The Auto estimate for this photo; dropping it with the document cancels it.
    pub(super) auto: super::task::Task,
    /// Point Color's dropper sampling the photo, off the UI thread.
    pub(super) point_color_pick: super::task::Task,
    /// The Targeted Adjustment Tool sampling the photo where a drag started.
    pub(super) targeted_pick: super::task::Task,
    /// What the running estimate measures: the recipe without the settings Auto sets.
    pub(super) auto_input: Option<Recipe>,
    /// The recipe as Auto last left it; while it is unchanged, Auto has nothing to do.
    pub(super) auto_applied: Option<Recipe>,
    /// The recipe [`Editor::auto_in_effect`] last answered for, and its answer, so the
    /// Basic panel does not rebuild what Auto measures on every frame.
    ///
    /// [`Editor::auto_in_effect`]: super::Editor::auto_in_effect
    pub(super) auto_effect: std::cell::RefCell<Option<(Recipe, bool)>>,
    /// The Transform panel's Upright analysis for this photo.
    pub(super) upright: super::task::Task,
    /// The Crop panel's Auto straighten analysis for this photo.
    pub(super) straighten: super::task::Task,
}

/// Where a photo's edit in Develop started from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum EditOrigin {
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
pub(super) enum TextureMode {
    #[default]
    Whole,
    Region([u32; 4]),
}
/// A texture the viewport draws: uploaded through egui, or presented by the GPU
/// renderer into a texture it registered.
#[derive(Clone)]
pub(super) struct Picture {
    id: egui::TextureId,
    size: [usize; 2],
    /// Keeps an uploaded texture alive; presented ones belong to the renderer.
    handle: Option<egui::TextureHandle>,
}
impl Picture {
    pub(super) fn presented(id: egui::TextureId, size: [usize; 2]) -> Self {
        Self {
            id,
            size,
            handle: None,
        }
    }
    pub(super) fn id(&self) -> egui::TextureId {
        self.id
    }
    /// Presented into a texture the renderer registered, rather than uploaded.
    fn is_presented(&self) -> bool {
        self.handle.is_none()
    }
    pub(super) fn size_vec2(&self) -> Vec2 {
        Vec2::new(self.size[0] as f32, self.size[1] as f32)
    }
    /// Shows `image` in `slot`, reusing its uploaded texture when it has one.
    pub(super) fn upload(
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
pub(super) struct Pending {
    /// What the pixels are rendered with, Visualize Range included, so a picker
    /// never takes a gray preview for the photo.
    pub(super) recipe: Option<Recipe>,
    pub(super) mode: TextureMode,
    pub(super) crop: [f32; 4],
}
impl Default for Pending {
    fn default() -> Self {
        Self {
            recipe: None,
            mode: TextureMode::Whole,
            crop: [0., 0., 1., 1.],
        }
    }
}
/// The document, photo, region and size a render shows.
pub(super) struct RenderView {
    pub(super) image: std::sync::Weak<CameraImage>,
    pub(super) path: Option<PathBuf>,
    pub(super) photo: Option<PhotoId>,
    pub(super) region: Option<[u32; 4]>,
    pub(super) max_edge: u32,
}
impl RenderView {
    pub(super) fn same(&self, other: &Self) -> bool {
        self.image.ptr_eq(&other.image)
            && self.path == other.path
            && self.photo == other.photo
            && self.region == other.region
            && self.max_edge == other.max_edge
    }
}
/// Earlier renders kept for an edit's overtaken jobs; the renderer keeps one
/// pending job, so few ever report.
const OVERTAKEN: usize = 8;
pub(super) struct PreviewState {
    pub(super) task: super::task::Task,
    /// The last whole-photo render, always drawn so zooming never shows a gap.
    pub(super) texture: Option<Picture>,
    /// The last 100% region render, drawn over `texture` while `mode` is a region.
    pub(super) region: Option<Picture>,
    /// Small copy of the last whole-photo render for the Navigator.
    pub(super) navigator: Option<Picture>,
    pub(super) histogram: crate::rendered::Histogram,
    /// The shown pixels of `texture` and `region` while the white balance selector
    /// is active, for its loupe.
    pub(super) samples: Option<image::RgbImage>,
    pub(super) region_samples: Option<image::RgbImage>,
    /// Whether renders keep their samples: while a loupe or the readout reads them.
    pub(super) samples_requested: bool,
    /// The recipe the shown samples were rendered with.
    pub(super) samples_recipe: Option<Recipe>,
    /// What the latest render was asked for, applied to its result when it lands.
    pub(super) pending: Pending,
    /// Earlier renders an edit let finish (see `Task::supersede`), oldest first.
    pub(super) overtaken: std::collections::VecDeque<(u64, Pending)>,
    /// The render whose result is shown, so an earlier one landing late is not.
    pub(super) shown: u64,
    /// What the latest render shows: an edit lets the running render finish only
    /// when it is still the same.
    pub(super) last_view: Option<RenderView>,
    pub(super) status: String,
    pub(super) last_fit_edge: u32,
    pub(super) last_region: Option<[u32; 4]>,
    pub(super) mode: TextureMode,
    /// The crop `texture` was rendered with, when known: until a render for a new
    /// crop lands, the old one is placed where its crop sits instead of stretched.
    pub(super) crop: Option<[f32; 4]>,
    /// Before's render, while it shows beside the edit.
    pub(super) before: super::before_after::BeforePreview,
    /// The photo's stored Standard preview, shown while it opens until the
    /// first live render; nothing else reads it.
    pub(super) stand_in: Option<Picture>,
    /// `texture` is the camera's embedded JPEG, not a render.
    pub(super) embedded: bool,
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
            stand_in: None,
            embedded: false,
        }
    }
}

/// The tool that owns clicks and drags on the photo, as in Lightroom's tool strip.
/// Only one is active; activating one closes the others.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Tool {
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
pub(super) enum MixerTab {
    #[default]
    Mixer,
    PointColor,
}
/// Point Color's panel: the selected swatch and the view options.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PointColorView {
    pub(super) selected: Option<usize>,
    /// Visualize Range: the selected swatch's selection in color, the rest gray.
    pub(super) visualize: bool,
    /// The Hue, Saturation and Luminance range controls are shown.
    pub(super) ranges: bool,
}
pub(super) struct ViewState {
    /// Fit or a zoom level, and where; Develop's and the Library's.
    pub(super) zoom: super::navigator::Zoom,
    /// The size, in pixels, of the area the edit is shown in: the viewport, or its
    /// half beside Before.
    pub(super) viewport: Vec2,
    /// Before alone or beside the edit.
    pub(super) compare: super::before_after::Compare,
    /// The histogram's clipping warnings.
    pub(super) clipping: super::clipping::ClippingView,
    /// The RGB values under the pointer, shown under the histogram.
    pub(super) readout: super::readout::Readout,
    /// A drag in the histogram in progress.
    pub(super) tone_drag: Option<super::tone_drag::ToneDrag>,
    pub(super) tool: Tool,
    pub(super) crop_drag: Option<([f32; 4], usize)>,
    pub(super) aspect: f32,
    /// Whether `aspect` was read from this photo's crop since the Crop tool opened.
    pub(super) aspect_read: bool,
    /// The crop guide overlay; saved in the session.
    pub(super) crop_guides: super::crop_tool::CropGuides,
    /// When the overlay was last changed; it shows for a moment after, even in Auto.
    pub(super) crop_guides_changed: Option<std::time::Instant>,
    /// The Crop tool's Straighten ruler.
    pub(super) ruler: super::crop_tool::Ruler,
    /// Spot removal settings, selection and drag in progress.
    pub(super) retouch: super::retouch_tool::RetouchTool,
    /// Red Eye Correction's selection, last size and drag in progress.
    pub(super) red_eye: super::red_eye_tool::RedEyeTool,
    /// A wheel scroll sizing a brush, recorded as one History step when it pauses.
    pub(super) wheel: super::brush_scroll::WheelGesture,
    /// Masking panel state.
    pub(super) masking: super::mask_tool::MaskTool,
    /// The Guided Upright tool's selection, drag and view options.
    pub(super) guided: super::guided_tool::GuidedTool,
    pub(super) monitor: Option<PathBuf>,
    pub(super) selected_band: usize,
    /// The Color Grading panel's view: 3-Way or one wheel.
    pub(super) grading: super::color_grading::GradingView,
    pub(super) selected_curve: usize,
    pub(super) parametric_curve: bool,
    pub(super) mixer_color: bool,
    pub(super) mixer_adjust: usize,
    /// The Color Mixer's tab: the mixer or Point Color.
    pub(super) mixer_tab: MixerTab,
    pub(super) point_color: PointColorView,
    /// A Targeted Adjustment Tool drag in progress.
    pub(super) targeted: Option<super::targeted_tool::TargetDrag>,
    pub(super) shortcuts: bool,
    pub(super) zoom_key: (bool, f32),
    pub(super) zoom_anim: Option<(f64, egui::Rect)>,
    pub(super) shown_rect: Option<egui::Rect>,
}
impl Default for ViewState {
    fn default() -> Self {
        Self {
            zoom: Default::default(),
            viewport: Vec2::ZERO,
            compare: Default::default(),
            clipping: Default::default(),
            readout: Default::default(),
            tone_drag: None,
            tool: Tool::None,
            crop_drag: None,
            aspect: -1.,
            aspect_read: false,
            crop_guides: Default::default(),
            crop_guides_changed: None,
            ruler: Default::default(),
            retouch: Default::default(),
            red_eye: Default::default(),
            wheel: Default::default(),
            masking: Default::default(),
            guided: Default::default(),
            monitor: None,
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
            zoom_key: (false, 1.),
            zoom_anim: None,
            shown_rect: None,
        }
    }
}

#[derive(Default)]
pub(super) struct PresetBrowser {
    pub(super) library: Arc<crate::presets::Library>,
    pub(super) issues: Vec<Option<String>>,
    /// Per preset: the profile it names and the one it renders with instead.
    pub(super) substitutes: Vec<Option<(String, String)>>,
    pub(super) filter: String,
    pub(super) favorites: std::collections::BTreeSet<String>,
    pub(super) compatible_only: bool,
    pub(super) favorites_only: bool,
    pub(super) selected: String,
    pub(super) preview: Option<Recipe>,
    pub(super) hover: Option<(usize, Instant)>,
    /// The list as last shown, kept until what it is built from changes.
    pub(super) list: Option<super::presets::PresetList>,
    /// Counts changes to `favorites` and `issues`, which the list depends on.
    pub(super) revision: u64,
    /// Numbers library scans, so only the latest one is shown.
    pub(super) scans: u64,
    /// Whether a scan has finished since the app started, so `library` lists
    /// every preset rather than none yet.
    pub(super) scanned: bool,
    /// The Amount of the preset just applied, while nothing else has changed.
    pub(super) amount: Option<super::presets::AmountSession>,
}

impl PresetBrowser {
    /// Starts counting a new library scan.
    pub(super) fn next_scan(&mut self) -> u64 {
        self.scans += 1;
        self.scans
    }
    /// Whether `scan` is the latest one started.
    pub(super) fn is_latest(&self, scan: u64) -> bool {
        scan == self.scans
    }
}

impl Document {
    pub(crate) fn reset(&mut self, catalog_photo: Option<PhotoId>) {
        *self = Self {
            catalog_photo,
            ..Default::default()
        };
    }
}
impl PreviewState {
    /// Starts the render of a change, cancelling the running one; with `edit`, an
    /// edit of the same view, lets it finish and show on the way.
    pub(super) fn start(
        &mut self,
        edit: bool,
        pending: Pending,
    ) -> (u64, Arc<std::sync::atomic::AtomicBool>) {
        if edit && self.task.is_running() {
            let earlier = std::mem::replace(&mut self.pending, pending);
            self.overtaken.push_back((self.task.id(), earlier));
            if self.overtaken.len() > OVERTAKEN {
                self.overtaken.pop_front();
            }
            self.task.supersede()
        } else {
            self.pending = pending;
            self.overtaken.clear();
            self.task.start()
        }
    }
    /// What render `id` was asked for, if its result is still to show: the latest
    /// render's, or an earlier one an edit let finish and not older than the shown.
    pub(super) fn request(&self, id: u64) -> Option<Pending> {
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
    /// Notes that render `id`'s result is shown: earlier ones no longer are.
    pub(super) fn showing(&mut self, id: u64) {
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
    pub(super) fn ask_for_samples(&mut self) -> bool {
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
    pub(super) fn stop_asking_for_samples(&mut self) {
        self.samples_requested = false;
    }
    /// Keeps a landed render's pixels while they are asked for. Otherwise drops
    /// all kept ones, the photo's and its region's, which no longer match it.
    pub(super) fn keep_samples(&mut self, region: bool, samples: Option<image::RgbImage>) {
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
        ] {
            if slot.as_ref().is_some_and(Picture::is_presented) {
                *slot = None;
            }
        }
    }
}
impl ViewState {
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
        }
        self.ruler = Default::default();
        self.guided.drag = None;
    }
    pub(crate) fn clear_document(&mut self) {
        self.zoom.on = false;
        self.zoom_anim = None;
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
    }
    /// How the decoded photo's colors spread, once it is decoded.
    pub(super) fn color_spread(&self) -> Option<crate::develop::ColorSpread> {
        self.colors().spread()
    }
    /// The recipe to edit, and what the photo's colors are measured from, borrowed
    /// apart so a panel can measure them only when it needs them.
    pub(super) fn recipe_and_colors(&mut self) -> (&mut Recipe, PhotoColorSource<'_>) {
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
pub(super) struct PendingTreatment {
    pub(super) treatment: crate::model::recipe::Treatment,
    pub(super) recipe: Recipe,
}

/// The decoded photo and its measured colors, for Auto black & white.
#[derive(Clone, Copy)]
pub(super) struct PhotoColorSource<'a> {
    image: Option<&'a Arc<CameraImage>>,
    spread: &'a std::cell::OnceCell<crate::develop::ColorSpread>,
    pub(super) metadata: Option<&'a Metadata>,
}
impl PhotoColorSource<'_> {
    /// How the photo's colors spread, measured on first use once it is decoded.
    pub(super) fn spread(&self) -> Option<crate::develop::ColorSpread> {
        let im = self.image?;
        Some(
            *self
                .spread
                .get_or_init(|| crate::develop::ColorSpread::measure(im)),
        )
    }
}
