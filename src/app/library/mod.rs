//! Catalog browsing; thumbnail work is bounded and independent of RAW development.
use crate::app::widgets::plural;
use crate::catalog::{Catalog, FolderId, Photo, PhotoId, RootId};
use crate::catalog_session::{CatalogSession, Committed, Opened};
use anyhow::Result;
use eframe::egui;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Action {
    None,
    Develop(PhotoId),
    RelinkRoot(RootId),
    /// The folder of a root at a logical path.
    RelinkFolder(RootId, String),
    AddFolder,
}
impl Action {
    /// Combines the actions of panels drawn in turn: a later panel's action
    /// replaces an earlier one, and none keeps it.
    pub(crate) fn then(&mut self, later: Action) {
        if later != Action::None {
            *self = later;
        }
    }
}
/// A folder of the Folders panel, with its subfolders, to remove from the
/// catalog.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FolderRemoval {
    /// The catalog it was chosen in; its folder ids name nothing elsewhere.
    pub(crate) catalog: crate::catalog::CatalogLocation,
    pub(crate) name: String,
    pub(crate) folders: HashSet<FolderId>,
    /// Photos in it and its subfolders, as the panel counts them.
    pub(crate) photos: usize,
    /// A root with no folder left in the catalog, removed on its own.
    pub(crate) empty_root: Option<RootId>,
    /// The folders as Export as Catalog wrote them: removed only while they
    /// are still that.
    pub(crate) exported: Option<crate::catalog::ExportedFolders>,
}
/// Export as Catalog chosen in the Folders panel or a photo's menu, for the
/// editor to run.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CatalogExportRequest {
    /// What to name the new catalog, unless the user names it otherwise.
    pub(crate) name: String,
    pub(crate) scope: crate::catalog::ExportScope,
    /// For a folder, removing it from the catalog, to offer once it is
    /// exported.
    pub(crate) removal: Option<FolderRemoval>,
}
/// Where the Library was: its source, filter bar and selection, for undo to
/// return to.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Place {
    filters: filter::Filters,
    folders: folder_selection::FolderSelection,
    selection: selection::Selection,
}
impl Place {
    /// Leaves a photo now removed out of the selection to return to.
    pub(in crate::app) fn forget_photo(&mut self, id: PhotoId) {
        self.selection.forget_photo(id);
    }
}
pub(in crate::app) use cell::copy_suffix;
pub(crate) use descriptive::DescriptiveCommand;
pub(crate) use filmstrip::{DraggedPhoto, Module, Pick};
pub(crate) use metadata::{Metadata, MetadataCommand};
pub(in crate::app) use previews::EditSource;
pub(crate) use quick::CollectionCommand;
pub(in crate::app) use stacks::StackAction;
pub(crate) use stacks::StackCommand;
/// Lightroom's virtual copy commands, carried out by the editor so the open
/// edit is saved first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CopyAction {
    Create(PhotoId),
    SetMaster(PhotoId),
    /// Asks first, as Lightroom does.
    Remove(PhotoId),
}
/// A file operation the Library asked for, for the editor to plan and
/// confirm.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum FileRequest {
    /// Remove from Catalog.
    Remove(Vec<PhotoId>),
    /// Delete: Move to Trash, or Remove from Catalog.
    Delete(Vec<PhotoId>),
    /// Move to Trash alone.
    Trash(Vec<PhotoId>),
    /// Delete Rejected Photos, of the source shown.
    DeleteRejected,
    /// Rename Photo… (one) or Rename Photos… (several).
    Rename(Vec<PhotoId>),
    /// Move Folder…: where the folder of `root` at logical path `from`
    /// goes is to be chosen.
    ChooseParent {
        root: crate::catalog::RootId,
        from: String,
    },
    /// Moves the folder of `root` at logical path `from` into logical
    /// folder `to` of `to_root` (a drop).
    MoveFolder {
        root: crate::catalog::RootId,
        from: String,
        to_root: crate::catalog::RootId,
        to: String,
    },
    /// Rename Folder… for the folder of `root` at logical path `logical`.
    RenameFolder {
        root: crate::catalog::RootId,
        logical: String,
    },
    /// Move to Trash… for the folder of `root` at logical path `logical`.
    TrashFolder {
        root: crate::catalog::RootId,
        logical: String,
    },
    /// Move photos into the folder of `root` at logical path `to`, named
    /// `name` (a drop on it).
    Move {
        photos: Vec<PhotoId>,
        root: RootId,
        to: String,
        name: String,
    },
    /// Move to Folder…: photos to move, once a folder is chosen.
    ChooseFolder(Vec<PhotoId>),
    /// New Folder Inside… logical folder `parent` of `root`, named `name`;
    /// the selection, and whether it is all in that folder.
    NewFolder {
        root: crate::catalog::RootId,
        parent: String,
        name: String,
        photos: Vec<PhotoId>,
        inside: bool,
    },
}
/// A folder being dragged in the Folders panel: the folder of `root` at
/// logical path `from` ("" for the root itself), named `name`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DraggedFolder {
    pub root: crate::catalog::RootId,
    pub from: String,
    pub name: String,
}
/// Photos being dragged from the grid: the selection, or the photo dragged
/// when it isn't in it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DraggedPhotos(pub Vec<PhotoId>);
/// The thumbnail menu's preview commands, for the photos chosen.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PreviewsRequest {
    Build(Vec<PhotoId>, crate::catalog::preview_cache::PreviewKind),
    Discard(
        Vec<PhotoId>,
        Vec<crate::catalog::preview_cache::PreviewKind>,
    ),
}
pub(crate) struct Library {
    /// The open catalog and the photos, folders and collections read from it.
    pub session: CatalogSession,
    /// The active photo and the photos selected with it.
    selection: selection::Selection,
    /// Grid columns last frame, for Up and Down.
    grid_columns: usize,
    /// Scroll the grid to the active photo, after a key moved it.
    scroll_to_active: bool,
    /// Whether the folders missing on this computer were reported since the
    /// catalog opened.
    missing_noted: bool,
    volumes: volumes::Volumes,
    /// The sidebar's folder and collection trees, built once per change to
    /// the catalog's lists rather than every frame: the sidebar draws in
    /// Develop too, whose frames come fast while a slider moves.
    sidebar_trees: Option<std::sync::Arc<sidebar::Trees>>,
    /// The source and filter bar; `visible` is their result.
    filters: filter::Filters,
    /// The folders shown, as tree keys (none is All Photographs).
    selected_folders: folder_selection::FolderSelection,
    /// Show Photos in Subfolders: whether a folder shows its subfolders'
    /// photos too; see `show_subfolders`.
    subfolders_shown: bool,
    expanded: HashSet<String>,
    /// A folder renamed or moved: its tree key there as planned, and its
    /// catalog folders by the keys that gave them, until the catalog is
    /// read again and names them as the disk does there.
    following: Option<(String, Vec<(FolderId, String)>)>,
    /// The tree key of the folder New Folder Inside was last asked in,
    /// unfolded once the folder is made.
    new_folder_in: Option<String>,
    thumb_size: f32,
    /// How grid cells show their photos (J).
    cell_style: cell::Style,
    /// The sort order's keys from the catalog (see `Sort::keys`), kept until
    /// edits or sizes change them.
    sort_keys: Option<(sort::Sort, sort::Keys)>,
    /// The frame the Library was last drawn in, to notice it showing again.
    drawn_pass: u64,
    /// The style the grid was last drawn with, to keep its rows in place
    /// when it changes.
    drawn_style: cell::Style,
    /// Grid cells' photo info, read once per photo while expanded cells
    /// show it.
    cell_info: HashMap<PhotoId, Option<crate::metadata::PhotoInfo>>,
    strip: filmstrip::State,
    /// Counts changes to the photos shown, their order or their metadata,
    /// so the filmstrip notices a change made after it was drawn.
    shown_version: u64,
    /// Indices into `photos` of the ones shown, in display order.
    visible: Vec<usize>,
    availability: availability::Availability,
    ctx: egui::Context,
    cache: textures::PreviewTextures,
    /// A virtual copy command from a thumbnail menu, for the editor.
    copy_request: Option<CopyAction>,
    /// A folder chosen in the Folders panel to be removed, for the editor
    /// to confirm.
    removal_request: Option<FolderRemoval>,
    /// Export as Catalog chosen in the Folders panel or a photo's menu.
    export_request: Option<CatalogExportRequest>,
    /// Synchronize Folder chosen in the Folders panel, for the editor.
    sync_request: Option<(crate::catalog::folder_sync::SyncScope, String)>,
    /// Find on Disk… chosen in a photo's menu, for the editor.
    find_request: Option<PhotoId>,
    /// After a synchronization added photos, the status bar offers to show
    /// them.
    pub(in crate::app) offer_previous_import: bool,
    copy_names: copy_name::CopyNames,
    /// Title, caption and the other descriptive fields being shown or typed.
    fields: metadata_fields::Fields,
    loupe: loupe::Loupe,
    compare: compare::Compare,
    survey: survey::Survey,
    /// Photos rendered at the size Compare and Survey show them.
    screen: screen::ScreenPreviews,
    stamps: stage::Stamps,
    /// Which way the Loupe last moved, so the photo after is prepared ahead.
    loupe_direction: i32,
    /// Metadata changes not yet handed to the shared undo log.
    done: Vec<MetadataCommand>,
    /// Collection changes not yet handed to the shared undo log.
    collection_done: Vec<quick::CollectionCommand>,
    /// Descriptive metadata changes not yet handed to the shared undo log.
    descriptive_done: Vec<DescriptiveCommand>,
    /// Stack changes not yet handed to the shared undo log.
    stack_done: Vec<StackCommand>,
    /// Auto-Stack by Capture Time's window, while it is open.
    auto_stack: Option<stacks::AutoStack>,
    /// Stack Camera Bursts for photos added: what was read of batches still
    /// to be stacked, until they can be.
    import_bursts: Option<crate::catalog_session::BurstsRead>,
    /// The Loupe's Info overlay.
    loupe_info: photo_info::Overlay,
    /// Times photo info read from files was saved, so views know to refresh.
    info_saves: u64,
    /// The hovered grid photo's info, for its tooltip.
    hover_info: Option<(PhotoId, Option<crate::metadata::PhotoInfo>)>,
    /// The active photo's info, as last read from the catalog.
    info: Option<(PhotoId, Option<crate::metadata::PhotoInfo>)>,
    /// A photo to keep in place in the grid after a re-sort, with its
    /// position before it.
    keep_in_place: Option<(PhotoId, usize)>,
    /// The grid's scroll offset last frame.
    grid_offset: f32,
    /// The grid's glide after a trackpad swipe.
    grid_kinetic: kinetic::Kinetic,
    /// Positions in `visible` the grid drew last frame; all until it is drawn.
    grid_shown: std::ops::Range<usize>,
    /// External volumes attached at the last check, to notice one returning;
    /// None before the first check.
    attached: Option<HashSet<std::path::PathBuf>>,
    pub(in crate::app) messages: crate::app::messages::Messages,
    /// Photos to Read Metadata from Files for, once confirmed.
    read_request: Option<Vec<PhotoId>>,
    /// A file operation asked for (Delete, the thumbnail menu), for the
    /// editor to confirm.
    file_request: Option<FileRequest>,
    /// Background reads are held off while a file operation holds the
    /// catalog: nothing starts them until it lets go.
    reads_suspended: bool,
    /// A thumbnail menu's Build or Discard Previews, for the editor to carry out.
    previews_request: Option<PreviewsRequest>,
    /// The offline RAW in the Loupe and its stored preview's identity, as the
    /// editor works it out.
    loupe_stored: Option<(PhotoId, String)>,
    /// The photo whose refusal the right panel last showed, and that refusal.
    refusal_shown: Option<((PhotoId, std::path::PathBuf, bool), Option<Refusal>)>,
    /// Read Metadata from Files while it reads.
    reread: Option<descriptive::Reread>,
    /// Read Metadata from Files finished since the editor last asked.
    reread_finished: bool,
    /// What photos without an edit are previewed with (see `set_defaults`).
    defaults: std::sync::Arc<crate::raw_defaults::DevelopDefaults>,
}
impl Library {
    /// Ends the preview workers at exit: the renders under way are cancelled and the
    /// results dropped, so none waits for the grid. They are then waited for.
    pub(in crate::app) fn close_previews(&mut self) -> Vec<crate::app::task::Stopping> {
        let mut stopping = Vec::from(self.cache.close());
        stopping.extend(self.screen.close());
        stopping
    }
    /// The Library over a catalog read with [`CatalogSession::open`].
    pub(crate) fn new(opened: Opened, ctx: egui::Context) -> Self {
        let Opened {
            mut session,
            keyword_export,
        } = opened;
        session.wake_with(wake(&ctx));
        let loupe = loupe::Loupe::new(&ctx);
        let screen = screen::ScreenPreviews::new(&ctx);
        let mut s = Self {
            session,
            selection: Default::default(),
            grid_columns: 1,
            scroll_to_active: false,
            missing_noted: false,
            volumes: Default::default(),
            sidebar_trees: None,
            filters: Default::default(),
            selected_folders: Default::default(),
            subfolders_shown: true,
            expanded: HashSet::new(),
            following: None,
            new_folder_in: None,
            thumb_size: 190.,
            cell_style: Default::default(),
            drawn_style: Default::default(),
            drawn_pass: 0,
            sort_keys: None,
            cell_info: HashMap::new(),
            strip: filmstrip::State::default(),
            shown_version: 0,
            visible: Vec::new(),
            availability: Default::default(),
            cache: textures::PreviewTextures::new(&ctx),
            ctx,
            copy_request: None,
            removal_request: None,
            export_request: None,
            sync_request: None,
            find_request: None,
            offer_previous_import: false,
            copy_names: Default::default(),
            fields: Default::default(),
            done: Vec::new(),
            collection_done: Vec::new(),
            descriptive_done: Vec::new(),
            stack_done: Vec::new(),
            auto_stack: None,
            import_bursts: None,
            loupe,
            compare: Default::default(),
            survey: Default::default(),
            screen,
            defaults: Default::default(),
            stamps: Default::default(),
            loupe_direction: 1,
            info: None,
            info_saves: 0,
            hover_info: None,
            loupe_info: Default::default(),
            keep_in_place: None,
            grid_offset: 0.,
            grid_kinetic: Default::default(),
            grid_shown: 0..usize::MAX,
            attached: None,
            messages: Default::default(),
            read_request: None,
            file_request: None,
            reads_suspended: false,
            previews_request: None,
            loupe_stored: None,
            refusal_shown: None,
            reread: None,
            reread_finished: false,
        };
        s.reloaded();
        s.check_files();
        if let Some(e) = keyword_export {
            s.messages.warn(format!(
                "Lightroom's keyword export options could not be read: {e:#}. \
                 Exports include every keyword's parents until they are."
            ));
        }
        // Start with a selection, as Lightroom does, so the side panels are filled.
        s.select(s.visible.first().map(|i| s.session.photos[*i].id));
        s
    }
    /// Opens the catalog at `path`, for tests.
    #[cfg(test)]
    pub(crate) fn load(path: &std::path::Path, ctx: egui::Context) -> Result<Self> {
        Ok(Self::new(CatalogSession::open(&path.into())?, ctx))
    }
    /// Reads the catalog again and checks which files are online, for tests.
    #[cfg(test)]
    pub(crate) fn refresh(&mut self) -> Result<()> {
        self.reload()?;
        self.check_files();
        Ok(())
    }
    /// Checks which files are online again, after the catalog was read.
    fn check_files(&mut self) {
        self.availability.start(&self.session.photos, &self.ctx);
        self.cache.failed.clear();
        self.screen.retry_failed();
        self.filter();
    }
    /// After a change made through another connection that could not be
    /// shown by opening the catalog again: forgets the previews of the
    /// photos `removed` (new photos may have their ids) and reads the
    /// catalog again. If that fails too, no photo is shown, so none can be
    /// changed under an id that now names another; the catalog stays open,
    /// for an edit still to be saved.
    pub(in crate::app) fn reread_after(&mut self, removed: &[PhotoId]) -> Result<()> {
        for id in removed {
            self.cache.forget(*id);
            self.screen.forget(*id);
        }
        let reread = self.reload();
        match &reread {
            // Moved and added photos are looked for where they are now.
            Ok(()) => self.check_files(),
            Err(_) => {
                // Neither photos nor folders: a folder removed may have
                // given its id to a new one.
                self.session.photos.clear();
                self.session.folders.clear();
                self.session.roots.clear();
                self.session.collection_photos.clear();
                self.session.previous_import.clear();
                self.filters.folder_scope = None;
                self.selected_folders.clear();
                self.reloaded();
            }
        }
        reread
    }
    /// Reads the catalog again into the lists shown.
    fn reload(&mut self) -> Result<()> {
        self.session.reload()?;
        self.reloaded();
        Ok(())
    }
    /// Brings what the Library shows in step with a catalog just read.
    fn reloaded(&mut self) {
        self.cell_info.clear();
        self.sort_keys = None;
        self.shown_version += 1;
        if self.filters.previous_import {
            self.filters.members = self.session.previous_import.clone();
        }
        if let Some(id) = self.filters.collection {
            if self.session.collections.iter().any(|c| c.id == id) {
                self.filters.members = self
                    .session
                    .collection_photos
                    .get(&id)
                    .cloned()
                    .unwrap_or_default();
            } else {
                self.filters.collection = None;
                self.filters.members.clear();
            }
        }
        if let Some((to, folders)) = self.following.take() {
            self.follow_catalog_names(&to, &folders);
        }
        // The folders shown as the catalog has them now: one may have
        // gained a catalog folder (photos moved into it) or subfolders.
        if let Some(scope) = self.current_scope(&self.selected_folders) {
            self.filters.folder_scope = Some(scope);
        }
        // Copy commands save a name being typed before they run, and so
        // does anything else that reads the catalog again.
        self.copy_names.clear();
        self.fields.clear();
        self.filter();
    }
    /// Waits for the online check, for callers that report on it.
    pub(crate) fn wait_for_availability(&mut self) {
        if self.availability.poll(true, &self.session.photos) {
            self.availability_known();
        }
    }
    /// Checks again which photos are online when an external volume comes
    /// back, so they show and their capture times are read.
    fn volumes_checked(&mut self, online: &HashMap<std::path::PathBuf, volumes::VolumeState>) {
        // Nothing to compare with until the first check has answered.
        if online.is_empty() {
            return;
        }
        let attached: HashSet<_> = online
            .iter()
            .filter(|(_, (on, _))| *on)
            .map(|(mount, _)| mount.clone())
            .collect();
        // The first answer may come during or after the online check (the
        // sidebar starts the volume check), so that check is made again
        // unless it finished with every photo online.
        let returned = match &self.attached {
            Some(before) => attached.iter().any(|mount| !before.contains(mount)),
            None => {
                self.availability.checking()
                    || self
                        .session
                        .photos
                        .iter()
                        .any(|p| !self.is_available(&p.path))
            }
        };
        self.attached = Some(attached);
        if returned {
            self.availability.start(&self.session.photos, &self.ctx);
        }
    }
    /// Checks again, in the background, which photos are online when one
    /// counted offline turns out to be there, e.g. restored in place on a
    /// drive that stayed attached.
    pub(in crate::app) fn found(&mut self, path: &std::path::Path) {
        if !self.is_available(path) {
            self.availability.start(&self.session.photos, &self.ctx);
        }
    }
    /// Asks for a repaint, for a background read with something to show.
    fn wake(&self) -> crate::catalog_session::Wake {
        wake(&self.ctx)
    }
    fn is_available(&self, path: &std::path::Path) -> bool {
        self.availability.is_available(path)
    }
    /// Why photo `id` can't be exported or built, if it can't, for work over many
    /// photos: [`availability_refusal`], as no file is read for it. A file Develop
    /// can't edit fails as that photo's export or build, with the reason.
    /// The file is looked at directly: the availability scan counts every photo
    /// as there until it has checked.
    pub(in crate::app) fn export_refusal(&self, id: PhotoId) -> Option<Refusal> {
        let photo = self.photo(id)?;
        availability_refusal(photo.path.is_file())
    }
    /// Whether Develop could open photo `id`, by what the Library last found
    /// online rather than by asking the file system: a file Develop can't read
    /// is found when it is opened.
    pub(in crate::app) fn known_developable(&self, id: PhotoId) -> bool {
        self.photo(id).is_some_and(|p| self.is_available(&p.path))
    }
    /// Why Develop cannot open `photo`, for the Library to show: as
    /// [`develop_refusal`] finds, with a file's header read once while it stays
    /// selected and online.
    pub(super) fn shown_refusal(&mut self, photo: &Photo) -> Option<Refusal> {
        let available = self.is_available(&photo.path);
        let key = (photo.id, photo.path.clone(), available);
        match &self.refusal_shown {
            Some((shown, refusal)) if *shown == key => refusal.clone(),
            _ => {
                let refusal = develop_refusal(photo, available);
                self.refusal_shown = Some((key, refusal.clone()));
                refusal
            }
        }
    }
    pub(crate) fn available_count(&self) -> usize {
        self.availability.count(&self.session.photos)
    }
    fn filter(&mut self) {
        // What the order needs from the catalog, read once until it changes.
        let sort = self.filters.sort;
        if self.sort_keys.as_ref().is_none_or(|(of, _)| *of != sort) {
            self.sort_keys = Some((sort, sort.keys(&self.session.catalog)));
        }
        let keys = &self.sort_keys.as_ref().unwrap().1;
        self.shown_version += 1;
        // Filtering follows every change to the photos, so the count does too.
        self.availability.forget_count();
        self.visible = self.filters.visible(
            &self.session.photos,
            |path| self.availability.is_available(path),
            keys,
        );
        self.keep_shown_selected();
    }
    /// Applies `change` and filters again, keeping the selected photo where
    /// it was on screen: for a re-sort the user did not ask for, such as
    /// capture times or sizes read in the background.
    fn resort_in_place(&mut self, change: impl FnOnce(&mut Self)) {
        let anchor = self.selected().and_then(|id| {
            self.visible
                .iter()
                .position(|i| self.session.photos[*i].id == id)
                .map(|at| (id, at))
        });
        // A selected photo scrolled out of view is no anchor: the view stays.
        let anchor = anchor.filter(|(_, at)| self.grid_shown.contains(at));
        change(self);
        self.filter();
        // Several batches before the grid is drawn again: the first position counts.
        if self.keep_in_place.is_none() {
            self.keep_in_place = anchor;
        }
    }
    pub(crate) fn photo(&self, id: PhotoId) -> Option<&Photo> {
        self.session.photos.iter().find(|p| p.id == id)
    }
    /// What photo `id` is developed from, as Develop would open it: its saved edit
    /// (checked as Develop checks it), else its Lightroom edit, else the defaults;
    /// for Develop's Reference View. Why not, for a photo Develop cannot open; None
    /// for a photo no longer in the catalog.
    pub(in crate::app) fn develop_source(
        &self,
        id: PhotoId,
        demosaic: crate::camera_data::Demosaic,
    ) -> Option<Result<DevelopSource, Refusal>> {
        let photo = self.photo(id)?;
        if let Some(refusal) = develop_refusal(photo, photo.path.is_file()) {
            return Some(Err(refusal));
        }
        let edit = match self.session.catalog.load_edit(id, &photo.path) {
            Ok(Some(saved)) => serde_json::to_string(&saved.recipe)
                .ok()
                .map(EditSource::Recipe),
            Ok(None) => self
                .session
                .catalog
                .edit_texts(id)
                .ok()
                .and_then(|(_, lightroom)| lightroom)
                .map(EditSource::Lightroom),
            // A protected edit: Develop shows the defaults too.
            Err(_) => None,
        }
        .unwrap_or_else(|| EditSource::Defaults(self.defaults.clone()));
        // The file and the demosaic too: a RAW replaced in place, or decoded another
        // way, is developed again.
        let file = crate::storage::Stamp::read(&photo.path).ok();
        Some(Ok(DevelopSource {
            path: photo.path.clone(),
            tag: format!("{}-{file:?}-{demosaic:?}", edit.tag()),
            edit,
        }))
    }
    pub(crate) fn navigate(&self, id: PhotoId, delta: i32) -> Option<PhotoId> {
        let at = self
            .visible
            .iter()
            .position(|i| self.session.photos[*i].id == id)?;
        let n = (at as i32 + delta).clamp(0, self.visible.len().saturating_sub(1) as i32) as usize;
        self.visible.get(n).map(|i| self.session.photos[*i].id)
    }
    fn labels(&self) -> Vec<String> {
        let mut labels: Vec<String> = crate::app::photo_metadata::LABELS
            .iter()
            .map(|s| (*s).into())
            .collect();
        let mut custom: Vec<_> = self
            .session
            .photos
            .iter()
            .map(|p| &p.label)
            .filter(|s| !s.is_empty() && !labels.contains(s))
            .cloned()
            .collect();
        custom.sort();
        custom.dedup();
        labels.extend(custom);
        labels
    }
    #[cfg(test)]
    pub(in crate::app) fn show_unflagged(&mut self) {
        self.filters.flags = [0].into();
        self.filter();
    }
    #[cfg(test)]
    pub(in crate::app) fn select_range_to(&mut self, id: PhotoId) {
        self.click(id, egui::Modifiers::SHIFT);
    }
    #[cfg(test)]
    pub(in crate::app) fn shown(&self) -> Vec<PhotoId> {
        self.visible
            .iter()
            .map(|i| self.session.photos[*i].id)
            .collect()
    }
    /// Edits written elsewhere (Sync, its Undo): their previews render again.
    pub(in crate::app) fn edits_changed(&mut self, ids: impl IntoIterator<Item = PhotoId>) {
        for id in ids {
            self.cache.forget(id);
        }
        // Edit Time order reads when each photo was last edited.
        self.resort_in_place(|l| l.sort_keys = None);
    }
    /// Whether `id` is selected, shown or hidden by the filters.
    pub(in crate::app) fn is_selected(&self, id: PhotoId) -> bool {
        self.selection.selected.contains(&id)
    }
    /// The selected photos in display order.
    pub(in crate::app) fn selected_photos(&self) -> Vec<PhotoId> {
        self.selected_ids()
    }
    pub(in crate::app) fn place(&self) -> Place {
        Place {
            filters: self.filters.clone(),
            folders: self.selected_folders.clone(),
            selection: self.selection.clone(),
        }
    }
    /// Returns to `place`, with the photos it had selected that are still
    /// there, and scrolls to its active photo.
    pub(in crate::app) fn go_to_place(&mut self, place: &Place) {
        self.filters = place.filters.clone();
        self.selected_folders = place.folders.clone();
        // The source as the catalog has it now, e.g. after a folder gained
        // subfolders since.
        if let Some(scope) = self.current_scope(&place.folders) {
            self.filters.folder_scope = Some(scope);
        }
        if let Some(id) = self.filters.collection {
            self.filters.members = self
                .session
                .collection_photos
                .get(&id)
                .cloned()
                .unwrap_or_default();
        }
        self.selection = place.selection.clone();
        self.filter();
        self.compare.restored = true;
        self.scroll_to_active = true;
    }
    /// Whether Read Metadata from Files finished since the last call, for
    /// the top bar to show its outcome.
    /// Read Metadata from Files is still applying what it read.
    pub(in crate::app) fn rereading(&self) -> bool {
        self.reread.is_some()
    }
    pub(in crate::app) fn take_reread_finished(&mut self) -> bool {
        std::mem::take(&mut self.reread_finished)
    }
    /// Photos Read Metadata from Files was chosen for, for the editor to
    /// confirm.
    pub(in crate::app) fn take_read_request(&mut self) -> Option<Vec<PhotoId>> {
        self.read_request.take()
    }
    /// The file operation asked for, for the editor to confirm.
    pub(in crate::app) fn take_file_request(&mut self) -> Option<FileRequest> {
        self.file_request.take()
    }
    /// Delete and Backspace (with Option too): in the Quick Collection, takes
    /// `ids` out of it, as one undoable step; in an imported collection, which
    /// can't be changed, nothing; elsewhere, asks to remove them from the
    /// catalog.
    pub(in crate::app) fn delete_key(&mut self, ids: Vec<PhotoId>) {
        if ids.is_empty() {
            return;
        }
        if self.showing_quick() {
            let Some(quick) = self.quick() else {
                return;
            };
            let summary = format!(
                "Removed from Quick Collection · {}",
                plural(ids.len(), "photo", "photos")
            );
            if let Err(e) = self.record_collection(quick, Vec::new(), ids, summary) {
                self.messages
                    .fail(format!("Quick Collection not changed: {e}"));
            }
        } else if self.filters.collection.is_some() {
            self.messages.warn("Imported collections can't be changed");
        } else {
            self.file_request = Some(FileRequest::Delete(ids));
        }
    }
    /// Delete and Backspace with `modifiers`, for `ids`: alone, as
    /// [`Self::delete_key`]; with Option, Remove from Catalog, even in a
    /// collection; with ⌘⌥⇧, Move to Trash; with ⌘, Delete Rejected Photos
    /// (Ctrl for ⌘ off the Mac).
    pub(in crate::app) fn delete_press(&mut self, modifiers: egui::Modifiers, ids: Vec<PhotoId>) {
        match (modifiers.command, modifiers.alt, modifiers.shift) {
            (true, true, true) => self.request_trash(ids),
            (true, false, false) => self.file_request = Some(FileRequest::DeleteRejected),
            (false, alt, false) if !modifiers.ctrl => {
                if alt {
                    self.request_removal(ids)
                } else {
                    self.delete_key(ids)
                }
            }
            _ => {}
        }
    }
    /// Rename Photo… or Rename Photos… for `ids` (F2, the thumbnail menu),
    /// for the editor to ask.
    pub(in crate::app) fn request_rename(&mut self, ids: Vec<PhotoId>) {
        if !ids.is_empty() {
            self.file_request = Some(FileRequest::Rename(ids));
        }
    }
    /// Move to Trash for `ids` (⌘⌥⇧Delete, the thumbnail menu), whatever is
    /// shown, for the editor to confirm.
    pub(in crate::app) fn request_trash(&mut self, ids: Vec<PhotoId>) {
        if !ids.is_empty() {
            self.file_request = Some(FileRequest::Trash(ids));
        }
    }
    /// The rejected photos of the source shown (its folders, collection,
    /// previous import or offline photos), whatever the filter bar shows:
    /// Delete Rejected Photos'.
    pub(in crate::app) fn rejected_in_source(&self) -> Vec<PhotoId> {
        self.session
            .photos
            .iter()
            .filter(|p| {
                p.flag == -1 && self.filters.source_holds(p, |path| self.is_available(path))
            })
            .map(|p| p.id)
            .collect()
    }
    /// Remove from Catalog for `ids` (⌥Delete, the thumbnail menu), whatever
    /// is shown, for the editor to confirm.
    pub(in crate::app) fn request_removal(&mut self, ids: Vec<PhotoId>) {
        if !ids.is_empty() {
            self.file_request = Some(FileRequest::Remove(ids));
        }
    }
    /// Moves `photos` into the folder of `root` at logical path `to`
    /// (dropped on it), for the editor to confirm; a drop where they all
    /// are already does nothing.
    pub(super) fn request_move(
        &mut self,
        photos: Vec<PhotoId>,
        root: RootId,
        to: String,
        name: String,
    ) {
        let place = Some((root, to.clone()));
        let there = photos.iter().all(|id| {
            self.photo(*id)
                .is_some_and(|p| self.folder_place(p.folder) == place)
        });
        if !photos.is_empty() && !there {
            self.file_request = Some(FileRequest::Move {
                photos,
                root,
                to,
                name,
            });
        }
    }
    /// New Folder Inside… folder `parent` of `root` (catalog folder
    /// `folder`, if it is one), offering to move the selection into it.
    pub(super) fn request_new_folder(
        &mut self,
        root: crate::catalog::RootId,
        parent: String,
        name: String,
        folder: Option<FolderId>,
    ) {
        let photos = self.selected_ids();
        let inside = !photos.is_empty()
            && photos
                .iter()
                .all(|id| self.photo(*id).is_some_and(|p| Some(p.folder) == folder));
        self.file_request = Some(FileRequest::NewFolder {
            root,
            parent,
            name,
            photos,
            inside,
        });
    }
    /// Notes that New Folder Inside asks for a folder in logical folder
    /// `parent` of `root`, to unfold it once the folder is made.
    pub(in crate::app) fn new_folder_in(&mut self, root: crate::catalog::RootId, parent: &str) {
        self.new_folder_in = Some(match parent {
            "" => format!("root:{root}"),
            parent => format!("root:{root}/{parent}"),
        });
    }
    /// Follows a folder of `root` renamed or moved from logical path `from`
    /// to `to` in the tree: what was selected or unfolded in it still is.
    pub(in crate::app) fn folder_renamed(
        &mut self,
        root: crate::catalog::RootId,
        from: &str,
        to_root: crate::catalog::RootId,
        to: &str,
    ) {
        let from = match from {
            "" => format!("root:{root}"),
            from => format!("root:{root}/{from}"),
        };
        let to = format!("root:{to_root}/{to}");
        // Named as planned now; as the disk spelled it there (composed, on
        // FAT, exFAT and SMB shares) once the catalog is read again.
        let folders = self
            .session
            .folders
            .iter()
            .filter(|f| f.root == root)
            .filter_map(|f| {
                let key = folder_selection::tree_key(root, &f.relative);
                let rest = folder_selection::rekeyed(&key, &from)?;
                Some((f.id, format!("{to}{rest}")))
            })
            .collect();
        self.following = Some((to.clone(), folders));
        self.selected_folders.rekey(&from, &to);
        self.expanded = std::mem::take(&mut self.expanded)
            .into_iter()
            .map(|key| match folder_selection::rekeyed(&key, &from) {
                Some(rest) => format!("{to}{rest}"),
                None => key,
            })
            .collect();
    }
    /// Names each selected or unfolded row in a folder renamed or moved to
    /// tree key `to` as the catalog names it now: a row is the folders below
    /// it, `folders` by the keys they were planned at, and takes the names
    /// one of them has now, as deep.
    fn follow_catalog_names(&mut self, to: &str, folders: &[(FolderId, String)]) {
        let now: HashMap<FolderId, String> = self
            .session
            .folders
            .iter()
            .map(|f| (f.id, folder_selection::tree_key(f.root, &f.relative)))
            .collect();
        let named = |key: &str| {
            folder_selection::rekeyed(key, to)?;
            let at = folders.iter().find_map(|(id, planned)| {
                folder_selection::rekeyed(planned, key)?;
                now.get(id)
            })?;
            let depth = key.split('/').count();
            Some(at.split('/').take(depth).collect::<Vec<_>>().join("/"))
        };
        self.selected_folders.rename(named);
        self.expanded = std::mem::take(&mut self.expanded)
            .into_iter()
            .map(|key| named(&key).unwrap_or(key))
            .collect();
    }
    /// After the folder of `root` at logical path `logical` went to the
    /// Trash: a selection in it moves to the folder it was in, which the
    /// catalog read again shows.
    pub(in crate::app) fn folder_trashed(&mut self, root: crate::catalog::RootId, logical: &str) {
        let key = format!("root:{root}/{logical}");
        let inside = self
            .selected_folders
            .keys()
            .iter()
            .any(|k| folder_selection::rekeyed(k, &key).is_some());
        if !inside {
            return;
        }
        let parent = match logical.rsplit_once('/') {
            Some((parent, _)) => format!("root:{root}/{parent}"),
            None => format!("root:{root}"),
        };
        self.selected_folders = folder_selection::FolderSelection::one(&parent);
        // Its folders as the catalog has them once read again; none until.
        self.filters.folder_scope = Some(HashSet::new());
    }
    /// Unfolds the folder New Folder Inside made a folder in, so the tree
    /// shows it.
    pub(in crate::app) fn unfold_new_folder(&mut self) {
        if let Some(key) = self.new_folder_in.take() {
            self.expanded.insert(key);
        }
    }
    /// Whether the folder tree shows the rows below tree key `key`.
    #[cfg(test)]
    pub(in crate::app) fn is_unfolded(&self, key: &str) -> bool {
        self.expanded.contains(key)
    }
    /// Holds off the background reads while a file operation holds the
    /// catalog: the readers stop, Read Metadata from Files with them, what
    /// they read before is dropped when it comes back, and nothing starts
    /// them again until [`Self::resume_background_reads`].
    pub(in crate::app) fn suspend_background_reads(&mut self) {
        self.reads_suspended = true;
        self.cancel_reread();
        self.stop_background_reads();
        self.session.bump_generation();
    }
    /// Stops the session's background reads before a change that may give
    /// removed photos' ids to new ones, and drops what was read of batches
    /// whose bursts are to be stacked: read again after.
    pub(in crate::app) fn stop_background_reads(&mut self) {
        self.import_bursts = None;
        self.session.stop_background_reads();
    }
    /// Background reads go on after a file operation let the catalog go:
    /// capture times, photo info and checksums, for what is still missing.
    pub(in crate::app) fn resume_background_reads(&mut self) {
        self.reads_suspended = false;
        self.start_capture_times();
        self.start_photo_info();
        self.start_import_stacks();
        self.start_checksums();
    }
    /// A virtual copy command chosen from a thumbnail menu since last asked.
    pub(in crate::app) fn take_previews_request(&mut self) -> Option<PreviewsRequest> {
        self.previews_request.take()
    }
    /// The grid's latest request for the photo's edited thumbnail, if any.
    pub(in crate::app) fn edited_ticket(&self, id: PhotoId) -> Option<u64> {
        self.cache.edited_ticket(id)
    }
    /// A built preview, as the photo's edited thumbnail; the caller has checked
    /// it is still the photo's latest.
    pub(in crate::app) fn show_built_preview(
        &mut self,
        ctx: &egui::Context,
        id: PhotoId,
        image: &image::RgbImage,
    ) {
        self.cache.insert_edited(ctx, id, image);
    }
    pub(super) fn take_copy_request(&mut self) -> Option<CopyAction> {
        self.copy_request.take()
    }
    pub(in crate::app) fn take_find_request(&mut self) -> Option<PhotoId> {
        self.find_request.take()
    }
    pub(in crate::app) fn take_sync_request(
        &mut self,
    ) -> Option<(crate::catalog::folder_sync::SyncScope, String)> {
        self.sync_request.take()
    }
    /// What Synchronize Folder covers for the folder selected in the
    /// Folders panel, and its name; none for All Photographs, a collection
    /// or several folders.
    pub(in crate::app) fn selected_sync_scope(
        &self,
    ) -> Option<(crate::catalog::folder_sync::SyncScope, String)> {
        use crate::catalog::folder_sync::SyncScope;
        let rest = self.selected_folders.single()?.strip_prefix("root:")?;
        let (root, relative) = rest.split_once('/').unwrap_or((rest, ""));
        let root = root.parse::<i64>().ok().map(RootId)?;
        if relative.is_empty() {
            let name = self
                .session
                .roots
                .iter()
                .find(|(id, ..)| *id == root)
                .map(|(_, original, _)| {
                    std::path::Path::new(original)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                })
                .unwrap_or_default();
            return Some((SyncScope::Root(root), name));
        }
        let folder = self
            .session
            .folders
            .iter()
            .find(|f| f.root == root && f.relative.trim_end_matches('/') == relative)?;
        let name = relative.rsplit('/').next().unwrap_or(relative).to_string();
        Some((SyncScope::Folder(folder.id), name))
    }
    /// The root and logical path of catalog folder `id`, as the Library has it.
    pub(in crate::app) fn folder_place(&self, id: FolderId) -> Option<(RootId, String)> {
        let folder = self.session.folders.iter().find(|f| f.id == id)?;
        Some((folder.root, folder.relative.clone()))
    }
    pub(in crate::app) fn take_removal_request(&mut self) -> Option<FolderRemoval> {
        self.removal_request.take()
    }
    pub(in crate::app) fn take_export_request(&mut self) -> Option<CatalogExportRequest> {
        self.export_request.take()
    }
    /// Creates a virtual copy of `id` and selects it.
    pub(super) fn create_virtual_copy(&mut self, id: PhotoId) -> Result<Committed<PhotoId>> {
        let made = self.session.create_virtual_copy(id)?;
        if self.listed_after(&made.listed, "Copy made") {
            self.show(made.value);
            if let Some(p) = self.photo(made.value) {
                self.messages
                    .done(format!("Created {} of {}", p.copy_name, p.filename));
            }
        }
        Ok(made)
    }
    pub(super) fn set_copy_as_master(&mut self, id: PhotoId) -> Result<Committed<()>> {
        let made = self.session.set_copy_as_master(id)?;
        if self.listed_after(&made.listed, "Master changed") {
            self.show(id);
            if let Some(p) = self.photo(id) {
                self.messages
                    .done(format!("This copy is now the master of {}", p.filename));
            }
        }
        Ok(made)
    }
    /// Brings what is shown in step after a change to the catalog, and says
    /// so when the catalog could not be read again after `done`. Returns
    /// whether it was.
    fn listed_after(&mut self, listed: &Result<()>, done: &str) -> bool {
        match listed {
            Ok(()) => {
                self.reloaded();
                true
            }
            Err(e) => {
                self.messages.fail(format!(
                    "{done}, but the catalog could not be read again: {e:#}"
                ));
                false
            }
        }
    }
    /// Removes root `root`, named `name`, which has no folder left in the
    /// catalog; its folder stays on disk.
    pub(in crate::app) fn remove_empty_root(
        &mut self,
        root: crate::catalog::RootId,
        name: &str,
    ) -> Result<()> {
        let removed = self.session.remove_empty_root(root)?;
        let key = format!("root:{root}");
        if self
            .selected_folders
            .keys()
            .iter()
            .any(|k| folder_selection::rekeyed(k, &key).is_some())
        {
            self.selected_folders.clear();
            self.filters.folder_scope = None;
        }
        if self.listed_after(&removed.listed, "Root removed") {
            match removed.value {
                true => self.messages.done(format!(
                    "Removed {name} from the catalog. Its folder is still on disk."
                )),
                false => self.messages.warn(format!(
                    "{name} was not removed: it has folders in the catalog again."
                )),
            }
        }
        Ok(())
    }
    /// Removes `folders` and their photos from the catalog, leaving the files;
    /// returns the photos removed. The Library shows All Photographs if it
    /// showed one of them. An error means nothing was removed. Folders
    /// `exported` are removed only while they are still what was exported.
    pub(in crate::app) fn remove_folders(
        &mut self,
        folders: &HashSet<FolderId>,
        name: &str,
        exported: Option<&crate::catalog::ExportedFolders>,
    ) -> Result<Committed<Vec<PhotoId>>> {
        // Forgotten first: once removed, their ids can be reused, so their
        // previews must go even if removing or reading the catalog again
        // fails. Copies of these photos are kept with them. What was read of
        // batches to stack too: read again once they are gone.
        self.import_bursts = None;
        let going: Vec<PhotoId> = self
            .session
            .photos
            .iter()
            .filter(|p| folders.contains(&p.folder))
            .map(|p| p.id)
            .collect();
        for photo in &self.session.photos {
            if going.contains(&photo.id) || photo.master.is_some_and(|m| going.contains(&m)) {
                self.cache.forget(photo.id);
                self.screen.forget(photo.id);
            }
        }
        // The folders chosen with their subfolders, shown or not: a folder
        // of folders shown alone has no photos of its own to tell.
        let shown_going = self
            .selected_folders
            .keys()
            .iter()
            .filter_map(|key| self.folder_ids(key, true))
            .flatten()
            .any(|f| folders.contains(&f));
        let ids: Vec<FolderId> = folders.iter().copied().collect();
        let removed = match self.session.remove_folders(&ids, exported) {
            Ok(removed) => removed,
            Err(e) => {
                // Nothing removed: the reads it stopped go on.
                self.start_background_reads();
                return Err(e);
            }
        };
        if shown_going {
            self.selected_folders.clear();
            self.filters.folder_scope = None;
        }
        for id in &removed.value {
            self.selection.forget_photo(*id);
        }
        if self.listed_after(&removed.listed, "Folder removed") {
            let n = removed.value.len();
            self.messages.done(format!(
                "Removed {name} and its {n} photo{} from the catalog. The files are still on disk.",
                if n == 1 { "" } else { "s" }
            ));
        } else {
            // The session dropped the removed photos from its lists anyway;
            // what is shown follows them before anything reads those lists.
            self.reloaded();
        }
        self.start_background_reads();
        Ok(removed)
    }
    /// Starts the background reads again for what is still missing, unless
    /// suspended: capture times, photo info, bursts to stack and checksums.
    pub(in crate::app) fn start_background_reads(&mut self) {
        self.start_capture_times();
        self.start_photo_info();
        self.start_import_stacks();
        self.start_checksums();
    }
    /// Removes virtual copy `id`; returns its master, which is selected. An
    /// error means the copy is still there.
    pub(super) fn remove_virtual_copy(
        &mut self,
        id: PhotoId,
    ) -> Result<Committed<Option<PhotoId>>> {
        let photo = self.photo(id).cloned();
        // Forgotten first: once the copy is removed its id can be reused, so its
        // previews must go even if reading the catalog again fails.
        self.cache.forget(id);
        self.screen.forget(id);
        let removed = self.session.remove_virtual_copy(id)?;
        let master = photo.as_ref().and_then(|p| p.master);
        if self.listed_after(&removed.listed, "Copy removed") {
            if let Some(master) = master {
                self.show(master);
            }
            if let Some(p) = photo {
                self.messages
                    .done(format!("Removed {} of {}", p.copy_name, p.filename));
            }
        }
        Ok(Committed {
            value: master,
            listed: removed.listed,
        })
    }
    /// Selects `id`, leaving filters that would hide it so it stays in view.
    fn show(&mut self, id: PhotoId) {
        // A photo a collapsed stack hides shows once its stack is expanded.
        self.expand_stack_of(id);
        if !self
            .visible
            .iter()
            .any(|i| self.session.photos[*i].id == id)
        {
            if !self.filters.members.contains(&id) {
                self.filters.collection = None;
                self.filters.previous_import = false;
            }
            // Filters turned off with Cmd+L hide nothing, and are kept.
            if self.filters.enabled {
                self.filters.clear_bar();
            }
            self.filter();
        }
        // Outside the folder shown, or no longer offline: All Photographs.
        if !self
            .visible
            .iter()
            .any(|i| self.session.photos[*i].id == id)
        {
            self.filters.folder_scope = None;
            self.filters.collection = None;
            self.filters.previous_import = false;
            self.filters.only_missing = false;
            self.selected_folders.clear();
            self.filter();
        }
        self.select(Some(id));
    }
    /// Drain in every workspace so the bounded worker never waits for the grid.
    pub(super) fn poll_previews(&mut self, ctx: &egui::Context) {
        if self.availability.poll(false, &self.session.photos) {
            self.availability_known();
        }
        self.poll_capture_times();
        self.poll_photo_info();
        self.poll_import_stacks();
        self.poll_checksums();
        self.poll_reread();
        self.cache.poll(ctx);
        self.screen.poll(ctx);
    }
    /// Hands the worker the photos shown last frame. Call once per frame.
    pub(super) fn publish_shown(&mut self) {
        self.cache.publish_shown();
        self.screen.publish_shown();
    }
    /// The photo's preview: its edit once rendered, else the embedded one.
    fn texture(&self, photo: &Photo) -> Option<&egui::TextureHandle> {
        self.cache.texture(photo)
    }
    /// Queues the previews a shown photo needs; its edit comes from the catalog.
    fn request_previews(&mut self, photo: &Photo, ctx: &egui::Context) {
        let catalog = &self.session.catalog;
        self.cache
            .request(photo, ctx, || edit_source(catalog, photo.id));
    }
    /// Shows Develop's latest render as the photo's thumbnail and caches it
    /// under the edit it was rendered with.
    pub(super) fn update_edited(
        &mut self,
        ctx: &egui::Context,
        id: PhotoId,
        image: image::RgbImage,
        recipe_json: String,
    ) {
        let Some(path) = self.photo(id).map(|p| p.path.clone()) else {
            return;
        };
        self.cache.store_edited(ctx, id, path, image, recipe_json);
    }
    /// The selected photo, or else the first one shown in the current
    /// folder or filter (which then becomes selected), as Lightroom does
    /// when switching to Develop.
    pub(super) fn selected_or_first(&mut self) -> Option<PhotoId> {
        if self.selection.active.is_none() {
            self.select(self.visible.first().map(|i| self.session.photos[*i].id));
        }
        self.selection.active
    }
    /// Previews photos without an edit with `defaults` from now on; the ones
    /// shown with the previous defaults are made again.
    pub(in crate::app) fn set_defaults(
        &mut self,
        defaults: std::sync::Arc<crate::raw_defaults::DevelopDefaults>,
    ) {
        self.set_defaults_preserving(defaults, None);
    }

