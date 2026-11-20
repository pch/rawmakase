//! Editor tests of `sync`: the whole editor driven through its frames.
use crate::app::testing::*;
use crate::app::*;

#[test]
fn select_all_shortcut_selects_the_filmstrip_for_sync() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF"])?;
    e.module = Module::Develop;
    e.document.catalog_photo = Some(ids[1]);
    e.library.as_mut().unwrap().select(Some(ids[1]));
    let ctx = e.context.clone();
    // Cmd+A on macOS, Ctrl+A elsewhere.
    let modifiers = egui::Modifiers::COMMAND;
    let input = egui::RawInput {
        events: vec![
            egui::Event::ModifiersChanged(modifiers),
            egui::Event::Key {
                key: egui::Key::A,
                physical_key: Some(egui::Key::A),
                pressed: true,
                repeat: false,
                modifiers,
            },
        ],
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |_| e.develop_shortcuts(&ctx));
    output.textures_delta.clear();
    let library = e.library.as_ref().unwrap();
    assert_eq!(library.selected_photos(), ids);
    assert_eq!(library.selected(), Some(ids[1]));
    Ok(())
}

/// In the Library grid, Shift+Cmd+S opens Sync Settings from the active photo to
/// the rest of the selection, with no photo open in Develop (issue #445).
#[test]
fn library_sync_shortcut_syncs_from_the_active_photo() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF"])?;
    let press = |e: &mut Editor| {
        let ctx = e.context.clone();
        let modifiers = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
        let input = egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key {
                    key: egui::Key::S,
                    physical_key: Some(egui::Key::S),
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
            ],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| e.draw(ui));
        output.textures_delta.clear();
    };
    // A photo alone has nothing to sync to.
    e.library.as_mut().unwrap().select(Some(ids[0]));
    assert_eq!(e.sync_target_count(), 0);
    press(&mut e);
    assert!(e.modal.is_none());
    // The last one clicked is active, and the source.
    e.library.as_mut().unwrap().select_range_to(ids[2]);
    assert_eq!(e.library.as_ref().unwrap().selected(), Some(ids[2]));
    let targets: Vec<_> = e.sync_targets().into_iter().map(|t| t.id).collect();
    assert_eq!(targets, ids[..2]);
    assert_eq!(e.sync_target_count(), 2);
    press(&mut e);
    assert!(
        matches!(&e.modal, Some(Modal::CopySettings(dialog))
            if dialog.purpose == crate::app::develop::settings_transfer::Transfer::Sync),
        "Shift+Cmd+S opens Synchronize Settings"
    );
    Ok(())
}

/// From the Library, the photo open in Develop can be a target: while an Auto
/// estimate is still to land on it, it would be saved over what the Sync wrote, so
/// Sync waits.
#[test]
fn library_sync_waits_for_auto_on_the_open_photo() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF"])?;
    let library = e.library.as_mut().unwrap();
    library.select(Some(ids[0]));
    library.select_range_to(ids[2]);
    // ids[0] is open in Develop and a target; ids[2] is the source.
    e.document.catalog_photo = Some(ids[0]);
    let _auto = e.document.auto.start();
    assert!(e.sync_targets().is_empty());
    assert_eq!(e.sync_target_count(), 0);
    e.document.auto = Default::default();
    assert_eq!(e.sync_target_count(), 2);
    // Not selected, it is not changed and does not hold the Sync back.
    let _auto = e.document.auto.start();
    let library = e.library.as_mut().unwrap();
    library.select(Some(ids[1]));
    library.select_range_to(ids[2]);
    assert_eq!(e.sync_target_count(), 1);
    Ok(())
}

