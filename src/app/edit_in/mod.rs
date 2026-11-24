//! Edit In: a photo opened in a program the user chose in Preferences >
//! External Editing, as Lightroom's Edit In does for a raw photo.
//!
//! The photos are chosen as Export chooses them. A JPEG, PNG or TIFF (any
//! photo that isn't a RAW) is asked about first, in Lightroom's words
//! (`dialog`): Edit Original opens the file itself, Edit a Copy copies it as
//! it is (`<name>-Edit.<ext>`), and Edit a Copy with RAWmakase Adjustments
//! renders it as a RAW is. A RAW is always rendered: each is rendered with its
//! edit through the app's export queue, as a 16-bit TIFF in the color space
//! of Preferences, beside its original: `<name>-Edit.tif`, or `-Edit-2` and so
//! on when that is taken. The last export's settings are left as they were.
//! Once a TIFF is written it is added to the catalog as a source's report is
//! (`app::extensions::sources`): the catalog's folder-change activity is
//! taken, retried each frame while other work holds it, and a worker of its
//! own imports the file on a connection of its own. Back on the interface the
//! Library reads the catalog again, the TIFF is stacked on top of its
//! original (one command, which Undo takes back) and the program is started
//! with it. Without a catalog, or with another one open by then, the TIFF is
//! opened in the program and not added.
//!
//! The files sent out (originals included) are followed while the app runs,
//! their catalog is open and their photo is still that file (`watch`): the
//! system's events for their folders, or a slow look of its own, announce a
//! change, which wakes the window; with nothing changing nothing runs. A
//! file with a change announced is looked at, on a one-shot worker, until it
//! has stayed as it is for [`SETTLE`], missing for a while included (a save
//! by deleting and writing again). One saved has its
//! edit in the catalog bound to the new version (so the edit is not
//! protected, nor a save refused, as for a photo replaced by another), its
//! previews made again (those shown staying until the new ones come: another
//! program's file can be slow to decode) and it is opened again when it is
//! the photo shown.
use crate::app::{Editor, task, worker::Event};
use crate::catalog::{CatalogLocation, PhotoId};
use crate::color::OutputSpace;
use crate::export_settings::{Destination, ExportSettings, Format, Include, Naming};
use crate::storage::Identity;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The text Edit In's files are named with, after the original's name.
const SUFFIX: &str = "Edit";

/// Preferences > External Editing, the TIFFs on their way to an editor and
/// the files sent out.
pub(in crate::app) struct EditIn {
    /// The programs offered, in the user's order; the first is Cmd+E's.
    editors: Vec<PathBuf>,
    /// Their names, as the menus show them.
    names: Arc<[String]>,
    /// The color space the TIFFs are rendered in.
    pub(in crate::app) space: OutputSpace,
    /// TIFFs written, waiting for the catalog, oldest first.
    waiting: VecDeque<Rendered>,
    /// The TIFF its import worker is adding, by that import's generation.
    adding: Option<(u64, Rendered)>,
    generation: u64,
    /// The files handed to a program, as they were then.
    sent: Vec<Sent>,
    /// When the files with a change announced are next looked at.
    due: Option<std::time::Instant>,
    /// A look at them is under way.
    looking: bool,
    /// What tells of their changes, and the files it follows now.
    watch: watch::Watch,
    watched: Vec<(PathBuf, crate::storage::Stamp)>,
    /// The question for photos the editor can open as they are.
    pub(in crate::app) asking: Option<Asking>,
    /// Stands in for starting a program, for a test: what was opened in what.
    #[cfg(test)]
    pub(in crate::app) launched: Option<Vec<(PathBuf, PathBuf)>>,
}

impl Default for EditIn {
    fn default() -> Self {
        Self::new(Vec::new(), None)
    }
}

/// What Edit In edits of a photo the editor can open as it is (a JPEG, PNG
/// or TIFF), as Lightroom asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum How {
    /// The file itself: nothing is copied or added.
    Original,
    /// A copy of the file as it is, added and stacked on top of it.
    Copy,
    /// A 16-bit TIFF with the edit, as for a RAW.
    Adjusted,
}

