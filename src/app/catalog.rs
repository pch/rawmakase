use super::Editor;
use super::dialogs::{CatalogDialog, FolderAction};
use super::folder_locations::{FolderQuestion, folder_added, reopened};
use super::widgets::confirm_modal;
use super::worker::Event;
use crate::app::Module;
use crate::catalog::{CatalogLocation, PhotoId};
use eframe::egui;
use std::path::PathBuf;

impl Editor {
    /// The catalog RAWmakase keeps in its data folder, beside the session:
    /// the one a first launch opens, and the setup view and an empty Library
    /// offer.
    pub(super) fn default_catalog(&self) -> PathBuf {
        self.session_file
            .as_deref()
            .and_then(std::path::Path::parent)
            .map_or_else(crate::storage::data_dir, std::path::Path::to_path_buf)
            .join("Photos.rawmakase")
    }
    /// Whether no work in progress stops a catalog change and the open edit
    /// is saved; it is saved only when nothing is in progress.
    pub(super) fn ready_for_catalog(&mut self) -> bool {
        !self.activity.is_busy() && self.flush()
    }
    pub(super) fn load_catalog(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.open_catalog_file(path, false, ctx);
    }
    /// Opens the default catalog, making it first if there is none yet, so
    /// photos can be added without choosing where a catalog goes.
    pub(super) fn open_default_catalog(&mut self, ctx: &egui::Context) {
        self.open_catalog_file(self.default_catalog(), true, ctx);
    }
    /// Imports Lightroom catalog `source`, or one the user picks, into a new
    /// catalog beside the default one without asking where, and opens it.
    pub(super) fn import_lightroom_here(&mut self, source: Option<PathBuf>, ctx: &egui::Context) {
        if !self.ready_for_catalog() || !self.activity.begin_dialog() {
            return;
        }
        let place = self.default_catalog();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        super::task::spawn(
            tx,
            ctx.clone(),
            move |tx| {
                let result = (|| -> anyhow::Result<Option<CatalogLocation>> {
                    let Some(source) = source.or_else(|| {
                        rfd::FileDialog::new()
                            .add_filter("Lightroom catalog", &["lrcat"])
                            .pick_file()
                    }) else {
                        return Ok(None);
                    };
                    let stem = source.file_stem().unwrap_or_default().to_string_lossy();
                    // A name not taken yet: "Catalog", "Catalog 2"…
                    let destination = (1..)
                        .map(|n| match n {
                            1 => place.with_file_name(format!("{stem}.rawmakase")),
                            n => place.with_file_name(format!("{stem} {n}.rawmakase")),
                        })
                        .find(|p| !p.exists())
                        .expect("a free name");
                    let _ = tx.send(Event::CatalogWorking(format!(
                        "Importing {}…",
                        source.file_name().unwrap_or_default().to_string_lossy()
                    )));
                    ctx.request_repaint();
                    Ok(Some(CatalogLocation::File(
                        crate::catalog::lightroom::import_lightroom(&source, &destination)?,
                    )))
                })();
                let event = match result {
                    Ok(Some(location)) => {
                        reopened(&location, false, &Default::default(), &[], &ctx)
                    }
                    Ok(None) => Event::DialogClosed,
                    Err(e) => Event::CatalogReady(Err(format!("{e:#}"))),
                };
                let _ = tx.send(event);
            },
            |tx, error| {
                let _ = tx.send(Event::CatalogReady(Err(error)));
            },
        );
    }
    fn open_catalog_file(&mut self, path: PathBuf, create: bool, ctx: &egui::Context) {
        if !self.ready_for_catalog() {
            return;
        }
        if !self.activity.begin_dialog() {
            return;
        }
        self.messages.note("Opening catalog…");
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        super::task::spawn(
            tx,
            ctx.clone(),
            move |tx| {
                let result = (|| -> anyhow::Result<_> {
                    if create && !path.exists() {
                        crate::catalog::Catalog::create(&path)?;
                    }
                    crate::catalog_session::CatalogSession::open(&CatalogLocation::File(path))
                })()
                .map(|opened| Box::new(crate::app::library::Library::new(opened, ctx.clone())))
                .map_err(|e| format!("{e:#}"));
                let _ = tx.send(Event::CatalogReady(result));
            },
            |tx, error| {
                let _ = tx.send(Event::CatalogReady(Err(error)));
            },
        );
    }
    pub(super) fn catalog_dialog(&mut self, kind: CatalogDialog, ctx: &egui::Context) {
        if !self.ready_for_catalog() {
            return;
        }
        if !self.activity.begin_dialog() {
            return;
        }
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let current = self
            .library
            .as_ref()
            .map(|l| l.session.catalog.location().clone());
        super::task::spawn(
            tx,
            ctx.clone(),
            move |tx| {
                // Sidecars of the folder added that could not be read, and
                // folders it skipped.
                let mut report = crate::catalog::SidecarReport::default();
                let mut conflicts = Vec::new();
                let relinks = matches!(
                    kind,
                    CatalogDialog::Folder(
                        FolderAction::RelinkRoot(_) | FolderAction::RelinkFolder(..)
                    )
                );
                let result = (|| -> anyhow::Result<Option<CatalogLocation>> {
                    Ok(match kind {
                        CatalogDialog::Create => {
                            let Some(path) = catalog_file_dialog()
                                .set_file_name("Photos.rawmakase")
                                .save_file()
                            else {
                                return Ok(None);
                            };
                            crate::catalog::Catalog::create(&path)?;
                            Some(CatalogLocation::File(path))
                        }
                        CatalogDialog::Open => {
                            catalog_file_dialog().pick_file().map(CatalogLocation::File)
                        }
                        CatalogDialog::ImportLightroom => {
                            let Some(source) = rfd::FileDialog::new()
                                .add_filter("Lightroom catalog", &["lrcat"])
                                .pick_file()
                            else {
                                return Ok(None);
                            };
                            let Some(destination) = catalog_file_dialog()
                                .set_file_name(format!(
                                    "{}.rawmakase",
                                    source.file_stem().unwrap_or_default().to_string_lossy()
                                ))
                                .save_file()
                            else {
                                return Ok(None);
                            };
                            let _ = tx.send(Event::CatalogWorking(format!(
                                "Importing {}…",
                                source.file_name().unwrap_or_default().to_string_lossy()
                            )));
                            ctx.request_repaint();
                            Some(CatalogLocation::File(
                                crate::catalog::lightroom::import_lightroom(&source, &destination)?,
                            ))
                        }
                        CatalogDialog::Folder(action) => {
                            crate::platform::network::prepare_filesystem_bridge();
                            let current =
                                current.ok_or_else(|| anyhow::anyhow!("Open a catalog first"))?;
                            let mut cat = crate::catalog::Catalog::open(&current)?;
                            let mut dialog = rfd::FileDialog::new().set_title(match action {
                                FolderAction::Add => "Add photo folder",
                                _ => "Find missing folder on this computer",
                            });
                            // Where another computer has it, when that is here too.
                            if let Some(there) = suggestion(&cat, &action) {
                                dialog = dialog.set_directory(there);
                            }
                            let Some(path) = dialog.pick_folder() else {
                                return Ok(None);
                            };
                            let ask = |question| {
                                let _ = tx.send(Event::FolderQuestion(Box::new(question)));
                            };
                            match action {
                                FolderAction::Add => {
                                    let added = cat.import_folder(
                                        &path,
                                        &crate::catalog_session::new_photo_defaults(),
                                        &[],
                                    )?;
                                    if !added.ambiguous.is_empty() {
                                        ask(FolderQuestion::Ambiguous {
                                            catalog: current,
                                            folder: path,
                                            open: added.ambiguous,
                                            chosen: Vec::new(),
                                        });
                                        return Ok(None);
                                    }
                                    report = added.report;
                                    conflicts = added.conflicts;
                                }
                                FolderAction::RelinkRoot(root) => {
                                    let overrides = cat.root_overrides(root)?;
                                    if !overrides.is_empty() {
                                        ask(FolderQuestion::Overrides {
                                            catalog: current,
                                            root,
                                            relative: String::new(),
                                            path,
                                            overrides,
                                        });
                                        return Ok(None);
                                    }
                                    cat.relink_root(root, &path)?
                                }
                                FolderAction::RelinkFolder(root, relative) => {
                                    let overrides = cat.folder_overrides(root, &relative)?;
                                    if !overrides.is_empty() {
                                        ask(FolderQuestion::Overrides {
                                            catalog: current,
                                            root,
                                            relative,
                                            path,
                                            overrides,
                                        });
                                        return Ok(None);
                                    }
                                    cat.relink_folder_at(
                                        root,
                                        &relative,
                                        &path,
                                        crate::catalog::Overrides::Keep,
                                    )?
                                }
                            }
                            Some(current)
                        }
                    })
                })();
                if let Ok(Some(location)) = &result {
                    let _ = tx.send(Event::CatalogWorking(format!(
                        "Opening {}…",
                        location.name()
                    )));
                    ctx.request_repaint();
                }
                let event = match result {
                    Ok(Some(location)) => reopened(&location, relinks, &report, &conflicts, &ctx),
                    Ok(None) => Event::DialogClosed,
                    Err(e) => Event::CatalogReady(Err(format!("{e:#}"))),
                };
                let _ = tx.send(event);
            },
            |tx, error| {
                let _ = tx.send(Event::CatalogReady(Err(error)));
            },
        );
    }
    /// What was dropped onto the window: one photo opens in Develop as
    /// [`Self::open`] does; several photos and folders have their folders
    /// added, all at once; a catalog among them is opened instead.
    pub(super) fn dropped(&mut self, paths: Vec<PathBuf>) {
        let catalog = paths.iter().find(|p| {
            p.extension()
                .is_some_and(|e| e == "rawmakase" || e == "lrcat")
        });
        if let Some(catalog) = catalog {
            self.open(catalog.clone());
            return;
        }
        if paths.len() == 1 {
            self.open(paths[0].clone());
            return;
        }
        let mut folders: Vec<PathBuf> = Vec::new();
        for path in paths {
            let path = path.canonicalize().unwrap_or(path);
            let folder = if path.is_dir() {
                path
            } else if let Some(parent) = path.parent() {
                parent.to_path_buf()
            } else {
                continue;
            };
            if !folders.contains(&folder) {
                folders.push(folder);
            }
        }
        self.module = Module::Library;
        let Some(current) = self
            .library
            .as_ref()
            .map(|l| l.session.catalog.location().clone())
        else {
            self.messages.warn("Open or create a catalog to add photos");
            return;
        };
        // A folder change: closing the window waits for it.
        if folders.is_empty() || !self.ready_for_catalog() || !self.activity.begin_folder_change() {
            return;
        }
        self.messages
            .note(format!("Adding {} folders to the Library…", folders.len()));
        let tx = self.tx.clone();
        let ctx = self.context.clone();
        super::task::spawn(
            tx,
            ctx.clone(),
            move |tx| {
                let mut imported = Vec::new();
                let result = (|| -> anyhow::Result<_> {
                    let mut catalog = crate::catalog::Catalog::open(&current)?;
                    let defaults = crate::catalog_session::new_photo_defaults();
                    let mut report = crate::catalog::SidecarReport::default();
                    let mut conflicts = Vec::new();
                    // Folders the catalog has twice over (Add Folder… asks
                    // which), and ones that failed: each folder is added on
                    // its own, so the others still are.
                    let mut skipped = Vec::new();
                    let mut failed = Vec::new();
                    for folder in &folders {
                        let name = folder
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned();
                        match catalog.import_folder(folder, &defaults, &[]) {
                            Ok(added) if !added.ambiguous.is_empty() => skipped.push(name),
                            Ok(added) => {
                                imported.push(folder.clone());
                                report.unreadable.extend(added.report.unreadable);
                                report.ignored.extend(added.report.ignored);
                                conflicts.extend(added.conflicts);
                            }
                            Err(e) => failed.push(format!("{name} ({e:#})")),
                        }
                    }
                    drop(catalog);
                    let mut library = crate::app::library::Library::new(
                        crate::catalog_session::CatalogSession::open(&current)?,
                        ctx.clone(),
                    );
                    folder_added(&mut library, &report, &conflicts);
                    let mut not_added = Vec::new();
                    if !failed.is_empty() {
                        not_added.push(format!("Not added: {}", failed.join(", ")));
                    }
                    if !skipped.is_empty() {
                        not_added.push(format!(
                            "Not added, as they match more than one folder of the catalog \
                             (add each with Add Folder…): {}",
                            skipped.join(", ")
                        ));
                    }
                    if !not_added.is_empty() {
                        library.messages.warn(not_added.join(". "));
                    }
                    Ok(library)
                })()
                .map(Box::new)
                .map_err(|e| format!("{e:#}"));
                let source = result
                    .as_ref()
                    .map(|library| library.added_folder_keys(&imported))
                    .unwrap_or_default();
                let _ = tx.send(Event::FoldersAdded { result, source });
            },
            |tx, error| {
                let _ = tx.send(Event::CatalogReady(Err(error)));
            },
        );
    }
    /// Adds a photo from outside the Library (dropped on the window or passed
    /// on the command line) by adding its folder to the catalog, then opens it
    /// in Develop. A folder is added itself, and the Library shows it.
    pub(super) fn add_to_library(&mut self, path: PathBuf) {
        let path = path.canonicalize().unwrap_or(path);
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        self.module = Module::Library;
        let is_folder = path.is_dir();
        if !is_folder && let Some(id) = self.catalog_photo_at(&path) {
            self.develop_catalog_photo(id);
            return;
        }
        let Some(current) = self
            .library
            .as_ref()
            .map(|l| l.session.catalog.location().clone())
        else {
            if self.activity.is_dialog() {
                // The catalog is still opening; add the photo once it is ready.
                self.pending_photo = Some(super::PendingPhoto {
                    path,
                    folder_added: false,
                });
            } else {
                self.messages
                    .warn(format!("Open or create a catalog to edit {name}"));
            }
            return;
        };
        let folder = if is_folder {
            path.clone()
        } else {
            let Some(parent) = path.parent() else {
                return;
            };
            parent.to_path_buf()
        };
        if !self.ready_for_catalog() || !self.activity.begin_folder_change() {
            return;
        }
        if is_folder {
            self.messages.note(format!("Adding {name} to the Library…"));
        } else {
            self.pending_photo = Some(super::PendingPhoto {
                path,
                folder_added: true,
            });
            self.messages
                .note(format!("Adding {name}'s folder to the Library…"));
        }
        let tx = self.tx.clone();
        let ctx = self.context.clone();
        super::task::spawn(
            tx,
            ctx.clone(),
            move |tx| {
                let result = (|| -> anyhow::Result<_> {
                    let added = crate::catalog::Catalog::open(&current)?.import_folder(
                        &folder,
                        &crate::catalog_session::new_photo_defaults(),
                        &[],
                    )?;
                    anyhow::ensure!(
                        added.ambiguous.is_empty(),
                        "{} matches more than one folder of the catalog; add it with Add Folder…",
                        folder.display()
                    );
                    let mut library = crate::app::library::Library::new(
                        crate::catalog_session::CatalogSession::open(&current)?,
                        ctx.clone(),
                    );
                    // Says why the photo wasn't added when its folder is linked elsewhere.
                    folder_added(&mut library, &added.report, &added.conflicts);
                    Ok(library)
                })()
                .map(Box::new)
                .map_err(|e| format!("{e:#}"));
                let event = if is_folder {
                    let source = result
                        .as_ref()
                        .map(|library| library.added_folder_keys(&[folder]))
                        .unwrap_or_default();
                    Event::FoldersAdded { result, source }
                } else {
                    Event::CatalogReady(result)
                };
                let _ = tx.send(event);
            },
            |tx, error| {
                let _ = tx.send(Event::CatalogReady(Err(error)));
            },
        );
    }
    /// The catalog photo stored at `path`, if any: its master rather than
    /// a virtual copy.
    pub(super) fn catalog_photo_at(&self, path: &std::path::Path) -> Option<PhotoId> {
        self.library
            .as_ref()?
            .session
            .photos
            .iter()
            // A location stored through a symlink spells the path differently;
            // only photos of the same name are looked up on disk.
            .filter(|p| {
                p.path == path
                    || (path.file_name() == Some(std::ffi::OsStr::new(&p.filename))
                        && p.path.canonicalize().is_ok_and(|real| real == path))
            })
            .min_by_key(|p| p.master.is_some())
            .map(|p| p.id)
    }
    /// Opens the photo waiting to be added once the catalog is ready, adding
    /// its folder first if that has not happened yet.
    pub(super) fn open_pending_photo(&mut self) {
        let Some(super::PendingPhoto { path, folder_added }) = self.pending_photo.take() else {
            return;
        };
        if let Some(id) = self.catalog_photo_at(&path) {
            self.develop_catalog_photo(id);
        } else if !folder_added {
            self.add_to_library(path);
        } else {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            // Adding its folder may have said why (linked elsewhere).
            let text: String = match self.library.as_ref().map(|l| l.messages.line()) {
                Some(why) if !why.is_empty() => {
                    format!("{name} could not be added to the Library · {why}")
                }
                _ => format!("{name} could not be added to the Library"),
            };
            self.messages.fail(text);
        }
    }
    pub(super) fn develop_catalog_photo(&mut self, id: PhotoId) {
        let Some(p) = self.library.as_ref().and_then(|l| l.photo(id)).cloned() else {
            return;
        };
        let exists = p.path.is_file();
        if exists && let Some(l) = &mut self.library {
            l.found(&p.path);
        }
        if let Some(refusal) = crate::app::library::develop_refusal(&p, exists) {
            // In the top bar, so the click is seen to have been answered, and the
            // right-click it may ask for can be made at once.
            self.messages.warn(format!(
                "{} can't be opened: {}",
                p.filename,
                refusal.detail()
            ));
            return;
        }
        // Already open, e.g. in the Loupe: Develop shows it as it is.
        if self.document.catalog_photo == Some(id)
            && (self.document.full().is_some() || self.load.is_running())
        {
            if let Some(l) = &mut self.library {
                l.make_active(id)
            }
            // Zoomed in meanwhile (the zoom is shared): Crop cannot stay open.
            if self.view.zoom.on && self.view.is(super::state::Tool::Crop) {
                self.view.tool = super::state::Tool::None;
            }
            self.module = Module::Develop;
            return;
        }
        if !self.ready_for_catalog() {
            return;
        }
        if let Some(l) = &mut self.library {
            l.make_active(id)
        }
        self.open_raw(p.path, Some(id));
    }
    /// Starts the open photo from its Lightroom settings `text`, converted as every
    /// photo's Lightroom edit is (`crate::edits`).
    pub(super) fn apply_lightroom_edits(&mut self, text: &str) {
        let Some(m) = &self.document.metadata else {
            return;
        };
        let result = crate::edits::lightroom_edit(text, m, &self.document.profiles);
        match result {
            Ok((r, warnings)) => {
                self.document.edit.replace(r);
                // Short for the status bar; the full list shows on hover.
                self.document.lightroom_notice = if warnings.is_empty() {
                    "Lightroom edit applied".into()
                } else {
                    format!(
                        "Lightroom edit applied · {} settings not rendered yet\n\n{}",
                        warnings.len(),
                        warnings.join("\n")
                    )
                };
            }
            Err(e) => {
                // Lightroom's edit starts from Adobe Default, not the raw defaults.
                self.document
                    .edit
                    .replace(crate::model::recipe::Recipe::with_profiles(
                        m,
                        &self.document.profiles,
                    ));
                self.document.lightroom_notice = format!("Lightroom settings not applied: {e:#}")
            }
        }
    }
}
impl Editor {
    /// Carries out a virtual copy command after saving the open edit, so a
    /// new copy starts from what is on screen. In Develop a new copy opens.
    pub(super) fn virtual_copy(&mut self, action: crate::app::library::CopyAction) {
        use crate::app::library::CopyAction;
        let result = match action {
            CopyAction::Remove(id) => {
                self.modal = Some(super::Modal::RemoveCopy(id));
                return;
            }
            _ if !self.ready_for_catalog() => return,
            CopyAction::Create(id) => self.library.as_mut().map(|l| {
                l.create_virtual_copy(id)
                    .map(|made| made.listed.is_ok().then_some(made.value))
            }),
            CopyAction::SetMaster(id) => self
                .library
                .as_mut()
                .map(|l| l.set_copy_as_master(id).map(|_| None)),
        };
        let (Some(result), Some(library)) = (result, &mut self.library) else {
            return;
        };
        match result {
            Ok(open) => {
                self.messages.absorb(&mut library.messages);
                // A copy the Library could not list yet is not opened.
                if let Some(id) = open
                    && self.module == Module::Develop
                {
                    self.develop_catalog_photo(id);
                }
            }
            Err(e) => self.messages.fail(format!("Virtual copy failed: {e:#}")),
        }
    }
    /// Asks before removing a virtual copy, as Lightroom does; a copy shown
    /// in Develop gives way to its master.
    pub(super) fn remove_copy_window(&mut self, ctx: &egui::Context) {
        let Some(super::Modal::RemoveCopy(id)) = self.modal else {
            return;
        };
        let Some(photo) = self.library.as_ref().and_then(|l| l.photo(id)).cloned() else {
            self.modal = None;
            return;
        };
        // Only a click or the focused button confirms: a stray Return must
        // never remove a copy.
        let Some(remove) = confirm_modal(
            ctx,
            "remove-virtual-copy",
            &format!("Remove “{}” of {}?", photo.copy_name, photo.filename),
            "Its edit, rating and keywords are removed from the catalog. \
             The photo file and its other copies are not affected.",
            false,
            &[("Cancel", false), ("Remove", true)],
            false,
        ) else {
            return;
        };
        self.modal = None;
        if remove {
            self.remove_virtual_copy(id);
        }
    }
    /// Asks before removing a folder from the catalog, as Lightroom does.
    pub(super) fn remove_folder_window(&mut self, ctx: &egui::Context) {
        let Some(super::Modal::RemoveFolder(removal)) = &self.modal else {
            return;
        };
        let removal = removal.clone();
        let photos = match removal.photos {
            1 => "its photo".to_string(),
            n => format!("its {n} photos"),
        };
        let body = match removal.empty_root {
            None if removal.exported.is_some() => format!(
                "The folder, its subfolders and {photos} leave this catalog. The \
                 exported catalog keeps them with their edits, ratings, keywords \
                 and places in collections. The files stay on disk."
            ),
            Some(_) => "The root folder has nothing left in the catalog. Its folder stays on disk; add it again to bring it back.".to_string(),
            None => format!(
                "The folder, its subfolders and {photos} leave the catalog, with \
                 their edits, ratings, keywords and places in collections. The \
                 files stay on disk; add the folder again to bring them back \
                 without those."
            ),
        };
        // Only a click or the focused button confirms: a stray Return must
        // never remove a folder.
        let Some(remove) = confirm_modal(
            ctx,
            "remove-folder",
            &format!("Remove “{}” from the catalog?", removal.name),
            &body,
            false,
            &[("Cancel", false), ("Remove", true)],
            false,
        ) else {
            return;
        };
        self.modal = None;
        if remove {
            self.remove_folders(&removal);
        }
    }
    /// Removes a folder of the Folders panel from the catalog once confirmed;
    /// its files stay on disk.
    pub(super) fn remove_folders(&mut self, removal: &crate::app::library::FolderRemoval) {
        if !self.ready_for_catalog() {
            return;
        }
        let Some(library) = &mut self.library else {
            return;
        };
        // Confirmed after another catalog opened: its ids are not these.
        if library.session.catalog.location() != &removal.catalog {
            self.messages.warn(format!(
                "{} was not removed: another catalog is open now",
                removal.name
            ));
            return;
        }
        if let Some(root) = removal.empty_root {
            match library.remove_empty_root(root, &removal.name) {
                Ok(()) => self.messages.absorb(&mut library.messages),
                Err(e) => self.messages.fail(format!("Root not removed: {e:#}")),
            }
            return;
        }
        match library.remove_folders(&removal.folders, &removal.name, removal.exported.as_ref()) {
            // Removed, even if the catalog could not be read again: their ids
            // may be given to new photos, so nothing may keep them.
            Ok(removed) => {
                self.messages.absorb(&mut library.messages);
                for &id in &removed.value {
                    self.forget_preview_intent(id);
                    self.undo_log.forget_photo(id);
                }
                // The reference goes with them, even locked and open as the
                // document too, where reloading it would keep it.
                if self
                    .reference
                    .photo
                    .is_some_and(|id| removed.value.contains(&id))
                {
                    self.clear_reference();
                }
                if self
                    .document
                    .catalog_photo
                    .is_some_and(|id| removed.value.contains(&id))
                {
                    self.close_removed_document();
                }
            }
            Err(e) => self.messages.fail(format!("Folder not removed: {e:#}")),
        }
    }
    /// Closes the open photo, removed from the catalog or its unsaved change
    /// discarded, and shows the Library.
    pub(super) fn close_removed_document(&mut self) {
        self.close_document();
        self.module = Module::Library;
    }
    /// Closes the open photo, leaving no photo open: nothing may save into it, nor
    /// a load still under way bring it back, and its picture, preset previews,
    /// tools and selection go with it. Opening another photo instead goes through
    /// `load_raw`, whose new load supersedes this one's.
    pub(super) fn close_document(&mut self) {
        self.messages.document_closed();
        self.load.invalidate();
        self.selection.clear_document();
        self.document.reset(None);
        self.preview.clear_document();
        self.presets.clear_document();
        self.view.clear_document();
    }
    /// Asks before Read Metadata from Files, as Lightroom does: it replaces
    /// the catalog's values, edits included.
    pub(super) fn read_metadata_window(&mut self, ctx: &egui::Context) {
        let Some(super::Modal::ReadMetadata(ids)) = &self.modal else {
            return;
        };
        let ids = ids.clone();
        let n = ids.len();
        let title = if n == 1 {
            "Read metadata from the file?".to_string()
        } else {
            format!("Read metadata from {n} files?")
        };
        let Some(read) = confirm_modal(
            ctx,
            "read-metadata",
            &title,
            "Replaces title, caption, keywords and other metadata in the \
             catalog with the values in the files, including your edits. \
             Fields the files don't have are kept. Virtual copies are not read.",
            false,
            &[("Cancel", false), ("Read", true)],
            false,
        ) else {
            return;
        };
        self.modal = None;
        if !read || !self.ready_for_catalog() {
            return;
        }
        let Some(library) = &mut self.library else {
            return;
        };
        library.read_metadata_from_files(&ids).ok();
        self.messages.absorb(&mut library.messages);
    }
    /// Removes virtual copy `id` once confirmed.
    pub(super) fn remove_virtual_copy(&mut self, id: PhotoId) {
        if !self.ready_for_catalog() {
            return;
        }
        let Some(library) = &mut self.library else {
            return;
        };
        match library.remove_virtual_copy(id) {
            // Removed, even if the catalog could not be read again: its id may
            // be given to the next new photo, so nothing may keep it.
            Ok(removed) => {
                let master = removed.value;
                self.messages.absorb(&mut library.messages);
                self.forget_preview_intent(id);
                self.undo_log.forget_photo(id);
                // The reference, when it was this copy, goes with it.
                self.load_reference();
                if self.document.catalog_photo == Some(id) {
                    // Removed from Develop: show its master there instead.
                    if let Some(master) = master
                        && self.module == Module::Develop
                    {
                        self.develop_catalog_photo(master);
                    }
                    // Nothing may save into the removed copy, nor its load refill it.
                    if self.document.catalog_photo == Some(id) {
                        self.close_removed_document();
                    }
                }
            }
            Err(e) => self
                .messages
                .fail(format!("Virtual copy not removed: {e:#}")),
        }
    }
}
/// A file dialog for RAWmakase catalogs.
pub(super) fn catalog_file_dialog() -> rfd::FileDialog {
    rfd::FileDialog::new().add_filter("RAWmakase catalog", &["rawmakase"])
}
/// Where another computer has the root or folder of `action`, when that
/// path is on this computer too: the place Find Missing Folder starts.
fn suggestion(catalog: &crate::catalog::Catalog, action: &FolderAction) -> Option<PathBuf> {
    let (root, relative) = match action {
        FolderAction::Add => return None,
        FolderAction::RelinkRoot(id) => (*id, ""),
        FolderAction::RelinkFolder(root, relative) => (*root, relative.as_str()),
    };
    let locations = catalog
        .folder_locations()
        .ok()?
        .into_iter()
        .find(|r| r.root == root)?;
    // Each other computer's own rows, so a folder found there through its
    // root or a parent is suggested too.
    let mut by_computer: std::collections::BTreeMap<&str, Vec<(String, PathBuf)>> =
        Default::default();
    for (computer, at, path) in &locations.elsewhere {
        by_computer
            .entry(computer)
            .or_default()
            .push((at.clone(), path.clone()));
    }
    by_computer.values().find_map(|rows| {
        crate::catalog::locations::resolve_in(&locations.original, rows, relative, cfg!(windows))
            .filter(|path| path.is_dir())
    })
}