/// From the Library, the photo open in Develop can be a Sync's target: it opens
/// again with what the Sync saved, so its stale edit is not saved over it.
#[test]
fn a_synced_photo_open_in_develop_opens_again() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    let library = e.library.as_ref().unwrap();
    let path = library.photo(ids[0]).unwrap().path.clone();
    let catalog = library.session.catalog().location().clone();
    e.document.catalog_photo = Some(ids[0]);
    e.document.opened_at(path.clone());
    let synced = |id| crate::app::library::sync::Synced {
        id,
        path: path.clone(),
        before: crate::app::library::sync::EditBefore::Saved(Box::default()),
        after: Default::default(),
        history: crate::catalog::SavedHistory {
            origin: Default::default(),
            steps: Vec::new(),
            applied: 0,
        },
        history_before: crate::catalog::SavedHistory {
            origin: Default::default(),
            steps: Vec::new(),
            applied: 0,
        },
        identity: crate::storage::Identity::read(&path).unwrap(),
    };
    let result = |id| crate::app::library::sync::SyncResult {
        change: crate::app::library::sync::BatchChange::Settings(Default::default()),
        catalog: catalog.clone(),
        synced: vec![synced(id)],
        failed: Vec::new(),
        notes: Vec::new(),
        cancelled: false,
    };
    let settled = || {
        save_state::SaveState::Pending(
            std::time::Instant::now() - std::time::Duration::from_secs(1),
        )
    };
    let ctx = e.context.clone();
    // Another photo synced: the open one stays as it is, and saves once it is done.
    assert!(e.activity.begin_sync(std::sync::Arc::new(
        crate::app::library::sync::BatchProgress::new("Sync", 1)
    )));
    e.document.edit.setup_mut().exposure = 0.4;
    *e.document.edit.save_state_mut() = settled();
    e.autosave(&ctx);
    assert!(!e.autosave.busy(), "nothing saves while a Sync writes");
    e.synced(result(ids[1]));
    assert!(!e.document.loading());
    assert!(e.document.edit.save_state().needs_save());
    // The open photo synced: what an analysis landing during the Sync changed on it
    // (as Upright's would) is not saved over the Sync, and it opens again.
    assert!(e.activity.begin_sync(std::sync::Arc::new(
        crate::app::library::sync::BatchProgress::new("Sync", 1)
    )));
    e.document.edit.setup_mut().exposure = 0.9;
    *e.document.edit.save_state_mut() = settled();
    e.autosave(&ctx);
    assert!(!e.autosave.busy(), "nothing saves while a Sync writes");
    // Nor a save asked for directly (Restart to Update, closing).
    assert!(!e.flush());
    assert_eq!(
        e.flush_by(std::time::Instant::now() + std::time::Duration::from_secs(1)),
        crate::app::develop::workflow::Flushed::Late
    );
    assert!(!e.autosave.busy());
    e.synced(result(ids[0]));
    assert!(e.document.loading(), "the open photo opens again");
    assert!(!e.document.edit.save_state().needs_save());
    let library = e.library.as_ref().unwrap();
    assert!(
        library
            .session
            .catalog()
            .load_edit(ids[0], &path)?
            .is_none()
    );
    Ok(())
}

#[test]
fn select_none_shortcut_clears_the_filmstrip_selection() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF"])?;
    e.module = Module::Develop;
    e.document.catalog_photo = Some(ids[1]);
    let ctx = e.context.clone();
    let press = |e: &mut Editor, key: egui::Key, modifiers: egui::Modifiers| {
        let input = egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
            ],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |_| e.develop_shortcuts(&ctx));
        output.textures_delta.clear();
    };
    // Cmd+A takes every shown photo, the open one active.
    e.library.as_mut().unwrap().select(Some(ids[1]));
    press(&mut e, egui::Key::A, egui::Modifiers::COMMAND);
    assert_eq!(e.library.as_ref().unwrap().selected_photos(), ids);
    // Cmd+D selects none.
    press(&mut e, egui::Key::D, egui::Modifiers::COMMAND);
    let library = e.library.as_ref().unwrap();
    assert!(library.selected_photos().is_empty());
    assert_eq!(library.selected(), None);
    Ok(())
}

/// Issue 998: a Sync, its Undo and a batch (presets, Auto) write on a
/// connection of their own; each tells the session, so smart collections see
/// the edits. A cancelled one wrote nothing.
#[test]
fn syncs_their_undo_and_batches_tell_the_session() -> anyhow::Result<()> {
    use crate::app::library::sync::{BatchChange, EditBefore, SyncResult, Synced};
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    let library = e.library.as_ref().unwrap();
    let path = library.photo(ids[1]).unwrap().path.clone();
    let catalog = library.session.catalog().location().clone();
    let history = || crate::catalog::SavedHistory {
        origin: Default::default(),
        steps: Vec::new(),
        applied: 0,
    };
    let result = |change: BatchChange, cancelled: bool| SyncResult {
        change,
        catalog: catalog.clone(),
        synced: vec![Synced {
            id: ids[1],
            path: path.clone(),
            before: EditBefore::Saved(Box::default()),
            after: Default::default(),
            history: history(),
            history_before: history(),
            identity: crate::storage::Identity::read(&path).unwrap(),
        }],
        failed: Vec::new(),
        notes: Vec::new(),
        cancelled,
    };
    let revision = |e: &Editor| e.library.as_ref().unwrap().session.query_revision();
    let before = revision(&e);
    e.synced(result(BatchChange::Settings(Default::default()), false));
    assert!(revision(&e) > before, "Sync");
    let before = revision(&e);
    e.undo();
    assert!(revision(&e) > before, "its Undo");
    let before = revision(&e);
    e.synced(result(BatchChange::AutoSettings, false));
    assert!(revision(&e) > before, "Auto Settings");
    let before = revision(&e);
    e.synced(result(BatchChange::AutoSettings, true));
    assert_eq!(revision(&e), before, "cancelled");
    Ok(())
}

