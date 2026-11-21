//! Editor tests of `profile_manager`: Manage Profiles on a temporary library, its
//! changes run as the window asks for them and their results delivered as events.
use super::*;
use crate::app::testing::*;
use crate::app::*;
use crate::camera_profiles::LutSpace;
use crate::catalog::file_ops::{TrashError, TrashReceipt};
use std::path::Path;

/// A Trash that moves files into a folder, or refuses every file.
struct Bin(Option<PathBuf>);
impl Trash for Bin {
    fn trash(&self, path: &Path) -> Result<TrashReceipt, TrashError> {
        let Some(bin) = &self.0 else {
            return Err(TrashError::Unsupported);
        };
        let to = bin.join(path.file_name().unwrap());
        std::fs::rename(path, &to).map_err(|e| TrashError::Failed(e.to_string()))?;
        Ok(TrashReceipt { location: to })
    }
}

/// A library of three LUT looks, two in "Film" and one in "Moody".
fn library(temp: &Path) -> Library {
    let mut picks = Vec::new();
    for (folder, name, shift) in [
        ("Film", "Portra", 0.1),
        ("Film", "Ektar", 0.2),
        ("Moody", "Dusk", 0.3),
    ] {
        let dir = temp.join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.cube"));
        let mut text = String::from("LUT_3D_SIZE 2\n");
        for b in [0., 1.] {
            for g in [0., 1.] {
                for r in [0f32, 1.] {
                    text += &format!("{} {g} {b}\n", (r + shift).min(1.));
                }
            }
        }
        std::fs::write(&path, text).unwrap();
        picks.push(path);
    }
    let dir = temp.join("library");
    crate::camera_profiles::import_into_dir(&picks, &dir, LutSpace::Srgb).unwrap();
    Library::at(dir)
}

/// Delivers events until the manager's job has finished.
fn settle(editor: &mut Editor, ctx: &egui::Context) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while editor.managing_profiles() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
        editor.events(ctx);
    }
    assert!(!editor.managing_profiles(), "the job never finished");
}
fn manager(editor: &Editor) -> &ProfileManager {
    editor
        .profile_manager
        .as_ref()
        .expect("Manage Profiles is open")
}
fn names(editor: &Editor) -> Vec<(String, String)> {
    manager(editor)
        .entries
        .iter()
        .flatten()
        .map(|e| (e.name.clone(), e.group.clone()))
        .collect()
}
fn file(editor: &Editor, name: &str) -> PathBuf {
    manager(editor)
        .entries
        .iter()
        .flatten()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("no {name}"))
        .file
        .clone()
}
fn open(temp: &Path, trash: Option<PathBuf>) -> (egui::Context, Editor) {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, Default::default(), None);
    editor.open_profile_manager_on(library(temp), Arc::new(Bin(trash)), &ctx);
    settle(&mut editor, &ctx);
    (ctx, editor)
}

/// Opening lists the library; a rename, a move and a group renamed through the
/// window change the files and the listing, and a hidden group keeps hidden under
/// its new name.
#[test]
fn looks_are_renamed_and_regrouped() {
    let temp = tempfile::tempdir().unwrap();
    let (ctx, mut editor) = open(temp.path(), None);
    assert_eq!(
        names(&editor),
        [
            ("Dusk".into(), "Moody".into()),
            ("Ektar".into(), "Film".into()),
            ("Portra".into(), "Film".into())
        ]
    );
    let portra = file(&editor, "Portra");
    editor.manage_profiles(
        Op::Rename {
            file: portra.clone(),
            name: "Kodak Portra".into(),
        },
        &ctx,
    );
    assert!(editor.managing_profiles());
    settle(&mut editor, &ctx);
    assert_eq!(file(&editor, "Kodak Portra"), portra);
    editor.manage_profiles(
        Op::Move {
            files: vec![file(&editor, "Dusk")],
            group: "Film".into(),
            renaming: None,
        },
        &ctx,
    );
    settle(&mut editor, &ctx);
    editor.profile_menu.hidden_groups.insert("Film".into());
    let film: Vec<PathBuf> = ["Dusk", "Ektar", "Kodak Portra"]
        .map(|n| file(&editor, n))
        .into();
    editor.manage_profiles(
        Op::Move {
            files: film,
            group: "Analog".into(),
            renaming: Some("Film".into()),
        },
        &ctx,
    );
    settle(&mut editor, &ctx);
    assert_eq!(
        names(&editor),
        [
            ("Dusk".into(), "Analog".into()),
            ("Ektar".into(), "Analog".into()),
            ("Kodak Portra".into(), "Analog".into())
        ]
    );
    assert_eq!(
        editor.profile_menu.hidden_groups,
        BTreeSet::from(["Analog".to_string()])
    );
    assert!(manager(&editor).error.is_none());
}

