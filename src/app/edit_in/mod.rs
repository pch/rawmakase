//! Edit In: a photo opened in a program the user chose in Preferences >
//! External Editing, as Lightroom's Edit In does for a raw photo.
//!
//! The photos are chosen as Export chooses them. Each is rendered with its
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
//! The files sent out are remembered while the app runs. When its window
//! comes to the front again each is checked: one saved meanwhile has its
//! edit in the catalog bound to the new version (so the edit is not
//! protected, nor a save refused, as for a photo replaced by another), its
//! previews made again, and it is opened again when it is the photo shown.
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
    /// Whether the window had the focus last frame.
    focused: bool,
    /// Stands in for starting a program, for a test: what was opened in what.
    #[cfg(test)]
    pub(in crate::app) launched: Option<Vec<(PathBuf, PathBuf)>>,
}

impl Default for EditIn {
    fn default() -> Self {
        Self::new(Vec::new(), None)
    }
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
    editor: PathBuf,
    catalog: Option<CatalogLocation>,
    original: Option<PhotoId>,
}

/// A file handed to a program, as it was then or last checked.
#[derive(Clone, Debug)]
pub(in crate::app) struct Sent {
    pub(in crate::app) path: PathBuf,
    pub(in crate::app) identity: Identity,
}

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
            focused: true,
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

/// The files of `sent` whose size or modification time changed since they
/// were sent or last checked, each with the version it was and the one it is
/// now; `sent` keeps the new ones. A file that can't be read (moved,
/// deleted) is forgotten.
pub(in crate::app) fn rewritten(sent: &mut Vec<Sent>) -> Vec<(PathBuf, Identity, Identity)> {
    let mut changed = Vec::new();
    sent.retain_mut(|file| {
        let Ok(stamp) = crate::storage::Stamp::read(&file.path) else {
            return false;
        };
        if stamp == file.identity.stamp() {
            return true;
        }
        let Ok(now) = Identity::read(&file.path) else {
            return false;
        };
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
            self.edit_in.adding = Some((generation, next));
            task::spawn(
                self.tx.clone(),
                self.context.clone(),
                move |tx| {
                    let result = import(&location, &path).map_err(|e| format!("{e:#}"));
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
        self.launch_editor(&rendered.editor, &rendered.path);
        self.add_edits();
    }

    /// Opens `file` in `editor`, and remembers it to check when the window
    /// comes back to the front.
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

    /// Follows the window's focus: back at the front, the files sent out are
    /// checked for a save made meanwhile.
    pub(in crate::app) fn edit_in_focus(&mut self, focused: bool) {
        let back = focused && !self.edit_in.focused;
        self.edit_in.focused = focused;
        if back && !self.edit_in.sent.is_empty() {
            self.edited_elsewhere();
        }
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

/// Adds the TIFF at `path` to the catalog at `location`, with what new photos
/// start with: false when the catalog had it already.
fn import(location: &CatalogLocation, path: &Path) -> anyhow::Result<bool> {
    use anyhow::Context;
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

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests;
