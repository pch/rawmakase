//! Editor tests of a photo opened from its editable preview
//! (docs/editable-previews.md): when the Library lets Develop open an offline
//! photo from its copy, what Develop turns off for it, and what its saves are
//! checked against. The decoder is stood in for
//! ([`crate::app::worker::editable_preview::stand_in`]).
use crate::app::library::{Refusal, Unusable};
use crate::app::worker::OpenedFrom;
use crate::app::worker::editable_preview::stand_in::{self, StandIn};
use crate::app::*;
use crate::catalog::PhotoId;
use crate::storage::Identity;
use std::path::{Path, PathBuf};

/// An editor with a catalog of one RAW, opened in the Library, whose editable
/// preview was built from the file as it is now.
struct Fixture {
    _dir: tempfile::TempDir,
    e: Editor,
    id: PhotoId,
    /// The original's path.
    path: PathBuf,
    /// The original's identity the catalog recorded for the copy.
    recorded: Identity,
    /// The copy's file.
    copy: PathBuf,
}

fn fixture() -> anyhow::Result<Fixture> {
    let (dir, mut e, ids) = testing::editor_with_catalog(&["a.dng"])?;
    let id = ids[0];
    e.copies = StandIn::copies();
    let library = e.library.as_mut().unwrap();
    let path = library.photo(id).unwrap().path.clone();
    // A RAW that decodes, for the tests that open the original.
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/charts/synthetic-d65.dng"),
        &path,
    )?;
    let recorded = Identity::read(&path)?;
    let file = crate::catalog::editable_preview_file_name(id, &recorded);
    let copy = library
        .session
        .catalog()
        .editable_previews_folder()
        .join(&file);
    stand_in::write(&copy, &recorded)?;
    library
        .session
        .catalog_mut_for_tests()
        .record_editable_preview(id, &recorded)?;
    library.editable_previews_changed();
    Ok(Fixture {
        _dir: dir,
        e,
        id,
        path,
        recorded,
        copy,
    })
}

/// Takes the original away, as a drive unplugged would.
fn take_away(path: &Path) -> anyhow::Result<PathBuf> {
    let away = path.with_extension("away");
    std::fs::rename(path, &away)?;
    Ok(away)
}

fn develop_from(e: &Editor, id: PhotoId) -> Result<OpenedFrom, Refusal> {
    let library = e.library.as_ref().unwrap();
    let photo = library.photo(id).unwrap();
    library.develop_from(photo, photo.path.is_file())
}

/// Delivers the loader's events until the open photo has loaded.
fn settle_load(e: &mut Editor) {
    let ctx = e.context.clone();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while e.document.loading() && std::time::Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(5));
        e.events(&ctx);
    }
    assert!(!e.document.loading(), "the photo never loaded");
}

/// The exposure of the edit saved for photo `id` of the original `identity`.
fn saved_exposure(e: &Editor, id: PhotoId, identity: &Identity) -> Option<f32> {
    let library = e.library.as_ref().unwrap();
    library
        .session
        .catalog()
        .edit_record(id)
        .unwrap()
        .saved_for(identity)
        .unwrap()
        .map(|saved| saved.recipe.exposure)
}

/// Opens the photo from its copy, its original away.
fn opened_offline() -> anyhow::Result<(Fixture, PathBuf)> {
    let mut f = fixture()?;
    let away = take_away(&f.path)?;
    f.e.develop_catalog_photo(f.id);
    settle_load(&mut f.e);
    assert_eq!(f.e.module, Module::Develop, "{}", f.e.messages.line());
    assert!(f.e.document.full().is_some(), "{}", f.e.messages.line());
    Ok((f, away))
}

