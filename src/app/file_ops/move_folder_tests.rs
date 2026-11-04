//! Moving a folder in the app: into another folder of its root, or to the
//! root itself, confirmed with Cancel focused; driven by keys and frames on
//! a catalog of synthetic files in a temporary folder.
use super::move_tests::{at, editor_with_folders, folder, photo, root};
use super::tests::{settle, tap};
use crate::app::{Editor, Module};
use crate::catalog::RootId;
use eframe::egui::{Key, Modifiers};

#[test]
fn a_folder_moves_into_another_with_its_photos_once_confirmed() -> anyhow::Result<()> {
    let (d, mut e, _) =
        editor_with_folders(&["2024/day1/a.RAF", "2024/day1/sub/b.RAF", "2025/c.RAF"])?;
    let (a, b) = (
        photo(&e, &d, "2024/day1/a.RAF"),
        photo(&e, &d, "2024/day1/sub/b.RAF"),
    );
    let day1 = folder(&e, "2024/day1").unwrap();
    let r = root(&e);
    e.request_folder_move(r, "2024/day1".into(), r, "2025".into());
    settle(&mut e);
    let (title, lines, actions) = e.file_ops.question().expect("it asks");
    assert_eq!(title, "Move folder “day1” into “2025”?");
    assert_eq!(actions, ["Move"]);
    assert!(
        lines
            .iter()
            .any(|l| l == "The folder moves on disk with the 2 photos and the folders in it."),
        "{lines:?}"
    );
    // Return alone cancels.
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(d.path().join("photos/2024/day1/a.RAF").is_file());
    e.request_folder_move(r, "2024/day1".into(), r, "2025".into());
    settle(&mut e);
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert!(d.path().join("photos/2025/day1/sub/b.RAF").is_file());
    assert!(!d.path().join("photos/2024/day1").exists());
    assert_eq!(at(&e, &d, a), "2025/day1/a.RAF");
    assert_eq!(at(&e, &d, b), "2025/day1/sub/b.RAF");
    assert_eq!(folder(&e, "2025/day1"), Some(day1), "the same folder");
    assert_eq!(e.messages.line(), "Moved folder day1 into 2025");
    Ok(())
}

#[test]
fn a_folder_moves_to_its_root() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["2024/day1/a.RAF"])?;
    let day1 = folder(&e, "2024/day1").unwrap();
    let r = root(&e);
    e.request_folder_move(r, "2024/day1".into(), r, String::new());
    settle(&mut e);
    let (title, ..) = e.file_ops.question().expect("it asks");
    assert_eq!(title, "Move folder “day1” into “photos”?");
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(d.path().join("photos/day1/a.RAF").is_file());
    assert_eq!(folder(&e, "day1"), Some(day1));
    Ok(())
}

#[test]
fn a_folder_into_itself_is_refused_and_says_why() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["2024/day1/a.RAF", "2024/day1/sub/b.RAF"])?;
    let r = root(&e);
    e.request_folder_move(r, "2024/day1".into(), r, "2024/day1/sub".into());
    settle(&mut e);
    assert!(!e.file_ops.confirming());
    assert!(!e.activity.is_busy());
    assert!(
        e.messages.line().contains("into itself"),
        "{}",
        e.messages.line()
    );
    assert!(d.path().join("photos/2024/day1/sub/b.RAF").is_file());
    Ok(())
}

#[test]
fn an_empty_folder_moved_is_shown_where_it_went() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["2025/c.RAF"])?;
    let r = root(&e);
    e.name_new_folder(r, String::new(), "photos".into(), Vec::new(), false);
    super::tests::frame(&mut e, Vec::new());
    super::move_tests::type_text(&mut e, "empty");
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    let empty = folder(&e, "empty").expect("made");
    e.request_folder_move(r, "empty".into(), r, "2025".into());
    settle(&mut e);
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(d.path().join("photos/2025/empty").is_dir());
    assert_eq!(folder(&e, "2025/empty"), Some(empty));
    assert!(folder(&e, "empty").is_none());
    Ok(())
}

#[test]
fn move_folder_offers_folders_with_no_catalog_row_of_their_own() -> anyhow::Result<()> {
    let (_d, mut e, _) = editor_with_folders(&["2024/day1/a.RAF", "2025/day2/b.RAF"])?;
    // 2025 has no folder row: only 2025/day2 has photos.
    assert!(folder(&e, "2025").is_none());
    let r = root(&e);
    e.choose_parent_for(r, "2024/day1".into());
    let offered = e.parent_choices();
    let has = |l: &str| offered.contains(&(r, l.to_string()));
    assert!(has("2025"), "{offered:?}");
    assert!(has("2025/day2"));
    assert!(has(""), "its root");
    // Not where it is, nor itself.
    assert!(!has("2024"));
    assert!(!has("2024/day1"));
    Ok(())
}

