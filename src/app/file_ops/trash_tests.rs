//! Move to Trash in the app: Delete asking whether to trash the files or
//! only remove the photos, ⌘⌥⇧Delete, Delete Rejected Photos, and a volume
//! with no Trash, whose photos Delete Permanently… deletes once confirmed
//! again; driven by keys and frames, with a Trash that moves files into a
//! temporary folder.
use super::tests::{frame, ids_left, select, settle, tap};
use super::*;
use crate::app::Editor;
use crate::app::testing::editor_with_catalog;
use crate::catalog::PhotoId;
use crate::catalog::file_ops::TrashSupport;
use eframe::egui::{Key, Modifiers};
use std::path::PathBuf;

/// A Trash that moves files into `dir`, or has none; `support` is what it
/// says, asked whether it takes a file.
struct TestTrash {
    dir: Option<PathBuf>,
    support: TrashSupport,
}

impl Trash for TestTrash {
    fn trash(&self, path: &Path) -> Result<TrashReceipt, TrashError> {
        let Some(dir) = &self.dir else {
            return Err(TrashError::Unsupported);
        };
        let location = dir.join(path.file_name().unwrap());
        std::fs::rename(path, &location).map_err(|e| TrashError::Failed(e.to_string()))?;
        Ok(TrashReceipt { location })
    }
    fn supports(&self, _: &Path) -> TrashSupport {
        self.support
    }
}

/// An editor on a catalog of `names`, whose Trash is `trash/` beside it.
fn editor(names: &[&str]) -> anyhow::Result<(tempfile::TempDir, Editor, PathBuf)> {
    let (d, mut e, _) = editor_with_catalog(names)?;
    let trash = d.path().join("trash");
    std::fs::create_dir(&trash)?;
    e.file_ops.trash = Arc::new(TestTrash {
        dir: Some(trash.clone()),
        support: TrashSupport::Unknown,
    });
    Ok((d, e, trash))
}

fn id(e: &Editor, name: &str) -> PhotoId {
    e.library
        .as_ref()
        .unwrap()
        .session
        .photos()
        .iter()
        .find(|p| p.filename == name)
        .unwrap_or_else(|| panic!("no photo {name}"))
        .id
}

const TRASH_KEYS: Modifiers = Modifiers {
    alt: true,
    shift: true,
    ..Modifiers::COMMAND
};

#[test]
fn delete_offers_the_trash_and_the_catalog_and_tab_twice_trashes() -> anyhow::Result<()> {
    let (d, mut e, trash) = editor(&["a.RAF", "b.RAF", "c.RAF"])?;
    std::fs::write(d.path().join("photos/a.RAF.xmp"), b"sidecar")?;
    let (a, b) = (id(&e, "a.RAF"), id(&e, "b.RAF"));
    select(&mut e, &[a, b]);
    tap(&mut e, Key::Delete, Modifiers::NONE);
    settle(&mut e);
    let (title, lines, actions) = e.file_ops.question().expect("it asks");
    assert_eq!(
        title,
        "Move the 2 selected photos to the Trash, or just remove them from the catalog?"
    );
    assert_eq!(actions, ["Remove from Catalog", "Move to Trash"]);
    assert!(
        lines
            .iter()
            .any(|l| l == "2 files (and 1 sidecar file) go to the Trash."),
        "{lines:?}"
    );
    // Return alone cancels.
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.file_ops.confirming());
    assert_eq!(ids_left(&e).len(), 3);
    tap(&mut e, Key::Delete, Modifiers::NONE);
    settle(&mut e);
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert!(trash.join("a.RAF").is_file() && trash.join("b.RAF").is_file());
    assert!(
        trash.join("a.RAF.xmp").is_file(),
        "its sidecar goes with it"
    );
    assert!(d.path().join("photos/c.RAF").is_file());
    assert_eq!(ids_left(&e), vec![id(&e, "c.RAF")]);
    assert_eq!(e.messages.line(), "Moved 2 photos to the Trash");
    Ok(())
}

