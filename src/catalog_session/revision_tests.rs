//! Issue 998: every write a smart collection's rules can see moves the query
//! revision, and another connection's write is seen only once the catalog is
//! read again.
use super::*;
use crate::catalog::StackChange;
use crate::metadata::TextField;

/// A session over a catalog of three JPEGs in one folder.
fn session() -> Result<(tempfile::TempDir, CatalogSession, Vec<PhotoId>)> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    for name in ["a.jpg", "b.jpg", "c.jpg"] {
        image::RgbImage::new(8, 8).save(folder.join(name))?;
    }
    let path = directory.path().join("library.rawmakase");
    Catalog::create(&path)?.add_folder(&folder)?;
    let session = CatalogSession::open(&CatalogLocation::from(&path))?.session;
    let ids = session.photos().iter().map(|p| p.id).collect();
    Ok((directory, session, ids))
}

/// Runs `write` and checks that it moved the revision.
fn moves(
    session: &mut CatalogSession,
    what: &str,
    write: impl FnOnce(&mut CatalogSession) -> Result<()>,
) -> Result<()> {
    let before = session.query_revision();
    write(session)?;
    assert!(session.query_revision() > before, "{what}");
    Ok(())
}

#[test]
fn every_write_class_moves_the_query_revision() -> Result<()> {
    let (_directory, mut session, ids) = session()?;
    let (a, b) = (ids[0], ids[1]);
    moves(&mut session, "ratings", |s| {
        s.set_ratings(&[(a, 3, 1, "Red".into())])
    })?;
    moves(&mut session, "descriptive metadata", |s| {
        s.edit_descriptive(
            &[a],
            DescriptiveEdit::Text(TextField::Title, "Kyoto".into()),
        )
        .map(drop)
    })?;
    moves(&mut session, "keywords", |s| {
        s.edit_descriptive(
            &[a],
            DescriptiveEdit::AddKeywords(vec![vec!["Japan".into()]]),
        )
        .map(drop)
    })?;
    let quick = session.ensure_quick_collection()?;
    moves(&mut session, "collection membership", |s| {
        s.change_collection(quick, &[a], &[]).map(drop)
    })?;
    let mut copy = PhotoId(0);
    moves(&mut session, "virtual copy", |s| {
        copy = s.create_virtual_copy(a, None)?.value;
        Ok(())
    })?;
    moves(&mut session, "copy name", |s| s.rename_copy(copy, "Copy 2"))?;
    moves(&mut session, "stacks", |s| {
        s.change_stacks(&StackChange::Group(vec![a, b])).map(drop)
    })?;
    let photo = session
        .photos()
        .iter()
        .find(|p| p.id == b)
        .cloned()
        .unwrap();
    moves(&mut session, "edit", |s| {
        s.save_edit(
            b,
            &photo.path,
            &crate::storage::Identity::read(&photo.path)?,
            &Default::default(),
            &Default::default(),
            crate::catalog::HistoryUpdate::Keep,
        )
    })?;
    moves(&mut session, "edits in one transaction", |s| {
        s.change_edits(&[])
    })?;
    moves(&mut session, "photos read again", |s| {
        s.refresh_photos(&[a])
    })?;
    moves(&mut session, "catalog read again", CatalogSession::reload)?;
    moves(&mut session, "copy removed", |s| {
        s.remove_virtual_copy(copy).map(drop)
    })?;
    moves(&mut session, "edits on another connection", |s| {
        s.note_external_change([b]);
        Ok(())
    })?;
    let before = session.query_revision();
    session.note_external_change([]);
    assert_eq!(session.query_revision(), before, "nothing noted");
    Ok(())
}

#[test]
fn photo_info_read_in_the_background_moves_the_query_revision() -> Result<()> {
    let (_directory, mut session, _) = session()?;
    session.start_photo_info(|_| true);
    let before = session.query_revision();
    let started = std::time::Instant::now();
    let mut saved = 0;
    while session.reading_photo_info() {
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        saved += session.poll_photo_info().saved;
        std::thread::yield_now();
    }
    assert!(saved > 0);
    assert!(session.query_revision() > before);
    Ok(())
}

/// Another process's write (the command line's, an MCP host's) is not seen
/// until the catalog is read again, as for every list the Library shows;
/// reading it again moves the revision.
#[test]
fn another_connections_write_is_seen_once_the_catalog_is_read_again() -> Result<()> {
    let (directory, mut session, ids) = session()?;
    let mut other = Catalog::open(CatalogLocation::from(
        &directory.path().join("library.rawmakase"),
    ))?;
    other.set_metadata_of(&[(ids[0], 5, 0, String::new())])?;
    let before = session.query_revision();
    assert_eq!(session.photos()[0].rating, 0);
    assert_eq!(session.query_revision(), before);
    session.reload()?;
    assert_eq!(session.photos()[0].rating, 5);
    assert!(session.query_revision() > before);
    Ok(())
}
