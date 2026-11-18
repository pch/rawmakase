//! The Collections panel as a person uses it: making, renaming, moving and
//! deleting collections and sets, with Undo and Redo; a set showing every
//! photo in it; photos dragged onto a collection, added with Add to
//! Collection and taken out with Delete.
use super::collections::{CollectionNode, RowAction, collection_row};
use super::{DraggedPhotos, Library};
use crate::app::Editor;
use crate::app::testing::editor_with_catalog;
use crate::catalog::{CollectionId, PhotoId};
use anyhow::Result;
use eframe::egui::{self, Vec2};
use std::collections::HashSet;

fn library(e: &mut Editor) -> &mut Library {
    e.library.as_mut().unwrap()
}

fn sorted(mut ids: Vec<PhotoId>) -> Vec<PhotoId> {
    ids.sort();
    ids
}

fn members(e: &mut Editor, id: CollectionId) -> Vec<PhotoId> {
    sorted(
        library(e)
            .session
            .collection_photos()
            .get(&id)
            .map(|m| m.iter().copied().collect())
            .unwrap_or_default(),
    )
}

fn exists(e: &mut Editor, id: CollectionId) -> bool {
    library(e).session.collections().iter().any(|c| c.id == id)
}

fn parent(e: &mut Editor, id: CollectionId) -> Option<CollectionId> {
    library(e)
        .session
        .collections()
        .iter()
        .find(|c| c.id == id)
        .and_then(|c| c.parent)
}

#[test]
fn collections_and_sets_are_made_renamed_moved_and_deleted_with_undo() -> Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF"])?;
    let picked = sorted(ids[..2].to_vec());
    let trips = library(&mut e).create_collection(true, "Trips", None, &[], false)?;
    let japan = library(&mut e).create_collection(false, " Japan ", Some(trips), &picked, false)?;
    e.sync_undo();
    assert_eq!(members(&mut e, japan), picked);
    assert_eq!(parent(&mut e, japan), Some(trips));
    // Names are unique among one set's children.
    assert!(
        library(&mut e)
            .create_collection(false, "Japan", Some(trips), &[], false)
            .is_err()
    );

    library(&mut e).rename_collection(japan, "Kyoto")?;
    library(&mut e).move_collection(japan, None)?;
    e.sync_undo();
    assert_eq!(parent(&mut e, japan), None);
    e.undo();
    assert_eq!(parent(&mut e, japan), Some(trips));
    e.undo();
    let name = |e: &mut Editor| {
        library(e)
            .session
            .collections()
            .iter()
            .find(|c| c.id == japan)
            .map(|c| c.name.clone())
    };
    assert_eq!(name(&mut e).as_deref(), Some("Japan"));
    e.redo();
    assert_eq!(name(&mut e).as_deref(), Some("Kyoto"));

    // Deleting the set takes the collection in it; the photos stay.
    library(&mut e).select_collection(japan);
    library(&mut e).delete_collection(trips)?;
    e.sync_undo();
    assert!(!exists(&mut e, trips) && !exists(&mut e, japan));
    assert_eq!(library(&mut e).filters.collection, None);
    assert_eq!(library(&mut e).session.photos().len(), 3);
    e.undo();
    assert!(exists(&mut e, trips));
    assert_eq!(parent(&mut e, japan), Some(trips));
    assert_eq!(members(&mut e, japan), picked);
    // Back where it was made: the collection shown.
    assert_eq!(library(&mut e).filters.collection, Some(japan));
    e.redo();
    assert!(!exists(&mut e, trips));

    // Making one is undone too, and Redo brings it back under its id.
    let made = library(&mut e).create_collection(false, "Later", None, &ids[2..], false)?;
    e.sync_undo();
    e.undo();
    assert!(!exists(&mut e, made));
    e.redo();
    assert!(exists(&mut e, made));
    assert_eq!(members(&mut e, made), ids[2..]);
    Ok(())
}

