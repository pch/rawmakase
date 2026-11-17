//! The listing of Adobe's folders: what the Profile menu offers from it, and
//! how it follows the Lightroom extension and its program's session. The
//! program is the bundled extension's (`rawmakase_lightroom`), run as this
//! test binary started again with [`PROGRAM`] set, listing a fake install.
use super::*;
use crate::app::extensions::Host;
use crate::app::testing::{
    community_editor, controls_frame, key_down, labelled, open_profile_menu,
};
use crate::camera_data::Metadata;
use eframe::egui;
use rawmakase_lightroom::fake::{Layout, install};
use std::path::Path;
use std::time::{Duration, Instant};

const ROOT: &str = "RAWMAKASE_LIGHTROOM_TEST_ROOT";

/// The Lightroom extension's program, listing the fake install under
/// [`ROOT`] in macOS's layout, when this binary is started as it; a test
/// that does nothing otherwise.
#[test]
fn lightroom_program() {
    let Some(root) = std::env::var_os(ROOT) else {
        return;
    };
    let root = Path::new(&root);
    let roots = rawmakase_lightroom::Roots::macos(root, Some(root.join("home")));
    let code = i32::from(rawmakase_lightroom::extension(roots).run().is_err());
    std::process::exit(code);
}

/// A host whose Lightroom extension lists the fake install under `root`, in
/// macOS's layout ([`install`]), with its data folder there too.
pub(in crate::app) fn host(root: &Path) -> Host {
    let program = rawmakase_extensions::session::Launch {
        program: std::env::current_exe().expect("the test binary"),
        args: [
            "app::extensions::adobe_files::tests::lightroom_program",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ]
        .map(Into::into)
        .to_vec(),
        dir: std::path::PathBuf::new(),
        env: vec![(ROOT.into(), root.into())],
        log: std::path::PathBuf::new(),
    };
    Host::with_bundled_for_tests(
        &root.join("data"),
        LIGHTROOM,
        program,
        rawmakase_extensions::session::QUEUE,
    )
}

/// A Develop panel for a photo of the [`example`] camera, with the Lightroom
/// extension listing a fake install.
fn fixture() -> (tempfile::TempDir, egui::Context, Editor) {
    let dir = tempfile::tempdir().unwrap();
    install(dir.path(), Layout::MacOs).unwrap();
    let ctx = egui::Context::default();
    let mut editor = community_editor(&ctx, "Z1", false);
    editor.document.metadata = Some(example());
    editor.extensions = host(dir.path());
    (dir, ctx, editor)
}

/// A camera only the fake install has profiles for: the app's own library,
/// which these tests share with this computer's, may hold real cameras'.
fn example() -> Metadata {
    Metadata {
        make: "Example".into(),
        model: "Z1".into(),
        ..Default::default()
    }
}

fn sony() -> Metadata {
    Metadata {
        make: "Sony".into(),
        model: "ILCE-7M2".into(),
        ..Default::default()
    }
}

/// Polls the host until `done` holds.
pub(in crate::app) fn poll_until(editor: &mut Editor, done: impl Fn(&mut Editor) -> bool) {
    let started = Instant::now();
    while !done(editor) {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "never happened"
        );
        let (outcomes, _) = editor.extensions_poll(Instant::now());
        editor.extension_outcomes(outcomes);
        std::thread::sleep(Duration::from_millis(5));
    }
}

const ITEM: &str = "Import Adobe profiles for this camera";

/// The Profile menu, opened and closed again.
fn profile_menu(ctx: &egui::Context, editor: &mut Editor) -> egui::accesskit::TreeUpdate {
    let menu = open_profile_menu(ctx, editor);
    controls_frame(
        ctx,
        editor,
        key_down(egui::Key::Escape, egui::Modifiers::NONE),
    );
    menu
}