/// Edit In waiting for the answer to how to edit `plain`, the photos of
/// `ids` the editor can open as they are; the others are RAWs, always
/// rendered.
pub(in crate::app) struct Asking {
    /// The catalog the photos are of: their ids name other photos in another.
    pub(in crate::app) catalog: CatalogLocation,
    pub(in crate::app) ids: Vec<PhotoId>,
    pub(in crate::app) plain: Vec<(PhotoId, PathBuf)>,
    pub(in crate::app) editor: PathBuf,
    pub(in crate::app) how: How,
    /// Some of `plain` have an edit in RAWmakase, which the file doesn't show.
    pub(in crate::app) adjusted: bool,
}

/// What an Edit In batch goes on to once its files are written.
pub(in crate::app) struct Sending {
    /// The program the files open in.
    pub(in crate::app) editor: PathBuf,
    /// The catalog the photos are in, if any.
    pub(in crate::app) catalog: Option<CatalogLocation>,
    /// Each photo of the batch, in its order: the original its file is
    /// stacked with, and that original's file then.
    pub(in crate::app) originals: Vec<Option<(PhotoId, PathBuf)>>,
}

/// A file on its way to the catalog and its program: a TIFF written, or for
/// Edit a Copy the photo's file, copied by the import worker.
struct Rendered {
    /// The TIFF written, or the file to copy.
    path: PathBuf,
    /// `path` is the photo's file, copied as it is beside it.
    copy: bool,
    editor: PathBuf,
    catalog: Option<CatalogLocation>,
    /// The photo it is stacked with, and that photo's file when it was chosen:
    /// a photo removed meanwhile can give its id to another.
    original: Option<(PhotoId, PathBuf)>,
}

/// A file handed to a program, as it was then or as last taken up.
#[derive(Clone, Debug)]
pub(in crate::app) struct Sent {
    pub(in crate::app) path: PathBuf,
    pub(in crate::app) identity: Identity,
    /// The catalog it was sent from, and its photo there: a save is followed
    /// only while that catalog is open and the photo is still this file.
    pub(in crate::app) catalog: Option<CatalogLocation>,
    pub(in crate::app) photo: Option<PhotoId>,
    /// A change not taken up yet: the file's size and modification time, and
    /// when they were first seen so.
    pub(in crate::app) seen: Option<(crate::storage::Stamp, std::time::Instant)>,
    /// A change was announced (an event, or the slow look) and is looked at
    /// until it is taken up, or the file is as it was.
    pub(in crate::app) pending: bool,
    /// Since when it has been missing while a change was looked at: an editor
    /// saving by deleting and writing again leaves a moment without it.
    missing: Option<std::time::Instant>,
    /// How many times in a row taking up its change failed for a reason that
    /// may clear; said once.
    retries: u32,
}

/// How long a file may be missing while its change is looked at before the
/// looks stop until the next change is announced; it is still followed.
const MISSING: std::time::Duration = std::time::Duration::from_secs(30);
/// How long after a failure that may clear (the catalog busy, the photo open
/// not reopened) the change is tried again, and how many times in a row
/// before it waits for the next save.
const RETRY: std::time::Duration = std::time::Duration::from_secs(1);
const RETRIES: u32 = 30;

/// Why a change could not be taken up.
enum NotFollowed {
    /// It may clear: tried again after [`RETRY`].
    Later(String),
    /// It can't: the file is no longer followed.
    Never(String),
}
/// How long a changed file must stay as it is before it is read, so one the
/// editor is still writing is not.
pub(in crate::app) const SETTLE: std::time::Duration = std::time::Duration::from_millis(300);

/// What the import worker sends back: the file added (for a copy, where it
/// was published) and whether the catalog added it (false when it had it
/// already), or why not.
pub(crate) struct Added {
    generation: u64,
    result: Result<(PathBuf, bool), String>,
}

/// What a look at a file sent out found.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) enum Look {
    /// It can't be read: moved or deleted.
    Gone,
    /// As it was taken up last.
    Same,
    /// Changed, with this size and modification time, and not settled yet.
    Changing(crate::storage::Stamp),
    /// Changed and settled: the version it is now.
    Settled(Identity),
}

/// What the look worker sends back: each file looked at, what it found and
/// when the look started.
pub(crate) struct Looked {
    looks: Vec<(PathBuf, Look)>,
    at: std::time::Instant,
}

/// What the launching worker sends back for `file`.
pub(crate) struct Launched {
    file: PathBuf,
    program: String,
    result: Result<(), String>,
}

