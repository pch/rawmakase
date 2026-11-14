//! The editor's side of `[[source]]`s: adding the files a program reports,
//! on a worker, and answering it once they are in the catalog.
//!
//! A report is admitted by the host only in its source's current scope
//! (see the module above). The editor then takes the catalog's folder-change
//! activity, so nothing else changes folders, files or photos' rows
//! meanwhile, and adds the files on a worker with
//! [`crate::catalog::Catalog::import_files`]: Add Folder's placement and
//! steps, one import batch, on a connection of its own. While other work
//! holds the catalog, the report is answered `busy` and the program reports
//! again. Once the files are read, right before its transaction, the
//! worker passes the report's [`WriteGate`]: closed (the scope ended, the
//! program cancelled the report, its session ended, quitting), it writes
//! nothing.
//!
//! Once the worker reports back, the Library reads the catalog again in
//! place, as after a file operation, so the photo open in Develop, its
//! edit, History and any save still pending stay as they were; only
//! the lists shown, and the background reads of what is new, change. A
//! Copy Name or metadata field being typed in the Library is saved first,
//! as anything else that reads the catalog again saves it; when it can't
//! be saved, the catalog isn't read again, so the text stays, and the new
//! photos show with the next read. The program is then answered with the
//! photos its files are.
use super::Answer;
use crate::app::Editor;
use crate::catalog::{CatalogLocation, FileRefusal as Refusal, FilesAdded};
use rawmakase_extensions::protocol::method::{
    FileRefusal, RefusedFile, Reported, ReportedPhoto, SourceReport,
};
use rawmakase_extensions::protocol::{ErrorCode, RemoteError};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// The catalog open in the editor, as sources' scopes know it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct OpenCatalog {
    /// What `extensions.json` records a source as on for: the catalog's file.
    pub key: String,
    /// What `source-started` names it.
    pub name: String,
}

impl OpenCatalog {
    pub(in crate::app) fn of(location: &CatalogLocation) -> Self {
        let CatalogLocation::File(path) = location;
        Self {
            key: path.to_string_lossy().into_owned(),
            name: location.name(),
        }
    }
}

/// Whether a report's worker may still write, decided once: closed before
/// the write began, it writes nothing; once begun, closing it changes
/// nothing, and the write's outcome stands.
#[derive(Debug, Default)]
pub(in crate::app) struct WriteGate(AtomicU8);

const OPEN: u8 = 0;
const CLOSED: u8 = 1;
const WRITING: u8 = 2;

impl WriteGate {
    /// Closes the gate; false when the write has begun.
    pub(in crate::app) fn close(&self) -> bool {
        match self
            .0
            .compare_exchange(OPEN, CLOSED, Ordering::SeqCst, Ordering::SeqCst)
        {
            Ok(_) => true,
            Err(state) => state == CLOSED,
        }
    }
    /// Whether the gate was closed: a worker that sees it closed doesn't
    /// read the files at all.
    fn is_closed(&self) -> bool {
        self.0.load(Ordering::SeqCst) == CLOSED
    }
    /// Begins the write; false when the gate was closed first.
    fn begin(&self) -> bool {
        self.0
            .compare_exchange(OPEN, WRITING, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }
}

/// What a report's worker sends back.
pub(crate) struct ReportDone {
    ticket: u64,
    title: String,
    /// The catalog the files were added to.
    catalog: CatalogLocation,
    show: bool,
    result: Result<FilesAdded, RemoteError>,
}

impl Editor {
    /// Starts adding the files of `report`, admitted in its scope, to the
    /// open catalog; `busy` while other work holds the catalog.
    pub(super) fn add_reported(
        &mut self,
        report: SourceReport,
        title: String,
        ticket: u64,
    ) -> Answer {
        let Some(location) = self
            .library
            .as_ref()
            .map(|l| l.session.catalog().location().clone())
        else {
            return Answer::Now(Err(RemoteError::new(
                ErrorCode::NotAdmitted,
                "no catalog is open",
            )));
        };
        // Excludes Add Folder, Synchronize, batch edits, file operations,
        // another report and a catalog change until the worker reports back.
        if !self.activity.begin_folder_change() {
            return Answer::Now(Err(RemoteError::new(
                ErrorCode::Busy,
                "RAWmakase is busy with the catalog; report again shortly",
            )));
        }
        let gate = Arc::new(WriteGate::default());
        let worker_gate = gate.clone();
        let failed_title = title.clone();
        let failed_catalog = location.clone();
        let show = report.show;
        #[cfg(test)]
        let hook = self.report_hook.clone();
        crate::app::task::spawn(
            self.tx.clone(),
            self.context.clone(),
            move |tx| {
                let withdrawn =
                    || RemoteError::new(ErrorCode::Cancelled, "the report was withdrawn");
                // The gate is claimed once the files are read, right before
                // the transaction, so a report withdrawn while a slow disk is
                // read writes nothing.
                let result = if worker_gate.is_closed() {
                    Err(withdrawn())
                } else {
                    crate::catalog::Catalog::open(&location)
                        .and_then(|mut catalog| {
                            catalog.import_files(
                                &report.folder,
                                &report.files,
                                &crate::catalog_session::new_photo_defaults(),
                                || {
                                    #[cfg(test)]
                                    if let Some(hook) = &hook {
                                        hook();
                                    }
                                    worker_gate.begin()
                                },
                            )
                        })
                        .map_err(|e| {
                            RemoteError::new(ErrorCode::Other("not_added".into()), format!("{e:#}"))
                        })
                        .and_then(|added| added.ok_or_else(withdrawn))
                };
                let _ = tx.send(crate::app::worker::Event::Extensions(Box::new(
                    super::Done::Reported(Box::new(ReportDone {
                        ticket,
                        title,
                        catalog: location,
                        show,
                        result,
                    })),
                )));
            },
            move |tx, why| {
                let _ = tx.send(crate::app::worker::Event::Extensions(Box::new(
                    super::Done::Reported(Box::new(ReportDone {
                        ticket,
                        title: failed_title,
                        catalog: failed_catalog,
                        show: false,
                        result: Err(RemoteError::new(ErrorCode::Other("not_added".into()), why)),
                    })),
                )));
            },
        );
        Answer::Adding(gate)
    }