/// For the fake install, the files `camera_profiles::adobe_installed` offered
/// before the listing came from the extension, by camera, relative to Camera
/// Raw's shared folder: frozen from that code run on the same install.
#[test]
fn the_profile_menu_offers_what_it_offered_before() {
    let standard =
        |camera: &str| format!("CameraProfiles/Adobe Standard/{camera} Adobe Standard.dcp");
    let matching =
        |camera: &str, name: &str| format!("CameraProfiles/Camera/{camera}/{camera} {name}.dcp");
    let pixel = vec![
        standard("Google Pixel 7 Pro"),
        standard("Google Pixel 7 Pro Telephoto Camera"),
        standard("Google Pixel 7 Pro Ultrawide Camera"),
    ];
    let cases: [(&str, &str, f32, Vec<String>); 7] = [
        (
            "Sony",
            "ILCE-7M2",
            0.,
            vec![
                standard("Sony ILCE-7M2"),
                matching("Sony ILCE-7M2", "Camera Standard"),
                matching("Sony ILCE-7M2", "Camera Vivid"),
            ],
        ),
        ("LEICA", "M10", 0., vec![standard("LEICA M10")]),
        (
            "Canon",
            "Canon EOS M10",
            0.,
            vec![standard("Canon EOS M10")],
        ),
        // A phone's every module, whichever took the photo.
        ("Google", "Pixel 7 Pro", 24., pixel.clone()),
        ("Google", "Pixel 7 Pro", 120., pixel.clone()),
        ("Google", "Pixel 7 Pro", 0., pixel),
        ("Fujifilm", "X-T5", 0., vec![]),
    ];
    for layout in [Layout::MacOs, Layout::Windows] {
        let dir = tempfile::tempdir().unwrap();
        let roots = install(dir.path(), layout).unwrap();
        let shared = roots.shared.clone().unwrap();
        let files = rawmakase_lightroom::list(&roots);
        let library = dir.path().join("library");
        for (make, model, focal_35mm, expected) in &cases {
            let m = Metadata {
                make: (*make).into(),
                model: (*model).into(),
                focal_35mm: *focal_35mm,
                ..Default::default()
            };
            let mut offered: Vec<String> = camera_profiles_for(&files, &m, &library)
                .iter()
                .map(|p| {
                    p.strip_prefix(&shared)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/")
                })
                .collect();
            offered.sort();
            assert_eq!(&offered, expected, "{layout:?} {make} {model} {focal_35mm}");
        }
        // One imported already isn't offered again.
        std::fs::create_dir_all(&library).unwrap();
        std::fs::write(library.join("Sony ILCE-7M2 Camera Vivid.dcp"), b"").unwrap();
        assert_eq!(camera_profiles_for(&files, &sony(), &library).len(), 2);
    }
}

#[test]
fn the_profile_menu_s_item_comes_and_goes_with_the_extension() {
    let (_dir, ctx, mut editor) = fixture();
    // Asked for by the first frame; the item appears when the listing does.
    let menu = profile_menu(&ctx, &mut editor);
    assert!(labelled(&menu, ITEM).is_none());
    poll_until(&mut editor, |e| e.adobe_files.listed().is_some());
    let menu = profile_menu(&ctx, &mut editor);
    assert!(labelled(&menu, ITEM).is_some());

    // Off, the listing goes and nothing is offered.
    editor.extensions.set_enabled(LIGHTROOM, false).unwrap();
    let menu = profile_menu(&ctx, &mut editor);
    assert!(labelled(&menu, ITEM).is_none());
    assert!(editor.adobe_files.listed().is_none());

    // On again, it lists again.
    editor.extensions.set_enabled(LIGHTROOM, true).unwrap();
    profile_menu(&ctx, &mut editor);
    poll_until(&mut editor, |e| e.adobe_files.listed().is_some());
    let menu = profile_menu(&ctx, &mut editor);
    assert!(labelled(&menu, ITEM).is_some());

    // A rendered file gets no offer.
    editor.document.metadata = Some(Metadata {
        source: crate::camera_data::Source::Rendered,
        ..example()
    });
    let menu = profile_menu(&ctx, &mut editor);
    assert!(labelled(&menu, ITEM).is_none());
}

#[test]
fn the_listing_is_the_program_s_whole_answer_and_is_kept() {
    let (dir, _ctx, mut editor) = fixture();
    let roots = rawmakase_lightroom::Roots::macos(dir.path(), Some(dir.path().join("home")));
    assert!(editor.adobe_files().is_none());
    poll_until(&mut editor, |e| e.adobe_files.listed().is_some());
    assert_eq!(
        editor.adobe_files.listed().unwrap(),
        &rawmakase_lightroom::list(&roots)
    );
    let generation = editor.adobe_files.generation();
    // Asked for again, it is the same listing.
    assert!(editor.adobe_files().is_some());
    assert_eq!(editor.adobe_files.generation(), generation);
    // Listed again on request, it is a new one.
    editor.adobe_files.relist();
    poll_until(&mut editor, |e| e.adobe_files().is_some());
    assert!(editor.adobe_files.generation() > generation);
}

