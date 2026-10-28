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
    #[cfg(target_os = "macos")]
    native_menu: bool,
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
    /// Whether the module's chrome is hidden, leaving the photo alone (see
    /// `chrome`); not saved, so every launch opens in the normal window.
    chrome: chrome::Chrome,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    loader: worker::Loader,
    /// Shared size policy for Develop, neighbour prefetch and Reference View.
    decode_cache: crate::decode_cache::DecodeCache,
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
    /// The Import .cube LUTs window's colour space, while it is open.
    lut_import: Option<crate::camera_profiles::LutSpace>,
    /// Collapsed panel sections as last saved to the session.
    collapsed: std::collections::BTreeSet<String>,
    /// The sides in Solo Mode as last saved to the session.
    solo: std::collections::BTreeSet<String>,
    /// Pinned panel sections as last saved to the session.
    pinned: std::collections::BTreeSet<String>,
    /// Whether the window is maximized, as last saved to the session.
    maximized: bool,
    /// Reopening it maximized, until the window manager has done so.
    restoring_maximized: workspace::RestoreMaximized,
    /// The panels each module shows (Tab, Shift+Tab, F6–F8).
    panels: panels::WorkspacePanels,
    /// What the last Tab or Shift+Tab hid, and in which module.
    tabbed: Option<(Module, panels::Tabbed)>,
    onboarding: onboarding::Onboarding,
    onboarding_done: bool,
    preferences: preferences::Preferences,
    updates: updates::Updates,
    /// Report an Issue…: its dialog and the system probe.
    report_issue: report_issue::ReportIssue,
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
    /// The window's main area this frame (grid, photo or setup), which files
    /// dragged over the window are dropped into.
    drop_area: Option<egui::Rect>,
    /// The catalog or photo the session last named, kept there while it
    /// can't be opened (a catalog on an unplugged drive): saving the session
    /// meanwhile must not make the next launch a first one.
    last_path: Option<PathBuf>,
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
    /// Synchronize Folder: the scan under way or the plan waiting for review.
    folder_sync: folder_sync::FolderSync,
    /// The file operation under way, from its plan to its outcome.
    file_ops: file_ops::FileOps,
    /// The photo open in Develop when a synchronization was applied, and its
    /// file's new path if it was found moved: the reload that follows keeps
    /// the document, unsaved edit included, following the file (see
    /// `catalog_ready`).
    relink_open: Option<(crate::catalog::PhotoId, Option<PathBuf>)>,
    /// Preferences > Catalog: the user paused reading checksums, saved in
    /// the session.
    checksums_paused: bool,
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
        session: crate::app::session::Session,
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
        // interface, now and when it changes. The whole interface zooms rather
        // than only its fonts, so panels sized for their labels grow with them.
        follow_text_scale(&cc.egui_ctx);
        library::remember_drives_in(crate::storage::data_dir().join("drives.json"));
        let mut editor = Self::with_backend(
            &cc.egui_ctx,
            path,
            session,
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
            d.insert_temp(widgets::pinned_sections_id(), session.pinned.clone());
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
        // Shown on a first launch only to bring over Lightroom's catalog,
        // profiles and presets, when there are some; otherwise first-run
        // setup is done, and the empty Library says how to add photos.
        let show_onboarding =
            !session.onboarding_done && session_file.is_some() && onboarding::lightroom_here();
        let onboarding_done =
            session.onboarding_done || (session_file.is_some() && !show_onboarding);
        let place = CatalogPlace::of(&session);
        // Only a real session checks GitHub, not an isolated test.
        let updates = updates::Updates::new(&session, session_file.is_some().then_some(ctx));
        let adapter = match &backend {
            worker::RenderBackend::Gpu(Some(render_state)) => Some(render_state.adapter.get_info()),
            _ => None,
        };
        let report_issue = report_issue::ReportIssue::new(adapter.as_ref());
        let last = session.last_path.clone().filter(|p| p.exists());
        // A first launch opens the default catalog. A catalog used before that
        // is missing now (an unplugged drive) is not replaced by it.
        let first_catalog = session.last_path.is_none() && session_file.is_some();
        let (tx, rx) = mpsc::channel();
        let decode_cache = crate::decode_cache::DecodeCache::default();
        decode_cache.set_limit(u64::from(session.decoded_cache_size_gib()) << 30);
        decode_cache.set_folder(session.decoded_cache_folder.as_deref());
        let loader = worker::Loader::new(tx.clone(), ctx.clone(), decode_cache.clone());
        let renderer = worker::renderer_with_backend(tx.clone(), ctx.clone(), backend);
        let reference_loader =
            worker::reference_loader(tx.clone(), ctx.clone(), decode_cache.clone());
        let standard_preview_size = session.standard_preview_size();
        let discard_one_to_one_after = session.discard_one_to_one_after();
        let stand_ins = stand_in::StandIns::new(
            tx.clone(),
            ctx.clone(),
            crate::catalog::preview_cache::PreviewCache::path(),
        );
        let mut app = Self {
            #[cfg(target_os = "macos")]
            native_menu: false,
            activity: Default::default(),
            load: Default::default(),
            presets: PresetBrowser {
                favorites: crate::presets::load_favorites(),
                ..Default::default()
            },
            view: ViewState {
                monitor: monitor::MonitorProfile::from_session(
                    session.monitor,
                    session.monitor_srgb,
                ),
                crop_guides: crop_tool::CropGuides::from_session(&session.crop_guides),
                backdrop: backdrop::Backdrop::from_key(&session.develop_backdrop),
                border: backdrop::border_from_session(session.develop_border),
                chosen_aspect: inspector::aspect_from_key(&session.crop_aspect),
                ..Default::default()
            },
            preview: Default::default(),
            document: Default::default(),
            context: ctx.clone(),
            session_file,
            library: None,
            module: Module::Develop,
            chrome: Default::default(),
            tx,
            rx,
            loader,
            decode_cache,
            renderer,
            reference: Default::default(),
            reference_loader,
            clipboard: None,
            previous_settings: None,
            curves: Default::default(),
            lut_import: None,
            copy_groups: session.copy_groups.clone().unwrap_or_default(),
            collapsed: session.collapsed.clone(),
            solo: session.solo.clone(),
            pinned: session.pinned.clone(),
            maximized: session.maximized,
            restoring_maximized: if session.maximized {
                workspace::RestoreMaximized::Pending
            } else {
                workspace::RestoreMaximized::Done
            },
            panels: session.panels,
            tabbed: None,
            onboarding: onboarding::Onboarding::new(show_onboarding),
            onboarding_done,
            preferences: Default::default(),
            updates,
            report_issue,
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
            drop_area: None,
            last_path: session.last_path.clone(),
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
            folder_sync: Default::default(),
            file_ops: Default::default(),
            relink_open: None,
            checksums_paused: session.pause_checksums,
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
                } else if first_catalog {
                    app.open_default_catalog(ctx);
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
                    monitor: self.view.monitor.session_path(),
                    monitor_srgb: self.view.monitor.session_srgb(),
                    maximized: self.maximized,
                    collapsed: self.collapsed.clone(),
                    solo: self.solo.clone(),
                    pinned: self.pinned.clone(),
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
                    pause_checksums: self.checksums_paused,
                    library_layout: self.saved_layout.clone(),
                    copy_groups: Some(self.copy_groups.clone()),
                    crop_guides: self.view.crop_guides.to_session(),
                    crop_aspect: inspector::aspect_key(self.view.chosen_aspect).into(),
                    develop_backdrop: self.view.backdrop.key().into(),
                    develop_border: Some(self.view.border as u32),
                    raw_defaults: self.raw_defaults.settings().clone(),
                    standard_preview_size: Some(self.preview_builds.standard_size),
                    decoded_cache_size_gib: Some((self.decode_cache.limit() >> 30) as u32),
                    decoded_cache_folder: self.decode_cache.folder(),
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
            .or_else(|| self.last_path.clone())
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
    /// The folder waiting for the user to confirm its removal from the catalog.
    RemoveFolder(library::FolderRemoval),
    /// Photos waiting for the user to confirm Read Metadata from Files.
    ReadMetadata(Vec<PhotoId>),
    /// A folder change waiting for the user's answer.
    FolderQuestion(folder_locations::FolderQuestion),
    /// Find on Disk's review, held while the user decides whether to discard
    /// the open photo's unsaved change for it.
    DiscardForFind(Box<folder_sync::Review>),
}
impl eframe::App for Editor {
    /// What shows where no panel paints, e.g. behind the Library grid: the
    /// theme's darkest grey rather than eframe's near-black.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // eframe gives no context here; the palette last applied to it.
        self.themes.applied().gray(12).to_normalized_gamma_f32()
    }
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        #[cfg(target_os = "macos")]
        crate::platform::titlebar::configure(frame, ui.ctx());
        // AppKit moves the traffic lights back during layout passes.
        fastframe_titlebar::align_traffic_lights(frame, ui.ctx(), workspace::BAR_HEIGHT);
        #[cfg(windows)]
        crate::platform::taskbar_icon(frame);
        self.draw(ui);
        if self.view.monitor.follows_system() {
            if self.view.system_display.update(frame) {
                self.schedule();
            }
            // Recheck for a newly assigned profile while nothing else repaints.
            if cfg!(windows) {
                ui.ctx().request_repaint_after(monitor::RECHECK);
            }
        }
        display::paint(ui, frame, self.view.monitor.device_rgb());
    }
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        workspace::clipboard_shortcut_keys(ctx, raw_input);
    }
    // fastframe-titlebar turns on eframe's glow feature, which adds the context.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        #[cfg(target_os = "macos")]
        if self.native_menu {
            crate::platform::menu::clear();
        }
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

