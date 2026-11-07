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

/// From the Library, the photo open in Develop can be a Sync's target: it opens
/// again with what the Sync saved, so its stale edit is not saved over it.
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

/// A slider held on the open photo while a Sync that changes it runs.
struct HeldDrag {
    _d: tempfile::TempDir,
    e: Editor,
    ids: Vec<crate::catalog::PhotoId>,
    path: std::path::PathBuf,
}
impl HeldDrag {
    /// Photo A open in Develop, saved at exposure 0.2; a Sync begins and,
    /// while it runs, a drag takes A's exposure to 0.9 and is still held.
    fn new() -> anyhow::Result<Self> {
        let (_d, e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
        let path = e
            .library
            .as_ref()
            .unwrap()
            .photo(ids[0])
            .unwrap()
            .path
            .clone();
        let mut held = Self { _d, e, ids, path };
        held.write(0.2)?;
        let e = &mut held.e;
        e.module = Module::Develop;
        e.document.catalog_photo = Some(held.ids[0]);
        e.document.opened_at(held.path.clone());
        e.document.edit.setup_mut().exposure = 0.2;
        assert!(e.activity.begin_sync(std::sync::Arc::new(
            crate::app::library::sync::BatchProgress::new("Sync", 1)
        )));
        let frame = e.document.edit.begin();
        e.document.edit.recipe_mut().exposure = 0.9;
        e.document
            .edit
            .finish(frame, crate::edit_session::Gesture::Held);
        assert!(e.document.edit.history().in_gesture());
        Ok(held)
    }
    /// Writes A's edit at `exposure`, as the Sync's worker does.
    fn write(&self, exposure: f32) -> anyhow::Result<()> {
        let library = self.e.library.as_ref().unwrap();
        let recipe = crate::model::recipe::Recipe {
            exposure,
            ..Default::default()
        };
        crate::catalog::Catalog::open(library.session.catalog().location())?.save_edit(
            self.ids[0],
            &self.path,
            &crate::storage::Identity::read(&self.path)?,
            &recipe,
            &Default::default(),
            crate::catalog::HistoryUpdate::Keep,
        )
    }
    /// The Sync's result: A changed from 0.2 to `after`, or, `cancelled`,
    /// nothing changed.
    fn result(&self, after: f32, cancelled: bool) -> crate::app::library::sync::SyncResult {
        let history = || crate::catalog::SavedHistory {
            origin: Default::default(),
            steps: Vec::new(),
            applied: 0,
        };
        let synced = crate::app::library::sync::Synced {
            id: self.ids[0],
            path: self.path.clone(),
            before: crate::app::library::sync::EditBefore::Saved(Box::new(
                crate::model::recipe::Recipe {
                    exposure: 0.2,
                    ..Default::default()
                },
            )),
            after: crate::model::recipe::Recipe {
                exposure: after,
                ..Default::default()
            },
            history: history(),
            history_before: history(),
            identity: crate::storage::Identity::read(&self.path).unwrap(),
        };
        crate::app::library::sync::SyncResult {
            change: crate::app::library::sync::BatchChange::Settings(Default::default()),
            catalog: self
                .e
                .library
                .as_ref()
                .unwrap()
                .session
                .catalog()
                .location()
                .clone(),
            synced: if cancelled { Vec::new() } else { vec![synced] },
            failed: Vec::new(),
            notes: Vec::new(),
            cancelled,
        }
    }
    fn saved_exposure(&self) -> f32 {
        let library = self.e.library.as_ref().unwrap();
        let edit = library.session.catalog().load_edit(self.ids[0], &self.path);
        edit.unwrap().unwrap().recipe.exposure
    }
}

/// The drag began on the settings from before the Sync: opening the photo
/// again with what the Sync wrote must not save the drag over it.
#[test]
fn a_drag_held_while_a_sync_of_the_open_photo_finishes_is_not_saved_over_it() -> anyhow::Result<()>
{
    let mut held = HeldDrag::new()?;
    held.write(0.5)?;
    let result = held.result(0.5, false);
    held.e.synced(result);
    assert_eq!(held.saved_exposure(), 0.5, "the Sync's edit was replaced");
    let e = &held.e;
    assert!(e.document.loading(), "the open photo opens again");
    assert!(!e.document.edit.history().in_gesture());
    assert!(!e.document.edit.save_state().needs_save());
    let line = e.messages.line();
    assert!(line.contains("drag"), "{line}");
    // Nothing of the drag reaches Undo: the Sync is the last change.
    assert_eq!(held.e.undo_log.len(), (1, 0));
    Ok(())
}

/// A Sync cancelled, or one that did not change the open photo, leaves the
/// drag going, and it is saved once released.
#[test]
fn a_drag_held_while_a_sync_is_cancelled_or_leaves_the_photo_is_kept() -> anyhow::Result<()> {
    for cancelled in [true, false] {
        let mut held = HeldDrag::new()?;
        let mut result = held.result(0.5, cancelled);
        // Not cancelled: the Sync failed for the open photo, and wrote nothing.
        if !cancelled {
            result.failed.push(crate::app::library::sync::SyncFailure {
                name: "a.RAF".into(),
                reason: "it could not be read".into(),
            });
            result.synced.clear();
        }
        held.e.synced(result);
        let e = &mut held.e;
        assert!(e.document.edit.history().in_gesture(), "{cancelled}");
        assert!(!e.document.loading());
        let frame = e.document.edit.begin();
        e.document
            .edit
            .finish(frame, crate::edit_session::Gesture::Released);
        assert!(e.flush(), "{cancelled}");
        assert_eq!(held.saved_exposure(), 0.9, "{cancelled}");
    }
    Ok(())
}
