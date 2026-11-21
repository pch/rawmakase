use crate::model::recipe::Recipe;
use crate::{
    camera_data::{CameraImage, Metadata},
    export_settings::ExportOptions,
    rendered::Rendered,
};
use eframe::egui;
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool, mpsc::Sender},
};

/// A loaded header can be installed as a unit before pixel development finishes.
pub(crate) struct LoadedHeader {
    pub id: u64,
    pub path: PathBuf,
    /// What the photo was opened from: its original, or its editable preview.
    pub from: OpenedFrom,
    /// The file as read before it was opened: what the header, and every image
    /// of this load, are of. For a photo opened from its editable preview, the
    /// original's identity the catalog recorded for the copy.
    pub file: Option<crate::storage::Identity>,
    pub metadata: Metadata,
    pub recipe: Recipe,
    pub export: ExportOptions,
    pub status: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RenderStage {
    Draft,
    Fit,
    Region,
}
impl RenderStage {
    fn label(self) -> &'static str {
        match self {
            Self::Draft => "Draft • refining",
            Self::Fit => "Fit • full quality",
            Self::Region => "100% • full quality",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TaskKind {
    Load,
    Render(Pane),
}
/// Which view of the edit a render is for: the edit itself, the Before beside it
/// in Lightroom's Before/After views, or the magnifier's 100% region over a fitted
/// photo. Each has its own render lane and caches, so an edit renders only the After.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Pane {
    #[default]
    After,
    Before,
    Magnifier,
}

/// What an Auto request sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AutoKind {
    /// The six Tone sliders, as Lightroom's Auto button; white balance is kept.
    Settings,
    /// White balance alone, the WB menu's Auto.
    WhiteBalance,
}
pub(crate) enum Event {
    /// A file dialog or background work of Preferences > Extensions.
    Extensions(Box<crate::app::extensions::Done>),
    /// Merge to HDR's merge, or its import, reported back.
    MergeHdr(Box<crate::app::merge_hdr::Done>),
    /// A verified camera profile was installed into the library, or why not.
    ProfileDownloaded {
        id: u64,
        result: Result<PathBuf, String>,
    },
    DialogClosed,
    /// The system report's probe answered, or stopped (see `app::shell::report_issue`).
    SystemProbed {
        generation: u64,
        system: Box<crate::diagnostics::System>,
    },
    /// A Subject or Background selection finished (or failed).
    Selection(Box<crate::app::develop::tools::subject_mask::Done>),
    /// An extension's mask request has its input rendered, ready to send.
    MaskPrepared(Box<crate::app::extensions::mask_generator::Prepared>),
    /// An AI Remove request's region, rendered into its buffers, or why it
    /// couldn't be.
    RetouchPrepared(Box<crate::app::extensions::retouch_generator::Prepared>),
    /// An AI Remove request ended: its patch kept, or why not.
    AiRemoved(Box<crate::app::develop::tools::ai_remove::Done>),
    /// News of AI Denoise for a photo Develop asked for (`app::develop::ai_denoise`).
    AiDenoise(Box<crate::ai_denoise::Update>),
    /// An AI Denoise result was stored for the photo at this path, by any job.
    AiDenoiseStored(std::path::PathBuf),
    /// AI Denoise's model finished installing, or why not.
    ModelInstalled(Result<(), crate::model_install::InstallError>),
    ModelRemoved(Result<(), String>),
    /// A Lensfun database job finished (`app::develop::lensfun`).
    Lensfun {
        generation: u64,
        job: crate::app::develop::lensfun::Job,
        result: Result<crate::app::develop::lensfun::Done, crate::app::develop::lensfun::Failure>,
    },
    /// A catalog import or open is under way, as a status line.
    CatalogWorking(String),
    CatalogReady(Result<Box<crate::app::library::Library>, String>),
    /// Source keys resolved by the import worker travel with its result.
    FoldersAdded {
        result: Result<Box<crate::app::library::Library>, String>,
        source: Vec<String>,
    },
    /// A Synchronize Folder scan finished, or failed.
    FolderSyncScanned(Box<crate::app::catalog::folder_sync::Scanned>),
    /// Find on Disk's file picked for a photo.
    FileOnDiskPicked(crate::catalog::PhotoId, PathBuf),
    /// Applying a synchronization finished: the catalog opened again.
    FolderSyncApplied(Result<Box<crate::app::catalog::folder_sync::Synced>, String>),
    /// A folder change waiting for the user's answer.
    FolderQuestion(Box<crate::app::catalog::folder_locations::FolderQuestion>),
    /// Export as Catalog finished: what it wrote, nothing when the user
    /// chose no file, or why it failed.
    CatalogExported(Box<Result<Option<crate::app::catalog::export_catalog::Exported>, String>>),
    /// What export file names need from the photos' files, for the read
    /// `request` (see `app::export::naming::read`).
    NamesRead {
        request: u64,
        read: Result<crate::app::export::naming::read::Reads, String>,
    },
    /// Rename Photo(s)' photos in the order they are numbered in, for the
    /// catalog they were read from.
    /// The cancel flag it was read with tells it from a later one.
    RenameOrdered(
        crate::catalog::CatalogLocation,
        std::sync::Arc<std::sync::atomic::AtomicBool>,
        Box<Result<crate::catalog::file_ops::RenameOrder, String>>,
    ),
    /// A plan of the rename dialog's names (see `app::file_ops::rename`).
    RenamePlanned(Box<crate::app::file_ops::RenamePlanned>),
    /// A file operation was planned (see `app::file_ops`): the plan, and
    /// Delete's Move to Trash beside it.
    FileOpPlanned(
        Box<
            Result<
                (
                    crate::catalog::file_ops::Plan,
                    Option<crate::catalog::file_ops::Plan>,
                ),
                String,
            >,
        >,
    ),
    /// Import Photos' attached drives, for dialog `.0` (see
    /// `app::file_ops::import`).
    ImportVolumes(u64, Vec<crate::app::file_ops::Drive>),
    /// Import Photos' scan of its source finished, or failed.
    ImportScanned(Box<crate::app::file_ops::ImportScanned>),
    /// One of the scan's photos' thumbnails.
    ImportThumbnail(Box<crate::app::file_ops::ImportThumbnail>),
    /// A plan of Import Photos' choices.
    ImportPlanned(Box<crate::app::file_ops::ImportPlanned>),
    /// Import Photos' run `.0` ended, its thread's report of it.
    ImportDone(
        u64,
        Box<Result<crate::catalog::file_ops::ImportOutcome, String>>,
    ),
    /// A file operation's run ended, its thread's report of it.
    FileOpDone(
        crate::catalog::file_ops::BatchId,
        Box<Result<crate::catalog::file_ops::Outcome, String>>,
    ),
    /// A catalog's unfinished file operations were recovered; `None` when
    /// another window holds them, or they can't run there.
    FileOpsRecovered(
        crate::catalog::CatalogLocation,
        Box<Result<Option<crate::catalog::file_ops::Recovery>, String>>,
    ),
    /// The user's choice for an unfinished file operation was carried out.
    FileOpResolved(
        crate::catalog::CatalogLocation,
        crate::catalog::file_ops::OperationId,
        Box<Result<crate::catalog::file_ops::ItemOutcome, String>>,
    ),
    Monitor(PathBuf),
    /// The folder chosen for decoded images.
    DecodeCacheFolder(PathBuf),
    /// Files and folders chosen to import profiles or presets from.
    Import(crate::app::presets::bulk_import::ImportKind, Vec<PathBuf>),
    Imported(Box<crate::app::presets::bulk_import::Summary>),
    /// A Manage Profiles change, or listing, finished.
    ProfilesManaged(Box<crate::app::presets::profile_manager::Managed>),
    /// A Sync Settings finished.
    Synced(Box<crate::app::library::sync::SyncResult>),
    Profiles {
        id: u64,
        profiles: Vec<Arc<crate::camera_profiles::CameraProfile>>,
        errors: Vec<String>,
        groups: crate::camera_profiles::Groups,
        /// Listed again after Adobe profiles were imported, which changes what
        /// the defaults of the Library's photos without an edit resolve to.
        imported: bool,
    },
    PresetLoad(PathBuf),
    /// Point Color's dropper sample for the photo loaded as `id`, taken with
    /// `sampled`: a swatch's `source`, or what to say instead.
    PointColorSample {
        id: u64,
        /// The sampling task's generation; a later sample or a put-away dropper
        /// supersedes it.
        generation: u64,
        sampled: Box<Recipe>,
        result: Result<[f32; 3], String>,
    },
    /// The Targeted Adjustment Tool's sample for the photo loaded as `id`, taken
    /// with `sampled` where a drag started, or what to say instead.
    TargetedSample {
        id: u64,
        /// The sampling task's generation; a later drag supersedes it.
        generation: u64,
        sampled: Box<Recipe>,
        result: Result<crate::develop::targeted::TargetSample, String>,
    },
    /// An Auto estimate for the photo loaded as `id`.
    Auto {
        id: u64,
        kind: AutoKind,
        result: Result<Box<Recipe>, String>,
    },
    /// Upright's corrections for the photo loaded as `id`, analysed from `analysed`.
    Upright {
        id: u64,
        generation: u64,
        analysed: Box<Recipe>,
        result: Result<Vec<[f32; 9]>, String>,
    },
    /// Camera Look (fitted) for the photo loaded as `id`, fitted while its profile
    /// was `profile`, or why none was.
    CameraLook {
        id: u64,
        generation: u64,
        profile: Option<Arc<crate::camera_profiles::CameraProfile>>,
        result: Result<Box<crate::camera_profiles::CameraProfile>, String>,
    },
    /// The embedded JPEG of the photo loaded as `id`, or why there is none.
    CameraThumbnail {
        id: u64,
        generation: u64,
        result: Result<crate::camera_look::CameraPreview, String>,
    },
    /// The Crop panel's Auto straighten angle for the photo loaded as `id`, analysed
    /// from `analysed`; None when the photo has nothing to level by.
    Straighten {
        id: u64,
        generation: u64,
        analysed: Box<Recipe>,
        result: Result<Option<f32>, String>,
    },
    /// The setup assistant's scan for cameras, profiles and presets.
    OnboardingScanned {
        generation: u64,
        found: Box<crate::app::catalog::onboarding::Found>,
    },
    XmpLibrary {
        scan: u64,
        library: Arc<crate::presets::Library>,
    },
    PresetScanFailed {
        scan: u64,
        error: String,
    },
    PresetSave(PathBuf),
    /// Export in Manage Presets, done: what to tell the user.
    PresetsExported(Result<String, String>),
    Header(Box<LoadedHeader>),
    Embedded {
        id: u64,
        image: image::RgbImage,
    },
    /// A stored Standard preview, read for a load or a neighbour.
    StandIn(Box<crate::app::develop::stand_in::StandIn>),
    Ready {
        id: u64,
        full: Arc<CameraImage>,
        /// The file as it was before `full` was decoded from it (`None` for a photo
        /// opened ahead of time): what AI Denoise's result must be made from.
        file: Option<crate::storage::Identity>,
        status: String,
    },
    Rendered {
        id: u64,
        pane: Pane,
        preview: Preview,
        histogram: Box<crate::rendered::Histogram>,
        /// A reduced copy for the library, without overlays, when the job asked for one.
        thumbnail: Option<image::RgbImage>,
        /// The shown pixels without overlays or monitor profile, when the job asked.
        samples: Option<image::RgbImage>,
        stage: RenderStage,
        status: String,
    },
    /// The whole photo's histogram, for a render that showed a 100% region.
    Histogram {
        id: u64,
        histogram: Box<crate::rendered::Histogram>,
    },
    Failed {
        id: u64,
        task: TaskKind,
        error: String,
    },
    /// The renderer recovered from a failure and no longer uses the textures its
    /// previews were presented into: stop drawing them, then drop this to free them.
    RendererReset(RetiredTextures),
    /// A batch of the export queue finished: each photo's outcome, in order.
    BatchExported {
        ticket: u64,
        outcomes: Vec<crate::export::batch::Outcome>,
    },
    /// Develop's Reference View photo, developed (half-size, then full), or why not.
    Reference {
        ticket: u64,
        result: Result<Box<ReferenceImage>, String>,
    },
}
/// Textures the renderer registered with the UI and no longer uses. Freed when
/// dropped, so the UI releases them only once it no longer draws them.
pub(crate) struct RetiredTextures {
    state: eframe::egui_wgpu::RenderState,
    ids: Vec<egui::TextureId>,
}
impl Drop for RetiredTextures {
    fn drop(&mut self) {
        let mut renderer = self.state.renderer.write();
        for id in &self.ids {
            renderer.free_texture(id);
        }
    }
}
/// A rendered preview as the viewport draws it.
pub(crate) enum Preview {
    /// Rendered on the CPU: display bytes for a texture upload.
    Pixels {
        image: Rendered,
        display_rgb: Vec<u8>,
        /// The minimap's copy, for whole-photo views.
        navigator: Option<image::RgbImage>,
    },
    /// Presented on the GPU into textures registered with the UI's renderer.
    Texture {
        id: egui::TextureId,
        size: [usize; 2],
        navigator: Option<(egui::TextureId, [usize; 2])>,
        /// The HDR master's preview of the same view, when the job asked for it
        /// (`RenderJob::hdr`) and the GPU renders it.
        hdr: Option<HdrTexture>,
    },
}
/// An HDR master's preview presented on the GPU (`PreviewRenderer::render_hdr_to`):
/// extended sRGB half floats held to `headroom`, the display headroom it was
/// rendered for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HdrTexture {
    pub id: egui::TextureId,
    pub size: [usize; 2],
    pub headroom: f32,
}
pub(crate) struct LoadJob {
    pub id: u64,
    /// The photo's original, which an editable preview stands in for.
    pub path: PathBuf,
    /// What to open: the original, or its editable preview while the original
    /// is absent.
    pub from: OpenedFrom,
    /// Opens an editable preview's file.
    pub copies: editable_preview::Copies,
    pub cancel: Arc<AtomicBool>,
    /// What the photo starts from when it has no edit.
    pub defaults: Arc<crate::raw_defaults::DevelopDefaults>,
    /// The demosaic of the full-size decode and of its decode-cache key.
    pub demosaic: crate::camera_data::Demosaic,
}
/// What a photo in Develop was opened from, which belongs to the document, not
/// to its pixels: only the loader sets it, from what the Library chose
/// ([`crate::app::library::Library::develop_from`]).
///
/// An editable preview is a reduced image, but unlike the half-size decode,
/// which a full-size decode replaces, it is final: nothing is decoded after it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum OpenedFrom {
    #[default]
    Original,
    EditablePreview(EditablePreviewSource),
}
impl OpenedFrom {
    /// What Develop calls a photo opened from its editable preview.
    pub(crate) const EDITABLE_PREVIEW: &'static str = "Editable Preview";
    /// What that means, on hover.
    pub(crate) const EDITABLE_PREVIEW_DETAIL: &'static str = "Editing its editable preview: the original is offline. Edits are saved for the original and render from it once it is back and the photo is opened again. AI masks, AI Denoise, Fit Camera Look and Export need the original.";
    /// The editable preview opened, if it was one.
    pub(crate) fn editable_preview(&self) -> Option<&EditablePreviewSource> {
        match self {
            Self::Original => None,
            Self::EditablePreview(source) => Some(source),
        }
    }
}
/// A photo's editable preview, as the catalog recorded it when the Library
/// chose to open the photo from it. The file's name is made from `original`
/// when the row is recorded (`catalog::editable_previews`), so the file is a
/// copy of that original for as long as it is there.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EditablePreviewSource {
    /// The copy's file.
    pub file: PathBuf,
    /// The original's identity when the copy was built: what the photo's edit
    /// is loaded and saved against while the original is absent.
    pub original: crate::storage::Identity,
}
/// A photo to develop into the decode cache ahead of time, so opening it next
/// skips decoding. It has its own cancel flag: the photo on screen finishing
/// must not stop it, moving to another photo must.
pub(crate) struct Prefetch {
    pub path: PathBuf,
    pub cancel: Arc<AtomicBool>,
    pub demosaic: crate::camera_data::Demosaic,
}
/// The open photo's neighbour, to develop ahead of time ([`Loader::prefetch`]).
/// Its inputs are captured as the photo starts opening and held here until the
/// photo's first full-size render is done; then the job is handed over, once.
/// Opening another photo, or quitting, cancels it wherever it is by then:
/// waiting here, in the loader's mailbox, or decoding.
#[derive(Default)]
pub(crate) struct NeighbourPrefetch {
    /// Shared with the job made for the photo now open.
    cancel: Arc<AtomicBool>,
    waiting: Option<Prefetch>,
}
impl NeighbourPrefetch {
    /// A photo starts opening: the prefetch made for the last one is cancelled,
    /// and `neighbour` (this photo's, when it has one) waits for this photo's
    /// first full-size render, to be decoded with `demosaic`.
    pub(crate) fn open(
        &mut self,
        neighbour: Option<PathBuf>,
        demosaic: crate::camera_data::Demosaic,
    ) {
        self.cancel();
        self.waiting = neighbour.map(|path| Prefetch {
            path,
            cancel: self.cancel.clone(),
            demosaic,
        });
    }
    /// The job to start now that the open photo's first full-size render is
    /// done: `None` when it has been taken, was cancelled, or the photo has no
    /// neighbour.
    pub(crate) fn take(&mut self) -> Option<Prefetch> {
        self.waiting.take()
    }
    /// Cancels the open photo's prefetch, waiting or handed over.
    pub(crate) fn cancel(&mut self) {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.cancel = Default::default();
        self.waiting = None;
    }
    /// The neighbour still waiting for the open photo's first full-size render.
    #[cfg(test)]
    pub(crate) fn waiting(&self) -> Option<&std::path::Path> {
        self.waiting.as_ref().map(|job| job.path.as_path())
    }
}
/// What is drawn over (or instead of) the rendered photo.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum Overlay {
    #[default]
    None,
    /// Visualize Spots with its threshold (0–1).
    Spots(f32),
    /// The mask at this index of the recipe's masks, tinted with this colour and
    /// opacity.
    Mask {
        index: usize,
        color: [u8; 3],
        opacity: f32,
    },
}
pub(crate) struct RenderJob {
    pub id: u64,
    pub pane: Pane,
    pub image: Arc<CameraImage>,
    pub max_edge: u32,
    pub cancel: Arc<AtomicBool>,
    pub recipe: Recipe,
    pub region: Option<[u32; 4]>,
    pub monitor: Option<PathBuf>,
    /// The clipping warnings painted over the shown pixels.
    pub clipping: crate::rendered::ClipOverlay,
    /// Update the minimap's copy (Fit views).
    pub navigator: bool,
    /// Also produce a library thumbnail of the result.
    pub thumbnail: bool,
    /// Also return the shown pixels, for the white balance selector's loupe.
    pub samples: bool,
    pub overlay: Overlay,
    /// The photo's AI Denoise source when the recipe renders it, as the job started.
    pub ai: Option<crate::develop::AiSource>,
    /// Presented textures the viewport draws until this job's preview arrives.
    pub drawn: Vec<egui::TextureId>,
    /// Also present the HDR master's preview, held to this display headroom: where
    /// the viewport is due to show the master (`Editor::hdr_preview_due`).
    pub hdr: Option<f32>,
}
fn send(tx: &Sender<Event>, ctx: &egui::Context, event: Event) {
    let _ = tx.send(event);
    ctx.request_repaint();
}

