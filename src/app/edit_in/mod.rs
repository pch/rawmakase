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
//! The files sent out (originals included) are remembered while the app
//! runs, and looked at about once a second, from the pass that runs even
//! while the window is behind another (`Editor::watch_sent`, which keeps
//! asking for the next pass while there are any): a stat each, and a change
//! is taken up once it has settled over two looks. One saved has its
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
    /// When the files sent out were last looked at.
    checked: Option<std::time::Instant>,
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
    /// Each photo of the batch, in its order: the original its file is stacked with.
    pub(in crate::app) originals: Vec<Option<PhotoId>>,
}

/// A TIFF written, on its way to the catalog and its program.
struct Rendered {
    path: PathBuf,
    /// For Edit a Copy, the file copied to `path` as it is, by the import
    /// worker.
    copy_of: Option<PathBuf>,
    editor: PathBuf,
    catalog: Option<CatalogLocation>,
    original: Option<PhotoId>,
}

/// A file handed to a program, as it was then or last checked.
#[derive(Clone, Debug)]
pub(in crate::app) struct Sent {
    pub(in crate::app) path: PathBuf,
    pub(in crate::app) identity: Identity,
    /// A change seen at the last look and not yet settled: the file's size
    /// and modification time then.
    pub(in crate::app) seen: Option<crate::storage::Stamp>,
}

/// How often the files sent out are looked at, while there are any.
const WATCH: std::time::Duration = std::time::Duration::from_secs(1);

