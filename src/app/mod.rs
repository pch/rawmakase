//! Desktop composition and presentation, built on the crate's domain APIs.
//!
//! `Editor` coordinates the workspace; `state` separates the document, preview,
//! viewport and preset browser. `workflow` starts operations, `events` accepts
//! worker results, and `workspace` composes each frame. Task cancellation, history,
//! foreground activity and save policy have their own modules.
//!
//! File formats, persistence and pixel processing belong in the domain modules.
//! See `docs/code-map.md` for panel, library and worker implementation locations.
use crate::app::ui::design::{Tone, button, space, text};
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
    presets: PresetBrowser,
    view: ViewState,
    /// The name the controls drawn this frame gave the History step they make.
    step_name: crate::app::develop::step_name::StepName,
    preview: PreviewState,
    document: Document,
    context: egui::Context,
    /// The session: the preferences it holds, the place it remembers, and its file.
    session: crate::app::preferences::session::SessionState,
    /// What waits for the next catalog: the launch place, a photo from outside.
    waiting: crate::app::catalog::Waiting,
    library: Option<Box<crate::app::library::Library>>,
    /// The module shown: the Library grid, or Develop.
    module: Module,
    /// Whether the module's chrome is hidden, leaving the photo alone (see
    /// `chrome`); not saved, so every launch opens in the normal window.
    chrome: crate::app::shell::chrome::Chrome,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    loader: worker::Loader,
    /// Shared size policy for Develop, neighbour prefetch and Reference View.
    decode_cache: crate::decode_cache::DecodeCache,
    /// AI Denoise's queue, worker and cache, shared with exports.
    ai_denoise: crate::ai_denoise::Service,
    /// Loads the finished AI Denoise results the Library's Loupe shows.
    ai_loupe: worker::Latest<crate::app::develop::ai_denoise::CachedLoad>,
    renderer: worker::Renderer,
    /// Develop's Reference View, and the worker developing its photo.
    reference: crate::app::develop::reference::ReferenceView,
    reference_loader: Latest<worker::ReferenceJob>,
    /// Develop's HDR preview and its worker.
    hdr_preview: crate::app::develop::hdr_preview::HdrPreview,
    /// The window's wgpu state this frame, for the HDR preview's texture.
    render_state: Option<eframe::egui_wgpu::RenderState>,
    clipboard: Option<crate::app::develop::settings_transfer::Clipboard>,
    /// The settings of the photo open before this one, for Paste from Previous.
    previous_settings: Option<crate::app::develop::settings_transfer::Settings>,
    /// The Point Curve menu's saved curves and its Save window.
    curves: crate::app::develop::panels::curve_menu::CurveMenu,
    /// The Import .cube LUTs window's colour space, while it is open.
    lut_import: Option<crate::camera_profiles::LutSpace>,
    /// Reopening it maximized, until the window manager has done so.
    restoring_maximized: crate::app::shell::workspace::RestoreMaximized,
    /// The panels each module shows (Tab, Shift+Tab, F6–F8).
    panels: crate::app::shell::panels::WorkspacePanels,
    /// What the last Tab or Shift+Tab hid, and in which module.
    tabbed: Option<(Module, crate::app::shell::panels::Tabbed)>,
    onboarding: crate::app::catalog::onboarding::Onboarding,
    preferences: preferences::Preferences,
    updates: crate::app::shell::updates::Updates,
    /// Report an Issue…: its dialog and the system probe.
    report_issue: crate::app::shell::report_issue::ReportIssue,
    themes: crate::app::ui::theme::Themes,
    exports: export::Exports,
    /// Build Standard-Sized Previews, in the background.
    preview_builds: crate::app::library::preview_build::PreviewBuilds,
    /// Stored Standard previews read for the photo opening and its neighbour.
    stand_ins: crate::app::develop::stand_in::StandIns,
    autosave: crate::app::develop::autosave::Autosave,
    /// The window's main area this frame (grid, photo or setup), which files
    /// dragged over the window are dropped into.
    drop_area: Option<egui::Rect>,
    /// Cancels the prefetch started for the photo on screen.
    prefetch_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// What the status line and the top bar say.
    messages: crate::app::shell::messages::Messages,
    /// What a running catalog import or open is doing.
    catalog_work: Option<String>,
    /// Progress of a running profile or preset import.
    importing: Option<std::sync::Arc<crate::app::presets::bulk_import::ImportProgress>>,
    profile_download: crate::app::develop::profile_download::Downloads,
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
    /// Cmd+Z across Library and Develop.
    undo_log: crate::app::shell::undo::UndoLog,
    /// Select Subject and Select Background: the running selection, the selection
    /// model's install and what the drawer asks.
    selection: crate::app::develop::tools::subject_mask::Selection,
    /// Lensfun's lens database: installing, updating and removing it.
    lensfun: crate::app::develop::lensfun::Lensfun,
    /// Synchronize Folder: the scan under way or the plan waiting for review.
    folder_sync: crate::app::catalog::folder_sync::FolderSync,
    /// The file operation under way, from its plan to its outcome.
    file_ops: file_ops::FileOps,
    /// The RAW the Loupe last started loading, so one that fails is not
    /// loaded again every frame.
    loupe_tried: Option<PhotoId>,
    /// Preferences > Raw Defaults, ready to apply; shared with the loader and the
    /// Library's previews.
    raw_defaults: std::sync::Arc<crate::raw_defaults::DevelopDefaults>,
    /// The profile library as unedited photos' previews were last built from
    /// (`camera_profiles::library_fingerprint`): read at start and after every
    /// profile import, rather than scanned for each preview.
    profile_library: String,
    /// Shared automation queue and independently configured input adapters.
    controls: crate::app::shell::automation::Hub,
    automation: crate::app::shell::commands::Automation,
}
impl Editor {
    pub(crate) fn new(
        cc: &eframe::CreationContext<'_>,
        path: Option<PathBuf>,
        session: crate::app::preferences::session::Session,
        launch: crate::updates::Launch,
    ) -> Self {
        // The desktop's hinting and antialiasing, read once (a D-Bus call on
        // Linux) and applied to the fonts here and to the visuals below.
        let text = fastframe_text::detect();
        if let Some(render_state) = &cc.wgpu_render_state {
            survive_surface_errors(&render_state.device);
        }
        install_fonts(&cc.egui_ctx, &text);
        crate::app::ui::icons::install(&cc.egui_ctx);
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
        editor.controls = crate::app::shell::automation::Hub::start(&cc.egui_ctx);
        // 1:1 previews left unused past the chosen time go, as in Lightroom.
        editor.expire_previews();
        editor.updates.launched(launch, &mut editor.messages);
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
        session: crate::app::preferences::session::Session,
        session_file: Option<PathBuf>,
    ) -> Self {
        Self::with_backend(ctx, path, session, session_file, worker::RenderBackend::Cpu)
    }
    fn with_backend(
        ctx: &egui::Context,
        path: Option<PathBuf>,
        session: crate::app::preferences::session::Session,
        session_file: Option<PathBuf>,
        backend: worker::RenderBackend,
    ) -> Self {
        crate::app::ui::theme::apply(
            ctx,
            crate::app::ui::theme::Palette::DEFAULT,
            &fastframe_text::TextRendering::platform_default(),
        );
        // Cmd/Ctrl + and − zoom the photo, not the whole interface.
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        ctx.data_mut(|d| {
            d.insert_temp(
                crate::app::ui::widgets::collapsed_sections_id(),
                session.collapsed.clone(),
            );
            d.insert_temp(
                crate::app::ui::widgets::solo_sections_id(),
                session.solo.clone(),
            );
        });
        ctx.all_styles_mut(|style| {
            style.spacing.item_spacing = Vec2::new(space::M, space::S);
            style.spacing.button_padding = button::PADDING;
            style.spacing.indent = space::XL;
            style.text_styles.insert(
                egui::TextStyle::Body,
                egui::FontId::proportional(text::BODY),
            );
            style.text_styles.insert(
                egui::TextStyle::Button,
                egui::FontId::proportional(text::BODY),
            );
            style.text_styles.insert(
                egui::TextStyle::Small,
                egui::FontId::proportional(text::SMALL),
            );
        });
        // Only a real session (not an isolated test) shows first-run setup.
        // Shown on a first launch only to bring over Lightroom's catalog,
        // profiles and presets, when there are some; otherwise first-run
        // setup is done, and the empty Library says how to add photos.
        let show_onboarding = !session.onboarding_done
            && session_file.is_some()
            && crate::app::catalog::onboarding::lightroom_here();
        let onboarding_done =
            session.onboarding_done || (session_file.is_some() && !show_onboarding);
        let place = CatalogPlace::of(&session);
        // Only a real session checks GitHub, not an isolated test.
        let updates = crate::app::shell::updates::Updates::new(
            &session,
            session_file.is_some().then_some(ctx),
        );
        let adapter = match &backend {
            worker::RenderBackend::Gpu(Some(render_state)) => Some(render_state.adapter.get_info()),
            _ => None,
        };
        let report_issue = crate::app::shell::report_issue::ReportIssue::new(adapter.as_ref());
        let last = session.last_path.clone().filter(|p| p.exists());
        // A first launch opens the default catalog. A catalog used before that
        // is missing now (an unplugged drive) is not replaced by it.
        let first_catalog = session.last_path.is_none() && session_file.is_some();
        let (tx, rx) = mpsc::channel();
        let copy_groups = session.copy_groups();
        let decode_cache = crate::decode_cache::DecodeCache::default();
        decode_cache.set_limit(u64::from(session.decoded_cache_size_gib()) << 30);
        crate::catalog::preview_cache::set_thumbnail_budget(
            u64::from(session.thumbnail_cache_size_mib()) << 20,
        );
        decode_cache.set_folder(session.decoded_cache_folder.as_deref());
        let ai_cache = crate::ai_cache::AiCache::default();
        ai_cache.set_folder(session.decoded_cache_folder.as_deref());
        let ai_denoise = crate::ai_denoise::Service::new(ai_cache);
        let ai_loupe = crate::app::develop::ai_denoise::cached_loader(tx.clone(), ctx.clone());
        let (stored_tx, stored_ctx) = (std::sync::Mutex::new(tx.clone()), ctx.clone());
        ai_denoise.on_stored(std::sync::Arc::new(move |path| {
            let _ = stored_tx
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .send(worker::Event::AiDenoiseStored(path.to_path_buf()));
            stored_ctx.request_repaint();
        }));
        let loader = worker::Loader::new(tx.clone(), ctx.clone(), decode_cache.clone());
        let renderer = worker::renderer_with_backend(tx.clone(), ctx.clone(), backend);
        let reference_loader =
            worker::reference_loader(tx.clone(), ctx.clone(), decode_cache.clone());
        let standard_preview_size = session.standard_preview_size();
        let discard_one_to_one_after = session.discard_one_to_one_after();
        let stand_ins = crate::app::develop::stand_in::StandIns::new(
            tx.clone(),
            ctx.clone(),
            crate::catalog::preview_cache::PreviewCache::path(),
        );
        let session_state = crate::app::preferences::session::SessionState::new(
            &session,
            session_file,
            place.clone(),
            onboarding_done,
            copy_groups.unwrap_or_default(),
        );
        let mut app = Self {
            #[cfg(target_os = "macos")]
            native_menu: false,
            activity: Default::default(),
            presets: PresetBrowser {
                favorites: crate::presets::load_favorites(),
                ..Default::default()
            },
            view: ViewState {
                monitor: crate::app::preferences::monitor::MonitorProfile::from_session(
                    session.monitor,
                    session.monitor_srgb,
                ),
                crop_guides: crate::app::develop::tools::crop_tool::CropGuides::from_session(
                    &session.crop_guides,
                ),
                chosen_aspect: crate::app::develop::panels::inspector::aspect_from_key(
                    &session.crop_aspect,
                ),
                ..Default::default()
            },
            step_name: Default::default(),
            preview: Default::default(),
            document: Default::default(),
            context: ctx.clone(),
            session: session_state,
            library: None,
            module: Module::Develop,
            chrome: Default::default(),
            tx,
            rx,
            loader,
            decode_cache,
            ai_denoise,
            ai_loupe,
            renderer,
            reference: Default::default(),
            reference_loader,
            hdr_preview: crate::app::develop::hdr_preview::HdrPreview::new(ctx.clone()),
            render_state: None,
            clipboard: None,
            previous_settings: None,
            curves: Default::default(),
            lut_import: None,
            restoring_maximized: if session.maximized {
                crate::app::shell::workspace::RestoreMaximized::Pending
            } else {
                crate::app::shell::workspace::RestoreMaximized::Done
            },
            panels: session.panels,
            tabbed: None,
            onboarding: crate::app::catalog::onboarding::Onboarding::new(show_onboarding),
            preferences: Default::default(),
            updates,
            report_issue,
            themes: crate::app::ui::theme::Themes::new(
                ctx,
                // Sessions from before `theme_chosen` saved only a palette.
                (session.theme_chosen || session.theme.is_some()).then(|| session.theme.clone()),
                fastframe_text::TextRendering::platform_default(),
            ),
            exports: Default::default(),
            preview_builds: crate::app::library::preview_build::PreviewBuilds::new(
                standard_preview_size,
                discard_one_to_one_after,
            ),
            stand_ins,
            autosave: Default::default(),
            waiting: crate::app::catalog::Waiting::restoring(place),
            drop_area: None,
            prefetch_cancel: Default::default(),
            messages: crate::app::shell::messages::Messages::starting_with(
                "Pick a photo in the Library to begin",
            ),
            catalog_work: None,
            importing: None,
            profile_download: crate::app::develop::profile_download::Downloads::with_dismissed(
                session.dismissed_community_profiles.clone(),
            ),
            close_confirm: false,
            close_after_work: false,
            quit_by: None,
            close_anyway: false,
            modal: None,
            undo_log: Default::default(),
            selection: Default::default(),
            lensfun: crate::app::develop::lensfun::Lensfun::suggesting(
                !session.no_lensfun_suggestions,
            ),
            folder_sync: Default::default(),
            file_ops: Default::default(),
            loupe_tried: None,
            raw_defaults: std::sync::Arc::new(crate::raw_defaults::DevelopDefaults::load(
                session.raw_defaults.clone(),
            )),
            profile_library: crate::camera_profiles::library_fingerprint(),
            controls: crate::app::shell::automation::Hub::inactive(),
            automation: crate::app::shell::commands::Automation::default(),
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
        self.session.write(
            crate::app::preferences::session::Session {
                monitor: self.view.monitor.session_path(),
                monitor_srgb: self.view.monitor.session_srgb(),
                panels: self.panels,
                no_update_checks: !self.updates.automatic,
                skipped_version: self.updates.skipped.clone(),
                theme: self.themes.chosen().flatten(),
                theme_chosen: self.themes.chosen().is_some(),
                no_lensfun_suggestions: !self.lensfun.suggest,
                dismissed_community_profiles: self.profile_download.dismissed().clone(),
                crop_guides: self.view.crop_guides.to_session(),
                crop_aspect: crate::app::develop::panels::inspector::aspect_key(
                    self.view.chosen_aspect,
                )
                .into(),
                raw_defaults: self.raw_defaults.settings().clone(),
                standard_preview_size: Some(self.preview_builds.standard_size),
                decoded_cache_size_gib: Some((self.decode_cache.limit() >> 30) as u32),
                thumbnail_cache_size_mib: Some(
                    (crate::catalog::preview_cache::thumbnail_budget() >> 20) as u32,
                ),
                decoded_cache_folder: self.decode_cache.folder(),
                one_to_one_discard_days: Some(
                    self.preview_builds.discard_one_to_one_after.unwrap_or(0),
                ),
                ..Default::default()
            },
            self.session_path(),
        )
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
                let CatalogLocation::File(path) = l.session.catalog().location();
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
    fn of(session: &crate::app::preferences::session::Session) -> Self {
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
/// The dialog that blocks the editor, one at a time, kept in `Editor::modal`.
/// Dialogs owned by a subsystem (Preferences, Export, Shortcuts, onboarding,
/// Save Curve) keep their state there instead.
enum Modal {
    /// Copy, Synchronize or New Preset settings, and the groups being chosen.
    CopySettings(crate::app::develop::settings_transfer::CopyDialog),
    /// A preset made here being renamed.
    RenamePreset(crate::app::presets::user_presets::PresetRename),
    /// The virtual copy waiting for the user to confirm its removal.
    RemoveCopy(PhotoId),
    /// The folder waiting for the user to confirm its removal from the catalog.
    RemoveFolder(library::FolderRemoval),
    /// What Export as Catalog wrote, and what it could not carry.
    CatalogExported(Box<crate::app::catalog::export_catalog::Exported>),
    /// Photos waiting for the user to confirm Read Metadata from Files.
    ReadMetadata(Vec<PhotoId>),
    /// A folder change waiting for the user's answer.
    FolderQuestion(crate::app::catalog::folder_locations::FolderQuestion),
    /// Find on Disk's review, held while the user decides whether to discard
    /// the open photo's unsaved change for it.
    DiscardForFind(Box<crate::app::catalog::folder_sync::Review>),
}
impl eframe::App for Editor {
    /// What shows where no panel paints, e.g. behind the Library grid: the
    /// theme's darkest grey rather than eframe's near-black.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // eframe gives no context here; the palette last applied to it.
        self.themes
            .applied()
            .tone(Tone::DEEP)
            .to_normalized_gamma_f32()
    }
    /// Runs before every `ui`, and alone while the window is occluded or
    /// minimized: macOS stops drawing a window behind other apps, and eframe
    /// then calls only this. Control requests and the work they wait on go on.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.background(ctx);
    }
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        #[cfg(target_os = "macos")]
        crate::platform::titlebar::configure(frame, ui.ctx());
        // AppKit moves the traffic lights back during layout passes.
        fastframe_titlebar::align_traffic_lights(
            frame,
            ui.ctx(),
            crate::app::shell::workspace::BAR_HEIGHT,
        );
        #[cfg(windows)]
        crate::platform::taskbar_icon(frame);
        self.view.hdr_display = crate::app::shell::display::HdrDisplay::of(frame);
        self.render_state = frame.wgpu_render_state().cloned();
        self.view.hdr_shown = false;
        self.draw(ui);
        // HDR previews the frame didn't draw are of views that are gone.
        self.hdr_preview.sweep(self.render_state.as_ref());
        if self.view.monitor.follows_system() {
            if self.view.system_display.update(frame) {
                self.schedule();
            }
            // Recheck for a newly assigned profile while nothing else repaints.
            if cfg!(windows) {
                ui.ctx()
                    .request_repaint_after(crate::app::preferences::monitor::RECHECK);
            }
        }
        // The display's headroom follows its brightness, and nothing repaints an idle
        // window when it moves: read it again while an HDR photo is due.
        if self.view.hdr_shown {
            ui.ctx()
                .request_repaint_after(crate::app::preferences::monitor::RECHECK);
        }
        crate::app::shell::display::paint(
            ui,
            frame,
            self.view.monitor.device_rgb(),
            self.view.hdr_shown,
        );
    }
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        crate::app::shell::workspace::clipboard_shortcut_keys(ctx, raw_input);
        #[cfg(target_os = "macos")]
        crate::platform::titlebar::refresh_window_state(ctx, raw_input);
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
/// recorded in the crash log; see [`crash_log`](shell::crash_log).
pub fn run(path: Option<PathBuf>, launch: crate::updates::Launch) -> anyhow::Result<()> {
    crate::app::shell::crash_log::install();
    crate::app::shell::crash_log::run_catching(|| run_window(path, launch))
}

fn run_window(path: Option<PathBuf>, launch: crate::updates::Launch) -> anyhow::Result<()> {
    let session = crate::app::preferences::session::load_session();
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
            .with_maximized(!cfg!(windows) && session.maximized)
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
/// that error; on our own event loop eframe only logs it. wgpu calls most driver
/// failures "Parent device is lost" and only logs the call that failed (#768).
struct EframeErrors;
static EFRAME_ERROR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
static DRIVER_ERROR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
impl EframeErrors {
    fn take() -> Option<String> {
        let error = EFRAME_ERROR
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()?;
        let driver = DRIVER_ERROR
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        Some(match driver {
            Some(driver) => format!("{error}\n\n{driver}"),
            None => error,
        })
    }
}
impl log::Log for EframeErrors {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Error
            && (metadata.target().starts_with("eframe")
                || metadata.target().starts_with("wgpu_hal"))
    }
    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        if record.target().starts_with("wgpu_hal") {
            *DRIVER_ERROR
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(record.args().to_string());
        } else if let Some(error) = record
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
/// is the one the system lists first; see `display_adapter`.
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
    // On macOS the window surface holds 16-bit floats where the GPU offers them, so
    // an HDR photo can be shown above SDR white (`platform::display::Layer`); egui
    // keeps writing sRGB-encoded values, which the layer reads as extended sRGB.
    #[cfg(target_os = "macos")]
    {
        options.surface_format_selector = Some(std::sync::Arc::new(|formats| {
            formats
                .contains(&wgpu::TextureFormat::Rgba16Float)
                .then_some(wgpu::TextureFormat::Rgba16Float)
        }));
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

pub use crate::app::develop::ai_denoise::user_ai_denoise_cache;
pub use crate::app::preferences::raw_defaults::user_raw_defaults;
pub use crate::app::presets::bulk_import::import_profiles;
pub(crate) mod catalog;
pub(crate) mod develop;
mod export;
mod file_ops;
pub(crate) mod library;
pub(crate) mod preferences;
mod presets;
mod shell;
#[cfg(test)]
mod testing;
#[cfg(test)]
mod tests;
mod ui;
mod view;
mod work;
#[cfg(test)]
mod work_policy_tests;
pub(crate) mod worker;

use crate::app::develop::state::{Document, PresetBrowser, PreviewState, ViewState};
use crate::edit_session::{history, save_state};
use library::Module;

mod task;

mod activity;

mod events;

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
