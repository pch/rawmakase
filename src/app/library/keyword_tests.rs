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
    assert_eq!(names(library, ids[1]), ["Paris < Places"]);
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
    assert_eq!(names(library, ids[1]), ["Paris < Places"]);
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

#[test]
fn option_digits_apply_a_slot_but_nothing_runs_behind_a_keyword_window() -> Result<()> {
    use eframe::egui::{Key, Modifiers};
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    let library = e.library.as_mut().unwrap();
    library.select(Some(ids[0]));
    let mut slots: [String; 9] = Default::default();
    slots[0] = "Sea".into();
    library.keyword_sets.shown = Shown::Edited {
        from: None,
        slots: Box::new(slots),
    };
    press(&mut e, Key::Num1, Modifiers::ALT);
    assert_eq!(names(e.library.as_ref().unwrap(), ids[0]), ["Sea"]);
    // Behind Delete's question, neither the slot's key nor Undo runs.
    let library = e.library.as_mut().unwrap();
    let sea = library
        .session
        .catalog()
        .find_typed_keyword(&["Sea".into()])?
        .unwrap();
    library.ask_to_delete_keyword(sea);
    assert!(e.keyword_dialog_open());
    press(&mut e, Key::Num1, Modifiers::ALT);
    press(&mut e, Key::Z, Modifiers::COMMAND);
    assert_eq!(names(e.library.as_ref().unwrap(), ids[0]), ["Sea"]);
    assert!(e.keyword_dialog_open());
    Ok(())
}

#[test]
fn the_arrow_shows_one_keyword_s_photos_beside_another_of_its_name() -> Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    let library = e.library.as_mut().unwrap();
    library.edit_descriptive(
        &ids[..1],
        DescriptiveEdit::AddKeywords(vec![vec!["France".into(), "Paris".into()]]),
    )?;
    library.edit_descriptive(
        &ids[1..],
        DescriptiveEdit::AddKeywords(vec![vec!["Texas".into(), "Paris".into()]]),
    )?;
    // Named apart on both photos.
    assert_eq!(names(library, ids[0]), ["Paris < France"]);
    assert_eq!(names(library, ids[1]), ["Paris < Texas"]);
    let texas_paris = library
        .session
        .catalog()
        .find_typed_keyword(&["Texas".into(), "Paris".into()])?
        .unwrap();
    library.show_keyword(texas_paris);
    assert_eq!(library.shown(), [ids[1]]);
    // Deleting the other leaves this one's name, and the filter, as they were.
    let france = library
        .session
        .catalog()
        .find_typed_keyword(&["France".into()])?
        .unwrap();
    library.edit_keyword_list(KeywordListEdit::Delete(vec![france]), "Delete Keyword");
    assert_eq!(names(library, ids[1]), ["Paris < Texas"]);
    assert_eq!(library.shown(), [ids[1]]);
    // And back, with Undo.
    e.sync_undo();
    e.undo();
    let library = e.library.as_ref().unwrap();
    assert_eq!(names(library, ids[1]), ["Paris < Texas"]);
    assert_eq!(library.shown(), [ids[1]]);
    Ok(())
}

#[test]
fn a_keyword_kept_chosen_by_its_name_alone_chooses_every_keyword_of_it() -> Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF"])?;
    let library = e.library.as_mut().unwrap();
    for (id, parent) in [(ids[0], "France"), (ids[1], "Texas")] {
        library.edit_descriptive(
            &[id],
            DescriptiveEdit::AddKeywords(vec![vec![parent.into(), "Paris".into()]]),
        )?;
    }
    // A top-level Paris too, on the third photo.
    library
        .session
        .catalog_mut_for_tests()
        .create_keyword("Paris", None)?;
    let top = library.session.catalog().keyword_list()?;
    let top = top
        .iter()
        .find(|k| k.name == "Paris" && k.parent.is_none())
        .unwrap()
        .id;
    library.edit_descriptive(&ids[2..], DescriptiveEdit::AddKeyword(top))?;
    // As a layout an earlier release kept: the name alone.
    let mut layout = library.layout();
    layout.keyword_paths = false;
    layout
        .metadata
        .insert("keyword".into(), vec![Some("Paris".into())]);
    library.apply_layout(&layout);
    let mut shown = library.shown();
    shown.sort();
    let mut all = ids.clone();
    all.sort();
    assert_eq!(shown, all);
    // A layout kept since chooses the top-level one alone.
    let mut layout = library.layout();
    layout
        .metadata
        .insert("keyword".into(), vec![Some("Paris".into())]);
    library.apply_layout(&layout);
    assert_eq!(library.shown(), [ids[2]]);
    Ok(())
}

#[test]
fn undoing_a_rename_keeps_the_keyword_chosen_in_the_filter() -> Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    let library = e.library.as_mut().unwrap();
    library.edit_descriptive(
        &ids[..1],
        DescriptiveEdit::AddKeywords(vec![vec!["Sea".into()]]),
    )?;
    let sea = library
        .session
        .catalog()
        .find_typed_keyword(&["Sea".into()])?
        .unwrap();
    library.show_keyword(sea);
    assert_eq!(library.shown(), [ids[0]]);
    e.sync_undo();
    let library = e.library.as_mut().unwrap();
    library.edit_keyword_list(
        KeywordListEdit::Rename {
            id: sea,
            name: "Ocean".into(),
        },
        "Rename Keyword",
    );
    assert_eq!(library.shown(), [ids[0]]);
    e.sync_undo();
    e.undo();
    assert_eq!(e.library.as_ref().unwrap().shown(), [ids[0]]);
    e.redo();
    assert_eq!(e.library.as_ref().unwrap().shown(), [ids[0]]);
    Ok(())
}