#[test]
fn a_folder_with_no_catalog_folder_of_its_own_moves_with_those_below() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["2024/day1/a.RAF", "2024/day2/b.RAF", "2025/c.RAF"])?;
    assert!(folder(&e, "2024").is_none());
    let day1 = folder(&e, "2024/day1").unwrap();
    let a = photo(&e, &d, "2024/day1/a.RAF");
    let r = root(&e);
    e.request_folder_move(r, "2024".into(), r, "2025".into());
    settle(&mut e);
    let (title, lines, _) = e.file_ops.question().expect("it asks");
    assert_eq!(title, "Move folder “2024” into “2025”?");
    assert!(
        lines
            .iter()
            .any(|l| l == "The folder moves on disk with the 2 photos and the folders in it."),
        "{lines:?}"
    );
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(d.path().join("photos/2025/2024/day2/b.RAF").is_file());
    assert_eq!(at(&e, &d, a), "2025/2024/day1/a.RAF");
    assert_eq!(folder(&e, "2025/2024/day1"), Some(day1), "the same folder");
    assert_eq!(e.messages.line(), "Moved folder 2024 into 2025");
    Ok(())
}

/// An editor over `photos/` holding `names` and a second root `elsewhere/`
/// beside it holding `others`, with both roots' ids.
fn editor_with_two_roots(
    names: &[&str],
    others: &[&str],
) -> anyhow::Result<(tempfile::TempDir, Editor, RootId, RootId)> {
    let d = tempfile::tempdir()?;
    for (dir, list) in [("photos", names), ("elsewhere", others)] {
        for name in list {
            let path = d.path().join(dir).join(name);
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(&path, format!("fixture {dir} {name}"))?;
        }
    }
    let path = d.path().join("test.rawmakase");
    let mut catalog = crate::catalog::Catalog::create(&path)?;
    catalog.add_folder(&d.path().join("photos"))?;
    catalog.add_folder(&d.path().join("elsewhere"))?;
    let roots = catalog.roots()?;
    let id = |dir: &str| {
        let wanted = d.path().join(dir);
        roots
            .iter()
            .find(|(_, original, _)| {
                std::path::Path::new(original).canonicalize().ok() == wanted.canonicalize().ok()
            })
            .map(|r| r.0)
            .unwrap()
    };
    let (mine, other) = (id("photos"), id("elsewhere"));
    drop(catalog);
    let ctx = eframe::egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.library = Some(Box::new(crate::app::library::Library::load(&path, ctx)?));
    e.module = Module::Library;
    Ok((d, e, mine, other))
}

#[test]
fn a_folder_moves_into_another_roots_folder_once_confirmed() -> anyhow::Result<()> {
    let (d, mut e, mine, other) = editor_with_two_roots(
        &["2024/day1/a.RAF", "2024/day1/sub/b.RAF"],
        &["trips/c.RAF"],
    )?;
    let day1 = folder(&e, "2024/day1").unwrap();
    let a = photo(&e, &d, "2024/day1/a.RAF");
    e.request_folder_move(mine, "2024/day1".into(), other, "trips".into());
    settle(&mut e);
    let (title, ..) = e.file_ops.question().expect("it asks");
    assert_eq!(title, "Move folder “day1” into “trips”?");
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert!(d.path().join("elsewhere/trips/day1/sub/b.RAF").is_file());
    assert!(!d.path().join("photos/2024/day1").exists());
    let now = &e.library.as_ref().unwrap().photo(a).unwrap().path;
    assert_eq!(
        now.canonicalize()?,
        d.path().join("elsewhere/trips/day1/a.RAF").canonicalize()?
    );
    assert_eq!(folder(&e, "trips/day1"), Some(day1), "the same folder");
    assert_eq!(e.messages.line(), "Moved folder day1 into trips");
    Ok(())
}

#[test]
fn move_folder_offers_every_roots_folders() -> anyhow::Result<()> {
    let (_d, mut e, mine, other) = editor_with_two_roots(&["2024/day1/a.RAF"], &["trips/c.RAF"])?;
    e.choose_parent_for(mine, "2024/day1".into());
    let offered = e.parent_choices();
    assert!(offered.contains(&(other, "trips".into())), "{offered:?}");
    assert!(offered.contains(&(other, String::new())), "its root");
    assert!(offered.contains(&(mine, String::new())));
    assert!(!offered.contains(&(mine, "2024".into())), "where it is");
    Ok(())
}

#[test]
fn move_folder_says_where_each_row_is_even_one_located_on_its_own() -> anyhow::Result<()> {
    let (d, mut e, mine, other) =
        editor_with_two_roots(&["2024/day1/a.RAF"], &["trips/day/c.RAF"])?;
    // `trips/day` of the other root is located on its own, elsewhere.
    let away = d.path().join("away");
    std::fs::create_dir(&away)?;
    std::fs::rename(d.path().join("elsewhere/trips/day"), away.join("day"))?;
    let path = d.path().join("test.rawmakase");
    let day = folder(&e, "trips/day").unwrap();
    crate::catalog::Catalog::open(&path)?.relink_folder(day, &away.join("day"))?;
    let ctx = eframe::egui::Context::default();
    e.library = Some(Box::new(crate::app::library::Library::load(&path, ctx)?));
    e.choose_parent_for(mine, "2024/day1".into());
    let same = |p: Option<std::path::PathBuf>, want: std::path::PathBuf| {
        p.and_then(|p| p.canonicalize().ok()) == want.canonicalize().ok()
    };
    assert!(same(e.parent_path(other, "trips/day"), away.join("day")));
    assert!(same(
        e.parent_path(other, "trips"),
        d.path().join("elsewhere/trips")
    ));
    assert!(same(e.parent_path(other, ""), d.path().join("elsewhere")));
    Ok(())
}

