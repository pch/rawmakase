//! Issue 998: smart collections in the Library: live members, the panel's
//! counts, the reason an unsupported one lists no photos, the synonyms
//! notice, and bulk actions waiting while members are worked out.
use super::collections::SmartCount;
use super::smart::SmartNote;
use super::*;
use crate::catalog::{Catalog, CollectionId, SmartRules};
use crate::collection_rules::{Combine, Group, Item, Rule, lua::Value};
use anyhow::Result;

/// A Library of three photos and smart collections `rules` (name, rules or
/// Lightroom text), made as an import makes them.
fn library_with(
    smart: &[(&str, Result<Group, &str>)],
) -> Result<(tempfile::TempDir, Library, Vec<CollectionId>)> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    for name in ["a.RAF", "b.RAF", "c.RAF"] {
        std::fs::write(folder.join(name), b"synthetic raw")?;
    }
    let path = directory.path().join("library.rawmakase");
    let mut catalog = Catalog::create(&path)?;
    catalog.add_folder(&folder)?;
    let mut ids = Vec::new();
    for (name, rules) in smart {
        let id: i64 = catalog.db_for_tests().query_row(
            "INSERT INTO collections(name, parent, kind)
             VALUES (?, NULL, 'com.adobe.ag.library.smart_collection') RETURNING id",
            [name],
            |r| r.get(0),
        )?;
        let id = CollectionId(id);
        let stored = match rules {
            Ok(group) => SmartRules::new(group),
            Err(text) => SmartRules::from_lightroom(text),
        };
        catalog.set_smart_rules(id, &stored)?;
        ids.push(id);
    }
    drop(catalog);
    let library = Library::load(&path, egui::Context::default())?;
    Ok((directory, library, ids))
}

fn rule(criteria: &str, operation: &str, value: Value) -> Group {
    Group::new(
        Combine::Intersect,
        vec![Item::Rule(Rule::new(criteria, operation, Some(value)))],
    )
}

/// Polls until the smart collections' members for this revision are taken.
fn settle(library: &mut Library) {
    let started = std::time::Instant::now();
    loop {
        library.poll_smart();
        if library
            .smart_counts()
            .values()
            .all(|c| *c != SmartCount::Pending)
            && library.source_settled()
            && !library.smart_counts().is_empty()
        {
            return;
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

fn shown(library: &Library) -> Vec<PhotoId> {
    let mut ids: Vec<PhotoId> = library
        .visible
        .iter()
        .map(|i| library.session.photos()[*i].id)
        .collect();
    ids.sort();
    ids
}

#[test]
fn a_smart_collection_shows_its_members_and_follows_changes() -> Result<()> {
    let (_d, mut library, smart) =
        library_with(&[("Picks", Ok(rule("pick", "==", Value::Number(1.))))])?;
    let photos: Vec<PhotoId> = library.session.photos().iter().map(|p| p.id).collect();
    settle(&mut library);
    assert_eq!(library.smart_counts()[&smart[0]], SmartCount::Photos(0));
    library.select_collection(smart[0]);
    assert!(shown(&library).is_empty());
    library
        .session
        .set_ratings(&[(photos[1], 0, 1, String::new())])?;
    // Until the members are worked out again, the smart collection says so
    // and Select All waits.
    assert_eq!(library.smart_note(), Some(SmartNote::Updating));
    library.select_all();
    assert!(library.selected_ids().is_empty());
    settle(&mut library);
    assert_eq!(shown(&library), [photos[1]]);
    assert_eq!(library.smart_note(), None);
    assert_eq!(library.smart_counts()[&smart[0]], SmartCount::Photos(1));
    library.select_all();
    assert_eq!(library.selected_ids(), [photos[1]]);
    // Edits written on another connection reach it through the one funnel.
    library
        .session
        .catalog_mut_for_tests()
        .set_metadata(photos[2], 0, 1, "")?;
    library.edits_written(&[photos[2]]);
    library.reload()?;
    settle(&mut library);
    assert_eq!(shown(&library), [photos[1], photos[2]]);
    Ok(())
}

#[test]
fn an_unsupported_smart_collection_lists_no_photos_and_says_why() -> Result<()> {
    let (_d, mut library, smart) = library_with(&[
        ("Liked", Ok(rule("likes", "isTrue", Value::Bool(true)))),
        ("Broken", Err("s = { os.exit() }")),
    ])?;
    settle(&mut library);
    for id in &smart {
        assert_eq!(library.smart_counts()[id], SmartCount::Unknown);
        library.select_collection(*id);
        assert!(shown(&library).is_empty());
        let Some(SmartNote::Unsupported(why)) = library.smart_note() else {
            panic!("{:?}", library.smart_note());
        };
        assert!(!why.is_empty());
    }
    Ok(())
}

#[test]
fn keyword_rules_say_when_synonyms_are_missing_until_dismissed() -> Result<()> {
    let (_d, mut library, smart) = library_with(&[
        (
            "Japan",
            Ok(rule("keywords", "any", Value::Str("japan".into()))),
        ),
        ("Rated", Ok(rule("rating", ">=", Value::Number(1.)))),
    ])?;
    library
        .session
        .catalog_mut_for_tests()
        .set_synonym_state(crate::catalog::SynonymState::Missing)?;
    library.reload()?;
    settle(&mut library);
    library.select_collection(smart[1]);
    assert_eq!(library.smart_note(), None);
    library.select_collection(smart[0]);
    assert_eq!(library.smart_note(), Some(SmartNote::SynonymsMissing));
    library.dismiss_missing_synonyms();
    assert_eq!(library.smart_note(), None);
    assert_eq!(
        library.session.catalog().synonym_state()?,
        crate::catalog::SynonymState::Complete
    );
    Ok(())
}

#[test]
fn an_import_says_how_many_smart_collections_list_no_photos() -> Result<()> {
    let (_d, mut library, _) = library_with(&[
        ("Liked", Ok(rule("likes", "isTrue", Value::Bool(true)))),
        ("Broken", Err("s = { os.exit() }")),
        ("Rated", Ok(rule("rating", ">=", Value::Number(1.)))),
    ])?;
    library.note_unsupported_smart();
    assert_eq!(
        library.messages.line(),
        "2 smart collections have rules RAWmakase can't evaluate; they list no photos until those rules are removed"
    );
    Ok(())
}
