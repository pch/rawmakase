//! Desktop composition and presentation, built on the crate's domain APIs.
//!
//! `Editor` coordinates the workspace; `state` separates the document, preview,
//! viewport and preset browser. `workflow` starts operations, `events` accepts
//! worker results, and `workspace` composes each frame. Task cancellation, history,
//! foreground activity and save policy have their own modules.
//!
//! File formats, persistence and pixel processing belong in the domain modules.
//! See `docs/code-map.md` for panel, library and worker implementation locations.
use crate::app::worker::{Event, Latest};
use crate::catalog::{CatalogLocation, PhotoId};
#[cfg(test)]
use crate::model::recipe::Recipe;
use eframe::egui::{self, Vec2};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
};

pub(crate) struct Editor {
    activity: activity::Activity,
    load: task::Task,
    presets: PresetBrowser,
    view: ViewState,
    preview: PreviewState,
    document: Document,
    context: egui::Context,
    session_file: Option<PathBuf>,
    library: Option<Box<crate::app::library::Library>>,
    /// The module shown: the Library grid, or Develop.
    module: Module,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    loader: worker::Loader,
    renderer: worker::Renderer,
    /// Develop's Reference View, and the worker developing its photo.
    reference: reference::ReferenceView,
    reference_loader: Latest<worker::ReferenceJob>,
    clipboard: Option<settings_transfer::Clipboard>,
    /// The settings of the photo open before this one, for Paste from Previous.
    previous_settings: Option<settings_transfer::Settings>,
    /// The groups Copy Settings last copied.
    copy_groups: crate::model::settings_groups::GroupSelection,
    /// The Point Curve menu's saved curves and its Save window.
    curves: curve_menu::CurveMenu,
    /// Collapsed panel sections as last saved to the session.
    collapsed: std::collections::BTreeSet<String>,
    /// The sides in Solo Mode as last saved to the session.
    solo: std::collections::BTreeSet<String>,
    /// The panels each module shows (Tab, Shift+Tab, F6–F8).
    panels: panels::WorkspacePanels,
    onboarding: onboarding::Onboarding,
    onboarding_done: bool,
    preferences: preferences::Preferences,
    updates: updates::Updates,
    #[cfg(feature = "telemetry")]
    stats: stats::UsageStats,
    themes: theme::Themes,
    exports: export::Exports,
    /// Build Standard-Sized Previews, in the background.
    preview_builds: preview_build::PreviewBuilds,
    /// Stored Standard previews read for the photo opening and its neighbour.
    stand_ins: stand_in::StandIns,
    autosave: autosave::Autosave,
    /// Library/Develop position to restore once the session's catalog opens.
    restore: Option<CatalogPlace>,
    /// A photo from outside the Library to open once the catalog is ready.
    pending_photo: Option<PendingPhoto>,
    /// Cancels the prefetch started for the photo on screen.
    prefetch_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Preferences > Performance's demosaic, for every full-size decode.
    demosaic: crate::camera_data::Demosaic,
    /// That position as last written to the session.
    saved_place: CatalogPlace,
    /// How the Library showed its photos, as last written to the session;
    /// returned to whenever the catalog is loaded.
    saved_layout: crate::app::session::LibraryLayout,
    status: String,
    /// What a running catalog import or open is doing.
    catalog_work: Option<String>,
    /// Progress of a running profile or preset import.
    importing: Option<std::sync::Arc<bulk_import::ImportProgress>>,
    close_confirm: bool,
    /// The close was refused only to let work finish (an export, Sync Settings, a
    /// folder change, a command output): it is asked for again once that is done.
    close_after_work: bool,
    /// When quitting must be done by, set as the close guard lets a close through,
    /// so the save it made and the exit hook share one deadline.
    quit_by: Option<std::time::Instant>,
    /// The next close goes ahead without waiting for a folder change or a command
    /// output, as the user chose; the exit hook still waits for them, under its
    /// deadline.
    close_anyway: bool,
    /// The dialog blocking the editor, if one is open.
    modal: Option<Modal>,
    /// A photo Develop could not open and why, until the user dismisses it.
    /// Its own field: a photo dropped on the window can be refused while
    /// a `modal` is open.
    not_editable: Option<(String, String)>,
    /// Cmd+Z across Library and Develop.
    undo_log: undo::UndoLog,
    /// Select Subject and Select Background: the running selection, the selection
    /// model's install and what the drawer asks.
    selection: subject_mask::Selection,
    /// Photo > Auto Advance, saved in the session.
    auto_advance: bool,
    /// Preferences: whether converting to black & white applies the Auto mix to a
    /// mix never set, as Lightroom's preference of that name (on by default).
    first_conversion: treatment::FirstConversion,
    /// The RAW the Loupe last started loading, so one that fails is not
    /// loaded again every frame.
    loupe_tried: Option<PhotoId>,
    /// Preferences > Raw Defaults, ready to apply; shared with the loader and the
    /// Library's previews.
    raw_defaults: std::sync::Arc<crate::raw_defaults::DevelopDefaults>,
    /// Shared automation queue and independently configured input adapters.
    controls: automation::Hub,
    automation: commands::Automation,
}
impl Editor {
    pub(crate) fn new(
        cc: &eframe::CreationContext<'_>,
        path: Option<PathBuf>,
        launch: crate::updates::Launch,
    ) -> Self {
        // The desktop's hinting and antialiasing, read once (a D-Bus call on
        // Linux) and applied to the fonts here and to the visuals below.
        let text = fastframe_text::detect();
        if let Some(render_state) = &cc.wgpu_render_state {
            survive_surface_errors(&render_state.device);
        }
        install_fonts(&cc.egui_ctx, &text);
        icons::install(&cc.egui_ctx);
        // The desktop's text size (Omarchy's, GNOME's Large Text) zooms the
        // interface, now and when it changes.
        crate::platform::text_scale::follow(&cc.egui_ctx);
        let mut editor = Self::with_backend(
            &cc.egui_ctx,
            path,
            crate::app::session::load_session(),
            Some(crate::storage::data_dir().join("session.json")),
            worker::RenderBackend::Gpu(cc.wgpu_render_state.clone()),
        );
        editor.controls = automation::Hub::start(&cc.egui_ctx);
        // 1:1 previews left unused past the chosen time go, as in Lightroom.
        editor.expire_previews();
        editor.updates.launched(launch, &mut editor.status);
        cc.egui_ctx
            .all_styles_mut(|style| text.apply_to_visuals(&mut style.visuals));
        editor.themes.text = text;
        editor.themes.start();
        editor
    }
    #[cfg(test)]
    fn with_context(
        ctx: &egui::Context,
        path: Option<PathBuf>,
        session: crate::app::session::Session,
        session_file: Option<PathBuf>,
    ) -> Self {
        Self::with_backend(ctx, path, session, session_file, worker::RenderBackend::Cpu)
    }
    fn with_backend(
        ctx: &egui::Context,
        path: Option<PathBuf>,
        session: crate::app::session::Session,
        session_file: Option<PathBuf>,
        backend: worker::RenderBackend,
    ) -> Self {
        theme::apply(
            ctx,
            theme::Palette::DEFAULT,
            &fastframe_text::TextRendering::platform_default(),
        );
        // Cmd/Ctrl + and − zoom the photo, not the whole interface.
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        ctx.data_mut(|d| {
            d.insert_temp(widgets::collapsed_sections_id(), session.collapsed.clone());
            d.insert_temp(widgets::solo_sections_id(), session.solo.clone());
        });
        ctx.all_styles_mut(|style| {
            style.spacing.item_spacing = Vec2::new(8., 5.);
            style.spacing.button_padding = Vec2::new(9., 5.);
            style.spacing.indent = 18.;
            style
                .text_styles
                .insert(egui::TextStyle::Body, egui::FontId::proportional(13.));
            style
                .text_styles
                .insert(egui::TextStyle::Button, egui::FontId::proportional(13.));
            style
                .text_styles
                .insert(egui::TextStyle::Small, egui::FontId::proportional(11.));
        });
        // Only a real session (not an isolated test) shows first-run setup.
        let show_onboarding = !session.onboarding_done && session_file.is_some();
        let place = CatalogPlace::of(&session);
        // Only a real session checks GitHub, not an isolated test.
        let updates = updates::Updates::new(&session, session_file.is_some().then_some(ctx));
        #[cfg(feature = "telemetry")]
        let adapter = match &backend {
            worker::RenderBackend::Gpu(Some(render_state)) => Some(render_state.adapter.get_info()),
            _ => None,
        };
        #[cfg(feature = "telemetry")]
        let stats = stats::UsageStats::new(
            adapter.as_ref(),
            session_file
                .as_deref()
                .and_then(std::path::Path::parent)
                .map(|dir| (dir.to_path_buf(), ctx)),
        );
        let last = session.last_path.clone().filter(|p| p.exists());
        let (tx, rx) = mpsc::channel();
        let loader = worker::Loader::new(tx.clone(), ctx.clone());
        let renderer = worker::renderer_with_backend(tx.clone(), ctx.clone(), backend);
        let reference_loader = worker::reference_loader(tx.clone(), ctx.clone());
        let standard_preview_size = session.standard_preview_size();
        let discard_one_to_one_after = session.discard_one_to_one_after();
        let stand_ins = stand_in::StandIns::new(
            tx.clone(),
            ctx.clone(),
            crate::catalog::preview_cache::PreviewCache::path(),
        );
        let mut app = Self {
            activity: Default::default(),
            load: Default::default(),
            presets: PresetBrowser {
                favorites: crate::presets::load_favorites(),
                ..Default::default()
            },
            view: ViewState {
                monitor: session.monitor,
                crop_guides: crop_tool::CropGuides::from_session(&session.crop_guides),
                ..Default::default()
            },
            preview: Default::default(),
            document: Default::default(),
            context: ctx.clone(),
            session_file,
            library: None,
            module: Module::Develop,
            tx,
            rx,
            loader,
            renderer,
            reference: Default::default(),
            reference_loader,
            clipboard: None,
            previous_settings: None,
            curves: Default::default(),
            copy_groups: session.copy_groups.clone().unwrap_or_default(),
            collapsed: session.collapsed.clone(),
            solo: session.solo.clone(),
            panels: session.panels,
            onboarding: onboarding::Onboarding::new(show_onboarding),
            onboarding_done: session.onboarding_done,
            preferences: Default::default(),
            updates,
            #[cfg(feature = "telemetry")]
            stats,
            themes: theme::Themes::new(
                ctx,
                // Sessions from before `theme_chosen` saved only a palette.
                (session.theme_chosen || session.theme.is_some()).then(|| session.theme.clone()),
                fastframe_text::TextRendering::platform_default(),
            ),
            exports: Default::default(),
            preview_builds: preview_build::PreviewBuilds::new(
                standard_preview_size,
                discard_one_to_one_after,
            ),
            stand_ins,
            autosave: Default::default(),
            restore: Some(place.clone()),
            saved_place: place,
            saved_layout: session.library_layout.clone(),
            pending_photo: None,
            prefetch_cancel: Default::default(),
            demosaic: session.demosaic,
            status: "Pick a photo in the Library to begin".into(),
            catalog_work: None,
            importing: None,
            close_confirm: false,
            close_after_work: false,
            quit_by: None,
            close_anyway: false,
            modal: None,
            not_editable: None,
            undo_log: Default::default(),
            selection: Default::default(),
            auto_advance: session.auto_advance,
            first_conversion: if session.no_auto_black_white_mix {
                treatment::FirstConversion::KeepMix
            } else {
                treatment::FirstConversion::AutoMix
            },
            loupe_tried: None,
            raw_defaults: std::sync::Arc::new(crate::raw_defaults::DevelopDefaults::load(
                session.raw_defaults.clone(),
            )),
            controls: automation::Hub::inactive(),
            automation: commands::Automation::default(),
        };
        app.reload_presets(ctx);
        // A catalog passed on the command line opens instead of the last one;
        // a photo passed there is added to the last catalog.
        match path {
            Some(path) if path.extension().is_some_and(|e| e == "rawmakase") => app.open(path),
            path => {
                if let Some(last) = last {
                    app.open(last);
                }
                if let Some(path) = path {
                    app.open(path);
                }
            }
        }
        app
    }
    // A caller may disable preference persistence, e.g. in an isolated UI test.
    fn save_session(&self) -> anyhow::Result<()> {
        if let Some(path) = &self.session_file {
            crate::storage::atomic_json(
                path,
                &crate::app::session::Session {
                    last_path: self.session_path(),
                    monitor: self.view.monitor.clone(),
                    collapsed: self.collapsed.clone(),
                    solo: self.solo.clone(),
                    panels: self.panels,
                    onboarding_done: self.onboarding_done,
                    library_source: self.saved_place.source.clone(),
                    selected_photo: self.saved_place.photo,
                    develop: self.saved_place.module == Module::Develop,
                    demosaic: self.demosaic,
                    no_update_checks: !self.updates.automatic,
                    skipped_version: self.updates.skipped.clone(),
                    theme: self.themes.chosen().flatten(),
                    theme_chosen: self.themes.chosen().is_some(),
                    auto_advance: self.auto_advance,
                    no_auto_black_white_mix: self.first_conversion
                        == treatment::FirstConversion::KeepMix,
                    library_layout: self.saved_layout.clone(),
                    copy_groups: Some(self.copy_groups.clone()),
                    crop_guides: self.view.crop_guides.to_session(),
                    raw_defaults: self.raw_defaults.settings().clone(),
                    standard_preview_size: Some(self.preview_builds.standard_size),
                    one_to_one_discard_days: Some(
                        self.preview_builds.discard_one_to_one_after.unwrap_or(0),
                    ),
                },
            )?;
        }
        Ok(())
    }
    /// The Library folder, selected photo and module, as saved in the session.
    fn current_place(&self) -> CatalogPlace {
        let Some(library) = &self.library else {
            return CatalogPlace::default();
        };
        let develop = self.module == Module::Develop && self.document.catalog_photo.is_some();
        CatalogPlace {
            source: library.source_key(),
            photo: if develop {
                self.document.catalog_photo
            } else {
                library.selected()
            },
            module: if develop {
                Module::Develop
            } else {
                Module::Library
            },
        }
    }
    fn session_path(&self) -> Option<PathBuf> {
        self.library
            .as_ref()
            .map(|l| {
                // session.json names a file catalog by its path, as before.
                let CatalogLocation::File(path) = l.session.catalog.location();
                path.clone()
            })
            .or_else(|| self.document.path.clone())
    }
}
/// Where the Library was: its folder or collection, the photo selected (or open
/// in Develop) and the module shown. Saved in the session and restored on launch.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CatalogPlace {
    source: String,
    photo: Option<PhotoId>,
    module: Module,
}
impl Default for CatalogPlace {
    fn default() -> Self {
        Self {
            source: String::new(),
            photo: None,
            module: Module::Library,
        }
    }
}
impl CatalogPlace {
    fn of(session: &session::Session) -> Self {
        Self {
            source: session.library_source.clone(),
            photo: session.selected_photo,
            module: if session.develop {
                Module::Develop
            } else {
                Module::Library
            },
        }
    }
}
/// A photo from outside the Library, to open once the catalog is ready.
struct PendingPhoto {
    path: PathBuf,
    /// Its folder has been added to the catalog already; if the photo is still
    /// not there, it could not be added.
    folder_added: bool,
}
/// The dialog that blocks the editor, one at a time, kept in `Editor::modal`.
/// Dialogs owned by a subsystem (Preferences, Export, Shortcuts, onboarding,
/// Save Curve) keep their state there instead.
enum Modal {
    /// Copy, Synchronize or New Preset settings, and the groups being chosen.
    CopySettings(settings_transfer::CopyDialog),
    /// A preset made here being renamed.
    RenamePreset(user_presets::PresetRename),
    /// The virtual copy waiting for the user to confirm its removal.
    RemoveCopy(PhotoId),
    /// Photos waiting for the user to confirm Read Metadata from Files.
    ReadMetadata(Vec<PhotoId>),
    /// A folder change waiting for the user's answer.
    FolderQuestion(folder_locations::FolderQuestion),
}
impl eframe::App for Editor {
    /// What shows where no panel paints, e.g. behind the Library grid: the
    /// theme's darkest grey rather than eframe's near-black.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // eframe gives no context here; the palette last applied to it.
        self.themes.applied().gray(12).to_normalized_gamma_f32()
    }
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // AppKit moves the traffic lights back during layout passes.
        fastframe_macos::align_traffic_lights(frame, ui.ctx(), workspace::BAR_HEIGHT);
        #[cfg(windows)]
        crate::platform::taskbar_icon(frame);
        self.draw(ui);
    }
    // fastframe-macos turns on eframe's glow feature, which adds the context.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.exit();
    }
}
/// The largest PNG stored in an ICO directory. RAWmakase's Windows icon is a
/// set of PNG images, and the window wants the 256×256 one.
#[cfg(windows)]
pub(crate) fn largest_png_in_ico(ico: &[u8]) -> Option<&[u8]> {
    if ico.len() < 6 {
        return None;
    }
    let count = u16::from_le_bytes(ico[4..6].try_into().ok()?) as usize;
    let mut best: Option<(u32, &[u8])> = None;
    for index in 0..count {
        let entry = ico.get(6 + index * 16..6 + (index + 1) * 16)?;
        let width = match entry[0] {
            0 => 256,
            width => u32::from(width),
        };
        let size = u32::from_le_bytes(entry[8..12].try_into().ok()?) as usize;
        let offset = u32::from_le_bytes(entry[12..16].try_into().ok()?) as usize;
        let bytes = ico.get(offset..offset.checked_add(size)?)?;
        if bytes.starts_with(b"\x89PNG") && best.is_none_or(|(chosen, _)| width > chosen) {
            best = Some((width, bytes));
        }
    }
    best.map(|(_, bytes)| bytes)
}