#[test]
fn delete_then_tab_once_only_removes_from_the_catalog() -> anyhow::Result<()> {
    let (d, mut e, trash) = editor(&["a.RAF", "b.RAF"])?;
    let a = id(&e, "a.RAF");
    select(&mut e, &[a]);
    tap(&mut e, Key::Delete, Modifiers::NONE);
    settle(&mut e);
    let (title, ..) = e.file_ops.question().expect("it asks");
    assert_eq!(
        title,
        "Move “a.RAF” to the Trash, or just remove it from the catalog?"
    );
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(d.path().join("photos/a.RAF").is_file(), "the file stays");
    assert!(!trash.join("a.RAF").exists());
    assert_eq!(ids_left(&e), vec![id(&e, "b.RAF")]);
    assert_eq!(e.messages.line(), "Removed 1 photo from the catalog");
    Ok(())
}

#[test]
fn a_missing_file_can_only_be_removed_from_the_catalog() -> anyhow::Result<()> {
    let (d, mut e, _trash) = editor(&["a.RAF", "b.RAF"])?;
    std::fs::remove_file(d.path().join("photos/a.RAF"))?;
    let a = id(&e, "a.RAF");
    select(&mut e, &[a]);
    tap(&mut e, Key::Delete, Modifiers::NONE);
    settle(&mut e);
    let (title, _, actions) = e.file_ops.question().expect("it asks");
    assert_eq!(title, "Remove “a.RAF” from the catalog?");
    assert_eq!(actions, ["Remove from Catalog"]);
    Ok(())
}

#[test]
fn command_option_shift_delete_asks_only_to_trash() -> anyhow::Result<()> {
    let (d, mut e, trash) = editor(&["a.RAF", "b.RAF"])?;
    let a = id(&e, "a.RAF");
    select(&mut e, &[a]);
    tap(&mut e, Key::Delete, TRASH_KEYS);
    settle(&mut e);
    let (title, lines, actions) = e.file_ops.question().expect("it asks");
    assert_eq!(title, "Move “a.RAF” to the Trash?");
    assert_eq!(actions, ["Move to Trash"]);
    assert!(
        lines.iter().any(|l| l.contains("1 file goes to the Trash")),
        "{lines:?}"
    );
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(trash.join("a.RAF").is_file());
    assert!(!d.path().join("photos/a.RAF").exists());
    assert_eq!(ids_left(&e), vec![id(&e, "b.RAF")]);
    Ok(())
}

#[test]
fn edits_are_said_to_be_lost_with_a_trashed_photo() -> anyhow::Result<()> {
    let (_d, mut e, _trash) = editor(&["a.RAF"])?;
    let a = id(&e, "a.RAF");
    let library = e.library.as_mut().unwrap();
    let edit = crate::app::Recipe {
        exposure: 1.,
        ..Default::default()
    };
    let path = library.photo(a).unwrap().path.clone();
    library.session.save_edit(
        a,
        &path,
        &crate::storage::Identity::read(&path)?,
        &edit,
        &Default::default(),
        crate::catalog::HistoryUpdate::Keep,
    )?;
    select(&mut e, &[a]);
    tap(&mut e, Key::Delete, TRASH_KEYS);
    settle(&mut e);
    let (_, lines, _) = e.file_ops.question().expect("it asks");
    assert!(
        lines.iter().any(|l| l
            == "1 photo has edits in RAWmakase; they are removed with it. Putting the file back from the Trash brings back the file, not its edits."),
        "{lines:?}"
    );
    Ok(())
}

#[test]
fn delete_rejected_photos_asks_for_the_rejected_ones_in_the_source() -> anyhow::Result<()> {
    let (_d, mut e, trash) = editor(&["a.RAF", "b.RAF", "c.RAF"])?;
    let (a, b, c) = (id(&e, "a.RAF"), id(&e, "b.RAF"), id(&e, "c.RAF"));
    for photo in [a, c] {
        select(&mut e, &[photo]);
        tap(&mut e, Key::X, Modifiers::NONE);
    }
    // The selection is b: Delete Rejected asks for a and c, whatever is
    // selected.
    select(&mut e, &[b]);
    tap(&mut e, Key::Delete, Modifiers::COMMAND);
    settle(&mut e);
    let (title, ..) = e.file_ops.question().expect("it asks");
    assert_eq!(
        title,
        "Move the 2 rejected photos to the Trash, or just remove them from the catalog?"
    );
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(trash.join("a.RAF").is_file() && trash.join("c.RAF").is_file());
    assert_eq!(ids_left(&e), vec![b]);
    Ok(())
}

#[test]
fn delete_rejected_with_none_rejected_says_so() -> anyhow::Result<()> {
    let (_d, mut e, _trash) = editor(&["a.RAF"])?;
    tap(&mut e, Key::Delete, Modifiers::COMMAND);
    settle(&mut e);
    assert!(!e.file_ops.confirming());
    assert_eq!(e.messages.line(), "No rejected photos here");
    Ok(())
}