/// What the import worker sends back: whether the catalog added the file
/// (false when it had it already), or why not.
pub(crate) struct Added {
    generation: u64,
    result: Result<bool, String>,
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
            checked: None,
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

/// The files of `sent` saved again since they were sent or last taken up,
/// each with the version it was and the one it is now; `sent` keeps the new
/// ones. A change counts once it has settled: the same size and
/// modification time at two looks in a row, so a file still being written
/// is not read. A file that can't be read (moved, deleted) is forgotten.
pub(in crate::app) fn rewritten(sent: &mut Vec<Sent>) -> Vec<(PathBuf, Identity, Identity)> {
    let mut changed = Vec::new();
    sent.retain_mut(|file| {
        let Ok(stamp) = crate::storage::Stamp::read(&file.path) else {
            return false;
        };
        if stamp == file.identity.stamp() {
            file.seen = None;
            return true;
        }
        if file.seen.replace(stamp) != Some(stamp) {
            return true;
        }
        let Ok(now) = Identity::read(&file.path) else {
            return false;
        };
        file.seen = None;
        let was = std::mem::replace(&mut file.identity, now.clone());
        changed.push((file.path.clone(), was, now));
        true
    });
    changed
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
            ids,
            plain,
            editor,
            how: How::Original,
            adjusted,
        });
    }
    /// Edit In as `asking` was answered: RAWs (and the rest, with How::Adjusted)
    /// rendered, the others opened or copied as they are.
    pub(in crate::app) fn edit_in_as(&mut self, asking: Asking) {
        let Asking {
            ids,
            plain,
            editor,
            how,
            ..
        } = asking;
        let rendered: Vec<PhotoId> = ids
            .into_iter()
            .filter(|id| how == How::Adjusted || !plain.iter().any(|(p, _)| p == id))
            .collect();
        if !rendered.is_empty() {
            self.render_for_editing(rendered, editor.clone());
        }
        let catalog = self
            .library
            .as_ref()
            .map(|l| l.session.catalog().location().clone());
        match how {
            How::Adjusted => {}
            How::Original => {
                for (_, path) in &plain {
                    self.launch_editor(&editor, path);
                }
            }
            How::Copy => {
                for (id, path) in plain {
                    self.edit_in.waiting.push_back(Rendered {
                        path: copy_name(&path),
                        copy_of: Some(path),
                        editor: editor.clone(),
                        catalog: catalog.clone(),
                        original: Some(id),
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
                    copy_of: None,
                    editor: editing.editor.clone(),
                    catalog: editing.catalog.clone(),
                    original: *original,
                });
            }
        }
        self.add_edits();
    }

    /// Adds the next TIFF waiting to the catalog once nothing else holds it;
    /// called each frame meanwhile. One with no catalog to go to opens in its
    /// program at once.
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
                self.launch_editor(&next.editor, &next.path);
                continue;
            };
            if open.as_ref() != Some(&location) {
                let next = self.edit_in.waiting.pop_front().expect("checked above");
                if next.copy_of.is_some() {
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
                self.launch_editor(&next.editor, &next.path);
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
            let path = next.path.clone();
            let copy_of = next.copy_of.clone();
            self.edit_in.adding = Some((generation, next));
            task::spawn(
                self.tx.clone(),
                self.context.clone(),
                move |tx| {
                    let result =
                        import(&location, &path, copy_of.as_deref()).map_err(|e| format!("{e:#}"));
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

    /// A TIFF's import reported back: the catalog is free again, the Library
    /// shows the TIFF stacked on its original, and its program starts.
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
        let name = file_name(&rendered.path);
        match added.result {
            Err(why) => self.messages.warn(format!(
                "Edit In: {name} was not added to the catalog: {why}"
            )),
            Ok(true) => {
                if self.commit_library_drafts()
                    && let Some(library) = &mut self.library
                {
                    match library.reread_after_added() {
                        Ok(()) => library.start_background_reads(),
                        Err(e) => self.messages.fail(format!(
                            "Edit In: {name} added, but the catalog could not be read again: {e:#}"
                        )),
                    }
                }
            }
            Ok(false) => {}
        }
        let photo = self.catalog_photo_at(&rendered.path);
        if let (Some(photo), Some(original), Some(library)) =
            (photo, rendered.original, &mut self.library)
        {
            if let Err(e) =
                library.stack_merge(photo, vec![original], "Stacked the edit with its original")
            {
                self.messages
                    .fail(format!("Edit In: {name} added, but not stacked: {e:#}"));
            }
            self.messages.absorb(&mut library.messages);
        }
        // A copy that could not be made has nothing to open.
        if rendered.path.exists() {
            self.launch_editor(&rendered.editor, &rendered.path);
        }
        self.add_edits();
    }

    /// Opens `file` in `editor`, and remembers it to follow its saves
    /// ([`Self::watch_sent`]).
    fn launch_editor(&mut self, editor: &Path, file: &Path) {
        #[cfg(test)]
        let launched = match &mut self.edit_in.launched {
            Some(launched) => {
                launched.push((editor.to_path_buf(), file.to_path_buf()));
                Ok(())
            }
            None => crate::platform::open_with::open_with(editor, file),
        };
        #[cfg(not(test))]
        let launched = crate::platform::open_with::open_with(editor, file);
        let program = crate::platform::open_with::program_name(editor);
        match launched {
            Ok(()) => {
                if let Ok(identity) = Identity::read(file) {
                    self.edit_in.sent.retain(|s| s.path != file);
                    self.edit_in.sent.push(Sent {
                        path: file.to_path_buf(),
                        identity,
                        seen: None,
                    });
                }
                self.messages
                    .done(format!("Opened {} in {program}", file_name(file)));
            }
            Err(e) => self.messages.warn(format!(
                "{} was saved, but not opened in {program}: {e:#}",
                file_name(file)
            )),
        }
    }

    /// Looks at the files sent out about once a second while there are any,
    /// whether the window is in front or not: a save the editor made is
    /// taken up once it has settled. Called each pass, also while the window
    /// is behind another, and keeps the passes coming while it waits.
    pub(in crate::app) fn watch_sent(&mut self, ctx: &eframe::egui::Context) {
        if self.edit_in.sent.is_empty() {
            self.edit_in.checked = None;
            return;
        }
        let now = std::time::Instant::now();
        let due = self.edit_in.checked.is_none_or(|at| now - at >= WATCH);
        if due {
            self.edit_in.checked = Some(now);
            self.edited_elsewhere();
        }
        ctx.request_repaint_after(WATCH);
    }

    /// The files sent out that another program saved since: their edits
    /// follow them, their previews are made again, and the one shown opens
    /// again.
    pub(in crate::app) fn edited_elsewhere(&mut self) {
        for (path, was, now) in rewritten(&mut self.edit_in.sent) {
            let Some(library) = &mut self.library else {
                continue;
            };
            let ids: Vec<PhotoId> = library
                .session
                .photos()
                .iter()
                .filter(|p| p.path == path)
                .map(|p| p.id)
                .collect();
            for id in &ids {
                if let Err(e) = library.session.follow_rewritten_file(*id, &was, &now) {
                    self.messages.fail(format!(
                        "{}'s edit could not follow the file saved again: {e:#}",
                        file_name(&path)
                    ));
                }
            }
            library.file_rewritten(&path);
            self.refresh_previews(&ids);
            let open = self.document.catalog_photo.filter(|id| ids.contains(id));
            if let Some(id) = open {
                // The edit open was made for the version before; it is now
                // this one's, as the catalog's is.
                if self
                    .document
                    .file
                    .as_ref()
                    .is_some_and(|f| f.same_photo(&was))
                {
                    self.document.file = Some(now);
                }
                self.load_raw(path.clone(), Some(id));
            }
            self.messages.note(format!(
                "{} was saved again, and is updated",
                file_name(&path)
            ));
        }
    }
}

/// Adds the file at `path` to the catalog at `location`, with what new photos
/// start with, first copying `copy_of` there as it is when given, never over
/// another file: false when the catalog had it already.
fn import(location: &CatalogLocation, path: &Path, copy_of: Option<&Path>) -> anyhow::Result<bool> {
    use anyhow::Context;
    if let Some(from) = copy_of {
        let mut to = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .with_context(|| format!("{} could not be created", path.display()))?;
        std::io::copy(&mut std::fs::File::open(from)?, &mut to)
            .with_context(|| format!("{} could not be copied", from.display()))?;
    }
    let mut catalog = crate::catalog::Catalog::open(location)?;
    let added = catalog
        .import_files(
            crate::storage::parent_dir(path),
            std::slice::from_ref(&path.to_path_buf()),
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
    Ok(!added.added.is_empty())
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