fn window_icon() -> egui::IconData {
    #[cfg(windows)]
    if let Some(icon) = windows_window_icon() {
        return icon;
    }
    egui::IconData::default()
}

#[cfg(windows)]
fn windows_window_icon() -> Option<egui::IconData> {
    let png = largest_png_in_ico(include_bytes!("../../packaging/windows/rawmakase.ico"))?;
    let image = image::load_from_memory(png).ok()?.into_rgba8();
    let (width, height) = (image.width(), image.height());
    Some(egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    })
}

pub fn run(path: Option<PathBuf>, launch: crate::updates::Launch) -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("RAWmakase")
            // An empty icon keeps the macOS bundle's and Linux desktop entry's
            // icon; otherwise eframe replaces it with egui's logo while running.
            // On Windows that empty value leaves the title bar blank, so the
            // window gets the same artwork embedded in the executable.
            .with_icon(window_icon())
            .with_inner_size([1440., 960.])
            .with_min_inner_size([900., 650.])
            // On macOS the workspace bar is the title bar, under the traffic
            // lights; elsewhere the window keeps its decorations.
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false),
        // fastframe-macos turns on eframe's glow renderer too; previews
        // share the UI's wgpu device.
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: wgpu_options(),
        ..Default::default()
    };
    // eframe::run_native, with window events passing through SurfaceGate.
    use winit::platform::run_on_demand::EventLoopExtRunOnDemand as _;
    if log::set_logger(&EframeErrors).is_ok() {
        log::set_max_level(log::LevelFilter::Error);
    }
    let mut event_loop = winit::event_loop::EventLoop::with_user_event().build()?;
    let mut app = SurfaceGate(eframe::create_native(
        "RAWmakase",
        options,
        Box::new(move |cc| {
            // The event loop has built the app menu by now.
            crate::platform::quit::through_close_guard(cc);
            Ok(Box::new(Editor::new(cc, path, launch)))
        }),
        &event_loop,
    ));
    event_loop.run_app_on_demand(&mut app)?;
    match EframeErrors::take() {
        Some(error) => Err(anyhow::anyhow!(error)),
        None => Ok(()),
    }
}
/// Why eframe gave up, e.g. on a GPU it could not start. `run_native` returned
/// that error; on our own event loop eframe only logs it.
struct EframeErrors;
static EFRAME_ERROR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
impl EframeErrors {
    fn take() -> Option<String> {
        EFRAME_ERROR
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}
impl log::Log for EframeErrors {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Error && metadata.target().starts_with("eframe")
    }
    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        if let Some(error) = record
            .args()
            .to_string()
            .strip_prefix("Exiting because of error: ")
        {
            *EFRAME_ERROR
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error.into());
        }
    }
    fn flush(&self) {}
}
/// Whether eframe reconfigures the window's surface for `event`.
fn reconfigures_surface(event: &winit::event::WindowEvent) -> bool {
    use winit::event::WindowEvent;
    matches!(
        event,
        WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. }
    )
}
/// Holds previews' GPU work while eframe reconfigures the window's surface;
/// see `develop::gpu::reconfiguring_surface`.
struct SurfaceGate<'a>(eframe::EframeWinitApplication<'a>);
impl winit::application::ApplicationHandler<eframe::UserEvent> for SurfaceGate<'_> {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        self.0.resumed(event_loop);
    }
    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        let _held = reconfigures_surface(&event).then(crate::develop::gpu::reconfiguring_surface);
        self.0.window_event(event_loop, window_id, event);
    }
    fn new_events(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        cause: winit::event::StartCause,
    ) {
        self.0.new_events(event_loop, cause);
    }
    fn user_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        event: eframe::UserEvent,
    ) {
        self.0.user_event(event_loop, event);
    }
    fn device_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        device_id: winit::event::DeviceId,
        event: winit::event::DeviceEvent,
    ) {
        self.0.device_event(event_loop, device_id, event);
    }
    fn about_to_wait(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        self.0.about_to_wait(event_loop);
    }
    fn suspended(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        self.0.suspended(event_loop);
    }
    fn exiting(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        self.0.exiting(event_loop);
    }
    fn memory_warning(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        self.0.memory_warning(event_loop);
    }
}
/// The UI's wgpu device also renders previews (see `develop::gpu`): prefer the
/// discrete GPU (egui's default, unless `WGPU_POWER_PREF` says otherwise) and ask
/// for the storage limits full-resolution regions need. On Linux the window's GPU
/// is the one the system lists first; see [`display_adapter`].
fn wgpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    let mut options = eframe::egui_wgpu::WgpuConfiguration::default();
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_setup {
        #[cfg(target_os = "linux")]
        {
            setup.native_adapter_selector = Some(std::sync::Arc::new(|adapters, surface| {
                let usable: Vec<_> = adapters
                    .iter()
                    .filter(|adapter| surface.is_none_or(|s| adapter.is_surface_supported(s)))
                    .collect();
                let infos: Vec<_> = usable.iter().map(|adapter| adapter.get_info()).collect();
                let listed: Vec<_> = infos
                    .iter()
                    .map(|info| (info.name.as_str(), info.device_type))
                    .collect();
                let name = std::env::var("WGPU_ADAPTER_NAME").ok();
                display_adapter(&listed, wgpu::PowerPreference::from_env(), name.as_deref())
                    .map(|i| usable[i].clone())
                    .ok_or_else(|| "No graphics adapter can draw the window".to_owned())
            }));
        }
        let default = setup.device_descriptor.clone();
        setup.device_descriptor = std::sync::Arc::new(move |adapter| {
            let mut descriptor = default(adapter);
            if adapter.get_info().backend != wgpu::Backend::Gl {
                descriptor.required_limits = crate::develop::gpu::required_limits(adapter);
            }
            descriptor
        });
    }
    // A surface whose configuration failed reports `Validation` until it is
    // configured again; the default would skip frames until the next resize.
    let default = options.on_surface_status.clone();
    options.on_surface_status = std::sync::Arc::new(move |status| match status {
        wgpu::CurrentSurfaceTexture::Validation => {
            eframe::egui_wgpu::SurfaceErrorAction::Reconfigure
        }
        _ => default(status),
    });
    options
}