impl EditIn {
    /// The programs and the color space of `session`'s Preferences.
    pub(in crate::app) fn new(editors: Vec<PathBuf>, space: Option<&str>) -> Self {
        let mut edit_in = Self {
            editors: Vec::new(),
            names: Arc::from(Vec::new()),
            space: space
                .and_then(OutputSpace::from_name)
                .unwrap_or(OutputSpace::ProPhoto),
            waiting: VecDeque::new(),
            adding: None,
            generation: 0,
            sent: Vec::new(),
            due: None,
            looking: false,
            watch: Default::default(),
            watched: Vec::new(),
            asking: None,
            #[cfg(test)]
            launched: None,
        };
        edit_in.set_editors(editors);
        edit_in
    }
    pub(in crate::app) fn editors(&self) -> &[PathBuf] {
        &self.editors
    }
    /// The programs' names, in their order.
    pub(in crate::app) fn names(&self) -> Arc<[String]> {
        self.names.clone()
    }
    /// Offers `editors`, in this order.
    pub(in crate::app) fn set_editors(&mut self, editors: Vec<PathBuf>) {
        self.names = editors
            .iter()
            .map(|e| crate::platform::open_with::program_name(e))
            .collect();
        self.editors = editors;
    }
    /// Stops following the files sent out, at exit: the system watcher's
    /// thread ends as it is dropped, the slow look's within its next wait.
    pub(in crate::app) fn stop_watching(&mut self) {
        self.watch.stop();
        self.watched.clear();
    }
    /// The color space's name, as the session saves it.
    pub(in crate::app) fn space_name(&self) -> String {
        self.space.name().into()
    }
}

/// The export settings Edit In renders with: a 16-bit TIFF in `space`, at full
/// size, beside its original, named `<name>-Edit` and never over another file.
/// The edit is in its pixels, so it carries the photo's metadata but not its
/// settings, which would be applied again to the TIFF.
pub(in crate::app) fn settings(space: OutputSpace) -> ExportSettings {
    ExportSettings {
        destination: Destination::SameFolder,
        existing: crate::export_settings::Existing::Unique,
        naming: Some(Naming::FilenameText),
        rename: true,
        custom_text: SUFFIX.into(),
        format: Format::Tiff,
        color_space: space,
        resize: false,
        include: Include::Custom,
        develop: false,
        ..Default::default()
    }
}

/// Looks at the file at `path`, taken up last as `baseline`, whose change was
/// `seen` so far, at `now`: settled once it has kept one size and
/// modification time for [`SETTLE`], so a file still being written is not
/// read. Reads its first bytes only then.
pub(in crate::app) fn look(
    path: &Path,
    baseline: crate::storage::Stamp,
    seen: Option<(crate::storage::Stamp, std::time::Instant)>,
    now: std::time::Instant,
) -> Look {
    let Ok(stamp) = crate::storage::Stamp::read(path) else {
        return Look::Gone;
    };
    if stamp == baseline {
        return Look::Same;
    }
    match seen {
        Some((seen, since)) if seen == stamp && now.duration_since(since) >= SETTLE => {
            match Identity::read(path) {
                Ok(identity) if identity.stamp() == stamp => Look::Settled(identity),
                Ok(identity) => Look::Changing(identity.stamp()),
                Err(_) => Look::Gone,
            }
        }
        _ => Look::Changing(stamp),
    }
}