/// Runs the desktop app. A panic that ends it is an error, and every panic is
/// recorded in the crash log; see [`crash_log`].
pub fn run(path: Option<PathBuf>, launch: crate::updates::Launch) -> anyhow::Result<()> {
    crash_log::install();
    crash_log::run_catching(|| run_window(path, launch))
}

fn run_window(path: Option<PathBuf>, launch: crate::updates::Launch) -> anyhow::Result<()> {
    let session = crate::app::session::load_session();
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
            // On Windows egui-winit applies the inner size after creating the
            // window. Resizing an already maximized window can leave it at that
            // size with its maximized flag still set (#553). Let the first
            // frame maximize it after sizing instead. X11 also needs that
            // request because its window manager waits for the window to show.
            .with_maximized(!cfg!(windows) && workspace::KEEPS_MAXIMIZED && session.maximized)
            // On macOS the workspace bar is the title bar, under the traffic
            // lights; elsewhere the window keeps its decorations.
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false),
        // fastframe-titlebar turns on eframe's glow renderer too; previews
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
            #[allow(unused_mut)]
            let mut editor = Editor::new(cc, path, session, launch);
            #[cfg(target_os = "macos")]
            {
                let ctx = cc.egui_ctx.clone();
                editor.native_menu = crate::platform::menu::install(move || ctx.request_repaint());
                editor.update_native_menu(&cc.egui_ctx);
            }
            Ok(Box::new(editor))
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
        setup.instance_descriptor.backends = crate::develop::gpu::backends();
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
mod backdrop;
mod basic_keys;
mod before_after;
mod brush_scroll;
mod bulk_import;
pub use bulk_import::import_profiles;
mod catalog;
mod chrome;
mod clipping;
mod color_grading;
mod commands;
mod crash_log;
mod crop_tool;
mod curve_menu;
mod dialogs;
mod display;
mod export;
mod file_ops;
pub(crate) mod folder_locations;
mod folder_sync;
mod guided_tool;
mod history_panel;
mod inspector;
pub(crate) mod library;
mod lut_import;
mod mask_tool;
mod monitor;
mod naming;
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
mod report_issue;
mod retouch_tool;
mod side_tabs;
mod stand_in;
mod stroke_outline;
mod subject_mask;
mod targeted_tool;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod ui_snapshot;
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
#[cfg(any(target_os = "macos", test))]
mod native_menu;

/// Zooms the interface to the desktop's text size, read through the portal
/// under Wayland (1 elsewhere), and follows its changes on fastframe-text's
/// watch thread, which lives as long as the process.
fn follow_text_scale(ctx: &egui::Context) {
    let scale = fastframe_text::text_scale();
    ctx.set_zoom_factor(scale);
    let watched = ctx.clone();
    match fastframe_text::watch::watch_text_scale(scale, move |scale| {
        watched.set_zoom_factor(scale);
    }) {
        Ok(()) | Err(fastframe_text::watch::WatchError::Unsupported) => {}
        Err(error) => eprintln!("RAWmakase: not following the desktop's text size: {error}"),
    }
}

/// Inter, with tabular figures so values keep their width as they change,
/// and an installed face for every script and symbol it lacks (fastframe-fonts),
/// rendered as the desktop renders text.
fn install_fonts(ctx: &egui::Context, text: &fastframe_text::TextRendering) {
    let mut fonts = fastframe_fonts::FontSetup::default().definitions();
    text.apply_to(&mut fonts);
    ctx.set_fonts(fonts);
}
