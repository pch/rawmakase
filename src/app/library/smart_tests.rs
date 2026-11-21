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

/// `ids` in the order [`shown`] lists them: ids taken from the session's
/// photos are in the library's order, which need not be the ids' order.
fn sorted<const N: usize>(mut ids: [PhotoId; N]) -> [PhotoId; N] {
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
    assert_eq!(shown(&library), sorted([photos[1], photos[2]]));
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

#[test]
fn create_smart_collection_makes_and_shows_it() -> Result<()> {
    let (_d, mut library, _) = library_with(&[])?;
    let photos: Vec<PhotoId> = library.session.photos().iter().map(|p| p.id).collect();
    library
        .session
        .set_ratings(&[(photos[2], 4, 0, String::new())])?;
    library.new_smart_collection();
    let mut dialog = library.smart_editor.take().unwrap();
    assert_eq!(dialog.rules.items.len(), 1);
    dialog.name = "Good".into();
    dialog.rules = rule("rating", ">=", Value::Number(4.));
    library.save_smart_editor(&dialog)?;
    let made = library
        .session
        .collections()
        .iter()
        .find(|c| c.name == "Good")
        .unwrap()
        .id;
    assert_eq!(library.filters.collection, Some(made));
    settle(&mut library);
    assert_eq!(shown(&library), [photos[2]]);
    // The name rules hold: not twice in one place.
    assert!(library.save_smart_editor(&dialog).is_err());
    Ok(())
}

#[test]
fn edit_smart_collection_shows_an_unsupported_rule_and_can_remove_it() -> Result<()> {
    let imported = Group::new(
        Combine::Union,
        vec![
            Item::Rule(Rule::new("rating", "==", Some(Value::Number(0.)))),
            Item::Rule(Rule::new("likes", "isTrue", Some(Value::Bool(true)))),
        ],
    );
    let (_d, mut library, smart) = library_with(&[("Imported", Ok(imported))])?;
    settle(&mut library);
    assert_eq!(library.smart_counts()[&smart[0]], SmartCount::Unknown);
    library.edit_smart_collection(smart[0]);
    let mut dialog = library.smart_editor.take().unwrap();
    assert_eq!(dialog.name, "Imported");
    assert!(dialog.rules.unsupported().is_some());
    dialog.rules.items.remove(1);
    library.save_smart_editor(&dialog)?;
    settle(&mut library);
    assert_eq!(library.smart_counts()[&smart[0]], SmartCount::Photos(3));
    // One step for Undo, with the rules as they were.
    let done = library.take_smart_done();
    assert_eq!(done.len(), 1);
    assert!(
        done[0]
            .before
            .as_ref()
            .unwrap()
            .rules()
            .unwrap()
            .unsupported()
            .is_some()
    );
    library.put_back_smart_rules(smart[0], done[0].before.as_ref())?;
    settle(&mut library);
    assert_eq!(library.smart_counts()[&smart[0]], SmartCount::Unknown);
    Ok(())
}

#[test]
fn rules_lightroom_wrote_that_cant_be_read_are_shown_as_text() -> Result<()> {
    let (_d, mut library, smart) = library_with(&[("Broken", Err("s = { os.exit() }"))])?;
    library.edit_smart_collection(smart[0]);
    let dialog = library.smart_editor.as_ref().unwrap();
    let (_, text) = dialog.unreadable.as_ref().unwrap();
    assert_eq!(text, "s = { os.exit() }");
    assert!(dialog.rules.items.is_empty());
    Ok(())
}

/// The dialog draws every field the editor offers, with each operation's
/// value, and a nested group.
#[test]
fn the_editor_draws_every_field_and_a_nested_group() -> Result<()> {
    use crate::collection_rules::fields::FIELDS;
    let (_d, mut library, _) = library_with(&[])?;
    library.new_smart_collection();
    let dialog = library.smart_editor.as_mut().unwrap();
    let mut items = Vec::new();
    for field in FIELDS {
        for op in field.operations {
            let mut rule = field.new_rule("2026-11-17");
            crate::collection_rules::fields::set_operation(&mut rule, op, "2026-11-17");
            items.push(Item::Rule(rule));
        }
    }
    let nested = Group::new(Combine::Exclude, vec![items[0].clone()]);
    items.push(Item::Group(nested));
    items.push(Item::Rule(Rule::new(
        "likes",
        "isTrue",
        Some(Value::Bool(true)),
    )));
    dialog.rules = Group::new(Combine::Union, items);
    let ctx = egui::Context::default();
    for _ in 0..2 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200., 900.),
                )),
                ..Default::default()
            },
            |ui| library.smart_editor_window(ui.ctx()),
        );
        output.textures_delta.clear();
    }
    assert!(library.smart_editor_open());
    Ok(())
}