impl Editor {
    /// Edit In with the `which`th program for photos `ids`, as Export takes
    /// them. With no such program, Preferences opens on External Editing.
    pub(in crate::app) fn edit_in(&mut self, ids: Vec<PhotoId>, which: usize) {
        let Some(editor) = self.edit_in.editors.get(which).cloned() else {
            self.open_preferences(crate::app::preferences::Tab::ExternalEditing);
            return;
        };
        if !editor.exists() {
            self.messages.warn(format!(
                "{} is no longer at {}: add it again in Preferences > External Editing",
                crate::platform::open_with::program_name(&editor),
                editor.display()
            ));
            return;
        }
        let Some(library) = &self.library else {
            self.render_for_editing(ids, editor);
            return;
        };
        let plain: Vec<(PhotoId, PathBuf)> = ids
            .iter()
            .filter_map(|id| library.photo(*id))
            .filter(|p| !crate::storage::is_raw(&p.path))
            .map(|p| (p.id, p.path.clone()))
            .collect();
        if plain.is_empty() {
            self.render_for_editing(ids, editor);
            return;
        }
        let catalog = library.session.catalog();
        let adjusted = plain
            .iter()
            .any(|(id, _)| catalog.has_edit(*id).unwrap_or(false));
        self.edit_in.asking = Some(Asking {
            catalog: catalog.location().clone(),
            ids,
            plain,
            editor,
            how: How::Original,
            adjusted,
        });
    }
    /// Whether `asking` is still about the catalog open.
    pub(in crate::app) fn asking_here(&self, asking: &Asking) -> bool {
        self.library
            .as_ref()
            .is_some_and(|l| *l.session.catalog().location() == asking.catalog)
    }
    /// Edit In as `asking` was answered: RAWs (and the rest, with How::Adjusted)
    /// rendered, the others opened or copied as they are. Photos no longer in
    /// the catalog, or at another file now, are left out.
    pub(in crate::app) fn edit_in_as(&mut self, asking: Asking) {
        if !self.asking_here(&asking) {
            self.messages
                .warn("Edit In cancelled: another catalog is open now");
            return;
        }
        let Asking {
            catalog,
            ids,
            plain,
            editor,
            how,
            ..
        } = asking;
        let library = self.library.as_ref().expect("checked above");
        let plain: Vec<(PhotoId, PathBuf)> = plain
            .into_iter()
            .filter(|(id, path)| library.photo(*id).is_some_and(|p| p.path == *path))
            .collect();
        let rendered: Vec<PhotoId> = ids
            .into_iter()
            .filter(|id| {
                library
                    .photo(*id)
                    .is_some_and(|p| how == How::Adjusted || crate::storage::is_raw(&p.path))
            })
            .collect();
        if !rendered.is_empty() {
            self.render_for_editing(rendered, editor.clone());
        }
        match how {
            How::Adjusted => {}
            How::Original => {
                // A photo and its virtual copies share one file, opened once.
                let mut opened: Vec<&PathBuf> = Vec::new();
                for (id, path) in &plain {
                    if !opened.contains(&path) {
                        opened.push(path);
                        self.launch_editor(&editor, path, Some(&catalog), Some(*id));
                    }
                }
            }
            How::Copy => {
                for (id, path) in plain {
                    self.edit_in.waiting.push_back(Rendered {
                        path: path.clone(),
                        copy: true,
                        editor: editor.clone(),
                        catalog: Some(catalog.clone()),
                        original: Some((id, path)),
                    });
                }
                self.add_edits();
            }
        }
    }
    /// Renders `ids` as TIFFs with their edits, for `editor`.
    fn render_for_editing(&mut self, ids: Vec<PhotoId>, editor: PathBuf) {
        let catalog = self
            .library
            .as_ref()
            .map(|l| l.session.catalog().location().clone());
        self.export_for_editing(
            ids,
            settings(self.edit_in.space),
            Sending {
                editor,
                catalog,
                originals: Vec::new(),
            },
        );
    }
    /// Edit In with the `which`th program for the photos Export would take
    /// now: the Library's selection, or in Develop the Filmstrip's when the
    /// open photo is part of it, else the open photo.
    pub(in crate::app) fn edit_in_chosen(&mut self, which: usize) {
        let ids = self.preview_command_scope();
        self.edit_in(ids, which);
    }

    /// An Edit In batch was written: its files wait for the catalog.
    pub(in crate::app) fn edit_in_rendered(
        &mut self,
        editing: Sending,
        outcomes: &[crate::export::batch::Outcome],
    ) {
        for (original, outcome) in editing.originals.iter().zip(outcomes) {
            if let crate::export::batch::Outcome::Exported { path, .. } = outcome {
                self.edit_in.waiting.push_back(Rendered {
                    path: path.clone(),
                    copy: false,
                    editor: editing.editor.clone(),
                    catalog: editing.catalog.clone(),
                    original: original.clone(),
                });
            }
        }
        self.add_edits();
    }