#[test]
fn make_new_virtual_copies_puts_only_the_copies_in() -> Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    let made = library(&mut e).create_collection(false, "Copies", None, &ids, true)?;
    let inside = members(&mut e, made);
    assert_eq!(inside.len(), 2);
    assert!(inside.iter().all(|id| !ids.contains(id)));
    assert_eq!(library(&mut e).session.photos().len(), 4);
    // Undo takes the collection; the copies, made as copies are, stay.
    e.sync_undo();
    e.undo();
    assert!(!exists(&mut e, made));
    assert_eq!(library(&mut e).session.photos().len(), 4);
    Ok(())
}

/// Lightroom Classic shows a selected set as every photo of the
/// collections inside it, nested sets included, each once.
#[test]
fn a_set_shows_every_photo_in_it_once() -> Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF", "d.RAF"])?;
    let ids = sorted(ids);
    let l = library(&mut e);
    let outer = l.create_collection(true, "Outer", None, &[], false)?;
    let inner = l.create_collection(true, "Inner", Some(outer), &[], false)?;
    l.create_collection(false, "One", Some(outer), &ids[..2], false)?;
    let two = l.create_collection(false, "Two", Some(inner), &ids[1..3], false)?;
    l.select_collection(outer);
    assert_eq!(sorted(l.shown()), ids[..3]);
    l.select_collection(inner);
    assert_eq!(sorted(l.shown()), ids[1..3]);
    // Photos added to a collection in the set shown show at once.
    l.select_collection(outer);
    l.add_to_collection(two, vec![ids[3]])?;
    assert_eq!(sorted(l.shown()), ids);
    // Delete in a set takes nothing out and asks nothing.
    l.delete_key(vec![ids[0]]);
    assert_eq!(sorted(l.shown()), ids);
    assert!(l.next_request().is_none());
    Ok(())
}

#[test]
fn delete_takes_photos_out_of_the_collection_shown_and_undo_puts_them_back() -> Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    let ids = sorted(ids);
    let picks = library(&mut e).create_collection(false, "Picks", None, &ids, false)?;
    let before = library(&mut e)
        .session
        .catalog()
        .collection_members(picks)?;
    library(&mut e).select_collection(picks);
    library(&mut e).delete_key(vec![ids[0]]);
    e.sync_undo();
    assert_eq!(library(&mut e).shown(), [ids[1]]);
    // Taken out of the collection only.
    assert_eq!(library(&mut e).session.photos().len(), 2);
    assert!(library(&mut e).next_request().is_none());
    e.undo();
    assert_eq!(sorted(library(&mut e).shown()), ids);
    assert_eq!(
        library(&mut e)
            .session
            .catalog()
            .collection_members(picks)?,
        before
    );
    Ok(())
}

#[test]
fn add_to_collection_adds_the_photos_not_in_it_yet() -> Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    let ids = sorted(ids);
    let picks = library(&mut e).create_collection(false, "Picks", None, &ids[..1], false)?;
    e.sync_undo();
    library(&mut e).add_to_collection(picks, ids.clone())?;
    e.sync_undo();
    assert_eq!(members(&mut e, picks), ids);
    e.undo();
    assert_eq!(members(&mut e, picks), ids[..1]);
    // Nothing new: nothing to undo.
    library(&mut e).add_to_collection(picks, ids[..1].to_vec())?;
    e.sync_undo();
    e.undo();
    assert!(!exists(&mut e, picks));
    Ok(())
}

/// Drives one frame of `rows` with `events`, returning what a row asked.
/// Photos are dragged from a cell at `source`.
fn rows_frame(
    ctx: &egui::Context,
    rows: &[CollectionNode],
    source: egui::Rect,
    events: Vec<egui::Event>,
) -> Option<RowAction> {
    let mut asked = None;
    let mut expanded = HashSet::new();
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(360., 240.),
            )),
            events,
            ..Default::default()
        },
        |ui| {
            for row in rows {
                if let Some(a) = collection_row(ui, row, 0, &mut expanded, None) {
                    asked = Some(a);
                }
            }
            let cell = ui.allocate_rect(source, egui::Sense::click_and_drag());
            super::grid::drag_payload(&cell, || vec![PhotoId(1), PhotoId(2)]);
        },
    );
    output.textures_delta.clear();
    asked
}