#[test]
fn a_volume_with_no_trash_keeps_the_file_and_the_photo() -> anyhow::Result<()> {
    let (d, mut e, _trash) = editor(&["a.RAF", "b.RAF"])?;
    e.file_ops.trash = Arc::new(TestTrash {
        dir: None,
        support: TrashSupport::Unknown,
    });
    let a = id(&e, "a.RAF");
    select(&mut e, &[a]);
    tap(&mut e, Key::Delete, TRASH_KEYS);
    settle(&mut e);
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert!(d.path().join("photos/a.RAF").is_file());
    assert_eq!(ids_left(&e).len(), 2);
    assert!(
        e.messages.line().contains("has no Trash")
            && e.messages.line().contains(&format!(
                "Delete it in {} if you're sure",
                crate::platform::reveal::MANAGER
            )),
        "{}",
        e.messages.line()
    );
    Ok(())
}

#[test]
fn the_open_photo_trashed_in_develop_is_closed() -> anyhow::Result<()> {
    let (_d, mut e, trash) = editor(&["a.RAF", "b.RAF"])?;
    let a = id(&e, "a.RAF");
    let path = e.library.as_ref().unwrap().photo(a).unwrap().path.clone();
    e.module = crate::app::Module::Develop;
    e.document.catalog_photo = Some(a);
    e.document.opened_at(path);
    frame(&mut e, Vec::new());
    tap(&mut e, Key::Delete, TRASH_KEYS);
    settle(&mut e);
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(trash.join("a.RAF").is_file());
    assert_eq!(e.document.catalog_photo, None);
    Ok(())
}

#[test]
fn delete_says_what_each_choice_takes_when_some_files_cant_go() -> anyhow::Result<()> {
    let (d, mut e, _trash) = editor(&["a.RAF", "b.RAF"])?;
    let (a, b) = (id(&e, "a.RAF"), id(&e, "b.RAF"));
    // b has an edit, and its file is missing.
    let library = e.library.as_mut().unwrap();
    let edit = crate::app::Recipe {
        exposure: 1.,
        ..Default::default()
    };
    let path = library.photo(b).unwrap().path.clone();
    library.session.save_edit(
        b,
        &path,
        &crate::storage::Identity::read(&path)?,
        &edit,
        &Default::default(),
        crate::catalog::HistoryUpdate::Keep,
    )?;
    std::fs::remove_file(d.path().join("photos/b.RAF"))?;
    select(&mut e, &[a, b]);
    tap(&mut e, Key::Delete, Modifiers::NONE);
    settle(&mut e);
    let (title, lines, actions) = e.file_ops.question().expect("it asks");
    assert_eq!(
        title,
        "Move the 2 selected photos to the Trash, or just remove them from the catalog?"
    );
    assert_eq!(actions, ["Remove from Catalog", "Move to Trash"]);
    assert!(
        lines.iter().any(|l| l == "1 file goes to the Trash."),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|l| l
            .starts_with("1 photo can only be removed from the catalog; Move to Trash leaves it:")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("1 photo has edits in RAWmakase")),
        "the edit removed either way is said: {lines:?}"
    );
    Ok(())
}

/// Trashes the Loupe's photo with ⌘⌥⇧Delete and confirms.
fn trash_in_loupe(e: &mut Editor) {
    tap(e, Key::Delete, TRASH_KEYS);
    settle(e);
    tap(e, Key::Tab, Modifiers::NONE);
    tap(e, Key::Enter, Modifiers::NONE);
    settle(e);
}

#[test]
fn the_loupe_shows_the_next_photo_after_its_photo_is_trashed() -> anyhow::Result<()> {
    let (_d, mut e, trash) = editor(&["a.RAF", "b.RAF", "c.RAF"])?;
    let (a, b, c) = (id(&e, "a.RAF"), id(&e, "b.RAF"), id(&e, "c.RAF"));
    // Several photos selected in the grid, then the Loupe on the middle one:
    // none of the others stands in for it once it is gone.
    select(&mut e, &[a, c]);
    let library = e.library.as_mut().unwrap();
    library.make_active(b);
    library.open_loupe();
    frame(&mut e, Vec::new());
    trash_in_loupe(&mut e);
    assert!(trash.join("b.RAF").is_file());
    let library = e.library.as_ref().unwrap();
    assert!(library.loupe_open(), "the Loupe stays open");
    assert_eq!(library.selected(), Some(c));
    assert_eq!(library.selected_photos(), vec![c]);
    // The last photo trashed: the one before it.
    trash_in_loupe(&mut e);
    assert!(trash.join("c.RAF").is_file());
    let library = e.library.as_ref().unwrap();
    assert!(library.loupe_open(), "the Loupe stays open");
    assert_eq!(library.selected(), Some(a));
    Ok(())
}

