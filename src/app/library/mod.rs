//! Catalog browsing; thumbnail work is bounded and independent of RAW development.
use crate::app::ui::widgets::plural;
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
    /// Import Photos…: copies from a card or folder.
    ImportPhotos,
    /// The backdrop chosen from the Loupe's menu, for the editor to keep.
    Backdrop(crate::app::view::backdrop::Backdrop),
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
pub(in crate::app) use cell::{
    EDIT_IN_SHORTCUT, EXPORT_SHORTCUTS, PhotoAction, copy_suffix, edit_in_submenu,
};
pub(crate) use collection_edit::CollectionTreeCommand;
pub(in crate::app) use collection_edit::NewCollection;
pub(in crate::app) use collections::ListedCollection;
pub(crate) use descriptive::DescriptiveCommand;
pub(in crate::app) use descriptive::parse_keywords;
pub(crate) use filmstrip::{DraggedPhoto, Module, Pick};
pub(crate) use keyword_list::KeywordCommand;
pub(crate) use metadata::{Metadata, MetadataCommand};
pub(in crate::app) use previews::{EditSource, Pace, PreviewAi, PreviewDefaults};
pub(crate) use quick::CollectionCommand;
pub(in crate::app) use stacks::StackAction;
pub(crate) use stacks::StackCommand;
pub(in crate::app) use thumbnails::thumbnail_of;
/// Lightroom's virtual copy commands, carried out by the editor so the open
/// edit is saved first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CopyAction {
    Create(PhotoId),
    SetMaster(PhotoId),
    /// Asks first, as Lightroom does.
    Remove(PhotoId),
}
/// What a delete key does (see [`Library::delete_command`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum DeleteKey {
    /// Move to Trash, or Remove from Catalog, as [`Library::delete_key`] decides.
    Delete,
    /// Remove from Catalog.
    Remove,
    DeleteRejected,
    /// Move to Trash.
    Trash,
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
/// A collection or set being dragged in the Collections panel, with the
/// set it is in and every collection and set inside it, which it can't be
/// dropped into.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DraggedCollection {
    pub id: crate::catalog::CollectionId,
    pub parent: Option<crate::catalog::CollectionId>,
    pub inside: std::collections::HashSet<crate::catalog::CollectionId>,
    pub name: String,
    pub set: bool,
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
    /// Build Editable Previews.
    BuildEditable(Vec<PhotoId>),
    /// Discard Editable Previews.
    DiscardEditable(Vec<PhotoId>),
}
pub(crate) struct Library {
    /// The open catalog and the photos, folders and collections read from it.
    pub session: CatalogSession,
    /// The active photo and the photos selected with it.
    selection: selection::Selection,
    /// The external editors' names, for the photo menu's Edit In.
    pub(in crate::app) external_editors: std::sync::Arc<[String]>,
    /// Grid columns last frame, for Up and Down.
    grid_columns: usize,
    /// Whole grid rows the view showed last frame, for Page Up and Page Down.
    grid_page_rows: usize,
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
    /// The text typed in Filter Folders, kept for this Library only.
    folder_filter: String,
    /// A folder row to scroll into view once it is drawn, by key.
    scroll_to_folder: Option<String>,
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
    /// Every photo's camera, lens and settings for the Metadata filter and
    /// the search, read once they are needed.
    facets: facets::Cache,
    /// The Metadata filter's strip and Find fields.
    metadata_bar: metadata_bar::MetadataBar,
    /// Smart collections' members, as last taken from the session.
    smart: smart::Smart,
    /// Create or Edit Smart Collection, while open.
    smart_editor: Option<smart_editor::SmartEditor>,
    /// Smart collection rule changes not yet handed to the undo log.
    smart_done: Vec<smart_editor::SmartRulesCommand>,
    strip: filmstrip::State,
    /// Counts changes to the photos shown, their order or their metadata,
    /// so the filmstrip notices a change made after it was drawn.
    shown_version: u64,
    /// Indices into `photos` of the ones shown, in display order.
    visible: Vec<usize>,
    availability: availability::Availability,
    ctx: egui::Context,
    cache: textures::PreviewTextures,
    /// What the Library asked the editor to do, in order ([`outbox`]).
    outbox: outbox::Outbox,
    /// After a synchronization added photos, the status bar offers to show
    /// them.
    pub(in crate::app) offer_previous_import: bool,
    copy_names: copy_name::CopyNames,
    /// Title, caption and the other descriptive fields being shown or typed.
    fields: metadata_fields::Fields,
    /// The map under the GPS row, off until Preferences turns it on.
    map: map::Map,
    keyword_list: keyword_list::KeywordList,
    keyword_sets: keyword_sets::KeywordSetPanel,
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
    /// Collections and sets made, renamed, moved or deleted since the undo
    /// log last took them.
    collection_tree_done: Vec<collection_edit::CollectionTreeCommand>,
    /// Create Collection…, Rename… or Delete's question, while open.
    collection_dialog: Option<collection_edit::CollectionDialog>,
    /// The folders' sort and direction while a collection with its own is
    /// shown; they come back when it is left. `None` when no collection is.
    folder_sort: Option<(sort::Sort, bool)>,
    /// Descriptive metadata changes not yet handed to the shared undo log.
    descriptive_done: Vec<DescriptiveCommand>,
    /// Stack changes not yet handed to the shared undo log.
    stack_done: Vec<StackCommand>,
    /// Keyword List changes made since the undo log last took them.
    keyword_done: Vec<KeywordCommand>,
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
    attached: Option<HashSet<crate::platform::volume::Volume>>,
    pub(in crate::app) messages: crate::app::shell::messages::Messages,
    /// Background reads are held off while a file operation holds the
    /// catalog: nothing starts them until it lets go.
    reads_suspended: bool,
    /// The offline RAW in the Loupe and its stored preview's identity, as the
    /// editor works it out.
    loupe_stored: Option<(PhotoId, String)>,
    /// The photo whose refusal the right panel last showed, and that refusal.
    refusal_shown: Option<((PhotoId, std::path::PathBuf, bool), Option<Refusal>)>,
    /// The editable previews the catalog records, by master photo, as read when
    /// the Library last filtered or was told of a build, discard or change.
    editable_previews: HashMap<PhotoId, crate::catalog::EditablePreview>,
    /// Read Metadata from Files while it reads.
    reread: Option<descriptive::Reread>,
    /// Read Metadata from Files finished since the editor last asked.
    reread_finished: bool,
    /// What photos without an edit are previewed with (see `set_defaults`).
    defaults: PreviewDefaults,
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
            not_compacted,
            upgraded,
        } = opened;
        session.wake_with(wake(&ctx));
        let loupe = loupe::Loupe::new(&ctx);
        let screen = screen::ScreenPreviews::new(&ctx);
        let mut s = Self {
            session,
            selection: Default::default(),
            external_editors: std::sync::Arc::from(Vec::new()),
            grid_columns: 1,
            grid_page_rows: 1,
            scroll_to_active: false,
            missing_noted: false,
            volumes: Default::default(),
            sidebar_trees: None,
            filters: Default::default(),
            selected_folders: Default::default(),
            subfolders_shown: true,
            expanded: HashSet::new(),
            folder_filter: String::new(),
            scroll_to_folder: None,
            following: None,
            new_folder_in: None,
            thumb_size: 190.,
            cell_style: Default::default(),
            drawn_style: Default::default(),
            drawn_pass: 0,
            sort_keys: None,
            cell_info: HashMap::new(),
            facets: Default::default(),
            metadata_bar: Default::default(),
            smart: Default::default(),
            smart_editor: None,
            smart_done: Vec::new(),
            strip: filmstrip::State::default(),
            shown_version: 0,
            visible: Vec::new(),
            availability: Default::default(),
            cache: textures::PreviewTextures::new(&ctx),
            ctx,
            offer_previous_import: false,
            copy_names: Default::default(),
            fields: Default::default(),
            map: map::Map::new(),
            keyword_list: Default::default(),
            keyword_sets: Default::default(),
            done: Vec::new(),
            collection_done: Vec::new(),
            collection_tree_done: Vec::new(),
            collection_dialog: None,
            folder_sort: None,
            descriptive_done: Vec::new(),
            stack_done: Vec::new(),
            keyword_done: Vec::new(),
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
            outbox: Default::default(),
            messages: Default::default(),
            reads_suspended: false,
            loupe_stored: None,
            refusal_shown: None,
            editable_previews: HashMap::new(),
            reread: None,
            reread_finished: false,
        };
        s.reloaded();
        s.check_files();
        if let Some(upgraded) = upgraded {
            s.messages.note(upgrade_note(&upgraded));
        }
        if let Some(e) = not_compacted {
            s.messages.warn(format!(
                "The catalog couldn't be made smaller: {e}. It is tried again the next time \
                 the catalog opens."
            ));
        }
        // Start with a selection, as Lightroom does, so the side panels are filled.
        s.select(s.visible.first().map(|i| s.session.photos()[*i].id));
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
        self.availability.start(self.session.photos(), &self.ctx);
        self.cache.retry_failed();
        self.screen.retry_failed();
        self.filter();
    }
    /// After a change made through another connection that could not be
    /// shown by opening the catalog again: forgets the previews of the
    /// photos `removed` (new photos may have their ids) and of those
    /// `moved` to another file (renamed included), and reads the
    /// catalog again. If that fails too, no photo is shown, so none can be
    /// changed under an id that now names another; the catalog stays open,
    /// for an edit still to be saved. A Loupe whose photo was removed stays
    /// open on the photo after it, as Lightroom's does.
    pub(in crate::app) fn reread_after(
        &mut self,
        removed: &[PhotoId],
        moved: &[PhotoId],
    ) -> Result<()> {
        let gone = self.selection.active;
        let successor = self.loupe_successor(removed);
        for id in removed.iter().chain(moved) {
            self.cache.forget(*id);
            self.screen.forget(*id);
        }
        let reread = self.reload();
        match &reread {
            // Moved and added photos are looked for where they are now.
            Ok(()) => {
                self.check_files();
                // Only when its photo is gone: one a failed operation may
                // have left is shown still.
                if let Some((id, direction)) = successor
                    && gone.is_some_and(|gone| self.photo(gone).is_none())
                    && self.position(id).is_some()
                {
                    self.select(Some(id));
                    self.loupe_direction = direction;
                    self.scroll_to_active = true;
                }
            }
            Err(_) => {
                // Neither photos nor folders: a folder removed may have
                // given its id to a new one.
                self.session.forget_lists();
                self.filters.folder_scope = None;
                self.selected_folders.clear();
                self.reloaded();
            }
        }
        reread
    }
    /// [`Self::reread_after`] when photos were only added, by a photo
    /// source's report: no id changed, so the metadata fields are read again
    /// rather than forgotten, and a keyword being typed (never saved until
    /// Enter) stays with the photos it is typed for. Callers save the other
    /// drafts first ([`Self::commit_drafts`]).
    pub(in crate::app) fn reread_after_added(&mut self) -> Result<()> {
        let fields = std::mem::take(&mut self.fields);
        let reread = self.reread_after(&[], &[]);
        if reread.is_ok() {
            self.fields = fields;
            self.fields.reload();
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
        self.forget_facets();
        self.sort_keys = None;
        self.shown_version += 1;
        if self.filters.previous_import {
            self.filters.members = self.session.previous_import().clone();
        }
        if let Some(id) = self.filters.collection {
            if self.session.collections().iter().any(|c| c.id == id) {
                self.filters.members = self.source_members(id);
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
        if self.availability.poll(true, self.session.photos()) {
            self.availability_known();
        }
    }
    /// Checks again which photos are online when an external volume comes
    /// back, so they show and their capture times are read.
    fn volumes_checked(
        &mut self,
        online: &HashMap<crate::platform::volume::Volume, volumes::VolumeState>,
    ) {
        // Nothing to compare with until the first check has answered.
        if online.is_empty() {
            return;
        }
        let attached: HashSet<_> = online
            .iter()
            .filter(|(_, (on, _))| *on)
            .map(|(volume, _)| volume.clone())
            .collect();
        // The first answer may come during or after the online check (the
        // sidebar starts the volume check), so that check is made again
        // unless it finished with every photo online.
        let returned = match &self.attached {
            Some(before) => attached.iter().any(|volume| !before.contains(volume)),
            None => {
                self.availability.checking()
                    || self
                        .session
                        .photos()
                        .iter()
                        .any(|p| !self.is_available(&p.path))
            }
        };
        self.attached = Some(attached);
        if returned {
            self.availability.start(self.session.photos(), &self.ctx);
        }
    }
    /// Checks again, in the background, which photos are online when one
    /// counted offline turns out to be there, e.g. restored in place on a
    /// drive that stayed attached.
    pub(in crate::app) fn found(&mut self, path: &std::path::Path) {
        if !self.is_available(path) {
            self.availability.start(self.session.photos(), &self.ctx);
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
    pub(in crate::app) fn shown_refusal(&mut self, photo: &Photo) -> Option<Refusal> {
        let available = self.is_available(&photo.path);
        let key = (photo.id, photo.path.clone(), available);
        match &self.refusal_shown {
            Some((shown, refusal)) if *shown == key => refusal.clone(),
            _ => {
                let refusal = self.develop_from(photo, available).err();
                self.refusal_shown = Some((key, refusal.clone()));
                refusal
            }
        }
    }
    /// What Develop opens `photo` from, given whether its file is `available`:
    /// its original, as [`develop_refusal`] allows; or, offline, its editable
    /// preview, only while nothing is at the original's path
    /// ([`Original::is_absent`](crate::edits::Original::is_absent)) and the
    /// catalog records the copy up to date with its file there
    /// ([`EditablePreview::usable`](crate::catalog::EditablePreview::usable)).
    /// Why not, if neither. The copy's contents are not read here: the loader
    /// opens it, and says why it can't.
    pub(in crate::app) fn develop_from(
        &self,
        photo: &Photo,
        available: bool,
    ) -> Result<crate::app::worker::OpenedFrom, Refusal> {
        use crate::app::worker::{EditablePreviewSource, OpenedFrom};
        match develop_refusal(photo, available) {
            None => return Ok(OpenedFrom::Original),
            Some(Refusal::Offline) => {}
            Some(refusal) => return Err(refusal),
        }
        let catalog = self.session.catalog();
        let copy = match catalog.editable_preview(photo.id) {
            Ok(Some(copy)) => copy,
            Ok(None) => return Err(Refusal::Offline),
            Err(e) => return Err(Refusal::Copy(Unusable::Unreadable(format!("{e:#}")))),
        };
        if !crate::edits::Original::is_absent(&photo.path) {
            return Err(Refusal::Copy(Unusable::NotAbsent));
        }
        let folder = catalog.editable_previews_folder();
        let file = copy.usable(&folder, |path| {
            anyhow::ensure!(path.is_file(), "its file is missing");
            Ok(())
        });
        match file {
            Some(file) => Ok(OpenedFrom::EditablePreview(EditablePreviewSource {
                file,
                original: copy.original,
            })),
            None => Err(Refusal::Copy(Unusable::OutOfDate)),
        }
    }
    /// Photo `id`'s editable preview as the catalog recorded it when last read:
    /// its master's, which a virtual copy shares.
    pub(in crate::app) fn editable_preview(
        &self,
        id: PhotoId,
    ) -> Option<&crate::catalog::EditablePreview> {
        let photo = self.photo(id)?;
        self.editable_previews
            .get(&photo.master.unwrap_or(photo.id))
    }
    /// Reads again which photos have an editable preview, after a build,
    /// discard or anything else that changed them; what the right panel says of
    /// the selected photo is worked out again.
    pub(in crate::app) fn editable_previews_changed(&mut self) {
        self.refusal_shown = None;
        // Unreadable, as none: opening such a photo says why.
        self.editable_previews = self
            .session
            .catalog()
            .editable_previews()
            .unwrap_or_default()
            .into_iter()
            .map(|copy| (copy.master, copy))
            .collect();
        // Offline photos with an up-to-date copy: Develop's renders of them from
        // it are looked up under the copy's file. Its file isn't looked at here,
        // as this runs with every filter: the preview cache checks the stamp of
        // a file that is there, and finds nothing for one never rendered.
        let folder = self.session.catalog().editable_previews_folder();
        let copies = self
            .session
            .photos()
            .iter()
            .filter(|photo| !self.availability.is_available(&photo.path))
            .filter_map(|photo| {
                let copy = self
                    .editable_previews
                    .get(&photo.master.unwrap_or(photo.id))
                    .filter(|copy| !copy.out_of_date)?;
                Some((photo.id, copy.path_in(&folder)?))
            })
            .collect();
        self.cache.set_copies(copies);
    }
    /// Notes that photo `id`'s original is the file `now` names, as Develop
    /// read it opening the photo: a copy built from another file goes out of
    /// date. A failure to note it is not worth stopping for: the copy is
    /// still checked against the file whenever the file is there.
    pub(in crate::app) fn note_original(&mut self, id: PhotoId, now: &crate::storage::Identity) {
        let before = self.editable_preview(id).map(|copy| copy.out_of_date);
        if let Ok(after) = self.session.note_original(id, now)
            && after.map(|copy| copy.out_of_date) != before
        {
            self.editable_previews_changed();
        }
    }
    pub(crate) fn available_count(&self) -> usize {
        self.availability.count(self.session.photos())
    }
    fn filter(&mut self) {
        self.editable_previews_changed();
        // Leaving the collections: the folders' order again.
        if self.filters.collection.is_none()
            && let Some((sort, reverse)) = self.folder_sort.take()
        {
            self.filters.sort = sort;
            self.filters.reverse = reverse;
        }
        self.follow_keyword_names();
        // What the order needs from the catalog, read once until it changes.
        let sort = self.filters.sort;
        // A custom order is read again each time: a drag or Undo changes it.
        if sort == sort::Sort::Custom || self.sort_keys.as_ref().is_none_or(|(of, _)| *of != sort) {
            let collection = self.filters.collection;
            self.sort_keys = Some((sort, sort.keys(self.session.catalog(), collection)));
        }

        if self.filters.needs_facets() {
            self.load_facets();
        }
        let keys = &self.sort_keys.as_ref().unwrap().1;
        self.shown_version += 1;
        // Filtering follows every change to the photos, so the count does too.
        self.availability.forget_count();
        self.visible = self.filters.visible(
            self.session.photos(),
            |path| self.availability.is_available(path),
            keys,
            self.facets(),
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
                .position(|i| self.session.photos()[*i].id == id)
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
        self.session.photos().iter().find(|p| p.id == id)
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
        let edit = match self.session.catalog().load_edit(id, &photo.path) {
            Ok(Some(saved)) => Some(EditSource::saved(saved.recipe)),
            Ok(None) => match self.session.catalog().preview_edit(id) {
                Ok(crate::catalog::PreviewEdit::Lightroom(text)) => {
                    Some(EditSource::Lightroom(text))
                }
                _ => None,
            },
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
            .position(|i| self.session.photos()[*i].id == id)?;
        let n = (at as i32 + delta).clamp(0, self.visible.len().saturating_sub(1) as i32) as usize;
        self.visible.get(n).map(|i| self.session.photos()[*i].id)
    }
    fn labels(&self) -> Vec<String> {
        let mut labels: Vec<String> = crate::app::library::photo_metadata::LABELS
            .iter()
            .map(|s| (*s).into())
            .collect();
        let mut custom: Vec<_> = self
            .session
            .photos()
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
            .map(|i| self.session.photos()[*i].id)
            .collect()
    }
    /// Edits of `ids` were written on another connection than the session's
    /// (autosave, Sync and its Undo, batch presets and Auto, an extension's
    /// proposal), whether or not each was: the one place the session hears of
    /// them, so smart collections see them. Autosave calls this alone; the
    /// others through [`Self::edits_changed`].
    pub(in crate::app) fn edits_written(&mut self, ids: &[PhotoId]) {
        self.session.note_external_change(ids.iter().copied());
    }
    /// Edits written elsewhere (Sync, its Undo): their previews render again.
    pub(in crate::app) fn edits_changed(&mut self, ids: impl IntoIterator<Item = PhotoId>) {
        let ids: Vec<PhotoId> = ids.into_iter().collect();
        self.edits_written(&ids);
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
        // The folders' order as it is now, whatever the place was shown in.
        let folders = self
            .folder_sort
            .unwrap_or((self.filters.sort, self.filters.reverse));
        self.filters = place.filters.clone();
        if self
            .filters
            .collection
            .is_some_and(|id| self.has_own_sort(id))
        {
            self.folder_sort = Some(folders);
        } else {
            self.folder_sort = None;
            (self.filters.sort, self.filters.reverse) = folders;
        }
        self.selected_folders = place.folders.clone();
        // The source as the catalog has it now, e.g. after a folder gained
        // subfolders since.
        if let Some(scope) = self.current_scope(&place.folders) {
            self.filters.folder_scope = Some(scope);
        }
        if let Some(id) = self.filters.collection {
            // Deleted since: All Photographs, as when it is deleted.
            if self.session.collections().iter().any(|c| c.id == id) {
                self.filters.members = self.source_members(id);
            } else {
                self.filters.collection = None;
                self.filters.members.clear();
            }
        }
        self.selection = place.selection.clone();
        self.filter();
        self.compare.restored = true;
        self.scroll_to_active = true;
    }
    /// Read Metadata from Files is still applying what it read.
    pub(in crate::app) fn rereading(&self) -> bool {
        self.reread.is_some()
    }
    /// Whether Read Metadata from Files finished since the last call, for
    /// the top bar to show its outcome.
    pub(in crate::app) fn take_reread_finished(&mut self) -> bool {
        std::mem::take(&mut self.reread_finished)
    }
    /// Asks the editor for `request`, as of the catalog shown now.
    fn ask(&mut self, request: outbox::Request) {
        let generation = self.session.generation();
        self.outbox.push(request, generation);
    }
    /// The oldest request not yet handed over, its photos checked again when the
    /// lists changed since it was made ([`outbox`]). One the change left with no
    /// photo is not handed over: it says so and the next comes instead.
    pub(in crate::app) fn next_request(&mut self) -> Option<outbox::Request> {
        while let Some((request, generation)) = self.outbox.pop() {
            if generation == self.session.generation() {
                return Some(request);
            }
            if let Some(request) = request.revalidated(|id| self.photo(id).is_some()) {
                return Some(request);
            }
            self.messages
                .warn("The photos chosen are no longer in the catalog; nothing was done.");
        }
        None
    }
    /// Delete and Backspace: in a collection (the Quick Collection too),
    /// takes `ids` out of it, as one undoable step, as Lightroom does; in a
    /// collection set, a smart collection or a saved print, book, slideshow
    /// or web creation, nothing but a warning; elsewhere, asks to remove them
    /// from the catalog.
    pub(in crate::app) fn delete_key(&mut self, ids: Vec<PhotoId>) {
        if ids.is_empty() {
            return;
        }
        if self.collection_removal().is_some() {
            self.remove_from_collection(ids);
        } else if let Some(id) = self.filters.collection {
            if self.is_set(id) {
                self.messages
                    .warn("Select a collection to take photos out of it");
            } else if self
                .session
                .collections()
                .iter()
                .any(|c| c.id == id && c.kind == crate::catalog::CollectionKind::Smart)
            {
                self.messages
                    .warn("A smart collection's photos are the ones its rules match");
            } else {
                self.messages.warn(
                    "Photos can’t be taken out of a saved print, book, slideshow or web gallery",
                );
            }
        } else {
            self.ask(outbox::Request::File(FileRequest::Delete(ids)));
        }
    }
    /// The thumbnail menu's label for taking photos out of the source shown:
    /// Remove from Quick Collection or Remove from Collection, as Lightroom
    /// Classic names them. `None` where no photo can be taken out by hand:
    /// a folder, All Photographs, a collection set, a smart collection, or a
    /// saved print, book, slideshow or web creation imported from Lightroom.
    pub(in crate::app) fn collection_removal(&self) -> Option<&'static str> {
        if self.showing_quick() {
            return Some("Remove from Quick Collection");
        }
        let id = self.filters.collection?;
        self.session
            .collections()
            .iter()
            .any(|c| {
                c.id == id && c.kind == crate::catalog::CollectionKind::Collection && c.takes_photos
            })
            .then_some("Remove from Collection")
    }
    /// Remove from Collection, or Delete in a collection: takes `ids` out of
    /// the collection shown, as one undoable step. The photos stay in the
    /// catalog and in their other collections; a virtual copy comes out
    /// alone, its master and other copies staying in. Does nothing where
    /// [`Self::collection_removal`] offers nothing.
    pub(in crate::app) fn remove_from_collection(&mut self, ids: Vec<PhotoId>) {
        let Some(id) = self.filters.collection else {
            return;
        };
        if ids.is_empty() || self.collection_removal().is_none() {
            return;
        }
        let (name, failed) = if self.showing_quick() {
            (
                "Quick Collection".to_string(),
                "Quick Collection not changed",
            )
        } else {
            let name = self
                .session
                .collections()
                .iter()
                .find(|c| c.id == id)
                .map_or_else(String::new, |c| c.name.clone());
            (format!("“{name}”"), "Collection not changed")
        };
        let summary = format!(
            "Removed from {name} · {}",
            plural(ids.len(), "photo", "photos")
        );
        if let Err(e) = self.record_collection(id, Vec::new(), ids, summary) {
            self.messages.fail(format!("{failed}: {e}"));
        }
    }
    /// The delete command a key ran, for `ids`. By default Delete and
    /// Backspace: alone, as [`Self::delete_key`]; with Option, Remove from
    /// Catalog, even in a collection; with ⌘⌥⇧, Move to Trash; with ⌘, Delete
    /// Rejected Photos (Ctrl for ⌘ off the Mac).
    pub(in crate::app) fn delete_command(&mut self, command: DeleteKey, ids: Vec<PhotoId>) {
        match command {
            DeleteKey::Trash => self.request_trash(ids),
            DeleteKey::DeleteRejected => {
                self.ask(outbox::Request::File(FileRequest::DeleteRejected))
            }
            DeleteKey::Remove => self.request_removal(ids),
            DeleteKey::Delete => self.delete_key(ids),
        }
    }
    /// Rename Photo… or Rename Photos… for `ids` (F2, the thumbnail menu),
    /// for the editor to ask.
    pub(in crate::app) fn request_rename(&mut self, ids: Vec<PhotoId>) {
        if !ids.is_empty() {
            self.ask(outbox::Request::File(FileRequest::Rename(ids)));
        }
    }
    /// Move to Trash for `ids` (⌘⌥⇧Delete, the thumbnail menu), whatever is
    /// shown, for the editor to confirm.
    pub(in crate::app) fn request_trash(&mut self, ids: Vec<PhotoId>) {
        if !ids.is_empty() {
            self.ask(outbox::Request::File(FileRequest::Trash(ids)));
        }
    }
    /// Collection `id`'s photos: a smart collection's as last worked out.
    pub(super) fn members_of_collection(
        &self,
        id: crate::catalog::CollectionId,
    ) -> HashSet<PhotoId> {
        if self
            .session
            .collections()
            .iter()
            .any(|c| c.id == id && c.kind == crate::catalog::CollectionKind::Smart)
        {
            return self.smart_members_of(id);
        }
        self.session
            .collection_photos()
            .get(&id)
            .cloned()
            .unwrap_or_default()
    }
    /// The rejected photos of the source shown (its folders, collection,
    /// previous import or offline photos), whatever the filter bar shows:
    /// Delete Rejected Photos'.
    pub(in crate::app) fn rejected_in_source(&self) -> Vec<PhotoId> {
        self.session
            .photos()
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
            self.ask(outbox::Request::File(FileRequest::Remove(ids)));
        }
    }
    /// Moves `photos` into the folder of `root` at logical path `to`
    /// (dropped on it), for the editor to confirm; a drop where they all
    /// are already does nothing.
    pub(in crate::app) fn request_move(
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
            self.ask(outbox::Request::File(FileRequest::Move {
                photos,
                root,
                to,
                name,
            }));
        }
    }
    /// New Folder Inside… folder `parent` of `root` (catalog folder
    /// `folder`, if it is one), offering to move the selection into it.
    pub(in crate::app) fn request_new_folder(
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
        self.ask(outbox::Request::File(FileRequest::NewFolder {
            root,
            parent,
            name,
            photos,
            inside,
        }));
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
            .folders()
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
            .folders()
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
    /// The grid's latest request for the photo's edited thumbnail, if any.
    pub(in crate::app) fn edited_ticket(&self, id: PhotoId) -> Option<u64> {
        self.cache.edited_ticket(id)
    }
    /// A built preview, as the photo's edited thumbnail (converted to the
    /// monitor profile first); the caller has checked it is still the photo's
    /// latest.
    pub(in crate::app) fn show_built_preview(
        &mut self,
        ctx: &egui::Context,
        id: PhotoId,
        image: &image::RgbImage,
    ) {
        self.cache.show_built(ctx, id, image);
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
                .roots()
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
            .folders()
            .iter()
            .find(|f| f.root == root && f.relative.trim_end_matches('/') == relative)?;
        let name = relative.rsplit('/').next().unwrap_or(relative).to_string();
        Some((SyncScope::Folder(folder.id), name))
    }
    /// The root and logical path of catalog folder `id`, as the Library has it.
    pub(in crate::app) fn folder_place(&self, id: FolderId) -> Option<(RootId, String)> {
        let folder = self.session.folders().iter().find(|f| f.id == id)?;
        Some((folder.root, folder.relative.clone()))
    }
    /// Creates a virtual copy of `id` and selects it. Made while a
    /// collection is shown, the copy joins it, as in Lightroom.
    pub(in crate::app) fn create_virtual_copy(
        &mut self,
        id: PhotoId,
    ) -> Result<Committed<PhotoId>> {
        let made = self
            .session
            .create_virtual_copy(id, self.filters.collection)?;
        if self.listed_after(&made.listed, "Copy made") {
            self.show(made.value);
            if let Some(p) = self.photo(made.value) {
                self.messages
                    .done(format!("Created {} of {}", p.copy_name, p.filename));
            }
        }
        Ok(made)
    }
    pub(in crate::app) fn set_copy_as_master(&mut self, id: PhotoId) -> Result<Committed<()>> {
        // What the background reads read of the masters before is dropped; read
        // again after.
        self.stop_background_reads();
        let made = match self.session.set_copy_as_master(id) {
            Ok(made) => made,
            Err(e) => {
                self.start_background_reads();
                return Err(e);
            }
        };
        if self.listed_after(&made.listed, "Master changed") {
            self.show(id);
            if let Some(p) = self.photo(id) {
                self.messages
                    .done(format!("This copy is now the master of {}", p.filename));
            }
        }
        self.start_background_reads();
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
            .photos()
            .iter()
            .filter(|p| folders.contains(&p.folder))
            .map(|p| p.id)
            .collect();
        for photo in self.session.photos() {
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
    pub(in crate::app) fn remove_virtual_copy(
        &mut self,
        id: PhotoId,
    ) -> Result<Committed<Option<PhotoId>>> {
        let photo = self.photo(id).cloned();
        // Forgotten first: once the copy is removed its id can be reused, so its
        // previews must go even if reading the catalog again fails.
        self.cache.forget(id);
        self.screen.forget(id);
        // What the background reads read before may hold the id: dropped, and read
        // again after.
        self.stop_background_reads();
        let removed = match self.session.remove_virtual_copy(id) {
            Ok(removed) => removed,
            Err(e) => {
                self.start_background_reads();
                return Err(e);
            }
        };
        self.selection.forget_photo(id);
        let master = photo.as_ref().and_then(|p| p.master);
        if self.listed_after(&removed.listed, "Copy removed") {
            if let Some(master) = master {
                self.show(master);
            }
            if let Some(p) = photo {
                self.messages
                    .done(format!("Removed {} of {}", p.copy_name, p.filename));
            }
        } else {
            // The session dropped the copy from its lists anyway; what is shown
            // follows them before anything reads those lists.
            self.reloaded();
        }
        self.start_background_reads();
        Ok(Committed {
            value: master,
            listed: removed.listed,
        })
    }
    /// Selects photo `id`, showing it: its stack expanded, and the filters
    /// and folder that hide it cleared.
    pub(in crate::app) fn show(&mut self, id: PhotoId) {
        // A photo a collapsed stack hides shows once its stack is expanded.
        self.expand_stack_of(id);
        if !self
            .visible
            .iter()
            .any(|i| self.session.photos()[*i].id == id)
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
            .any(|i| self.session.photos()[*i].id == id)
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
    /// Converts the Library's previews to the monitor profile `profile` from
    /// now on (none: sRGB as it is); the workspace sets it every frame.
    pub(in crate::app) fn set_display(&mut self, profile: Option<std::path::PathBuf>) {
        let display = crate::app::preferences::monitor::DisplayProfile::new(profile);
        self.loupe.set_display(display.clone());
        self.screen.set_display(display.clone());
        self.cache.set_display(display);
    }
    /// Shows the Metadata panel's map from `source`, or none; the workspace
    /// sets it every frame.
    pub(in crate::app) fn set_map(&mut self, source: Option<map::Source>) {
        self.map.set_source(source);
    }
    /// Ends the map's tile workers at exit; they are then waited for.
    pub(in crate::app) fn close_map(&mut self) -> Vec<crate::app::task::Stopping> {
        self.map.close()
    }
    /// Drain in every workspace so the bounded worker never waits for the grid.
    pub(in crate::app) fn poll_previews(&mut self, ctx: &egui::Context) {
        if self.availability.poll(false, self.session.photos()) {
            self.availability_known();
        }
        self.poll_smart();
        self.poll_capture_times();
        self.poll_photo_info();
        self.poll_import_stacks();
        self.poll_checksums();
        self.poll_reread();
        self.cache.poll(ctx);
        self.screen.poll(ctx);
        for e in self.screen.take_failures() {
            self.messages.warn(format!("Preview unavailable: {e}"));
        }
    }
    /// Hands the worker the photos shown last frame, and at full pace asks
    /// for the edited previews of the ones after them. Call once per frame.
    pub(in crate::app) fn publish_shown(&mut self) {
        self.cache.publish_shown();
        self.screen.publish_shown();
        let catalog = self.session.catalog();
        self.cache.request_ahead(
            self.session.photos(),
            (self.shown_version, &self.visible),
            self.grid_shown.end,
            |id| edit_source(catalog, id),
        );
    }
    /// How fast edited previews are rendered from now on: at full pace while
    /// the Library is in front and nothing else renders.
    pub(in crate::app) fn set_preview_pace(&mut self, pace: previews::Pace) {
        self.cache.set_pace(pace);
    }
    /// The photo's preview: its edit once rendered, else the embedded one.
    fn texture(&self, photo: &Photo) -> Option<&egui::TextureHandle> {
        self.cache.texture(photo)
    }
    /// Queues the previews a shown photo needs; its edit comes from the catalog,
    /// and without one, should its file have no embedded preview to show, it is
    /// developed with the defaults.
    fn request_previews(&mut self, photo: &Photo, ctx: &egui::Context) {
        let (catalog, defaults) = (self.session.catalog(), &self.defaults);
        self.cache.request(
            photo,
            ctx,
            || edit_source(catalog, photo.id),
            || EditSource::Defaults(defaults.clone()),
        );
    }
    /// Replaces the edited-preview worker's queue with one nothing takes from:
    /// what is sent to it is read from the returned end.
    #[cfg(test)]
    pub(in crate::app) fn detach_edited_previews(&mut self) -> previews::Sent {
        let (queue, sent) = previews::EditQueue::detached();
        self.cache.edit_tx = queue;
        sent
    }
    /// Shows Develop's latest render as the photo's thumbnail and caches it
    /// under the edit it was rendered with, and what it shows of AI Denoise
    /// (`ai`), for the file `rendered` names (the one its pixels were decoded
    /// from); without one it is not cached. A render of the photo's editable
    /// preview is cached under the copy's file, never the original's, so the
    /// original's renders never find it.
    pub(in crate::app) fn update_edited(
        &mut self,
        ctx: &egui::Context,
        id: PhotoId,
        rendered: Option<RenderedFrom<'_>>,
        ai: PreviewAi,
        image: image::RgbImage,
        recipe: &crate::model::recipe::Recipe,
    ) {
        let Some(original) = self.photo(id).map(|p| p.path.clone()) else {
            return;
        };
        let (path, stamp) = match rendered {
            None => (original, None),
            Some(RenderedFrom::Original(file)) => (original, Some(file.stamp())),
            Some(RenderedFrom::Copy(copy, file)) => (copy.to_path_buf(), Some(file.stamp())),
        };
        let source = EditSource::saved(recipe.clone());
        self.cache
            .store_edited(ctx, id, path, stamp, &source, ai, image);
    }
    /// Renders photo `id` with the edit `recipe` as its thumbnail, the one shown
    /// staying until it comes: for an edit Develop shows but renders no
    /// thumbnail of, as with the Crop tool open. Not kept in the preview cache
    /// until rendered; the latest request for the photo wins.
    pub(in crate::app) fn render_edited(
        &mut self,
        id: PhotoId,
        recipe: &crate::model::recipe::Recipe,
    ) {
        let Some(photo) = self.photo(id).cloned() else {
            return;
        };
        self.cache
            .render_edited(&photo, EditSource::saved(recipe.clone()));
    }
    /// An AI Denoise result was stored for the file at `path`: its photos' edited
    /// previews are asked for again, so one shown plain shows the result. What is
    /// shown stays until the new preview arrives.
    pub(in crate::app) fn ai_result_stored(&mut self, path: &std::path::Path) {
        let ids: Vec<PhotoId> = self
            .session
            .photos()
            .iter()
            .filter(|p| p.path == path)
            .map(|p| p.id)
            .collect();
        for id in ids {
            self.cache.ask_again(id);
            self.screen.forget(id);
        }
    }
    /// The file at `path` was saved again by another program: its photos'
    /// thumbnails and previews are made again from it, those shown staying
    /// until the new ones come, as a file saved with another program's
    /// compression can take a while to decode.
    pub(in crate::app) fn file_rewritten(&mut self, path: &std::path::Path) {
        self.cache.renew_file(path);
        self.loupe.renew(path);
        let ids: Vec<PhotoId> = self
            .session
            .photos()
            .iter()
            .filter(|p| p.path == path)
            .map(|p| p.id)
            .collect();
        for id in ids {
            self.cache.ask_again(id);
            self.screen.forget(id);
        }
    }
    /// Keeps the map's tiles in the cache folder `cache` (the context's).
    pub(in crate::app) fn set_cache_dir(&mut self, cache: Option<&std::path::Path>) {
        self.map.keep_tiles_in(cache);
    }
    /// Edited previews with AI Denoise render from the finished results in `cache`.
    pub(in crate::app) fn set_ai_cache(&mut self, cache: &rawmakase_decode::ai::cache::AiCache) {
        self.cache.edit_tx.set_ai_cache(cache.clone());
        self.screen.set_ai_cache(cache);
    }
    /// The libraries the Library's previews open and resolve photos with, and
    /// its photo info is read with (the context's), set before it is shown.
    pub(in crate::app) fn set_libraries(&mut self, libraries: &crate::environment::Libraries) {
        self.cache.edit_tx.set_libraries(libraries.clone());
        self.screen.set_libraries(libraries);
        self.session.set_lens_profiles(libraries.lenses.clone());
        self.keyword_sets.set_store(libraries.keyword_sets.clone());
    }
    /// The selected photo, or else the first one shown in the current
    /// folder or filter (which then becomes selected), as Lightroom does
    /// when switching to Develop.
    pub(in crate::app) fn selected_or_first(&mut self) -> Option<PhotoId> {
        if self.selection.active.is_none() {
            self.select(self.visible.first().map(|i| self.session.photos()[*i].id));
        }
        self.selection.active
    }
    /// Previews photos without an edit with `defaults` from now on; the ones
    /// shown with the previous defaults are made again.
    pub(in crate::app) fn set_defaults(&mut self, defaults: PreviewDefaults) {
        self.set_defaults_preserving(defaults, None);
    }

    /// A manual import may leave the current Develop recipe unchanged. Its
    /// existing render remains valid; other unedited photos still invalidate.
    pub(in crate::app) fn set_defaults_preserving(
        &mut self,
        defaults: PreviewDefaults,
        current: Option<PhotoId>,
    ) {
        self.defaults = defaults;
        self.screen.clear();
        self.cache.defaults_changed();
        // Develop's renders of photos without an edit: back to the embedded
        // preview until Develop shows one with the new defaults.
        let unedited: Vec<PhotoId> = self
            .cache
            .edited_ids()
            .filter(|id| Some(*id) != current && edit_source(self.session.catalog(), *id).is_none())
            .collect();
        for id in unedited {
            self.cache.forget(id);
        }
    }
    /// Whether the photo's thumbnail already shows its edit (crop included).
    pub(in crate::app) fn has_edited_thumbnail(&self, id: PhotoId) -> bool {
        self.cache.has_edited(id)
    }
    /// The Library's cached preview for a photo, if one is loaded.
    pub(in crate::app) fn thumbnail(&self, id: PhotoId) -> Option<&egui::TextureHandle> {
        self.texture(self.photo(id)?)
    }
    pub(in crate::app) fn preview_progress_active(&self) -> bool {
        self.cache.progress_active()
    }
    pub(in crate::app) fn preview_progress(&self, ui: &mut egui::Ui) {
        self.cache.show_progress(ui);
    }
}
impl Library {
    /// Saves a Copy Name or metadata field still being typed, e.g. when the
    /// Library panel goes away before the field loses focus. On failure it
    /// stays pending, to be saved again or discarded.
    pub(in crate::app) fn commit_drafts(&mut self) -> Result<()> {
        if self.copy_names.commit(&mut self.session)? {
            self.filter();
        }
        self.commit_fields()
    }
    /// Types `text` into Add Keywords for the photos shown, as the panel
    /// would, without pressing Enter.
    #[cfg(test)]
    pub(in crate::app) fn type_keyword_for_tests(&mut self, text: &str) {
        self.sync_fields();
        self.fields.keyword_entry = text.into();
    }
    /// What Add Keywords holds once the panel has read the photos shown.
    #[cfg(test)]
    pub(in crate::app) fn keyword_entry_for_tests(&mut self) -> String {
        self.sync_fields();
        self.fields.keyword_entry.clone()
    }
    #[cfg(test)]
    pub(in crate::app) fn set_copy_name_draft(&mut self, id: PhotoId, name: &str) {
        self.copy_names.draft = Some((id, name.into()));
    }
    /// Whether a Copy Name or metadata field holds typing not saved yet.
    pub(in crate::app) fn has_drafts(&self) -> bool {
        self.copy_names.is_unsaved(self.session.photos()) || self.has_unsaved_fields()
    }
    /// Drops a Copy Name or metadata field that could not be saved, e.g.
    /// closing without saving.
    pub(in crate::app) fn discard_drafts(&mut self) {
        self.copy_names.clear();
        self.fields.clear();
    }
}
/// Why Develop cannot open a photo: it edits RAW, JPEG, PNG, TIFF, WebP, AVIF,
/// HEIF and JPEG XL files that are online and whose pixels it can read.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) enum Refusal {
    /// Offline, without an editable preview.
    Offline,
    /// Offline, with an editable preview that can't stand in for it.
    Copy(Unusable),
    /// A JPEG, PNG, TIFF, WebP, AVIF, HEIF or JPEG XL of a kind Develop can't edit: its `format` (e.g. "JPG")
    /// and the `reason` (e.g. "CMYK").
    Unsupported { format: String, reason: String },
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
            Self::Offline | Self::Copy(_) => "Offline".into(),
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
            Self::Copy(Unusable::OutOfDate) => {
                "The photo is offline, and its editable preview is out of date. Build it again while the original is online, or right-click the photo and choose Find on Disk… to pick its file."
                    .into()
            }
            Self::Copy(Unusable::NotAbsent) => {
                "The photo's file is there but can't be read, so its editable preview can't stand in for it. Check that its drive or folder can be read."
                    .into()
            }
            Self::Copy(Unusable::Unreadable(why)) => format!(
                "The photo is offline, and its editable preview can't be read from the catalog: {why}"
            ),
            Self::Unsupported { reason, .. } => format!("RAWmakase can't edit {reason} images"),
        }
    }
}
/// The file a render of a photo in Develop was decoded from, as read before it
/// was decoded: the photo's original, or its editable preview's file.
#[derive(Clone, Copy, Debug)]
pub(in crate::app) enum RenderedFrom<'a> {
    Original(&'a crate::storage::Identity),
    Copy(&'a std::path::Path, &'a crate::storage::Identity),
}
/// Why an offline photo's editable preview can't stand in for its original.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) enum Unusable {
    /// The catalog marks it out of date (the original came back as another
    /// file), or its file is missing: it is built again while the original is
    /// online.
    OutOfDate,
    /// Something is at the original's path that can't be read (a folder
    /// without permission, a share that stopped answering): a copy stands in
    /// only for a file that is certainly not there.
    NotAbsent,
    /// The catalog's record of it can't be read.
    Unreadable(String),
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
/// What the Library says after the catalog it opened was upgraded.
fn upgrade_note(upgraded: &crate::catalog::Upgraded) -> String {
    let mut note = format!(
        "This catalog was upgraded for this release. A copy of it as it was is at {}; \
         older releases of RAWmakase open that copy, not the catalog.",
        upgraded.backup.display()
    );
    if let Some(earlier) = upgraded.earlier_backups_note() {
        note.push(' ');
        note.push_str(&earlier);
    }
    if upgraded.unreadable > 0 {
        note.push_str(&format!(
            " {} stored edits could not be read and were left as they were.",
            upgraded.unreadable
        ));
    }
    if let Some(error) = &upgraded.vacuum_error {
        note.push_str(&format!(
            " The disk space it no longer needs could not be given back ({error})."
        ));
    }
    note
}
/// The edit a photo's previews are rendered with: its RAWmakase recipe, or
/// else its Lightroom settings.
fn edit_source(catalog: &Catalog, id: PhotoId) -> Option<previews::EditSource> {
    previews::EditSource::of(catalog.preview_edit(id).ok()?)
}

mod availability;
mod capture;
/// Lightroom-style grid cells: the label tints the cell, while selection uses
/// a lighter surround instead of the app's blue button fill.
mod cell;
mod checksums;
mod collection_edit;
mod collections;
mod compare;
mod copy_name;
mod descriptive;
pub(in crate::app) mod editable_previews;
mod facets;
pub(crate) mod filmstrip;
mod filter;
mod filter_bar;
mod folder_selection;
mod grid;
mod import_stacks;
#[cfg(test)]
mod import_stacks_tests;
mod info;
mod keyword_entry;
mod keyword_list;
mod keyword_sets;
#[cfg(test)]
mod keyword_tests;
mod kinetic;
mod layout;
mod loupe;
pub(in crate::app) mod map;
mod metadata;
mod metadata_bar;
mod metadata_fields;
pub(in crate::app) mod outbox;
mod photo_info;
mod previews;
mod quick;
mod rows;
mod screen;
mod selection;
mod sidebar;
pub(in crate::app) mod smart;
pub(in crate::app) mod smart_editor;
#[cfg(test)]
mod smart_tests;
pub(crate) use sidebar::take_folder_filter_escape;
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
mod monitor_tests;
#[cfg(test)]
mod no_preview_tests;
#[cfg(test)]
mod tests;

impl Library {}

/// Asks `ctx` for a repaint, for a background read with something to show.
fn wake(ctx: &egui::Context) -> crate::catalog_session::Wake {
    let ctx = ctx.clone();
    std::sync::Arc::new(move || ctx.request_repaint())
}

#[cfg(test)]
mod editor_tests;

pub(in crate::app) mod batch;
#[cfg(test)]
mod collections_tests;
pub(in crate::app) mod photo_metadata;
pub(in crate::app) mod preview_build;
#[cfg(test)]
pub(in crate::app) mod stacks_tests;
pub(in crate::app) mod sync;