pub(crate) mod editable_preview;
mod latest;
mod loader;
#[cfg(test)]
pub(in crate::app) use loader::tests::cmyk_profile;
mod reference;
mod renderer;
pub(crate) use latest::Latest;
pub(crate) use latest::panic_message;
pub(crate) use loader::Loader;
pub(in crate::app) use reference::{ReferenceImage, ReferenceJob, Resolution, reference_loader};
pub(crate) use renderer::Renderer;
#[cfg(test)]
pub(in crate::app) use renderer::holding;
pub(in crate::app) use renderer::{RenderBackend, renderer_with_backend};

#[cfg(test)]
mod neighbour_prefetch_tests {
    use super::NeighbourPrefetch;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::Ordering;

    #[test]
    fn the_neighbour_is_handed_over_once() {
        let mut ahead = NeighbourPrefetch::default();
        assert!(ahead.take().is_none(), "nothing before a photo opens");
        ahead.open(Some(PathBuf::from("b.arw")), Default::default());
        assert_eq!(ahead.waiting(), Some(Path::new("b.arw")));
        let job = ahead.take().expect("the neighbour, at the first render");
        assert_eq!(job.path, Path::new("b.arw"));
        assert!(!job.cancel.load(Ordering::Relaxed));
        assert!(ahead.take().is_none(), "later renders start nothing");
    }