fn press(at: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

fn node(id: i64, set: bool, parent: Option<i64>, children: Vec<CollectionNode>) -> CollectionNode {
    CollectionNode {
        id: CollectionId(id),
        name: format!("row {id}"),
        parent: parent.map(CollectionId),
        set,
        count: 0,
        smart: None,
        target: false,
        children,
    }
}

/// Drags from `from` to `to` and releases there.
fn drag(
    ctx: &egui::Context,
    rows: &[CollectionNode],
    source: egui::Rect,
    from: egui::Pos2,
    to: egui::Pos2,
) -> Option<RowAction> {
    rows_frame(ctx, rows, source, vec![]);
    rows_frame(
        ctx,
        rows,
        source,
        vec![egui::Event::PointerMoved(from), press(from, true)],
    );
    rows_frame(
        ctx,
        rows,
        source,
        vec![egui::Event::PointerMoved(from + Vec2::new(0., 20.))],
    );
    rows_frame(ctx, rows, source, vec![egui::Event::PointerMoved(to)]);
    rows_frame(ctx, rows, source, vec![press(to, false)])
}

#[test]
fn photos_dropped_on_a_collection_row_go_into_it_but_not_into_a_set() {
    let ctx = egui::Context::default();
    // Rows 22 points tall: a collection, then an empty set.
    let rows = [node(7, false, None, vec![]), node(8, true, None, vec![])];
    let source = egui::Rect::from_min_size(egui::pos2(20., 150.), Vec2::splat(60.));
    let start = source.center();
    let action = drag(&ctx, &rows, source, start, egui::pos2(120., 10.));
    assert_eq!(
        action,
        Some(RowAction::AddPhotos(
            CollectionId(7),
            vec![PhotoId(1), PhotoId(2)]
        ))
    );
    assert_eq!(
        drag(&ctx, &rows, source, start, egui::pos2(120., 32.)),
        None
    );
    // Nor onto a smart collection, which takes no photos by hand.
    let mut smart = node(9, false, None, vec![]);
    smart.smart = Some(super::collections::SmartCount::Photos(0));
    assert_eq!(
        drag(&ctx, &[smart], source, start, egui::pos2(120., 10.)),
        None
    );
    assert!(!egui::DragAndDrop::has_payload_of_type::<DraggedPhotos>(
        &ctx
    ));
}

#[test]
fn a_collection_dragged_onto_a_set_moves_into_it_but_never_into_itself() {
    let ctx = egui::Context::default();
    // Set 1 holds set 2; collection 3 is at the top.
    let rows = [
        node(1, true, None, vec![node(2, true, Some(1), vec![])]),
        node(3, false, None, vec![]),
    ];
    // Far from the rows: no photos dragged.
    let source = egui::Rect::from_min_size(egui::pos2(300., 200.), Vec2::splat(10.));
    let row = |i: f32| egui::pos2(120., 11. + 22. * i);
    assert_eq!(
        drag(&ctx, &rows, source, row(2.), row(1.)),
        Some(RowAction::Move(CollectionId(3), Some(CollectionId(2))))
    );
    // A set into one inside it, or into itself: nothing.
    assert_eq!(drag(&ctx, &rows, source, row(0.), row(1.)), None);
    assert_eq!(drag(&ctx, &rows, source, row(1.), row(1.)), None);
    // Nor into the set it is in already.
    assert_eq!(drag(&ctx, &rows, source, row(1.), row(0.)), None);
}

/// The collection windows are the workspace's: they show, and Escape
/// closes them, in Develop and in a catalog with no photos too.
#[test]
fn collection_windows_open_in_develop_and_in_an_empty_catalog() -> Result<()> {
    use crate::app::testing::press;
    for (names, module) in [
        (&["a.RAF"][..], super::Module::Develop),
        (&[][..], super::Module::Library),
    ] {
        let (_d, mut e, _) = editor_with_catalog(names)?;
        e.module = module;
        library(&mut e).ask_create_collection(false, None);
        assert!(e.collection_dialog_open());
        // Drawn once, as the window is before anyone presses a key.
        let ctx = e.context.clone();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| e.draw(ui));
        output.textures_delta.clear();
        assert!(
            ctx.memory(|m| m.areas().visible_last_frame(&egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("collection-dialog"),
            )))
        );
        press(&mut e, egui::Key::Escape, egui::Modifiers::NONE);
        assert!(!e.collection_dialog_open(), "{module:?}");
    }
    Ok(())
}