    /// A manual import may leave the current Develop recipe unchanged. Its
    /// existing render remains valid; other unedited photos still invalidate.
    pub(in crate::app) fn set_defaults_preserving(
        &mut self,
        defaults: std::sync::Arc<crate::raw_defaults::DevelopDefaults>,
        current: Option<PhotoId>,
    ) {
        self.defaults = defaults;
        self.screen.clear();
        // Develop's renders of photos without an edit: back to the embedded
        // preview until Develop shows one with the new defaults.
        let unedited: Vec<PhotoId> = self
            .cache
            .edited_ids()
            .filter(|id| Some(*id) != current && edit_source(&self.session.catalog, *id).is_none())
            .collect();
        for id in unedited {
            self.cache.forget(id);
        }
    }
    /// Whether the photo's thumbnail already shows its edit (crop included).
    pub(super) fn has_edited_thumbnail(&self, id: PhotoId) -> bool {
        self.cache.has_edited(id)
    }
    /// The Library's cached preview for a photo, if one is loaded.
    pub(super) fn thumbnail(&self, id: PhotoId) -> Option<&egui::TextureHandle> {
        self.texture(self.photo(id)?)
    }
    pub(super) fn preview_progress_active(&self) -> bool {
        self.cache.progress_active()
    }
    pub(super) fn preview_progress(&self, ui: &mut egui::Ui) {
        self.cache.show_progress(ui);
    }
}
impl Library {
    /// Saves a Copy Name or metadata field still being typed, e.g. when the
    /// Library panel goes away before the field loses focus. On failure it
    /// stays pending, to be saved again or discarded.
    pub(super) fn commit_drafts(&mut self) -> Result<()> {
        if self.copy_names.commit(&mut self.session)? {
            self.filter();
        }
        self.commit_fields()
    }
    #[cfg(test)]
    pub(super) fn set_copy_name_draft(&mut self, id: PhotoId, name: &str) {
        self.copy_names.draft = Some((id, name.into()));
    }
    /// Whether a Copy Name or metadata field holds typing not saved yet.
    pub(super) fn has_drafts(&self) -> bool {
        self.copy_names.is_unsaved(&self.session.photos) || self.has_unsaved_fields()
    }
    /// Drops a Copy Name or metadata field that could not be saved, e.g.
    /// closing without saving.
    pub(super) fn discard_drafts(&mut self) {
        self.copy_names.clear();
        self.fields.clear();
    }
}
/// Why Develop cannot open a photo: it edits RAW, JPEG, PNG, TIFF, WebP, AVIF,
/// HEIF and JPEG XL files that are online and whose pixels it can read.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) enum Refusal {
    Offline,
    /// A JPEG, PNG, TIFF, WebP, AVIF, HEIF or JPEG XL of a kind Develop can't edit: its `format` (e.g. "JPG")
    /// and the `reason` (e.g. "CMYK").
    Unsupported {
        format: String,
        reason: String,
    },
}
impl Refusal {
    /// What opening `path` failed with, when it is a refusal rather than an error:
    /// a JPEG, PNG, TIFF, WebP, AVIF, HEIF or JPEG XL of a kind Develop can't edit.
    pub(in crate::app) fn opening(path: &std::path::Path, error: &anyhow::Error) -> Option<Self> {
        error
            .downcast_ref::<crate::raster::Unsupported>()
            .map(|unsupported| Self::unsupported(path, unsupported))
    }
    fn unsupported(path: &std::path::Path, unsupported: &crate::raster::Unsupported) -> Self {
        Self::Unsupported {
            format: path
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_uppercase())
                .unwrap_or_default(),
            reason: unsupported.0.clone(),
        }
    }
    /// A word or two, shown beside a greyed-out Open in Develop.
    pub(in crate::app) fn label(&self) -> String {
        match self {
            Self::Offline => "Offline".into(),
            Self::Unsupported { format, .. } => format!("{format} file"),
        }
    }
    /// The reason in full, with what to do about it.
    pub(in crate::app) fn detail(&self) -> String {
        match self {
            Self::Offline => {
                "The photo is offline. Right-click it and choose Find on Disk… to pick its file, or relink its folder with Locate root folder or Locate this folder."
                    .into()
            }
            Self::Unsupported { reason, .. } => format!("RAWmakase can't edit {reason} images"),
        }
    }
}
/// A catalog photo as Develop would open it, from `Library::develop_source`.
#[derive(Clone)]
pub(in crate::app) struct DevelopSource {
    pub path: std::path::PathBuf,
    pub edit: EditSource,
    /// Identifies `edit` (the defaults included), the file and the demosaic: it
    /// changes whenever any of them does.
    pub tag: String,
}
/// Why Develop cannot open a photo whose file is `available` or not, by what is
/// known without reading the file: for work over many photos (export, preview
/// builds), where reading every file's header first would hold the app up. Such
/// work meets a file Develop can't edit when it opens it, as that photo's failure.
pub(in crate::app) fn availability_refusal(available: bool) -> Option<Refusal> {
    (!available).then_some(Refusal::Offline)
}
/// Why Develop cannot open `photo`, if it cannot, given whether its file is
/// `available`: [`availability_refusal`], or a JPEG, PNG, TIFF, WebP, AVIF, HEIF or JPEG XL whose header
/// says Develop can't edit its pixels (`raster::probe`). It reads the file (a
/// whole JPEG), so it is for one photo at a time: opening it, or showing why it
/// can't be. A RAW's file is not read.
pub(in crate::app) fn develop_refusal(photo: &Photo, available: bool) -> Option<Refusal> {
    if let Some(refusal) = availability_refusal(available) {
        return Some(refusal);
    }
    if crate::storage::is_raw(&photo.path) {
        return None;
    }
    crate::raster::probe(&photo.path)
        .err()
        .map(|unsupported| Refusal::unsupported(&photo.path, &unsupported))
}
/// The edit a photo's previews are rendered with: its RAWmakase recipe, or
/// else its Lightroom settings.
fn edit_source(catalog: &Catalog, id: PhotoId) -> Option<previews::EditSource> {
    let (recipe, lightroom) = catalog.edit_texts(id).ok()?;
    recipe
        .map(previews::EditSource::Recipe)
        .or(lightroom.map(previews::EditSource::Lightroom))
}

mod availability;
mod capture;
/// Lightroom-style grid cells: the label tints the cell, while selection uses
/// a lighter surround instead of the app's blue button fill.
mod cell;
mod checksums;
mod collections;
mod compare;
mod copy_name;
mod descriptive;
pub(crate) mod filmstrip;
mod filter;
mod filter_bar;
mod folder_selection;
mod grid;
mod import_stacks;
#[cfg(test)]
mod import_stacks_tests;
mod info;
mod kinetic;
mod layout;
mod loupe;
mod metadata;
mod metadata_fields;
mod photo_info;
mod previews;
mod quick;
mod rows;
mod screen;
mod selection;
mod sidebar;
mod sort;
mod stacks;
mod stage;
mod survey;
mod textures;
mod thumbnails;
mod tree;
mod views;
mod volumes;
pub(crate) use volumes::remember_drives_in;
mod zoom;
use thumbnails::thumbnail;
#[cfg(test)]
mod tests;

impl Library {}

/// Asks `ctx` for a repaint, for a background read with something to show.
fn wake(ctx: &egui::Context) -> crate::catalog_session::Wake {
    let ctx = ctx.clone();
    std::sync::Arc::new(move || ctx.request_repaint())
}