/// An editor whose photos are on a drive the Trash says it can't take
/// (#1273).
fn editor_without_trash(names: &[&str]) -> anyhow::Result<(tempfile::TempDir, Editor)> {
    let (d, mut e, _) = editor(names)?;
    e.file_ops.trash = Arc::new(TestTrash {
        dir: None,
        support: TrashSupport::No,
    });
    Ok((d, e))
}

#[test]
fn delete_on_a_drive_without_a_trash_asks_twice_before_deleting_permanently() -> anyhow::Result<()>
{
    let (d, mut e) = editor_without_trash(&["a.RAF", "b.RAF"])?;
    std::fs::write(d.path().join("photos/a.RAF.xmp"), b"sidecar")?;
    let (a, b) = (id(&e, "a.RAF"), id(&e, "b.RAF"));
    select(&mut e, &[a]);
    tap(&mut e, Key::Delete, Modifiers::NONE);
    settle(&mut e);
    let (title, lines, actions) = e.file_ops.question().expect("it asks");
    assert_eq!(
        title,
        "Remove “a.RAF” from the catalog, or delete it permanently?"
    );
    // Remove from Catalog, which loses no file, stays the primary action.
    assert_eq!(actions, ["Delete Permanently…", "Remove from Catalog"]);
    assert!(
        lines.iter().any(|l| l
            == "Delete Permanently… deletes the file of the 1 photo on a drive with no Trash for good, once you confirm."),
        "{lines:?}"
    );
    // Delete Permanently… only asks again.
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    let (title, lines, actions) = e.file_ops.question().expect("it asks again");
    assert_eq!(title, "Delete “a.RAF” permanently?");
    assert_eq!(actions, ["Delete Permanently"]);
    assert_eq!(e.file_ops.warning(), Some(PERMANENT_WARNING));
    assert!(
        lines
            .iter()
            .any(|l| l == "1 file (and 1 sidecar file) will be deleted from disk."),
        "{lines:?}"
    );
    assert!(d.path().join("photos/a.RAF").is_file());
    // Return alone cancels it too: Cancel has the focus.
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.file_ops.confirming());
    assert!(d.path().join("photos/a.RAF").is_file());
    assert_eq!(ids_left(&e).len(), 2);
    // Confirmed twice, it is deleted, with its sidecar.
    tap(&mut e, Key::Delete, Modifiers::NONE);
    settle(&mut e);
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert!(!d.path().join("photos/a.RAF").exists());
    assert!(!d.path().join("photos/a.RAF.xmp").exists());
    assert!(d.path().join("photos/b.RAF").is_file());
    assert_eq!(ids_left(&e), vec![b]);
    assert_eq!(e.messages.line(), "Deleted 1 photo permanently");
    Ok(())
}

#[test]
fn delete_on_a_drive_without_a_trash_can_still_only_remove_from_the_catalog() -> anyhow::Result<()>
{
    let (d, mut e) = editor_without_trash(&["a.RAF", "b.RAF"])?;
    let a = id(&e, "a.RAF");
    select(&mut e, &[a]);
    tap(&mut e, Key::Delete, Modifiers::NONE);
    settle(&mut e);
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(d.path().join("photos/a.RAF").is_file(), "the file stays");
    assert_eq!(ids_left(&e), vec![id(&e, "b.RAF")]);
    assert_eq!(e.messages.line(), "Removed 1 photo from the catalog");
    Ok(())
}

#[test]
fn move_to_trash_on_a_drive_without_a_trash_asks_to_delete_permanently() -> anyhow::Result<()> {
    let (d, mut e) = editor_without_trash(&["a.RAF", "b.RAF"])?;
    let (a, b) = (id(&e, "a.RAF"), id(&e, "b.RAF"));
    select(&mut e, &[a, b]);
    tap(&mut e, Key::Delete, TRASH_KEYS);
    settle(&mut e);
    let (title, _, actions) = e.file_ops.question().expect("it asks");
    assert_eq!(title, "Delete 2 photos permanently?");
    assert_eq!(actions, ["Delete Permanently"]);
    assert_eq!(e.file_ops.warning(), Some(PERMANENT_WARNING));
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!d.path().join("photos/a.RAF").exists());
    assert!(!d.path().join("photos/b.RAF").exists());
    assert!(ids_left(&e).is_empty());
    assert_eq!(e.messages.line(), "Deleted 2 photos permanently");
    Ok(())
}