#[test]
fn an_offline_photo_opens_from_its_copy_only_while_the_copy_can_stand_in() -> anyhow::Result<()> {
    let mut f = fixture()?;
    // Online: from the original, whatever the copy.
    assert_eq!(develop_from(&f.e, f.id), Ok(OpenedFrom::Original));
    // Absent, with an up-to-date copy: from the copy, as recorded.
    let away = take_away(&f.path)?;
    match develop_from(&f.e, f.id) {
        Ok(OpenedFrom::EditablePreview(source)) => {
            assert_eq!(source.file, f.copy);
            assert_eq!(source.original, f.recorded);
        }
        other => panic!("{other:?}"),
    }
    // Something at the path that isn't the file (here a folder): never absent,
    // so the copy doesn't stand in.
    std::fs::create_dir(&f.path)?;
    assert_eq!(
        develop_from(&f.e, f.id),
        Err(Refusal::Copy(Unusable::NotAbsent))
    );
    std::fs::remove_dir(&f.path)?;
    // Its file missing: out of date.
    let kept = f.copy.with_extension("kept");
    std::fs::rename(&f.copy, &kept)?;
    assert_eq!(
        develop_from(&f.e, f.id),
        Err(Refusal::Copy(Unusable::OutOfDate))
    );
    std::fs::rename(&kept, &f.copy)?;
    // Marked out of date in the catalog.
    let library = f.e.library.as_mut().unwrap();
    library
        .session
        .catalog_mut_for_tests()
        .mark_editable_preview_out_of_date(f.id)?;
    assert_eq!(
        develop_from(&f.e, f.id),
        Err(Refusal::Copy(Unusable::OutOfDate))
    );
    // None recorded: offline, as before.
    let library = f.e.library.as_mut().unwrap();
    library
        .session
        .catalog_mut_for_tests()
        .discard_editable_previews(&[f.id])?;
    assert_eq!(develop_from(&f.e, f.id), Err(Refusal::Offline));
    std::fs::rename(&away, &f.path)?;
    Ok(())
}

/// Each refusal says why, in the top bar, and nothing opens.
#[test]
fn opening_an_offline_photo_whose_copy_cant_stand_in_is_refused_with_the_reason()
-> anyhow::Result<()> {
    let mut f = fixture()?;
    let _away = take_away(&f.path)?;
    f.e.library
        .as_mut()
        .unwrap()
        .session
        .catalog_mut_for_tests()
        .mark_editable_preview_out_of_date(f.id)?;
    f.e.develop_catalog_photo(f.id);
    assert_eq!(f.e.module, Module::Library);
    assert_eq!(f.e.document.catalog_photo, None);
    assert!(
        f.e.messages.line().contains("out of date"),
        "{}",
        f.e.messages.line()
    );
    Ok(())
}

#[test]
fn develop_opens_an_offline_photo_from_its_copy_with_its_ai_tools_off() -> anyhow::Result<()> {
    let (mut f, _away) = opened_offline()?;
    let e = &mut f.e;
    assert!(e.document.opened_from_copy());
    // Its edit is the original's: the identity the catalog recorded.
    assert_eq!(e.document.file.as_ref(), Some(&f.recorded));
    assert_eq!(
        e.selection_unavailable(),
        Some("Connect the original to use AI masks")
    );
    assert_eq!(
        e.camera_look_unavailable(),
        Some("Connect the original to fit a Camera Look")
    );
    assert!(!e.can_fit_camera_look());
    assert_eq!(
        e.ai_remove_unavailable().as_deref(),
        Some("Connect the original to use AI Remove")
    );
    // AI Denoise on in the edit stays as saved, and asks for nothing.
    e.document.edit.setup_mut().ai_denoise.enabled = true;
    let image = e.document.full().unwrap().clone();
    let geometry = crate::develop::Geometry::new(&image, e.document.edit.recipe(), 0);
    e.request_ai_denoise(&image, &geometry, None);
    assert!(e.document.ai.status().is_none());
    assert!(e.document.edit.recipe().ai_denoise.enabled);
    // Labelled as a preview in Develop's status row.
    let ctx = e.context.clone();
    ctx.enable_accesskit();
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200., 800.),
            )),
            ..Default::default()
        },
        |ui| e.draw(ui),
    );
    output.textures_delta.clear();
    let tree = output.platform_output.accesskit_update.expect("accessible");
    assert!(testing::labelled(&tree, OpenedFrom::EDITABLE_PREVIEW).is_some());
    Ok(())
}