/// Make new virtual copies stopping part way: the copies made show in the
/// Library and the error says they stay.
#[test]
fn copies_made_before_a_failure_show_and_are_named() -> Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    let error = library(&mut e)
        .create_collection(false, "Copies", None, &[ids[0], PhotoId(999)], true)
        .unwrap_err();
    assert!(format!("{error:#}").contains("after 1 copy"), "{error:#}");
    let l = library(&mut e);
    assert_eq!(l.session.photos().len(), 2);
    assert_eq!(l.shown().len(), 2);
    assert!(l.session.collections().iter().all(|c| c.name != "Copies"));
    Ok(())
}

/// Make new virtual copies saves the edit open in Develop first, as Create
/// Virtual Copy does, so each copy starts from what is on screen; when
/// that save fails, nothing is made.
#[test]
fn a_collection_of_new_copies_saves_the_open_edit_first() -> Result<()> {
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    let path = d.path().join("photos/a.RAF");
    e.module = super::Module::Develop;
    e.document.catalog_photo = Some(ids[0]);
    e.document.opened_at(path.clone());
    let before = e.document.edit.recipe().clone();
    e.document.edit.setup_mut().exposure = 0.7;
    e.commit_edit(before, None);
    let asked = super::NewCollection {
        name: "Copies".into(),
        parent: None,
        photos: vec![ids[0]],
        target: false,
    };
    let made = |e: &mut Editor| {
        library(e)
            .session
            .collections()
            .iter()
            .find(|c| c.name == "Copies")
            .map(|c| c.id)
    };
    // The save fails, as on a full disk: no copy, no collection.
    let fail = "CREATE TEMP TRIGGER fail_save BEFORE UPDATE OF recipe ON photos
                BEGIN SELECT RAISE(FAIL, 'disk full'); END;";
    library(&mut e)
        .session
        .catalog()
        .db_for_tests()
        .execute_batch(fail)?;
    e.create_collection_of_copies(asked.clone());
    assert_eq!(made(&mut e), None);
    assert_eq!(library(&mut e).session.photos().len(), 1);
    library(&mut e)
        .session
        .catalog()
        .db_for_tests()
        .execute_batch("DROP TRIGGER fail_save")?;
    e.create_collection_of_copies(asked);
    let collection = made(&mut e).expect("made once the edit is saved");
    let copies = members(&mut e, collection);
    assert_eq!(copies.len(), 1);
    assert_ne!(copies[0], ids[0]);
    let saved = library(&mut e)
        .session
        .catalog()
        .load_edit(copies[0], &path)?
        .expect("the copy's edit");
    assert_eq!(saved.recipe.exposure, 0.7);
    Ok(())
}

/// Create Smart Collection is one step Undo takes back and Redo makes
/// again, rules and all.
#[test]
fn creating_a_smart_collection_is_undone_and_redone() -> Result<()> {
    let (_d, mut e, _) = editor_with_catalog(&["a.RAF"])?;
    library(&mut e).new_smart_collection();
    let mut dialog = library(&mut e).smart_editor.take().unwrap();
    dialog.name = "Good".into();
    library(&mut e).save_smart_editor(&dialog)?;
    e.sync_undo();
    let made = |e: &mut Editor| {
        library(e)
            .session
            .collections()
            .iter()
            .find(|c| c.name == "Good")
            .map(|c| c.id)
    };
    let id = made(&mut e).expect("made");
    e.undo();
    assert_eq!(made(&mut e), None);
    e.redo();
    assert_eq!(made(&mut e), Some(id));
    assert!(library(&mut e).session.smart_rules_of(id).is_some());
    Ok(())
}