    /// Stale work: the job of a photo moved past must not decode, whether the
    /// loader has it by then or not.
    #[test]
    fn opening_another_photo_cancels_the_last_one_s_neighbour() {
        let mut ahead = NeighbourPrefetch::default();
        ahead.open(Some(PathBuf::from("b.arw")), Default::default());
        let handed_over = ahead.take().unwrap();
        ahead.open(Some(PathBuf::from("c.arw")), Default::default());
        assert!(handed_over.cancel.load(Ordering::Relaxed));
        // Moved on again before this photo's first render: it never starts.
        ahead.open(None, Default::default());
        assert!(ahead.take().is_none());
        // The job made next has a flag of its own.
        ahead.open(Some(PathBuf::from("d.arw")), Default::default());
        let next = ahead.take().unwrap();
        assert!(!next.cancel.load(Ordering::Relaxed));
        assert_eq!(next.path, Path::new("d.arw"));
    }

    #[test]
    fn cancelling_stops_it_waiting_or_handed_over() {
        let mut ahead = NeighbourPrefetch::default();
        ahead.open(Some(PathBuf::from("b.arw")), Default::default());
        ahead.cancel();
        assert!(ahead.take().is_none());
        ahead.open(Some(PathBuf::from("b.arw")), Default::default());
        let job = ahead.take().unwrap();
        ahead.cancel();
        assert!(job.cancel.load(Ordering::Relaxed));
    }
}