    /// Adds the next file waiting to the catalog once nothing else holds it;
    /// called each frame meanwhile. A TIFF with no catalog to go to opens in
    /// its program at once.
    pub(in crate::app) fn add_edits(&mut self) {
        while self.edit_in.adding.is_none() {
            let Some(next) = self.edit_in.waiting.front() else {
                return;
            };
            let open = self
                .library
                .as_ref()
                .map(|l| l.session.catalog().location().clone());
            let to = next.catalog.clone().filter(|_| next.original.is_some());
            let Some(location) = to else {
                let next = self.edit_in.waiting.pop_front().expect("checked above");
                self.launch_editor(&next.editor, &next.path, None, None);
                continue;
            };
            if open.as_ref() != Some(&location) {
                let next = self.edit_in.waiting.pop_front().expect("checked above");
                if next.copy {
                    self.messages.warn(format!(
                        "{} was not copied: another catalog is open now",
                        file_name(&next.path)
                    ));
                    continue;
                }
                self.messages.warn(format!(
                    "{} was not added to {}: another catalog is open now",
                    file_name(&next.path),
                    location.name()
                ));
                self.launch_editor(&next.editor, &next.path, None, None);
                continue;
            }
            // Excludes Add Folder, Synchronize, batch edits, file operations,
            // a merge and a catalog change until the worker reports back.
            if !self.activity.begin_folder_change() {
                return;
            }
            let next = self.edit_in.waiting.pop_front().expect("checked above");
            self.edit_in.generation += 1;
            let generation = self.edit_in.generation;
            let (path, copy) = (next.path.clone(), next.copy);
            self.edit_in.adding = Some((generation, next));
            task::spawn(
                self.tx.clone(),
                self.context.clone(),
                move |tx| {
                    let result = import(&location, &path, copy).map_err(|e| format!("{e:#}"));
                    let _ = tx.send(Event::EditIn(Box::new(Added { generation, result })));
                },
                move |tx, why| {
                    let _ = tx.send(Event::EditIn(Box::new(Added {
                        generation,
                        result: Err(why),
                    })));
                },
            );
            return;
        }
    }

    /// A file's import reported back: the catalog is free again, the Library
    /// shows the file stacked on its original, and its program starts with
    /// the file the job published, and no other.
    pub(in crate::app) fn edit_added(&mut self, added: Added) {
        let ours = self
            .edit_in
            .adding
            .as_ref()
            .is_some_and(|(generation, _)| *generation == added.generation);
        if !ours {
            return;
        }
        self.activity.finish_dialog();
        let (_, rendered) = self.edit_in.adding.take().expect("checked above");
        let (path, added) = match added.result {
            Ok(done) => done,
            Err(why) => {
                let name = file_name(&rendered.path);
                if rendered.copy {
                    self.messages
                        .warn(format!("Edit In: {name} was not copied: {why}"));
                } else {
                    self.messages.warn(format!(
                        "Edit In: {name} was not added to the catalog: {why}"
                    ));
                    // The TIFF is Edit In's own, written by its export.
                    self.launch_editor(&rendered.editor, &rendered.path, None, None);
                }
                self.add_edits();
                return;
            }
        };
        let name = file_name(&path);
        if added
            && self.commit_library_drafts()
            && let Some(library) = &mut self.library
        {
            match library.reread_after_added() {
                Ok(()) => library.start_background_reads(),
                Err(e) => self.messages.fail(format!(
                    "Edit In: {name} added, but the catalog could not be read again: {e:#}"
                )),
            }
        }
        let photo = self.catalog_photo_at(&path);
        // Stacked only with the photo chosen, still at the file it had.
        let original = rendered.original.filter(|(id, file)| {
            self.library
                .as_ref()
                .and_then(|l| l.photo(*id))
                .is_some_and(|p| p.path == *file)
        });
        if let (Some(photo), Some((original, _)), Some(library)) =
            (photo, original, &mut self.library)
        {
            if let Err(e) =
                library.stack_merge(photo, vec![original], "Stacked the edit with its original")
            {
                self.messages
                    .fail(format!("Edit In: {name} added, but not stacked: {e:#}"));
            }
            self.messages.absorb(&mut library.messages);
        }
        let catalog = rendered.catalog.clone();
        self.launch_editor(&rendered.editor, &path, catalog.as_ref(), photo);
        self.add_edits();
    }