/// B adds to the target collection, as Lightroom's does; a deleted target
/// gives B back to the Quick Collection.
#[test]
fn b_adds_to_the_target_collection_and_falls_back_to_quick() -> Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    let ids = sorted(ids);
    let picks = library(&mut e).create_collection(false, "Picks", None, &[], false)?;
    library(&mut e).collection_row_action(RowAction::SetTarget(Some(picks)));
    assert_eq!(library(&mut e).target(), Some(picks));
    library(&mut e).toggle_quick(&ids)?;
    assert_eq!(members(&mut e, picks), ids);
    assert_eq!(library(&mut e).quick_count(), 0);
    // All in it already: B takes them out.
    library(&mut e).toggle_quick(&ids[..1])?;
    assert_eq!(members(&mut e, picks), ids[1..]);
    // The panel marks the target.
    let tree = library(&mut e).collection_tree();
    assert!(tree.iter().any(|n| n.id == picks && n.target));
    library(&mut e).delete_collection(picks)?;
    assert_eq!(library(&mut e).target(), None);
    library(&mut e).toggle_quick(&ids[..1])?;
    assert_eq!(library(&mut e).quick_count(), 1);
    Ok(())
}

#[test]
fn save_quick_collection_makes_a_collection_and_clears_it_in_one_step() -> Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    let ids = sorted(ids);
    library(&mut e).toggle_quick(&ids)?;
    e.sync_undo();
    let quick = library(&mut e).quick().unwrap();
    let positions = library(&mut e)
        .session
        .catalog()
        .collection_members(quick)?;
    let saved = library(&mut e).save_quick("Keepers", true)?;
    e.sync_undo();
    assert_eq!(members(&mut e, saved), ids);
    assert_eq!(library(&mut e).quick_count(), 0);
    // One step back: the Quick Collection as it was, and no collection.
    e.undo();
    assert!(!exists(&mut e, saved));
    assert_eq!(
        library(&mut e)
            .session
            .catalog()
            .collection_members(quick)?,
        positions
    );
    e.redo();
    assert!(exists(&mut e, saved));
    assert_eq!(library(&mut e).quick_count(), 0);
    // Not cleared when asked not to.
    library(&mut e).toggle_quick(&ids[..1])?;
    let kept = library(&mut e).save_quick("Again", false)?;
    assert_eq!(members(&mut e, kept), ids[..1]);
    assert_eq!(library(&mut e).quick_count(), 1);
    // The Quick Collection's name is not for a collection.
    assert!(
        library(&mut e)
            .save_quick("quick collection", false)
            .is_err()
    );
    Ok(())
}

/// The collection badge and Go to Collection list the photo's collections,
/// the Quick Collection too, and choosing one shows it.
#[test]
fn go_to_collection_lists_the_photos_collections_and_shows_one() -> Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    let ids = sorted(ids);
    let l = library(&mut e);
    let picks = l.create_collection(false, "Picks", None, &ids[..1], false)?;
    l.create_collection(false, "Others", None, &ids[1..], false)?;
    l.toggle_quick(&ids[..1])?;
    let quick = l.quick().unwrap();
    assert_eq!(
        l.photo_collections(ids[0]),
        [(picks, "Picks".into()), (quick, "Quick Collection".into())]
    );
    let photo = l.photo(ids[0]).unwrap().clone();
    l.photo_action(
        &l_ctx(),
        &photo,
        super::cell::PhotoAction::GoToCollection(picks),
        true,
    );
    assert_eq!(l.filters.collection, Some(picks));
    Ok(())
}

fn l_ctx() -> egui::Context {
    egui::Context::default()
}

/// ⌘N opens Create Collection and ⌘⌥B Save Quick Collection, in the grid.
#[test]
fn command_n_and_command_option_b_open_their_windows() -> Result<()> {
    use crate::app::testing::press;
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    press(&mut e, egui::Key::N, egui::Modifiers::COMMAND);
    assert!(e.collection_dialog_open());
    library(&mut e).collection_dialog = None;
    // An empty Quick Collection has nothing to save.
    let save = egui::Modifiers::COMMAND | egui::Modifiers::ALT;
    press(&mut e, egui::Key::B, save);
    assert!(!e.collection_dialog_open());
    library(&mut e).toggle_quick(&ids)?;
    press(&mut e, egui::Key::B, save);
    assert!(e.collection_dialog_open());
    Ok(())
}