#[test]
fn a_root_moves_into_another_roots_folder_once_confirmed() -> anyhow::Result<()> {
    let (d, mut e, mine, other) = editor_with_two_roots(
        &["a.RAF", "2024/day1/b.RAF", "2024/day1/b.xmp"],
        &["trips/c.RAF"],
    )?;
    let day1 = folder(&e, "2024/day1").unwrap();
    let b = photo(&e, &d, "2024/day1/b.RAF");
    e.request_folder_move(mine, String::new(), other, "trips".into());
    settle(&mut e);
    let (title, lines, actions) = e.file_ops.question().expect("it asks");
    assert_eq!(title, "Move folder “photos” into “trips”?");
    assert_eq!(actions, ["Move"]);
    assert!(
        lines
            .iter()
            .any(|l| l == "The folder moves on disk with the 2 photos and the folders in it."),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(
            |l| l == "“photos” is no longer a root folder: it becomes a folder of “elsewhere”."
        ),
        "{lines:?}"
    );
    // Return alone cancels.
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(d.path().join("photos/2024/day1/b.RAF").is_file());
    e.request_folder_move(mine, String::new(), other, "trips".into());
    settle(&mut e);
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert!(
        d.path()
            .join("elsewhere/trips/photos/2024/day1/b.xmp")
            .is_file()
    );
    assert!(!d.path().join("photos").exists());
    let now = &e.library.as_ref().unwrap().photo(b).unwrap().path;
    assert_eq!(
        now.canonicalize()?,
        d.path()
            .join("elsewhere/trips/photos/2024/day1/b.RAF")
            .canonicalize()?
    );
    assert_eq!(
        folder(&e, "trips/photos/2024/day1"),
        Some(day1),
        "the same folder"
    );
    let roots: Vec<RootId> = e
        .library
        .as_ref()
        .unwrap()
        .session
        .roots()
        .iter()
        .map(|r| r.0)
        .collect();
    assert_eq!(roots, [other], "the root left");
    assert_eq!(e.messages.line(), "Moved folder photos into trips");
    Ok(())
}

#[test]
fn a_root_into_itself_is_refused_and_says_why() -> anyhow::Result<()> {
    let (d, mut e, mine, _) = editor_with_two_roots(&["2024/day1/a.RAF"], &["trips/c.RAF"])?;
    e.request_folder_move(mine, String::new(), mine, "2024".into());
    settle(&mut e);
    assert!(!e.file_ops.confirming());
    assert!(!e.activity.is_busy());
    assert!(
        e.messages.line().contains("into itself"),
        "{}",
        e.messages.line()
    );
    assert!(d.path().join("photos/2024/day1/a.RAF").is_file());
    Ok(())
}

#[test]
fn move_folder_offers_a_root_only_other_roots_and_their_folders() -> anyhow::Result<()> {
    let (_d, mut e, mine, other) = editor_with_two_roots(&["2024/day1/a.RAF"], &["trips/c.RAF"])?;
    e.choose_parent_for(mine, String::new());
    let offered = e.parent_choices();
    assert!(offered.contains(&(other, "trips".into())), "{offered:?}");
    assert!(offered.contains(&(other, String::new())), "{offered:?}");
    assert!(offered.iter().all(|(r, _)| *r == other), "{offered:?}");
    Ok(())
}

#[test]
fn a_root_located_again_under_a_new_name_is_followed_by_that_name() -> anyhow::Result<()> {
    let (d, mut e, mine, other) = editor_with_two_roots(&["2024/day1/a.RAF"], &["trips/c.RAF"])?;
    // Renamed in Finder, then located again: added as `photos`, `renamed`
    // here.
    std::fs::rename(d.path().join("photos"), d.path().join("renamed"))?;
    let path = d.path().join("test.rawmakase");
    crate::catalog::Catalog::open(&path)?.relink_root(mine, &d.path().join("renamed"))?;
    let ctx = eframe::egui::Context::default();
    e.library = Some(Box::new(crate::app::library::Library::load(&path, ctx)?));
    e.library
        .as_mut()
        .unwrap()
        .click_folder_for_tests(&format!("root:{mine}/2024/day1"));
    e.request_folder_move(mine, String::new(), other, "trips".into());
    settle(&mut e);
    let (title, ..) = e.file_ops.question().expect("it asks");
    assert_eq!(title, "Move folder “renamed” into “trips”?");
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(
        d.path()
            .join("elsewhere/trips/renamed/2024/day1/a.RAF")
            .is_file()
    );
    assert_eq!(
        e.library.as_ref().unwrap().source_key(),
        format!("root:{other}/trips/renamed/2024/day1")
    );
    assert_eq!(e.messages.line(), "Moved folder renamed into trips");
    Ok(())
}
