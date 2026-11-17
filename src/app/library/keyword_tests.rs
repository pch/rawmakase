//! The Library's keyword tools: suggestions picked by id, the Keyword
//! List's boxes and edits, and the Keyword Set's buttons, at the Library's
//! and the undo log's boundary.
use super::descriptive::DescriptiveEdit;
use super::keyword_sets::{KEYPAD, Shown, Slot};
use crate::app::testing::*;
use crate::catalog::PhotoId;
use crate::catalog_session::KeywordListEdit;
use anyhow::Result;

fn names(library: &super::Library, id: PhotoId) -> Vec<String> {
    library
        .session
        .photos()
        .iter()
        .find(|p| p.id == id)
        .map(|p| p.keywords.clone())
        .unwrap_or_default()
}

fn listed(library: &super::Library) -> Vec<String> {
    library
        .session
        .keywords()
        .iter()
        .map(|k| k.name.clone())
        .collect()
}

#[test]
fn a_keyword_chosen_by_id_is_that_one_among_duplicates() -> Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    let library = e.library.as_mut().unwrap();
    library.session.catalog().db_for_tests().execute_batch(
        "INSERT INTO keywords(id, name, parent) VALUES (1, 'A', NULL), (2, 'Dup', 1), (3, 'B', NULL), (4, 'Dup', 3);",
    )?;
    library.session.reload()?;
    library.select(Some(ids[0]));
    library.sync_fields();
    library.edit_descriptive(&ids, DescriptiveEdit::AddKeyword(4))?;
    let tagged: Vec<i64> = library
        .session
        .catalog()
        .keywords(ids[0])?
        .into_iter()
        .map(|k| k.id)
        .collect();
    assert_eq!(tagged, [4]);
    // The session's list follows: its count, and Recent Keywords.
    let dup = library
        .session
        .keywords()
        .iter()
        .find(|k| k.id == 4)
        .unwrap();
    assert_eq!(dup.photos, 1);
    assert_eq!(library.session.recent_keywords()[0], 4);
    Ok(())
}

#[test]
fn deleting_a_keyword_and_its_children_is_one_step_undo_puts_back() -> Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    let library = e.library.as_mut().unwrap();
    library.edit_descriptive(
        &ids,
        DescriptiveEdit::AddKeywords(vec![vec!["Places".into(), "Paris".into()]]),
    )?;
    let places = library
        .session
        .catalog()
        .find_typed_keyword(&["Places".into()])?
        .unwrap();
    e.sync_undo();
    let library = e.library.as_mut().unwrap();
    library.edit_keyword_list(KeywordListEdit::Delete(vec![places]), "Delete Keyword");
    assert!(listed(library).is_empty());
    assert!(names(library, ids[0]).is_empty());
    e.sync_undo();
    assert_eq!(e.undo_log.len(), (2, 0));
    e.undo();
    let library = e.library.as_ref().unwrap();
    assert_eq!(listed(library), ["Places", "Paris"]);
    assert_eq!(names(library, ids[1]), ["Paris"]);
    assert!(e.messages.line().starts_with("Undo Delete Keyword"));
    e.redo();
    let library = e.library.as_ref().unwrap();
    assert!(listed(library).is_empty());
    assert!(names(library, ids[0]).is_empty());
    // And back again, then the keyword itself is undone.
    e.undo();
    e.undo();
    let library = e.library.as_ref().unwrap();
    assert!(names(library, ids[0]).is_empty());
    Ok(())
}

#[test]
fn rename_create_and_purge_are_each_undone() -> Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    let library = e.library.as_mut().unwrap();
    library.edit_descriptive(&ids, DescriptiveEdit::AddKeywords(vec![vec!["Sea".into()]]))?;
    let sea = library
        .session
        .catalog()
        .find_typed_keyword(&["Sea".into()])?
        .unwrap();
    library.edit_keyword_list(
        KeywordListEdit::Rename {
            id: sea,
            name: "Ocean".into(),
        },
        "Rename Keyword",
    );
    assert_eq!(names(library, ids[0]), ["Ocean"]);
    library.edit_keyword_list(
        KeywordListEdit::Create {
            name: "Unused".into(),
            parent: None,
        },
        "Create Keyword",
    );
    // A name its parent has is refused, nothing merged.
    library.edit_keyword_list(
        KeywordListEdit::Create {
            name: "ocean".into(),
            parent: None,
        },
        "Create Keyword",
    );
    assert!(library.messages.line().contains("is already taken"));
    library.edit_keyword_list(KeywordListEdit::PurgeUnused, "Purge Unused Keywords");
    assert_eq!(listed(library), ["Ocean"]);
    e.sync_undo();
    e.undo();
    assert_eq!(listed(e.library.as_ref().unwrap()), ["Ocean", "Unused"]);
    e.undo();
    assert_eq!(listed(e.library.as_ref().unwrap()), ["Ocean"]);
    e.undo();
    let library = e.library.as_ref().unwrap();
    assert_eq!(names(library, ids[0]), ["Sea"]);
    Ok(())
}

#[test]
fn a_set_button_finds_its_keyword_as_typed_and_toggles_it() -> Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    let library = e.library.as_mut().unwrap();
    library.edit_descriptive(
        &ids[..1],
        DescriptiveEdit::AddKeywords(vec![vec!["Places".into(), "Paris".into()]]),
    )?;
    let paris = library
        .session
        .catalog()
        .find_typed_keyword(&["paris".into()])?
        .unwrap();
    let mut slots: [String; 9] = Default::default();
    slots[0] = "paris".into();
    slots[1] = "New One".into();
    library.keyword_sets.shown = Shown::Edited {
        from: None,
        slots: Box::new(slots),
    };
    library.select(Some(ids[0]));
    library.select_range_to(ids[1]);
    library.sync_fields();
    assert!(matches!(
        &library.keyword_slots()[0],
        Slot::Typed { found: Some(id), .. } if *id == paris
    ));
    // Only one photo has it: added to both, no new keyword made.
    library.click_keyword_slot(0);
    assert_eq!(names(library, ids[1]), ["Paris"]);
    assert_eq!(listed(library), ["Places", "Paris"]);
    // All have it: removed from both.
    library.sync_fields();
    library.click_keyword_slot(0);
    assert!(names(library, ids[0]).is_empty() && names(library, ids[1]).is_empty());
    // A keyword the catalog lacks is made, as typed.
    library.sync_fields();
    library.click_keyword_slot(1);
    assert_eq!(names(library, ids[0]), ["New One"]);
    // An empty slot does nothing.
    library.click_keyword_slot(2);
    Ok(())
}

#[test]
fn recent_keywords_fill_the_top_row_first_and_toggle_by_id() -> Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    let library = e.library.as_mut().unwrap();
    library.select(Some(ids[0]));
    for name in ["One", "Two"] {
        library.edit_descriptive(&ids, DescriptiveEdit::AddKeywords(vec![vec![name.into()]]))?;
    }
    library.keyword_sets.shown = Shown::Recent;
    let slots = library.keyword_slots();
    let label = |slot: &Slot| match slot {
        Slot::Keyword { label, .. } => label.clone(),
        _ => String::new(),
    };
    assert_eq!(label(&slots[KEYPAD[0]]), "Two");
    assert_eq!(label(&slots[KEYPAD[1]]), "One");
    library.sync_fields();
    library.click_keyword_slot(KEYPAD[0]);
    assert_eq!(names(library, ids[0]), ["One"]);
    Ok(())
}