/// A Redo of Save Quick Collection that fails changes nothing and can be
/// tried again.
#[test]
fn a_failed_redo_of_save_quick_collection_can_be_tried_again() -> Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    library(&mut e).toggle_quick(&ids)?;
    e.sync_undo();
    let saved = library(&mut e).save_quick("Keepers", true)?;
    e.sync_undo();
    e.undo();
    let db = |e: &mut Editor, sql: &str| -> Result<()> {
        library(e)
            .session
            .catalog()
            .db_for_tests()
            .execute_batch(sql)?;
        Ok(())
    };
    db(
        &mut e,
        "CREATE TEMP TRIGGER fail_clear BEFORE DELETE ON collection_photos
         BEGIN SELECT RAISE(ABORT, 'disk full'); END",
    )?;
    e.redo();
    assert!(!exists(&mut e, saved));
    assert_eq!(library(&mut e).quick_count(), 2);
    db(&mut e, "DROP TRIGGER fail_clear")?;
    e.redo();
    assert!(exists(&mut e, saved));
    assert_eq!(library(&mut e).quick_count(), 0);
    Ok(())
}

/// Go to Collection from a filmstrip cell shrinks the photos shown while
/// the strip is drawn: the strip stops there instead of reading past them.
#[test]
fn go_to_collection_from_the_filmstrip_stops_drawing_the_old_list() -> Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF"])?;
    let ctx = e.context.clone();
    let l = library(&mut e);
    let first = l.session.photos()[l.visible[0]].id;
    l.create_collection(false, "Only", None, &[first], false)?;
    assert_eq!(l.shown().len(), ids.len());
    let frame = |l: &mut Library, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(1200., 800.),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                let active = l.selected();
                l.filmstrip_panel(ui, active, super::Module::Library);
            },
        );
        output.textures_delta.clear();
        output
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => {
                    Some((t.galley.text().to_owned(), t.visual_bounding_rect()))
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let button = |at: egui::Pos2, button, pressed| egui::Event::PointerButton {
        pos: at,
        button,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let find = |texts: &[(String, egui::Rect)], text: &str| {
        texts
            .iter()
            .find(|(t, _)| t == text)
            .map(|(_, r)| r.center())
    };
    let l = library(&mut e);
    frame(l, vec![]);
    // The first cell, at the strip's left, below its header at the
    // screen's foot.
    let cell = egui::pos2(40., 760.);
    let secondary = egui::PointerButton::Secondary;
    frame(
        l,
        vec![
            egui::Event::PointerMoved(cell),
            button(cell, secondary, true),
            button(cell, secondary, false),
        ],
    );
    let mut texts = frame(l, vec![]);
    let submenu = find(&texts, "Go to Collection").expect("Go to Collection");
    for _ in 0..4 {
        texts = frame(l, vec![egui::Event::PointerMoved(submenu)]);
    }
    let item = find(&texts, "Only").expect("the collection in the submenu");
    let primary = egui::PointerButton::Primary;
    frame(l, vec![egui::Event::PointerMoved(item)]);
    frame(
        l,
        vec![button(item, primary, true), button(item, primary, false)],
    );
    frame(l, vec![]);
    assert_eq!(l.shown(), [first]);
    Ok(())
}

/// A collection shows in Capture Time until another sort is chosen for it
/// (as Lightroom Classic 15.6 starts one), which is kept with the
/// collection as Lightroom's sortType; the folders keep their own sort,
/// back when the collection is left.
#[test]
fn a_collection_keeps_its_own_sort_apart_from_the_folders() -> Result<()> {
    use super::sort::Sort;
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF"])?;
    let ids = sorted(ids);
    let l = library(&mut e);
    l.filters.sort = Sort::FileName;
    l.filter();
    let picks = l.create_collection(false, "Picks", None, &[ids[2], ids[0], ids[1]], false)?;
    l.select_collection(picks);
    assert_eq!(l.filters.sort, Sort::CaptureTime);
    // Custom Order shows them as added.
    l.filters.sort = Sort::Custom;
    l.sort_chosen();
    l.filter();
    assert_eq!(l.shown(), [ids[2], ids[0], ids[1]]);
    assert_eq!(
        l.session.catalog().collection_options()?[&picks].sort,
        "positionInCollection"
    );
    assert_eq!(l.layout().sort, "file_name");
    // Another sort chosen here is the collection's.
    l.filters.sort = Sort::Rating;
    l.sort_chosen();
    l.filters.collection = None;
    l.filter();
    assert_eq!(l.filters.sort, Sort::FileName);
    l.select_collection(picks);
    assert_eq!(l.filters.sort, Sort::Rating);
    let options = l.session.catalog().collection_options()?;
    assert_eq!(options[&picks].sort, "rating");
    assert_eq!(options[&picks].direction, "ascending");
    Ok(())
}

#[test]
fn dragging_in_custom_order_moves_photos_with_undo_either_way_up() -> Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF", "d.RAF"])?;
    let ids = sorted(ids);
    let picks = library(&mut e).create_collection(false, "Picks", None, &ids, false)?;
    library(&mut e).select_collection(picks);
    library(&mut e).filters.sort = super::sort::Sort::Custom;
    library(&mut e).filter();
    assert_eq!(library(&mut e).shown(), ids);
    // The last two dropped before the second.
    library(&mut e).reorder_shown(vec![ids[2], ids[3]], 1)?;
    e.sync_undo();
    let moved = [ids[0], ids[2], ids[3], ids[1]];
    assert_eq!(library(&mut e).shown(), moved);
    e.undo();
    assert_eq!(library(&mut e).shown(), ids);
    e.redo();
    assert_eq!(library(&mut e).shown(), moved);
    // Shown the other way up, a photo dropped first goes last in the order.
    let l = library(&mut e);
    l.filters.reverse = true;
    l.filter();
    assert_eq!(l.shown(), [ids[1], ids[3], ids[2], ids[0]]);
    l.reorder_shown(vec![ids[0]], 0)?;
    assert_eq!(l.shown(), [ids[0], ids[1], ids[3], ids[2]]);
    // Dropped where it is: nothing to undo.
    e.sync_undo();
    let before = e.undo_log.len();
    library(&mut e).reorder_shown(vec![ids[0]], 0)?;
    e.sync_undo();
    assert_eq!(e.undo_log.len(), before);
    Ok(())
}