/// A refused rename leaves the file and says why in the window; Move to Trash
/// moves the selected files to the Trash, and one the Trash refuses stays and is
/// reported.
#[test]
fn refusals_are_said_and_removal_goes_to_the_trash() {
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let (ctx, mut editor) = open(temp.path(), Some(bin.clone()));
    let portra = file(&editor, "Portra");
    editor.manage_profiles(
        Op::Rename {
            file: portra.clone(),
            name: "Ektar".into(),
        },
        &ctx,
    );
    settle(&mut editor, &ctx);
    let error = manager(&editor).error.clone().expect("a refusal");
    assert!(error.contains("Another profile is named Ektar"), "{error}");
    assert_eq!(file(&editor, "Portra"), portra);
    editor
        .profile_manager
        .as_mut()
        .unwrap()
        .selected
        .insert(portra.clone());
    editor.manage_profiles(Op::Trash(vec![portra.clone()]), &ctx);
    settle(&mut editor, &ctx);
    assert!(!portra.exists() && bin.join(portra.file_name().unwrap()).exists());
    assert!(
        manager(&editor).selected.is_empty(),
        "the selection follows the files"
    );
    assert_eq!(names(&editor).len(), 2);

    let temp = tempfile::tempdir().unwrap();
    let (ctx, mut editor) = open(temp.path(), None);
    let dusk = file(&editor, "Dusk");
    editor.manage_profiles(Op::Trash(vec![dusk.clone()]), &ctx);
    settle(&mut editor, &ctx);
    assert!(dusk.exists());
    let error = manager(&editor).error.clone().expect("a failure");
    assert!(error.contains("Its volume has no Trash"), "{error}");
}

/// While an import writes to the library, a change is refused and said; listing
/// still runs. While a change runs, an import waits.
#[test]
fn changes_and_imports_never_overlap() {
    let temp = tempfile::tempdir().unwrap();
    let (ctx, mut editor) = open(temp.path(), None);
    let portra = file(&editor, "Portra");
    editor.importing = Some(Arc::new(
        crate::app::presets::bulk_import::ImportProgress::for_test(ctx.clone()),
    ));
    editor.manage_profiles(
        Op::Rename {
            file: portra.clone(),
            name: "Kodak Portra".into(),
        },
        &ctx,
    );
    assert!(!editor.managing_profiles());
    assert!(
        manager(&editor)
            .error
            .as_deref()
            .is_some_and(|e| e.contains("An import"))
    );
    editor.importing = None;
    editor.manage_profiles(
        Op::Rename {
            file: portra,
            name: "Kodak Portra".into(),
        },
        &ctx,
    );
    assert!(editor.managing_profiles());
    editor.import(
        crate::app::presets::bulk_import::ImportKind::CameraProfiles,
        vec![temp.path().join("Film/Portra.cube")],
        &ctx,
    );
    assert!(editor.importing.is_none(), "the import waits");
    settle(&mut editor, &ctx);
    assert!(names(&editor).iter().any(|(n, _)| n == "Kodak Portra"));
}