/// A date or period typed that doesn't read isn't saved.
#[test]
fn the_editor_refuses_values_that_dont_read() -> Result<()> {
    let (_d, mut library, _) = library_with(&[])?;
    library.new_smart_collection();
    let mut dialog = library.smart_editor.take().unwrap();
    dialog.name = "Bad date".into();
    dialog.rules = rule("captureTime", "==", Value::Str("2026-02-30".into()));
    let error = library.save_smart_editor(&dialog).unwrap_err().to_string();
    assert!(error.contains("YYYY-MM-DD"), "{error}");
    let mut period = Rule::new("captureTime", "inLast", Some(Value::Number(1.5)));
    period.units = Some("days".into());
    dialog.rules = Group::new(Combine::Intersect, vec![Item::Rule(period)]);
    assert!(library.save_smart_editor(&dialog).is_err());
    assert!(
        !library
            .session
            .collections()
            .iter()
            .any(|c| c.name == "Bad date")
    );
    Ok(())
}

/// A selected set shows its smart collections' photos too (measured in
/// Lightroom Classic 15.6), and follows them as they are worked out again.
#[test]
fn a_set_shows_and_follows_its_smart_collections_photos() -> Result<()> {
    let (_d, mut library, smart) =
        library_with(&[("Picks", Ok(rule("pick", "==", Value::Number(1.))))])?;
    let mut photos: Vec<PhotoId> = library.session.photos().iter().map(|p| p.id).collect();
    photos.sort();
    let set = library.create_collection(true, "Set", None, &[], false)?;
    library.create_collection(false, "Static", Some(set), &photos[..1], false)?;
    library.move_collection(smart[0], Some(set))?;
    settle(&mut library);
    library.select_collection(set);
    assert_eq!(shown(&library), photos[..1]);
    library
        .session
        .set_ratings(&[(photos[2], 0, 1, String::new())])?;
    // Until the smart members are worked out again, Select All waits.
    assert!(!library.source_settled());
    library.select(None);
    library.select_all();
    assert!(library.selected_ids().is_empty());
    settle(&mut library);
    assert_eq!(shown(&library), sorted([photos[0], photos[2]]));
    Ok(())
}

/// Undo of a deleted smart collection brings its rules back with it, even
/// when the lists were read again while it was gone.
#[test]
fn a_restored_smart_collection_keeps_its_rules_after_a_reload() -> Result<()> {
    let (_d, mut library, smart) =
        library_with(&[("Picks", Ok(rule("pick", "==", Value::Number(1.))))])?;
    let photos: Vec<PhotoId> = library.session.photos().iter().map(|p| p.id).collect();
    library
        .session
        .set_ratings(&[(photos[0], 0, 1, String::new())])?;
    let gone = library.session.remove_collections(&[smart[0]])?;
    library.reload()?;
    library.session.restore_collections(&gone)?;
    settle(&mut library);
    assert_eq!(library.smart_counts()[&smart[0]], SmartCount::Photos(1));
    Ok(())
}