/// Photos dragged over a grid cell in Custom Order drop before it, or after
/// it on its right half.
#[test]
fn photos_dropped_on_a_cell_go_before_or_after_it() {
    let ctx = egui::Context::default();
    let source = egui::Rect::from_min_size(egui::pos2(20., 150.), Vec2::splat(60.));
    let target = egui::Rect::from_min_size(egui::pos2(200., 10.), Vec2::new(100., 80.));
    let frame = |events: Vec<egui::Event>| {
        let mut dropped = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(400., 300.),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                let cell = ui.allocate_rect(target, egui::Sense::click_and_drag());
                dropped = super::grid::reorder_target(ui, &cell, 5);
                let from = ui.allocate_rect(source, egui::Sense::click_and_drag());
                super::grid::drag_payload(&from, || vec![PhotoId(1)]);
            },
        );
        output.textures_delta.clear();
        dropped
    };
    for (x, at) in [(220., 5), (280., 6)] {
        let start = source.center();
        let to = egui::pos2(x, 50.);
        frame(vec![]);
        frame(vec![egui::Event::PointerMoved(start), press(start, true)]);
        frame(vec![egui::Event::PointerMoved(start + Vec2::new(0., -20.))]);
        frame(vec![egui::Event::PointerMoved(to)]);
        assert_eq!(frame(vec![press(to, false)]), Some((vec![PhotoId(1)], at)));
    }
}

/// Undo back into a collection, and out of it again, keeps its sort its own:
/// the folders' comes back, and the saved layout keeps the folders'.
#[test]
fn undo_into_a_collection_keeps_the_folders_sort_apart() -> Result<()> {
    use super::sort::Sort;
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    let ids = sorted(ids);
    let l = library(&mut e);
    l.filters.sort = Sort::FileName;
    l.filter();
    let picks = l.create_collection(false, "Picks", None, &ids, false)?;
    l.select_collection(picks);
    l.filters.sort = Sort::Custom;
    l.sort_chosen();
    l.filter();
    l.reorder_shown(vec![ids[1]], 0)?;
    e.sync_undo();
    // To All Photographs, then Undo goes back to the collection.
    let l = library(&mut e);
    l.filters.collection = None;
    l.filter();
    assert_eq!(l.filters.sort, Sort::FileName);
    e.undo();
    let l = library(&mut e);
    assert_eq!(l.filters.collection, Some(picks));
    assert_eq!(l.filters.sort, Sort::Custom);
    assert_eq!(l.layout().sort, "file_name");
    l.filters.collection = None;
    l.filter();
    assert_eq!(l.filters.sort, Sort::FileName);
    Ok(())
}

