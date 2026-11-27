//! Import from Catalog in the app: what the status line says once it ran,
//! and the Catalog menu's row.
use crate::app::Event;
use crate::app::library::Library;
use crate::catalog::{Catalog, CatalogImport};
use eframe::egui;

/// A Library on a catalog with one photo, in `dir`.
fn library(dir: &std::path::Path) -> anyhow::Result<Box<Library>> {
    let photos = dir.join("photos");
    std::fs::create_dir_all(&photos)?;
    std::fs::write(photos.join("home.ARW"), b"identity fixture")?;
    let path = dir.join("photos.rawmakase");
    Catalog::create(&path)?.add_folder(&photos)?;
    Ok(Box::new(Library::load(&path, egui::Context::default())?))
}

/// The Library's status line after `imported` was reported.
fn said(imported: &CatalogImport) -> anyhow::Result<(String, Option<String>)> {
    let dir = tempfile::tempdir()?;
    let event = super::catalog_imported(
        Event::CatalogReady(Ok(library(dir.path())?)),
        "Trip",
        imported,
    );
    let Event::CatalogReady(Ok(library)) = event else {
        panic!("not a ready catalog");
    };
    Ok((
        library.messages.line().to_string(),
        library.messages.detail().map(str::to_string),
    ))
}

#[test]
fn the_status_line_says_what_was_imported() -> anyhow::Result<()> {
    let (line, detail) = said(&CatalogImport {
        added: 12,
        present: 3,
        ..Default::default()
    })?;
    assert_eq!(
        line,
        "Imported 12 photos from Trip. 3 photos were in this catalog already."
    );
    assert_eq!(detail, None);
    Ok(())
}

#[test]
fn the_status_line_names_the_photos_left_out_and_why() -> anyhow::Result<()> {
    let (line, detail) = said(&CatalogImport {
        added: 1,
        missing: vec!["/Volumes/Card/a.ARW".into()],
        unplaced: vec!["/elsewhere/b.ARW".into(), "/elsewhere/c.ARW".into()],
        ..Default::default()
    })?;
    assert_eq!(
        line,
        "Imported 1 photo from Trip. Not imported: 1 photo not found on this computer, \
         2 photos in a folder this catalog has elsewhere."
    );
    assert_eq!(
        detail.as_deref(),
        Some("/Volumes/Card/a.ARW\n/elsewhere/b.ARW\n/elsewhere/c.ARW")
    );
    Ok(())
}

/// The Catalog menu with Import from Catalog… under Import from Lightroom,
/// for `RAWMAKASE_UI_SHOTS` ([`crate::app::ui::shots`]). The menu opens on a
/// click, which a shot can't make, so it is drawn on a panel.
#[test]
fn shots_of_import_from_catalog_in_the_catalog_menu() {
    if std::env::var_os("RAWMAKASE_UI_SHOTS").is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    crate::app::ui::shots::shot(
        "import-from-catalog-menu",
        egui::Vec2::new(300., 340.),
        move |e, _ctx| {
            e.library = Some(library(&path).unwrap());
            e.onboarding.visible = false;
        },
        |e, ctx| {
            crate::app::ui::shots::panel(ctx, 268., |ui| e.catalog_menu(ui, ctx));
        },
    );
}