/// The control protocol's `collections` lists a smart collection as pending
/// while it is worked out again after a change it may see, not with the
/// count kept from before as if it were current (issue 1207).
#[test]
fn a_listed_smart_count_is_pending_until_worked_out_again() -> Result<()> {
    let (_d, mut library, smart) =
        library_with(&[("Picks", Ok(rule("pick", "==", Value::Number(1.))))])?;
    settle(&mut library);
    let listed = |library: &Library| {
        let entry = library
            .listed_collections()
            .into_iter()
            .find(|c| c.id == smart[0])
            .unwrap();
        (entry.photos, entry.smart)
    };
    assert_eq!(listed(&library), (Some(0), Some("ready")));
    let photos: Vec<PhotoId> = library.session.photos().iter().map(|p| p.id).collect();
    library.create_collection(false, "Static", None, &photos[..1], false)?;
    assert_eq!(listed(&library), (None, Some("pending")));
    let started = std::time::Instant::now();
    while !library.smart_current() {
        library.poll_smart();
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(listed(&library), (Some(0), Some("ready")));
    Ok(())
}

/// The Smart Collection editor and the New Collection window, for
/// `RAWMAKASE_UI_SHOTS` ([`crate::app::ui::shots`]): rules with a rating, a
/// period's units, a choice and text, a nested group, and the collection set
/// chosen in each.
#[test]
fn shots_of_smart_collections() -> Result<()> {
    use crate::collection_rules::fields::{field_of, set_operation};
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("a.RAF"), b"synthetic raw")?;
    // A catalog for each shot, as each makes the same sets.
    for name in ["smart.rawmakase", "new.rawmakase"] {
        Catalog::create(&directory.path().join(name))?.add_folder(&folder)?;
    }
    let dir = directory.path().to_path_buf();
    let open = move |e: &mut crate::app::Editor, ctx: &egui::Context, name: &str| {
        let mut library = Library::load(&dir.join(name), ctx.clone()).unwrap();
        let trips = library
            .create_collection(true, "Trips", None, &[], false)
            .unwrap();
        library
            .create_collection(true, "Japan", Some(trips), &[], false)
            .unwrap();
        e.library = Some(Box::new(library));
        trips
    };
    let today = "2026-11-21";
    let rule = |criteria: &str, op: &str| {
        let field = field_of(criteria).unwrap();
        let mut rule = field.new_rule(today);
        set_operation(&mut rule, field.operation(op).unwrap(), today);
        Item::Rule(rule)
    };
    let rules = Group::new(
        Combine::Intersect,
        vec![
            rule("rating", ">="),
            rule("captureTime", "inLast"),
            rule("pick", "=="),
            rule("filename", "any"),
            Item::Group(Group::new(
                Combine::Union,
                vec![rule("rating", "in"), rule("pick", "!=")],
            )),
        ],
    );
    {
        let open = open.clone();
        crate::app::ui::shots::shot(
            "smart-collection-editor",
            egui::vec2(1000., 640.),
            move |e, ctx| {
                let trips = open(e, ctx, "smart.rawmakase");
                let library = e.library.as_mut().unwrap();
                library.new_smart_collection();
                let dialog = library.smart_editor.as_mut().unwrap();
                dialog.name = "Best of Japan".into();
                dialog.inside = true;
                dialog.parent = Some(trips);
                dialog.rules = rules;
            },
            |e, ctx| e.library.as_mut().unwrap().smart_editor_window(ctx),
        );
    }
    crate::app::ui::shots::shot(
        "new-collection",
        egui::vec2(640., 480.),
        move |e, ctx| {
            let trips = open(e, ctx, "new.rawmakase");
            e.library
                .as_mut()
                .unwrap()
                .ask_create_collection(false, Some(trips));
        },
        |e, ctx| {
            e.library.as_mut().unwrap().collection_dialog_window(ctx);
        },
    );
    Ok(())
}