/// A drop that leaves the photos shown as they were changes nothing, and
/// photos the filter bar hides keep their places.
#[test]
fn hidden_photos_keep_their_places_when_shown_ones_move() -> Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF"])?;
    let ids = sorted(ids);
    let l = library(&mut e);
    let picks = l.create_collection(false, "Picks", None, &ids, false)?;
    l.select_collection(picks);
    l.filters.sort = super::sort::Sort::Custom;
    // The middle one hidden by the filter bar.
    l.session
        .set_ratings(&[(ids[0], 3, 0, String::new()), (ids[2], 3, 0, String::new())])?;
    l.reloaded();
    l.filters.rating = Some(3);
    l.filter();
    assert_eq!(l.shown(), [ids[0], ids[2]]);
    // Dropped where it is.
    l.reorder_shown(vec![ids[0]], 1)?;
    assert_eq!(l.session.catalog().collection_order(picks)?, ids);
    // Swapped: the hidden one stays in the middle.
    l.reorder_shown(vec![ids[0]], 2)?;
    assert_eq!(
        l.session.catalog().collection_order(picks)?,
        [ids[2], ids[1], ids[0]]
    );
    Ok(())
}

/// Photos Lightroom left without a position: taking one out, adding
/// another and undoing both gives back the order as it was, numbering and
/// all.
#[test]
fn undo_keeps_the_order_of_photos_without_a_position() -> Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF", "d.RAF"])?;
    let ids = sorted(ids);
    let picks = library(&mut e).create_collection(false, "Picks", None, &ids[..3], false)?;
    let order = |e: &mut Editor| library(e).session.catalog().collection_order(picks);
    library(&mut e)
        .session
        .catalog()
        .db_for_tests()
        .execute("UPDATE collection_photos SET position=NULL", [])?;
    let before = library(&mut e)
        .session
        .catalog()
        .collection_members(picks)?;
    assert_eq!(order(&mut e)?, ids[..3]);
    library(&mut e).select_collection(picks);
    // Taken out, then undone.
    library(&mut e).delete_key(vec![ids[0]]);
    e.sync_undo();
    e.undo();
    assert_eq!(order(&mut e)?, ids[..3]);
    // Taken out, another added, both undone.
    library(&mut e).delete_key(vec![ids[0]]);
    e.sync_undo();
    library(&mut e).add_to_collection(picks, vec![ids[3]])?;
    e.sync_undo();
    assert_eq!(order(&mut e)?, [ids[1], ids[2], ids[3]]);
    e.undo();
    e.undo();
    assert_eq!(order(&mut e)?, ids[..3]);
    assert_eq!(
        library(&mut e)
            .session
            .catalog()
            .collection_members(picks)?,
        before
    );
    // And redone, in order.
    e.redo();
    e.redo();
    assert_eq!(order(&mut e)?, [ids[1], ids[2], ids[3]]);
    Ok(())
}

/// The photo added is removed from the catalog before the Undos: the
/// numbering its adding did still comes undone, so the earlier removal's
/// Undo gives the order back as it was.
#[test]
fn undo_keeps_the_order_when_the_photo_added_is_gone() -> Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF"])?;
    let ids = sorted(ids);
    let picks = library(&mut e).create_collection(false, "Picks", None, &ids, false)?;
    let copy = library(&mut e).create_virtual_copy(ids[0])?.value;
    library(&mut e)
        .session
        .catalog()
        .db_for_tests()
        .execute("UPDATE collection_photos SET position=NULL", [])?;
    library(&mut e).select_collection(picks);
    library(&mut e).delete_key(vec![ids[0]]);
    e.sync_undo();
    library(&mut e).add_to_collection(picks, vec![copy])?;
    e.sync_undo();
    // The copy goes, and with it its own part of the command; the
    // numbering stays to undo, then the removal.
    e.remove_virtual_copy(copy);
    e.undo();
    e.undo();
    assert_eq!(
        library(&mut e).session.catalog().collection_order(picks)?,
        ids
    );
    Ok(())
}