/// An edit made from the copy is the original's: saved against the identity
/// the catalog recorded while nothing is at the original's path, before navigating
/// (flush) and in the background (autosave) alike.
#[test]
fn edits_save_as_the_originals_while_it_is_absent() -> anyhow::Result<()> {
    let (mut f, away) = opened_offline()?;
    let ctx = f.e.context.clone();
    f.e.document.edit.setup_mut().exposure = 0.7;
    f.e.document.edit.save_state_mut().mark_changed();
    assert!(f.e.flush(), "{}", f.e.messages.line());
    assert_eq!(saved_exposure(&f.e, f.id, &f.recorded), Some(0.7));
    // In the background too.
    f.e.document.edit.setup_mut().exposure = 0.9;
    *f.e.document.edit.save_state_mut() = save_state::SaveState::Pending(
        std::time::Instant::now() - std::time::Duration::from_secs(1),
    );
    f.e.autosave(&ctx);
    while f.e.autosave.busy() || f.e.document.edit.save_state().needs_save() {
        std::thread::sleep(std::time::Duration::from_millis(5));
        f.e.autosave(&ctx);
        if matches!(
            f.e.document.edit.save_state(),
            save_state::SaveState::Failed { .. }
        ) {
            break;
        }
    }
    assert_eq!(saved_exposure(&f.e, f.id, &f.recorded), Some(0.9));
    // Back as the same file: checked against it, and still the photo's edit.
    std::fs::rename(&away, &f.path)?;
    f.e.document.edit.setup_mut().exposure = 1.1;
    f.e.document.edit.save_state_mut().mark_changed();
    assert!(f.e.flush(), "{}", f.e.messages.line());
    assert_eq!(saved_exposure(&f.e, f.id, &f.recorded), Some(1.1));
    Ok(())
}

/// Once another file is at the original's path, the edit made from the copy
/// is never saved over: the file is read, and it isn't the photo.
#[test]
fn edits_are_refused_once_another_file_is_at_the_originals_path() -> anyhow::Result<()> {
    let (mut f, _away) = opened_offline()?;
    let ctx = f.e.context.clone();
    f.e.document.edit.setup_mut().exposure = 0.7;
    f.e.document.edit.save_state_mut().mark_changed();
    assert!(f.e.flush());
    std::fs::write(&f.path, b"another photo")?;
    f.e.document.edit.setup_mut().exposure = 1.5;
    f.e.document.edit.save_state_mut().mark_changed();
    assert!(!f.e.flush());
    assert_eq!(saved_exposure(&f.e, f.id, &f.recorded), Some(0.7));
    // The background save is checked the same way.
    *f.e.document.edit.save_state_mut() = save_state::SaveState::Pending(
        std::time::Instant::now() - std::time::Duration::from_secs(1),
    );
    f.e.autosave(&ctx);
    while f.e.autosave.busy() {
        std::thread::sleep(std::time::Duration::from_millis(5));
        f.e.autosave(&ctx);
    }
    assert!(f.e.document.edit.save_state().needs_save());
    assert_eq!(saved_exposure(&f.e, f.id, &f.recorded), Some(0.7));
    Ok(())
}

/// A photo opened from its original tells the catalog which file it is: a copy
/// built from another file goes out of date, and is never opened.
#[test]
fn opening_a_changed_original_puts_its_copy_out_of_date() -> anyhow::Result<()> {
    let mut f = fixture()?;
    // Another version of the photo: longer, so not the one the copy is of.
    let mut file = std::fs::OpenOptions::new().append(true).open(&f.path)?;
    std::io::Write::write_all(&mut file, &[0; 16])?;
    drop(file);
    f.e.develop_catalog_photo(f.id);
    settle_load(&mut f.e);
    let library = f.e.library.as_ref().unwrap();
    assert!(library.editable_preview(f.id).unwrap().out_of_date);
    std::fs::remove_file(&f.path)?;
    assert_eq!(
        develop_from(&f.e, f.id),
        Err(Refusal::Copy(Unusable::OutOfDate))
    );
    Ok(())
}

/// Upright analyses the copy: it is final as it opens, reduced, with no
/// full-size image after it, so waiting for one, as an original's half-size
/// draft does, would never analyse it.
#[test]
fn upright_is_analysed_on_the_copy() -> anyhow::Result<()> {
    use crate::model::transform::UprightMode;
    let (mut f, _away) = opened_offline()?;
    let e = &mut f.e;
    assert!(e.document.full().unwrap().fast, "a reduced image");
    e.document.edit.setup_mut().upright.mode = UprightMode::Auto;
    assert!(e.document.edit.recipe().upright.needs_analysis());
    e.ensure_analysis();
    assert!(e.document.upright.is_running());
    Ok(())
}
