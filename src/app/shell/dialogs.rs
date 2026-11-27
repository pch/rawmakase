//! File chooser intent, independent of menu order and numeric UI identifiers.
use crate::app::{Editor, presets::bulk_import::ImportKind, worker::Event};
use eframe::egui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum FileDialog {
    MonitorProfile,
    LoadPreset,
    SavePreset,
    CameraProfile,
    /// LUTs expecting colors in this space (Import LUTs).
    Luts(crate::camera_profiles::LutSpace),
    LensProfile,
    ImportXmp,
    /// A folder to scan for profiles or presets, subfolders included.
    ImportFolder(ImportKind),
    /// Where decoded images are cached.
    DecodeCacheFolder,
    /// A program for Edit In: an application on macOS, a program elsewhere.
    ExternalEditor,
}

#[derive(Clone)]
pub(in crate::app) enum CatalogDialog {
    Create,
    Open,
    ImportLightroom,
    /// Another RAWmakase catalog's photos added to the open one.
    ImportCatalog,
    Folder(FolderAction),
}

#[derive(Clone)]
pub(in crate::app) enum FolderAction {
    Add,
    RelinkRoot(crate::catalog::RootId),
    /// The folder of a root at a logical path, with or without a catalog
    /// folder of its own.
    RelinkFolder(crate::catalog::RootId, String),
}

impl Editor {
    pub(in crate::app) fn dialog(&mut self, kind: FileDialog, ctx: &egui::Context) {
        if !self.activity.begin_dialog() {
            return;
        }
        crate::app::task::spawn(
            self.tx.clone(),
            ctx.clone(),
            move |tx| {
                let import = match kind {
                    FileDialog::CameraProfile => Some(ImportKind::CameraProfiles),
                    FileDialog::Luts(space) => Some(ImportKind::Luts(space)),
                    FileDialog::LensProfile => Some(ImportKind::LensProfiles),
                    FileDialog::ImportXmp => Some(ImportKind::Presets),
                    _ => None,
                };
                if let Some(import) = import {
                    let (name, extensions) = import.filter();
                    let event = rfd::FileDialog::new()
                        .add_filter(name, extensions)
                        .pick_files()
                        .map(|paths| Event::Import(import, paths))
                        .unwrap_or(Event::DialogClosed);
                    let _ = tx.send(event);
                    return;
                }
                if let FileDialog::DecodeCacheFolder = kind {
                    let event = rfd::FileDialog::new()
                        .pick_folder()
                        .map(Event::DecodeCacheFolder)
                        .unwrap_or(Event::DialogClosed);
                    let _ = tx.send(event);
                    return;
                }
                if let FileDialog::ImportFolder(import) = kind {
                    let event = rfd::FileDialog::new()
                        .pick_folder()
                        .map(|folder| Event::Import(import, vec![folder]))
                        .unwrap_or(Event::DialogClosed);
                    let _ = tx.send(event);
                    return;
                }
                let selected = match kind {
                    FileDialog::ExternalEditor => external_editor_dialog().pick_file(),
                    FileDialog::MonitorProfile => rfd::FileDialog::new()
                        .add_filter("ICC profile", &["icc", "icm"])
                        .pick_file(),
                    FileDialog::CameraProfile
                    | FileDialog::Luts(_)
                    | FileDialog::LensProfile
                    | FileDialog::ImportXmp
                    | FileDialog::ImportFolder(_)
                    | FileDialog::DecodeCacheFolder => {
                        unreachable!("Handled by the folder choosers")
                    }
                    FileDialog::LoadPreset => rfd::FileDialog::new()
                        .add_filter("RAWmakase preset", &["json"])
                        .pick_file(),
                    FileDialog::SavePreset => rfd::FileDialog::new()
                        .set_file_name("preset.json")
                        .save_file(),
                };
                let event = selected
                    .map(|p| match kind {
                        FileDialog::ExternalEditor => Event::ExternalEditor(p),
                        FileDialog::MonitorProfile => Event::Monitor(p),
                        FileDialog::CameraProfile
                        | FileDialog::Luts(_)
                        | FileDialog::LensProfile
                        | FileDialog::ImportXmp
                        | FileDialog::ImportFolder(_)
                        | FileDialog::DecodeCacheFolder => unreachable!(),
                        FileDialog::LoadPreset => Event::PresetLoad(p),
                        FileDialog::SavePreset => Event::PresetSave(p),
                    })
                    .unwrap_or(Event::DialogClosed);
                let _ = tx.send(event);
            },
            |tx, _| {
                let _ = tx.send(Event::DialogClosed);
            },
        );
    }
}

/// Add Editor…'s chooser: on macOS an application bundle from Applications,
/// on Windows a program file, elsewhere any file.
fn external_editor_dialog() -> rfd::FileDialog {
    let dialog = rfd::FileDialog::new().set_title("Choose an External Editor");
    if cfg!(target_os = "macos") {
        dialog
            .add_filter("Application", &["app"])
            .set_directory("/Applications")
    } else if cfg!(windows) {
        dialog.add_filter("Program", &["exe"])
    } else {
        dialog
    }
}
