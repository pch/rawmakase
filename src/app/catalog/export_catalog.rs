//! Export as Catalog (issue #738): a folder's or the selection's photos into
//! a new catalog of their own (see `catalog::export_catalog`), and then, for
//! a folder, removing it from this catalog while it is still what was
//! exported.
//!
//! The open edit is saved first; the export reads the catalog as it is once
//! the user has named the file, in a task of its own. It is a dialog
//! activity from the file dialog until its result is shown, so nothing else
//! that waits for one starts meanwhile, and its end (success, failure or
//! panic) always ends it. Quitting meanwhile leaves no catalog at the name
//! chosen: at most a hidden folder beside it with the unfinished copy.
use crate::app::Editor;
use crate::app::library::{CatalogExportRequest, FolderRemoval};
use crate::app::ui::widgets::confirm_modal;
use crate::app::worker::Event;
use crate::catalog::CatalogExport;

/// What Export as Catalog wrote.
#[derive(Debug)]
pub(crate) struct Exported {
    pub(crate) export: CatalogExport,
    /// For a folder, removing it from the catalog it was exported from, only
    /// while it is still what was exported.
    pub(crate) removal: Option<FolderRemoval>,
}

/// What the dialog after an export was closed with.
#[derive(Clone, Copy, PartialEq)]
enum Next {
    Done,
    Open,
    Remove,
}

impl Editor {
    /// Saves the open edit, asks where to write the catalog and writes it.
    pub(in crate::app) fn export_as_catalog(&mut self, request: CatalogExportRequest) {
        if !self.ready_for_catalog() || !self.activity.begin_dialog() {
            self.messages
                .warn("Export as Catalog waits for the work in progress");
            return;
        }
        let Some(library) = &self.library else {
            self.activity.finish_dialog();
            return;
        };
        let catalog = library.session.catalog().location().clone();
        let ctx = self.context.clone();
        crate::app::task::spawn(
            self.tx.clone(),
            ctx.clone(),
            move |tx| {
                let result = (|| -> anyhow::Result<Option<Exported>> {
                    let Some(destination) = crate::app::catalog::catalog_file_dialog()
                        .set_title("Export as Catalog")
                        .set_file_name(format!("{}.rawmakase", request.name))
                        .save_file()
                    else {
                        return Ok(None);
                    };
                    let _ = tx.send(Event::CatalogWorking(format!(
                        "Exporting {} as a catalog…",
                        request.name
                    )));
                    ctx.request_repaint();
                    let source = crate::catalog::Catalog::open(&catalog)?;
                    let export = source.export_catalog(&request.scope, &destination)?;
                    let removal = request.removal.map(|removal| FolderRemoval {
                        exported: export.removal.clone(),
                        ..removal
                    });
                    Ok(Some(Exported { export, removal }))
                })()
                .map_err(|e| format!("{e:#}"));
                let _ = tx.send(Event::CatalogExported(Box::new(result)));
            },
            |tx, error| {
                let _ = tx.send(Event::CatalogExported(Box::new(Err(error))));
            },
        );
    }

    /// Shows what an export wrote, or why it failed.
    pub(in crate::app) fn catalog_exported(&mut self, result: Result<Option<Exported>, String>) {
        self.catalog_work = None;
        self.activity.finish_dialog();
        match result {
            Ok(Some(exported)) => {
                self.messages.note(format!(
                    "Exported {} to {}",
                    photos(exported.export.photos),
                    exported.export.path.display()
                ));
                self.modal = Some(crate::app::Modal::CatalogExported(Box::new(exported)));
            }
            Ok(None) => {}
            Err(e) => self.messages.fail(format!("Export as Catalog failed: {e}")),
        }
    }

    /// The dialog after an export: what it holds and what it could not
    /// carry, with opening it and, for a folder, removing it from this
    /// catalog.
    pub(in crate::app) fn catalog_exported_window(&mut self, ctx: &eframe::egui::Context) {
        let Some(crate::app::Modal::CatalogExported(exported)) = &self.modal else {
            return;
        };
        let export = &exported.export;
        let file = export
            .path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let title = format!("Exported {} to “{file}”", photos(export.photos));
        let detail = summary(export);
        let remove = exported
            .removal
            .as_ref()
            .map(|r| format!("Remove “{}” from This Catalog…", r.name));
        let mut buttons = vec![("Done", Next::Done), ("Open Exported Catalog", Next::Open)];
        if let Some(label) = &remove {
            buttons.push((label.as_str(), Next::Remove));
        }
        let Some(next) = confirm_modal(
            ctx,
            "catalog-exported",
            &title,
            &detail,
            false,
            &buttons,
            Next::Done,
        ) else {
            return;
        };
        let Some(crate::app::Modal::CatalogExported(exported)) = self.modal.take() else {
            return;
        };
        match next {
            Next::Done => {}
            Next::Open => self.load_catalog(exported.export.path, ctx),
            // Confirmed as any removal is; refused there if the catalog or
            // the folder changed since.
            Next::Remove => self.modal = exported.removal.map(crate::app::Modal::RemoveFolder),
        }
    }
}

/// "1 photo", "3 photos".
fn photos(n: usize) -> String {
    match n {
        1 => "1 photo".into(),
        n => format!("{n} photos"),
    }
}

/// What the export holds and what it could not carry, as the dialog says it.
fn summary(export: &CatalogExport) -> String {
    let mut lines = vec![format!(
        "It opened and lists every photo, with their edits, History, snapshots, \
         ratings, keywords and collections, and a collection “{}” of them all. \
         The photos stay where they are: the catalog refers to them there.",
        export.collection
    )];
    if export.family > 0 {
        lines.push(format!(
            "{} the photos chosen went with them: a virtual copy never goes without its master.",
            match export.family {
                1 => "A master or virtual copy of".to_string(),
                n => format!("{n} masters or virtual copies of"),
            }
        ));
    }
    if !export.offline.is_empty() {
        let names: Vec<String> = export
            .offline
            .iter()
            .take(3)
            .map(|p| {
                p.file_name()
                    .map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into())
            })
            .collect();
        let more = export.offline.len().saturating_sub(names.len());
        lines.push(format!(
            "{} not found on this computer ({}{}); the catalog has {} as before.",
            match export.offline.len() {
                1 => "1 photo is".to_string(),
                n => format!("{n} photos are"),
            },
            names.join(", "),
            if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            },
            if export.offline.len() == 1 {
                "it"
            } else {
                "them"
            },
        ));
    }
    if !export.lens_profiles.is_empty() {
        lines.push(format!(
            "Edits use lens profiles imported into RAWmakase on this computer ({}); \
             another computer renders them the same only once it imports them too.",
            export.lens_profiles.join(", ")
        ));
    }
    if export.kept_every_mask {
        lines.push(
            "Some Develop History could not be read, so every mask of this catalog went \
             with the export rather than one it needs be left out."
                .into(),
        );
    }
    lines.join("\n\n")
}