/// Which of the adapters able to draw the window, in the system's order, the UI
/// uses. Ranking a discrete GPU first breaks hybrid laptops whose discrete GPU only
/// renders for the one driving the display: NVIDIA's driver claims it can present
/// to the window, then fails to configure the surface (#375). The system's order
/// already puts the display's GPU first (Mesa's device-select layer), or the
/// discrete one under `prime-run` or `DRI_PRIME=1`. `preference` and `name` come
/// from `WGPU_POWER_PREF` and `WGPU_ADAPTER_NAME`; software rendering comes last.
#[cfg(target_os = "linux")]
fn display_adapter(
    adapters: &[(&str, wgpu::DeviceType)],
    preference: Option<wgpu::PowerPreference>,
    name: Option<&str>,
) -> Option<usize> {
    use wgpu::{DeviceType, PowerPreference};
    if let Some(name) = name.map(str::to_lowercase) {
        let named = adapters
            .iter()
            .position(|(adapter, _)| adapter.to_lowercase().contains(&name));
        if named.is_some() {
            return named;
        }
    }
    let rank = |device_type| match (device_type, preference) {
        (DeviceType::DiscreteGpu, Some(PowerPreference::LowPower)) => 1,
        (DeviceType::IntegratedGpu, Some(PowerPreference::HighPerformance)) => 1,
        (DeviceType::DiscreteGpu | DeviceType::IntegratedGpu, _) => 0,
        (DeviceType::Other, _) => 2,
        (DeviceType::VirtualGpu, _) => 3,
        (DeviceType::Cpu, _) => 4,
    };
    // The first of the best-ranked, keeping the system's order.
    (0..adapters.len()).min_by_key(|&i| rank(adapters[i].1))
}