    /// A report's worker finished, failed or panicked: the catalog is free
    /// again, the Library shows what was added, and the program is answered.
    pub(super) fn report_finished(&mut self, done: ReportDone) {
        let ReportDone {
            ticket,
            title,
            catalog,
            show,
            result,
        } = done;
        self.activity.finish_dialog();
        let added = match result {
            Ok(added) => added,
            Err(err) => {
                if err.code != ErrorCode::Cancelled {
                    self.messages
                        .fail(format!("{title}: photos not added: {}", err.message));
                }
                self.extensions.finish_report(ticket, Err(err));
                return;
            }
        };
        let same = self
            .library
            .as_ref()
            .is_some_and(|l| *l.session.catalog().location() == catalog);
        // Reading the catalog again drops what is being typed in the
        // Library: it is saved first, and kept when it can't be.
        if same
            && !added.added.is_empty()
            && self.commit_library_drafts()
            && let Some(library) = &mut self.library
        {
            match library.reread_after(&[]) {
                Ok(()) => library.start_background_reads(),
                Err(e) => self.messages.fail(format!(
                    "{title}: photos added, but the catalog could not be read again: {e:#}"
                )),
            }
        }
        if !added.report.unreadable.is_empty() {
            self.messages.warn(format!(
                "{title}: {} sidecars could not be read",
                added.report.unreadable.len()
            ));
        }
        let mut reported = Reported {
            added: u32::try_from(added.added.len()).unwrap_or(u32::MAX),
            photos: Vec::new(),
            refused: added
                .refused
                .into_iter()
                .map(|(path, why)| RefusedFile {
                    path,
                    reason: refusal(why),
                })
                .collect(),
        };
        // The last photo this report added, in the order reported: a file
        // the catalog had already is listed, but not shown.
        let mut last_added = None;
        if same {
            for (path, file) in added.placed {
                if let Some(photo) = self.catalog_photo_at(&file) {
                    if added.added.contains(&file) {
                        last_added = Some(photo);
                    }
                    reported.photos.push(ReportedPhoto {
                        path,
                        photo: photo.0,
                    });
                }
            }
        }
        if !added.added.is_empty() {
            let n = added.added.len();
            let s = if n == 1 { "" } else { "s" };
            self.messages.note(format!("{title}: added {n} photo{s}"));
        }
        if show && let Some(last) = last_added {
            self.show_reported(last);
        }
        let value = serde_json::to_value(&reported).expect("a reply is JSON");
        self.extensions.finish_report(ticket, Ok(value));
    }

    /// Shows a photo a source just added, as a tethered capture is shown:
    /// opened in Develop when Develop shows a photo, through the path a
    /// click takes, which saves the open edit first and opens nothing when
    /// that fails; otherwise selected in the Library.
    fn show_reported(&mut self, id: crate::catalog::PhotoId) {
        if self.module == crate::app::Module::Develop && self.document.catalog_photo.is_some() {
            if self.document.catalog_photo != Some(id) {
                self.develop_catalog_photo(id);
            }
        } else if let Some(library) = &mut self.library {
            library.show(id);
        }
    }
}

fn refusal(why: Refusal) -> FileRefusal {
    match why {
        Refusal::NotFound => FileRefusal::NotFound,
        Refusal::NotAPhoto => FileRefusal::NotAPhoto,
        Refusal::OutsideFolder => FileRefusal::OutsideFolder,
        Refusal::LinkedElsewhere => FileRefusal::LinkedElsewhere,
        Refusal::Ambiguous => FileRefusal::Ambiguous,
    }
}