    /// Opens `file` in `editor` on a worker of its own, and follows its saves
    /// ([`Self::watch_sent`]) from the version it is now, read before the
    /// editor can change it: photo `photo` of `catalog`, if it is one.
    fn launch_editor(
        &mut self,
        editor: &Path,
        file: &Path,
        catalog: Option<&CatalogLocation>,
        photo: Option<PhotoId>,
    ) {
        let program = crate::platform::open_with::program_name(editor);
        match Identity::read(file) {
            Ok(identity) => {
                self.edit_in.sent.retain(|s| s.path != file);
                self.edit_in.sent.push(Sent {
                    path: file.to_path_buf(),
                    identity,
                    catalog: catalog.cloned(),
                    photo,
                    seen: None,
                    pending: false,
                    missing: None,
                    retries: 0,
                });
            }
            Err(e) => {
                self.messages.warn(format!(
                    "{} was not opened in {program}: {e:#}",
                    file_name(file)
                ));
                return;
            }
        }
        #[cfg(test)]
        if let Some(launched) = &mut self.edit_in.launched {
            launched.push((editor.to_path_buf(), file.to_path_buf()));
            self.messages
                .done(format!("Opened {} in {program}", file_name(file)));
            return;
        }
        let (editor, file) = (editor.to_path_buf(), file.to_path_buf());
        let failed_file = file.clone();
        let failed_program = program.clone();
        task::spawn(
            self.tx.clone(),
            self.context.clone(),
            move |tx| {
                let result = crate::platform::open_with::open_with(&editor, &file)
                    .map_err(|e| format!("{e:#}"));
                let _ = tx.send(Event::EditInLaunched(Box::new(Launched {
                    file,
                    program,
                    result,
                })));
            },
            move |tx, why| {
                let _ = tx.send(Event::EditInLaunched(Box::new(Launched {
                    file: failed_file,
                    program: failed_program,
                    result: Err(why),
                })));
            },
        );
    }

    /// The program for a file was started, or could not be: one that wasn't
    /// is no longer followed.
    pub(in crate::app) fn editor_launched(&mut self, launched: Launched) {
        let name = file_name(&launched.file);
        match launched.result {
            Ok(()) => self
                .messages
                .done(format!("Opened {name} in {}", launched.program)),
            Err(why) => {
                self.edit_in.sent.retain(|s| s.path != launched.file);
                self.messages.warn(format!(
                    "{name} was saved, but not opened in {}: {why}",
                    launched.program
                ));
            }
        }
    }

    /// Keeps what tells of the files' changes in step with the files sent
    /// out, and looks at those with a change announced. Files sent from a
    /// catalog no longer open, or whose photo has left it or now is another
    /// file, and those whose folder can't be watched, stop being followed.
    /// With no change announced nothing is looked at and no pass is asked
    /// for: following costs nothing while the files rest. Called each pass,
    /// also while the window is behind another.
    pub(in crate::app) fn watch_sent(&mut self, ctx: &eframe::egui::Context) {
        let library = self.library.as_deref();
        let open = library.map(|l| l.session.catalog().location());
        self.edit_in.sent.retain(|s| match (&s.catalog, s.photo) {
            (None, _) => true,
            (Some(catalog), photo) => {
                open == Some(catalog)
                    && photo.is_none_or(|id| {
                        library
                            .and_then(|l| l.photo(id))
                            .is_some_and(|p| p.path == s.path)
                    })
            }
        });
        let followed: Vec<(PathBuf, crate::storage::Stamp)> = self
            .edit_in
            .sent
            .iter()
            .map(|s| (s.path.clone(), s.identity.stamp()))
            .collect();
        if followed != self.edit_in.watched {
            let unwatched = self.edit_in.watch.follow(&followed, &self.tx, ctx);
            for path in &unwatched {
                self.messages.warn(format!(
                    "{}: its folder can't be watched, so saves to it are not followed",
                    file_name(path)
                ));
            }
            self.edit_in.sent.retain(|s| !unwatched.contains(&s.path));
            self.edit_in.watched = followed;
            self.edit_in.watched.retain(|(p, _)| !unwatched.contains(p));
        }
        if !self.edit_in.sent.iter().any(|s| s.pending) {
            self.edit_in.due = None;
            return;
        }
        let now = std::time::Instant::now();
        let due = *self.edit_in.due.get_or_insert(now);
        if due > now {
            ctx.request_repaint_after(due - now);
            return;
        }
        if self.edit_in.looking {
            return;
        }
        self.edit_in.due = None;
        self.edit_in.looking = true;
        let files: Vec<_> = self
            .edit_in
            .sent
            .iter()
            .filter(|s| s.pending)
            .map(|s| (s.path.clone(), s.identity.stamp(), s.seen))
            .collect();
        task::spawn(
            self.tx.clone(),
            self.context.clone(),
            move |tx| {
                let looks = files
                    .into_iter()
                    .map(|(path, baseline, seen)| {
                        let found = look(&path, baseline, seen, now);
                        (path, found)
                    })
                    .collect();
                let _ = tx.send(Event::EditInLooked(Box::new(Looked { looks, at: now })));
            },
            move |tx, _| {
                let _ = tx.send(Event::EditInLooked(Box::new(Looked {
                    looks: Vec::new(),
                    at: now,
                })));
            },
        );
    }