/// Issue 998: editing a smart collection's rules is one step Undo and Redo
/// take back and forth.
#[test]
fn smart_collection_rule_edits_undo_and_redo() -> anyhow::Result<()> {
    use crate::collection_rules::{Combine, Group, Item, Rule, lua::Value};
    let (_d, mut e, _) = editor_with_catalog(&["a.RAF"])?;
    let rules = |n: f64| {
        Group::new(
            Combine::Intersect,
            vec![Item::Rule(Rule::new(
                "rating",
                ">=",
                Some(Value::Number(n)),
            ))],
        )
    };
    let library = e.library.as_mut().unwrap();
    let id = library.session.create_smart_collection(
        "Rated",
        None,
        &crate::catalog::SmartRules::new(&rules(1.)),
    )?;
    library.edit_smart_collection(id);
    let mut dialog = library.smart_editor.take().unwrap();
    dialog.rules = rules(3.);
    library.save_smart_editor(&dialog)?;
    e.sync_undo();
    let stored = |e: &Editor| {
        e.library
            .as_ref()
            .unwrap()
            .session
            .catalog()
            .smart_rules()
            .unwrap()[&id]
            .rules()
            .unwrap()
    };
    assert_eq!(stored(&e), rules(3.));
    e.undo();
    assert_eq!(stored(&e), rules(1.));
    e.redo();
    assert_eq!(stored(&e), rules(3.));
    Ok(())
}

/// The smart collection editor is drawn wherever it is opened, an empty
/// catalog's Library and Develop included, so the keys it holds always have
/// a window to go to (Escape closes it).
#[test]
fn the_smart_collection_editor_shows_in_an_empty_library_and_in_develop() -> anyhow::Result<()> {
    for module in [Module::Library, Module::Develop] {
        let (_d, mut e, _) = editor_with_catalog(&[])?;
        e.module = module;
        e.library.as_mut().unwrap().new_smart_collection();
        assert!(e.smart_editor_open());
        let ctx = e.context.clone();
        let escape = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: Some(egui::Key::Escape),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        for events in [Vec::new(), vec![escape]] {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1200., 800.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| e.draw(ui),
            );
            output.textures_delta.clear();
        }
        assert!(!e.smart_editor_open(), "{module:?}");
    }
    Ok(())
}

/// The first rules saved for a smart collection an older release imported
/// without rules are undone back to none.
#[test]
fn undoing_the_first_rules_of_a_smart_collection_leaves_it_without() -> anyhow::Result<()> {
    use crate::collection_rules::{Combine, Group, Item, Rule, lua::Value};
    let (_d, mut e, _) = editor_with_catalog(&["a.RAF"])?;
    let library = e.library.as_mut().unwrap();
    let id: i64 = library
        .session
        .catalog_mut_for_tests()
        .db_for_tests()
        .query_row(
            "INSERT INTO collections(name, parent, kind)
             VALUES ('Old', NULL, 'com.adobe.ag.library.smart_collection') RETURNING id",
            [],
            |r| r.get(0),
        )?;
    let id = crate::catalog::CollectionId(id);
    library.session.reload()?;
    library.edit_smart_collection(id);
    let mut dialog = library.smart_editor.take().unwrap();
    dialog.rules = Group::new(
        Combine::Intersect,
        vec![Item::Rule(Rule::new(
            "rating",
            ">=",
            Some(Value::Number(2.)),
        ))],
    );
    library.save_smart_editor(&dialog)?;
    e.sync_undo();
    let has_rules = |e: &Editor| {
        e.library
            .as_ref()
            .unwrap()
            .session
            .catalog()
            .smart_rules()
            .unwrap()
            .contains_key(&id)
    };
    assert!(has_rules(&e));
    e.undo();
    assert!(!has_rules(&e));
    e.redo();
    assert!(has_rules(&e));
    Ok(())
}