/// wgpu panics on uncaptured errors by default. Reconfiguring the window's
/// surface can fail transiently, for example when a tiling compositor resizes
/// the window while the GPU is busy ("Failed to wait for GPU to come idle"),
/// and the next frame configures it again. Other errors are still bugs.
fn survive_surface_errors(device: &wgpu::Device) {
    device.on_uncaptured_error(std::sync::Arc::new(|error| {
        if error.to_string().contains("In Surface::") {
            eprintln!("Ignoring window surface error: {error}");
        } else {
            panic!("wgpu error: {error}");
        }
    }));
}

mod auto;
mod automation;
mod before_after;
mod brush_scroll;
mod bulk_import;
mod catalog;
mod clipping;
mod color_grading;
mod commands;
mod crop_tool;
mod curve_menu;
mod dialogs;
mod export;
pub(crate) mod folder_locations;
mod guided_tool;
mod inspector;
pub(crate) mod library;
mod mask_tool;
mod navigator;
mod onboarding;
mod overlay;
mod panels;
mod photo_metadata;
mod point_color_panel;
mod preferences;
mod presets;
mod preview_build;
mod raw_defaults;
mod readout;
mod red_eye_tool;
mod reference;
mod retouch_tool;
mod stand_in;
#[cfg(feature = "telemetry")]
mod stats;
mod stroke_outline;
mod subject_mask;
mod targeted_tool;
#[cfg(test)]
mod tests;
mod undo;
mod upright;
mod viewport;
mod widgets;
pub(crate) mod worker;
mod workflow;
mod workspace;

mod state;
use crate::edit_session::{history, save_state};
use library::Module;
use state::{Document, PresetBrowser, PreviewState, ViewState};

mod icons;

mod task;

mod activity;

mod autosave;
pub(crate) mod session;

mod editing;
mod exit;
mod settings_transfer;
mod shortcuts;
mod snapshots;
mod sync;
mod theme;
mod tone_drag;
mod toolbar;
mod treatment;
mod updates;
mod user_presets;

mod events;

/// Inter, with tabular figures so values keep their width as they change,
/// and an installed face for every script and symbol it lacks (fastframe-fonts),
/// rendered as the desktop renders text.
fn install_fonts(ctx: &egui::Context, text: &fastframe_text::TextRendering) {
    let mut fonts = fastframe_fonts::FontSetup::default().definitions();
    text.apply_to(&mut fonts);
    ctx.set_fonts(fonts);
}