/// Turning the extension off and on drops the listing at once, whatever is
/// drawn meanwhile; a listing it cut short isn't left failed.
#[test]
fn turning_the_extension_off_and_on_lists_again() {
    let (_dir, _ctx, mut editor) = fixture();
    assert!(editor.adobe_files().is_none());
    poll_until(&mut editor, |e| e.adobe_files.listed().is_some());
    editor.set_extension_enabled(LIGHTROOM, false);
    editor.set_extension_enabled(LIGHTROOM, true);
    assert!(editor.adobe_files.listed().is_none());
    // Cut short by turning it off.
    assert!(editor.adobe_files().is_none());
    editor.set_extension_enabled(LIGHTROOM, false);
    editor.set_extension_enabled(LIGHTROOM, true);
    assert!(editor.adobe_files.failure().is_none());
    poll_until(&mut editor, |e| e.adobe_files().is_some());
}

/// The fake install's profiles are empty files, which the import skips as
/// unreadable: once taken, they aren't offered again.
#[test]
fn profiles_an_import_skipped_are_not_offered_again() {
    let (_dir, ctx, mut editor) = fixture();
    profile_menu(&ctx, &mut editor);
    poll_until(&mut editor, |e| e.adobe_files.listed().is_some());
    let menu = open_profile_menu(&ctx, &mut editor);
    let item = labelled(&menu, ITEM).expect("offered").clone();
    crate::app::testing::click(&ctx, &mut editor, &item);
    controls_frame(&ctx, &mut editor, vec![]);
    poll_until(&mut editor, |e| e.adobe_files.listed().is_some());
    let menu = profile_menu(&ctx, &mut editor);
    assert!(labelled(&menu, ITEM).is_none());
}

#[test]
fn a_listing_from_a_session_that_ended_is_dropped() {
    let (_dir, _ctx, mut editor) = fixture();
    // The program ends before it answers: the listing fails, and the item
    // stays away.
    assert!(editor.adobe_files().is_none());
    editor.extensions.kill_program_for_tests(LIGHTROOM);
    poll_until(&mut editor, |e| {
        matches!(e.adobe_files.state, Listing::Failed(_))
    });
    assert!(editor.adobe_profiles_for(&example()).is_empty());

    // An answer the editor no longer waits for is dropped: here, one for a
    // listing asked for before the editor asked again.
    editor.adobe_files.relist();
    assert!(editor.adobe_files().is_none());
    let Listing::Asked(current) = editor.adobe_files.state else {
        panic!("asked for")
    };
    editor.files_listed(current - 1, Ok(Default::default()));
    assert!(matches!(editor.adobe_files.state, Listing::Asked(t) if t == current));
    poll_until(&mut editor, |e| e.adobe_files.listed().is_some());
    assert!(!editor.adobe_profiles_for(&example()).is_empty());
}

#[test]
fn a_first_launch_opens_setup_once_the_listing_finds_lightroom_s() {
    for (found, opens) in [(true, true), (false, false)] {
        let (dir, _ctx, mut editor) = fixture();
        if !found {
            std::fs::remove_dir_all(dir.path().join("Library")).unwrap();
            std::fs::remove_dir_all(dir.path().join("home")).unwrap();
        }
        editor.onboarding.visible = false;
        editor.session.onboarding_done = false;
        editor.adobe_files.first_launch = true;
        let ctx = editor.context.clone();
        let started = Instant::now();
        while editor.adobe_files.first_launch {
            assert!(started.elapsed() < Duration::from_secs(30), "never listed");
            editor.extensions_frame(&ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(editor.onboarding.visible, opens, "found {found}");
        assert_eq!(editor.session.onboarding_done, !opens, "found {found}");
    }
}

/// The program answers one request at a time, so while it imports a catalog
/// the listing isn't asked for: it waits, as looking, and is asked for once
/// the import has ended, instead of failing past its deadline.
#[test]
fn a_listing_waits_for_an_import_with_the_lightroom_extension() {
    let (dir, ctx, mut editor) = fixture();
    let source = dir.path().join("Lightroom.lrcat");
    rawmakase_lightroom::fake::catalog(&source).unwrap();
    let offer = editor
        .importer_items()
        .into_iter()
        .find(|o| o.extension == LIGHTROOM)
        .unwrap();
    assert!(editor.ready_for_catalog() && editor.activity.begin_dialog());
    let job = crate::catalog::plan_import::PlanImport::prepare(
        &source,
        &dir.path().join("Photos.rawmakase"),
    )
    .unwrap();
    editor.start_catalog_import(offer, job);
    assert!(editor.adobe_files().is_none());
    assert!(
        !matches!(editor.adobe_files.state, Listing::Asked(_)),
        "not asked during the import"
    );
    assert!(editor.adobe_files.asking(), "setup says it is looking");
    let started = Instant::now();
    while editor.catalog_importing().is_some() || editor.adobe_files.listed().is_none() {
        assert!(started.elapsed() < Duration::from_secs(30), "never listed");
        editor.events(&ctx);
        editor.extensions_frame(&ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(editor.adobe_files.failure().is_none());
}
