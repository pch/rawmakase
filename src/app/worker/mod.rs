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
    DialogClosed,
    /// The system report's probe answered, or stopped (see `app::report_issue`).
    SystemProbed {
        generation: u64,
        system: Box<crate::diagnostics::System>,
    },
    /// A Subject or Background selection finished (or failed).
    Selection(Box<crate::app::subject_mask::Done>),
    /// The selection model finished installing, or why not.
    ModelInstalled(crate::app::subject_mask::Pack, Result<(), String>),
    ModelRemoved(crate::app::subject_mask::Pack, Result<(), String>),
    /// A Lensfun database job finished (`app::lensfun`).
    Lensfun {
        generation: u64,
        job: crate::app::lensfun::Job,
        result: Result<crate::app::lensfun::Done, crate::app::lensfun::Failure>,
    },
    /// A catalog upgrade for raster masks finished: the backup it made, or why not.
    CatalogUpgraded {
        generation: u64,
        result: Result<Option<PathBuf>, String>,
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
    FolderSyncScanned(Box<crate::app::folder_sync::Scanned>),
    /// Find on Disk's file picked for a photo.
    FileOnDiskPicked(crate::catalog::PhotoId, PathBuf),
    /// Applying a synchronization finished: the catalog opened again.
    FolderSyncApplied(Result<Box<crate::app::folder_sync::Synced>, String>),
    /// A folder change waiting for the user's answer.
    FolderQuestion(Box<crate::app::folder_locations::FolderQuestion>),
    /// Export as Catalog finished: what it wrote, nothing when the user
    /// chose no file, or why it failed.
    CatalogExported(Box<Result<Option<crate::app::export_catalog::Exported>, String>>),
    /// A file operation was planned (see `app::file_ops`).
    /// What export file names need from the photos' files, for the read
    /// `request` (see `app::naming::read`).
    NamesRead {
        request: u64,
        read: Result<crate::app::naming::read::Reads, String>,
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
    /// The plan, and Delete's Move to Trash beside it.
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
    Import(crate::app::bulk_import::ImportKind, Vec<PathBuf>),
    Imported(Box<crate::app::bulk_import::Summary>),
    /// A Sync Settings finished.
    Synced(Box<crate::app::sync::SyncResult>),
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
        found: Box<crate::app::onboarding::Found>,
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
    Header(Box<LoadedHeader>),
    Embedded {
        id: u64,
        image: image::RgbImage,
    },
    /// A stored Standard preview, read for a load or a neighbour.
    StandIn(Box<crate::app::stand_in::StandIn>),
    Ready {
        id: u64,
        full: Arc<CameraImage>,
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
    },
}
pub(crate) struct LoadJob {
    pub id: u64,
    pub path: PathBuf,
    pub cancel: Arc<AtomicBool>,
    /// The photo to decode ahead of time once this one is fully developed.
    pub prefetch: Option<Prefetch>,
    /// What the photo starts from when it has no edit.
    pub defaults: Arc<crate::raw_defaults::DevelopDefaults>,
    /// The demosaic of the full-size decode and of its decode-cache key.
    pub demosaic: crate::camera_data::Demosaic,
}
/// A photo to develop into the decode cache ahead of time, so opening it next
/// skips decoding. It has its own cancel flag: the photo on screen finishing
/// must not stop it, moving to another photo must.
pub(crate) struct Prefetch {
    pub path: PathBuf,
    pub cancel: Arc<AtomicBool>,
    pub demosaic: crate::camera_data::Demosaic,
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
    /// Presented textures the viewport draws until this job's preview arrives.
    pub drawn: Vec<egui::TextureId>,
}
fn send(tx: &Sender<Event>, ctx: &egui::Context, event: Event) {
    let _ = tx.send(event);
    ctx.request_repaint();
}

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
pub(super) use renderer::holding;
pub(super) use renderer::{RenderBackend, renderer_with_backend};