    /// The system, or the slow look, says these files may have changed: they
    /// are looked at at once.
    pub(in crate::app) fn sent_changed(&mut self, paths: Vec<PathBuf>) {
        for sent in &mut self.edit_in.sent {
            if paths.contains(&sent.path) {
                sent.pending = true;
            }
        }
        self.edit_in.due = Some(std::time::Instant::now());
    }

    /// Looks again at `at`, or sooner when already due.
    fn look_again(&mut self, at: std::time::Instant) {
        self.edit_in.due = Some(self.edit_in.due.map_or(at, |due| due.min(at)));
    }

    /// A look at the files sent out came back: a change seen is looked at
    /// again until it settles; a settled one is taken up
    /// ([`Self::follow_save`]) and becomes the file's version only once that
    /// worked. One that can't be taken up yet is tried again a while later,
    /// one that never can ends the following. A file missing is a save under
    /// way, looked at again until [`MISSING`] has passed.
    pub(in crate::app) fn sent_looked(&mut self, looked: Looked) {
        self.edit_in.looking = false;
        for (path, found) in looked.looks {
            let Some(index) = self.edit_in.sent.iter().position(|s| s.path == path) else {
                continue;
            };
            match found {
                Look::Same => {
                    let sent = &mut self.edit_in.sent[index];
                    (sent.seen, sent.missing, sent.pending) = (None, None, false);
                }
                Look::Gone => {
                    let sent = &mut self.edit_in.sent[index];
                    let since = *sent.missing.get_or_insert(looked.at);
                    if looked.at.duration_since(since) < MISSING {
                        self.look_again(looked.at + SETTLE);
                    } else {
                        sent.pending = false;
                    }
                }
                Look::Changing(stamp) => {
                    let sent = &mut self.edit_in.sent[index];
                    sent.missing = None;
                    if sent.seen.is_none_or(|(seen, _)| seen != stamp) {
                        sent.seen = Some((stamp, looked.at));
                    }
                    self.look_again(looked.at + SETTLE);
                }
                Look::Settled(now) => {
                    let sent = self.edit_in.sent[index].clone();
                    match self.follow_save(&sent, &now) {
                        Ok(()) => {
                            let sent = &mut self.edit_in.sent[index];
                            sent.identity = now;
                            (sent.seen, sent.missing, sent.pending) = (None, None, false);
                            sent.retries = 0;
                        }
                        Err(NotFollowed::Never(why)) => {
                            self.edit_in.sent.remove(index);
                            self.messages.warn(format!(
                                "{} was saved again, and is no longer followed: {why}",
                                file_name(&path)
                            ));
                        }
                        Err(NotFollowed::Later(why)) => {
                            let sent = &mut self.edit_in.sent[index];
                            sent.retries += 1;
                            sent.seen = Some((now.stamp(), looked.at));
                            if sent.retries == 1 {
                                self.messages.warn(format!(
                                    "{} was saved again, but is not updated yet: {why}",
                                    file_name(&path)
                                ));
                            }
                            if sent.retries < RETRIES {
                                self.look_again(std::time::Instant::now() + RETRY);
                            } else {
                                // Waits for the next save to try again.
                                sent.pending = false;
                                sent.retries = 0;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Takes up `now`, a version of `sent`'s file another program saved:
    /// while its catalog is open, nothing else holds the catalog and its
    /// photo is still this file, the photo's edit (made for the version
    /// before) follows it, its previews are made again and, shown in Develop
    /// or the Loupe, it opens again. Why not, otherwise.
    fn follow_save(&mut self, sent: &Sent, now: &Identity) -> Result<(), NotFollowed> {
        if self.activity.is_busy() {
            return Err(NotFollowed::Later("the catalog is busy".into()));
        }
        let was = &sent.identity;
        let path = &sent.path;
        let mut ids = Vec::new();
        if let (Some(catalog), Some(photo)) = (&sent.catalog, sent.photo) {
            let Some(library) = &mut self.library else {
                return Err(NotFollowed::Never("its catalog is closed".into()));
            };
            if library.session.catalog().location() != catalog {
                return Err(NotFollowed::Never("its catalog is closed".into()));
            }
            if library.photo(photo).is_none_or(|p| p.path != *path) {
                return Err(NotFollowed::Never(
                    "its photo is no longer in the catalog".into(),
                ));
            }
            ids = library
                .session
                .photos()
                .iter()
                .filter(|p| p.path == *path)
                .map(|p| p.id)
                .collect();
            for id in &ids {
                library
                    .session
                    .follow_rewritten_file(*id, was, now)
                    .map_err(|e| {
                        NotFollowed::Later(format!("its edit could not follow it: {e:#}"))
                    })?;
            }
            library.file_rewritten(path);
            self.refresh_previews(&ids);
        }
        let shown = self.document.path.as_deref() == Some(path.as_path())
            && match self.document.catalog_photo {
                Some(id) => ids.contains(&id),
                None => sent.photo.is_none(),
            };
        if shown {
            // The edit open was made for the version before; it is now this
            // one's, as the catalog's is.
            if self
                .document
                .file
                .as_ref()
                .is_some_and(|f| f.same_photo(was))
            {
                self.document.file = Some(now.clone());
            }
            if !self.load_raw(path.clone(), self.document.catalog_photo) {
                return Err(NotFollowed::Later(
                    "the photo open could not be opened again".into(),
                ));
            }
        }
        self.messages.note(format!(
            "{} was saved again, and is updated",
            file_name(path)
        ));
        Ok(())
    }
}

/// Adds the file at `path` to the catalog at `location`, with what new photos
/// start with; for a `copy`, `path` is the photo's file, copied first beside
/// it as it is under the next free `-Edit` name, staged and published without
/// ever replacing a file, the name taken by a file that appears meanwhile
/// moved on. The file added, and false when the catalog had it already.
fn import(location: &CatalogLocation, path: &Path, copy: bool) -> anyhow::Result<(PathBuf, bool)> {
    use anyhow::Context;
    let path = if copy {
        let folder = crate::storage::parent_dir(path);
        let mut staged = crate::storage::staging_file(folder)
            .with_context(|| format!("a copy could not be made in {}", folder.display()))?;
        std::io::copy(&mut std::fs::File::open(path)?, staged.as_file_mut())
            .with_context(|| format!("{} could not be copied", path.display()))?;
        staged.as_file().sync_all()?;
        loop {
            let target = copy_name(path);
            match crate::storage::persist_noclobber(staged, &target) {
                Ok(_) => break target,
                Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => staged = e.file,
                Err(e) => return Err(e.error.into()),
            }
        }
    } else {
        path.to_path_buf()
    };
    let mut catalog = crate::catalog::Catalog::open(location)?;
    let added = catalog
        .import_files(
            crate::storage::parent_dir(&path),
            std::slice::from_ref(&path),
            &crate::catalog_session::new_photo_defaults(),
            || true,
        )?
        .context("the import was withdrawn")?;
    if let Some((_, why)) = added.refused.first() {
        use crate::catalog::FileRefusal;
        anyhow::bail!(match why {
            FileRefusal::NotFound => "it is no longer there",
            FileRefusal::NotAPhoto => "the catalog doesn't read it as a photo",
            FileRefusal::OutsideFolder => "it isn't in its folder",
            FileRefusal::LinkedElsewhere =>
                "its folder in the catalog is linked to another place on this computer",
            FileRefusal::Ambiguous => "more than one folder of the catalog is where it is",
        });
    }
    let added = !added.added.is_empty();
    Ok((path, added))
}

/// Where Edit a Copy copies `source`: beside it, `<name>-Edit` with its own
/// extension, or `-Edit-2` and so on when that is taken.
pub(in crate::app) fn copy_name(source: &Path) -> PathBuf {
    let stem = source.file_stem().unwrap_or_default().to_string_lossy();
    let extension = source
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (1..)
        .map(|n| {
            let number = if n == 1 {
                String::new()
            } else {
                format!("-{n}")
            };
            source.with_file_name(format!("{stem}-{SUFFIX}{number}{extension}"))
        })
        .find(|path| !path.exists())
        .expect("an unused name")
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

mod dialog;
#[cfg(test)]
mod tests;
mod watch;