#[test]
fn photos_a_trash_takes_are_never_offered_for_deleting_permanently() -> anyhow::Result<()> {
    let (_d, mut e, _trash) = editor(&["a.RAF"])?;
    let a = id(&e, "a.RAF");
    select(&mut e, &[a]);
    for keys in [Modifiers::NONE, TRASH_KEYS] {
        tap(&mut e, Key::Delete, keys);
        settle(&mut e);
        let (_, _, actions) = e.file_ops.question().expect("it asks");
        assert!(!actions.contains(&"Delete Permanently…"), "{actions:?}");
        assert_eq!(e.file_ops.warning(), None);
        tap(&mut e, Key::Escape, Modifiers::NONE);
        settle(&mut e);
    }
    Ok(())
}

/// Delete's question for photos on a drive with no Trash, and Delete
/// Permanently's own, for `RAWMAKASE_UI_SHOTS` ([`crate::app::ui::shots`]).
#[test]
fn shots_of_delete_permanently() -> anyhow::Result<()> {
    let d = tempfile::tempdir()?;
    let photos = d.path().join("photos");
    std::fs::create_dir_all(&photos)?;
    let names = [
        "DSCF0001.RAF",
        "DSCF0002.RAF",
        "DSCF0003.RAF",
        "DSCF0004.RAF",
    ];
    for name in names {
        std::fs::write(photos.join(name), format!("fixture {name}"))?;
    }
    std::fs::write(photos.join("DSCF0001.xmp"), b"sidecar")?;
    let path = d.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&path)?.add_folder(&photos)?;
    let no_trash = TestTrash {
        dir: None,
        support: TrashSupport::No,
    };
    type Ask = fn(Vec<PhotoId>) -> (Option<FileOp>, FileOp, Option<FileOp>, Verb);
    let cases: [(&str, usize, Ask); 3] = [
        ("delete-no-trash-one", 1, |photos| {
            (
                None,
                FileOp::RemoveFromCatalog {
                    photos: photos.clone(),
                },
                Some(FileOp::DeletePhotos { photos }),
                Verb::Delete { rejected: false },
            )
        }),
        ("delete-no-trash-rejected", 4, |photos| {
            (
                None,
                FileOp::RemoveFromCatalog {
                    photos: photos.clone(),
                },
                Some(FileOp::DeletePhotos { photos }),
                Verb::Delete { rejected: true },
            )
        }),
        ("delete-permanently", 4, |photos| {
            (
                None,
                FileOp::DeletePhotos { photos },
                None,
                Verb::DeletePermanently,
            )
        }),
    ];
    for (name, n, ask) in cases {
        let path = path.clone();
        let trash = Arc::new(TestTrash {
            dir: None,
            support: no_trash.support,
        });
        crate::app::ui::shots::shot(
            &format!("file-ops-{name}"),
            eframe::egui::vec2(900., 560.),
            move |e, ctx| {
                let library = crate::app::library::Library::load(&path, ctx.clone()).unwrap();
                let ids: Vec<_> = library
                    .session
                    .photos()
                    .iter()
                    .map(|p| p.id)
                    .take(n)
                    .collect();
                let catalog = library.session.catalog();
                let (trash_op, op, permanent, verb) = ask(ids);
                let planned = |op| file_ops::plan_with_trash(catalog, op, &*trash).unwrap();
                let plan = planned(op);
                let trash_plan = trash_op.map(planned);
                let permanent = permanent.map(planned);
                let location = catalog.location().clone();
                let generation = library.session.generation();
                e.library = Some(Box::new(library));
                e.module = crate::app::Module::Library;
                e.file_ops.state = State::Confirming {
                    plan: Box::new(plan),
                    trash: trash_plan.map(Box::new),
                    permanent: permanent.map(Box::new),
                    catalog: location,
                    generation,
                    verb,
                };
            },
            |e, ctx| e.file_op_window(ctx),
        );
    }
    Ok(())
}
