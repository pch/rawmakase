use crate::app::catalog::export_catalog::Exported;
use crate::app::library::{self, FolderRemoval};
use crate::app::{Editor, Modal};
use crate::catalog::{Catalog, ExportScope};
use eframe::egui;

/// An editor on a catalog of folders `trip` (one photo) and `home` (one),
/// with `trip` exported, and its removal as the export dialog offers it.
fn exported_trip() -> anyhow::Result<(tempfile::TempDir, Editor, FolderRemoval)> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("photos.rawmakase");
    let photos = dir.path().join("photos");
    std::fs::create_dir_all(photos.join("trip"))?;
    std::fs::create_dir_all(photos.join("home"))?;
    std::fs::write(photos.join("home/home.ARW"), b"identity fixture")?;
    std::fs::write(photos.join("trip/away.ARW"), b"identity fixture 2")?;
    Catalog::create(&path)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.library = Some(Box::new(library::Library::load(&path, ctx)?));
    let session = &editor.library.as_ref().unwrap().session;
    let trip: std::collections::HashSet<_> = session
        .folders()
        .iter()
        .filter(|f| f.relative.trim_end_matches('/') == "trip")
        .map(|f| f.id)
        .collect();
    let export = Catalog::open(&path)?.export_catalog(
        &ExportScope::Folders(trip.iter().copied().collect()),
        &dir.path().join("trip.rawmakase"),
    )?;
    let removal = FolderRemoval {
        catalog: session.catalog().location().clone(),
        name: "trip".into(),
        folders: trip,
        photos: 1,
        empty_root: None,
        exported: export.removal.clone(),
    };
    assert!(editor.activity.begin_dialog());
    editor.catalog_exported(Ok(Some(Exported {
        export,
        removal: Some(removal.clone()),
    })));
    Ok((dir, editor, removal))
}

#[test]
fn an_export_ends_its_activity_and_offers_the_folders_removal() -> anyhow::Result<()> {
    let (_dir, mut editor, removal) = exported_trip()?;
    assert!(!editor.activity.is_busy());
    let Some(Modal::CatalogExported(exported)) = &editor.modal else {
        panic!("no export dialog");
    };
    assert_eq!(exported.removal.as_ref(), Some(&removal));
    assert!(
        editor.messages.line().contains("Exported 1 photo"),
        "{}",
        editor.messages.line()
    );

    // A failure, or a panic, ends it too.
    assert!(editor.activity.begin_dialog());
    editor.catalog_exported(Err("Stopped unexpectedly: boom".into()));
    assert!(!editor.activity.is_busy());
    assert!(
        editor.messages.line().contains("boom"),
        "{}",
        editor.messages.line()
    );
    Ok(())
}

#[test]
fn an_exported_folder_changed_since_is_not_removed() -> anyhow::Result<()> {
    let (_dir, mut editor, removal) = exported_trip()?;
    editor.modal = None;
    let away = {
        let session = &editor.library.as_ref().unwrap().session;
        session
            .photos()
            .iter()
            .find(|p| p.filename == "away.ARW")
            .unwrap()
            .id
    };
    let library = editor.library.as_mut().unwrap();
    library
        .session
        .set_ratings(&[(away, 5, 0, String::new())])?;
    editor.remove_folders(&removal);
    let session = &editor.library.as_ref().unwrap().session;
    assert_eq!(session.photos().len(), 2);
    assert!(
        editor
            .messages
            .line()
            .contains("changed since it was exported"),
        "{}",
        editor.messages.line()
    );

    let library = editor.library.as_mut().unwrap();
    library
        .session
        .set_ratings(&[(away, 0, 0, String::new())])?;
    editor.remove_folders(&removal);
    let names: Vec<_> = editor
        .library
        .as_ref()
        .unwrap()
        .session
        .photos()
        .iter()
        .map(|p| p.filename.clone())
        .collect();
    assert_eq!(names, ["home.ARW"]);
    Ok(())
}