/// The window opens from the Profile menu's Manage Profiles… and closes with Done.
#[test]
fn the_window_opens_from_the_profile_menu() {
    use egui::accesskit::Role;
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    ctx.all_styles_mut(|s| s.animation_time = 0.);
    let mut editor = Editor::with_context(&ctx, None, Default::default(), None);
    let m = crate::camera_data::Metadata {
        make: "Test".into(),
        model: "Camera".into(),
        cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
        width: 64,
        height: 48,
        ..Default::default()
    };
    let profiles = crate::camera_profiles::listed_from(&[], &m).profiles;
    editor.document.reset(None);
    let (generation, _) = editor.document.start_load();
    for event in [
        Event::Header(Box::new(worker::LoadedHeader {
            from: Default::default(),
            id: generation,
            file: None,
            path: "photo.dng".into(),
            metadata: m,
            recipe: Recipe::default(),
            export: Default::default(),
            status: "Original".into(),
        })),
        Event::Profiles {
            id: generation,
            profiles,
            errors: Vec::new(),
            groups: Default::default(),
            imported: false,
        },
    ] {
        editor.tx.send(event).unwrap();
    }
    editor.events(&ctx);
    controls_frame(&ctx, &mut editor, vec![]);
    let update = controls_frame(&ctx, &mut editor, vec![]);
    let profile = update
        .nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.role() == Role::ComboBox && n.value() == Some("RAWmakase Color"))
        .cloned()
        .expect("the Profile menu");
    click(&ctx, &mut editor, &profile);
    let menu = controls_frame(&ctx, &mut editor, vec![]);
    let item = labelled(&menu, "Manage Profiles…")
        .expect("Manage Profiles…")
        .clone();
    click(&ctx, &mut editor, &item);
    assert!(editor.profile_manager_open());
    // Replace the user's library, which the menu opened, before its listing lands.
    settle(&mut editor, &ctx);
    let temp = tempfile::tempdir().unwrap();
    editor.profile_manager = None;
    editor.open_profile_manager_on(library(temp.path()), Arc::new(Bin(None)), &ctx);
    settle(&mut editor, &ctx);
    let frame = |editor: &mut Editor, events| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200., 900.),
                )),
                events,
                ..Default::default()
            },
            |ui| editor.profile_manager_window(ui.ctx()),
        );
        output.textures_delta.clear();
        output
            .platform_output
            .accesskit_update
            .expect("accessibility is on")
    };
    frame(&mut editor, vec![]);
    let shown = frame(&mut editor, vec![]);
    assert!(labelled(&shown, "Portra").is_some());
    let done = labelled(&shown, "Done").expect("Done").clone();
    let [down, up] = press_and_release(centre(&done));
    frame(&mut editor, down);
    frame(&mut editor, up);
    assert!(!editor.profile_manager_open());
}

/// Closing the window while a change runs keeps the change in charge of the
/// library: imports still wait, a window opened again starts no second change and
/// shows the first one's result.
#[test]
fn closing_the_window_keeps_a_running_change_in_charge() {
    let temp = tempfile::tempdir().unwrap();
    let (ctx, mut editor) = open(temp.path(), None);
    let portra = file(&editor, "Portra");
    let library = manager(&editor).library.clone();
    editor.manage_profiles(
        Op::Rename {
            file: portra.clone(),
            name: "Kodak Portra".into(),
        },
        &ctx,
    );
    editor.profile_manager = None;
    assert!(editor.managing_profiles());
    editor.import(
        crate::app::presets::bulk_import::ImportKind::CameraProfiles,
        vec![temp.path().join("Film/Portra.cube")],
        &ctx,
    );
    assert!(editor.importing.is_none(), "the import waits");
    editor.open_profile_manager_on(library, Arc::new(Bin(None)), &ctx);
    let generation = editor.profile_jobs.generation;
    editor.manage_profiles(
        Op::Rename {
            file: portra.clone(),
            name: "Portra 400".into(),
        },
        &ctx,
    );
    assert_eq!(
        editor.profile_jobs.generation, generation,
        "no second change"
    );
    settle(&mut editor, &ctx);
    assert_eq!(file(&editor, "Kodak Portra"), portra);
}
