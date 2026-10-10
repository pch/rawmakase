use super::*;
use crate::app::tests::editor_with_catalog;
use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

/// Every render the test renderer was asked for, by file.
static RENDERED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
/// "slow" renders that have started, and those let go.
static STARTED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static RELEASED: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

/// Renders by the file's name: "panic…" panics, "fail…" fails, "slow…" waits
/// until released or cancelled, "edited…" resolves its edit first as a build does;
/// anything else is a small solid image.
fn test_render(item: &Item, cancel: &AtomicBool) -> anyhow::Result<image::RgbImage> {
    RENDERED.lock().unwrap().push(item.path.clone());
    let name = item.path.file_name().unwrap().to_string_lossy().to_string();
    if name.starts_with("panic") {
        panic!("test panic");
    }
    anyhow::ensure!(!name.starts_with("fail"), "test failure");
    if name.starts_with("edited") {
        item_recipe(item, &Metadata::default(), &[])?;
    }
    if name.starts_with("slow") {
        STARTED.lock().unwrap().push(item.path.clone());
        // Wait until this test releases the file or cancels the build. A clock
        // limit stored the preview when the test thread was slow to cancel.
        while !released(&item.path) {
            anyhow::ensure!(!cancel.load(Ordering::Relaxed), "Cancelled");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    Ok(image::RgbImage::from_pixel(
        64,
        32,
        image::Rgb([200, 120, 60]),
    ))
}
fn released(path: &Path) -> bool {
    RELEASED
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|r| r.contains(path))
}
fn release(path: &Path) {
    RELEASED
        .lock()
        .unwrap()
        .get_or_insert_with(HashSet::new)
        .insert(path.to_path_buf());
}
fn renders_of(path: &Path) -> usize {
    RENDERED
        .lock()
        .unwrap()
        .iter()
        .filter(|p| *p == path)
        .count()
}
fn wait_started(path: &Path) {
    let until = Instant::now() + Duration::from_secs(10);
    while !STARTED.lock().unwrap().iter().any(|p| p == path) {
        assert!(Instant::now() < until, "the build never started");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// An editor whose builds go to a cache in its own folder, rendered by `test_render`.
fn editor(names: &[&str]) -> (tempfile::TempDir, Editor, Vec<PhotoId>, PathBuf) {
    let (dir, mut e, ids) = editor_with_catalog(names).unwrap();
    let cache = dir.path().join("previews.sqlite3");
    e.preview_builds = PreviewBuilds::for_tests(cache.clone(), test_render);
    (dir, e, ids, cache)
}
fn path_of(e: &Editor, id: PhotoId) -> PathBuf {
    e.library.as_ref().unwrap().photo(id).unwrap().path.clone()
}
/// Polls until every build asked for has ended.
fn settle(e: &mut Editor) {
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        e.poll_preview_builds();
        if !e.preview_builds.progress().is_some_and(|p| p.running()) {
            // The last result may still be on its way.
            std::thread::sleep(Duration::from_millis(5));
            e.poll_preview_builds();
            return;
        }
        assert!(Instant::now() < until, "builds never ended");
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn identity_of(e: &Editor, id: PhotoId) -> String {
    let record = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog
        .edit_record(id)
        .unwrap();
    identity(&record, &e.raw_defaults, e.demosaic)
}
fn fresh(cache: &Path, e: &Editor, id: PhotoId) -> bool {
    PreviewCache::open(cache)
        .unwrap()
        .sized_fresh(
            &path_of(e, id),
            &identity_of(e, id),
            PreviewKind::Standard,
            DEFAULT_STANDARD_SIZE,
        )
        .unwrap()
}
fn save_edit(e: &mut Editor, id: PhotoId, exposure: f32) {
    let path = path_of(e, id);
    let recipe = Recipe {
        exposure,
        ..Default::default()
    };
    e.library
        .as_mut()
        .unwrap()
        .session
        .catalog
        .save_edit(
            id,
            &path,
            &recipe,
            &Default::default(),
            crate::catalog::HistoryUpdate::Keep,
        )
        .unwrap();
}

#[test]
fn only_stale_raws_of_the_selection_are_built() {
    let (_dir, mut e, ids, cache) = editor(&["a.ARW", "b.ARW", "c.JPG", "d.ARW"]);
    let [a, b, c, d] = ids[..] else {
        panic!("four photos")
    };
    e.build_previews(&[a], PreviewKind::Standard).unwrap();
    settle(&mut e);
    assert!(fresh(&cache, &e, a));
    std::fs::remove_file(path_of(&e, d)).unwrap();
    let queued = e
        .build_previews(&[a, b, c, d], PreviewKind::Standard)
        .unwrap();
    // The JPEG and the offline RAW are skipped; the fresh one costs nothing.
    assert_eq!(
        queued,
        Queued {
            queued: 2,
            skipped: 2
        }
    );
    settle(&mut e);
    assert_eq!(renders_of(&path_of(&e, a)), 1);
    assert_eq!(renders_of(&path_of(&e, b)), 1);
    assert_eq!(renders_of(&path_of(&e, c)), 0);
    assert!(fresh(&cache, &e, b));
    let progress = e.preview_builds.progress().unwrap();
    assert_eq!((progress.done, progress.total), (2, 2));
    assert!(progress.failed.is_empty());
}

#[test]
fn a_replaced_raw_with_a_saved_edit_fails_as_protected_and_stores_nothing() {
    let (_dir, mut e, ids, cache) = editor(&["edited.ARW"]);
    save_edit(&mut e, ids[0], 1.);
    std::fs::write(path_of(&e, ids[0]), b"another photo, same name").unwrap();
    e.build_previews(&ids, PreviewKind::Standard).unwrap();
    settle(&mut e);
    let progress = e.preview_builds.progress().unwrap();
    assert_eq!(progress.failed.len(), 1);
    assert!(progress.failed[0].1.contains("protected"), "{progress:?}");
    assert!(!fresh(&cache, &e, ids[0]));
}

#[test]
fn a_failure_or_a_panic_fails_one_photo_and_the_next_still_builds() {
    let (_dir, mut e, ids, cache) = editor(&["fail.ARW", "good.ARW", "panic.ARW"]);
    e.build_previews(&ids, PreviewKind::Standard).unwrap();
    settle(&mut e);
    let progress = e.preview_builds.progress().unwrap();
    let mut failed: Vec<&str> = progress.failed.iter().map(|(n, _)| n.as_str()).collect();
    failed.sort();
    assert_eq!(failed, ["fail.ARW", "panic.ARW"]);
    let good = ids
        .iter()
        .copied()
        .find(|id| path_of(&e, *id).ends_with("good.ARW"))
        .unwrap();
    assert!(fresh(&cache, &e, good));
    // The worker carries on after the panic.
    let again = ids
        .iter()
        .copied()
        .find(|id| path_of(&e, *id).ends_with("fail.ARW"))
        .unwrap();
    e.build_previews(&[again], PreviewKind::Standard).unwrap();
    settle(&mut e);
    assert_eq!(e.preview_builds.progress().unwrap().failed.len(), 1);
}

#[test]
fn cancel_during_a_render_stores_nothing_and_drops_the_rest() {
    let (_dir, mut e, ids, cache) = editor(&["slow-cancel.ARW", "z-after.ARW"]);
    e.build_previews(&ids, PreviewKind::Standard).unwrap();
    wait_started(&path_of(&e, ids[0]));
    e.preview_builds.builder.as_ref().unwrap().cancel();
    settle(&mut e);
    assert!(!fresh(&cache, &e, ids[0]));
    assert_eq!(renders_of(&path_of(&e, ids[1])), 0);
    assert!(e.preview_builds.progress().unwrap().failed.is_empty());
}

#[test]
fn a_cancel_seen_after_the_render_still_stores_nothing() {
    fn cancels(_: &Item, cancel: &AtomicBool) -> anyhow::Result<image::RgbImage> {
        cancel.store(true, Ordering::Relaxed);
        Ok(image::RgbImage::new(8, 8))
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("photo.ARW");
    std::fs::write(&path, b"raw").unwrap();
    let mut cache = PreviewCache::open(&dir.path().join("previews.sqlite3")).unwrap();
    let item = Item {
        catalog: CatalogLocation::File(dir.path().join("c.rawmakase")),
        photo: PhotoId(1),
        name: "photo.ARW".into(),
        path: path.clone(),
        record: EditRecord::default(),
        defaults: Default::default(),
        demosaic: Demosaic::default(),
        identity: "a".into(),
        kind: PreviewKind::Standard,
        edge: 2048,
        ticket: None,
    };
    let outcome = build_one(&item, Some(&mut cache), cancels, &AtomicBool::new(false));
    assert!(matches!(outcome, Outcome::Cancelled));
    assert!(
        !cache
            .sized_fresh(&path, "a", PreviewKind::Standard, 2048)
            .unwrap()
    );
}

#[test]
fn another_catalog_cancels_the_build_and_drops_its_result() {
    let (_dir, mut e, ids, cache) = editor(&["slow-switch.ARW"]);
    let path = path_of(&e, ids[0]);
    let identity = identity_of(&e, ids[0]);
    e.build_previews(&ids, PreviewKind::Standard).unwrap();
    wait_started(&path);
    let (_other_dir, other, _) = editor_with_catalog(&["x.ARW"]).unwrap();
    e.library = other.library;
    settle(&mut e);
    assert!(
        !PreviewCache::open(&cache)
            .unwrap()
            .sized_fresh(&path, &identity, PreviewKind::Standard, 2048)
            .unwrap()
    );
}

/// The grid's texture for the photo, by id, to tell which image it shows.
fn shown(e: &Editor, id: PhotoId) -> Option<egui::TextureId> {
    e.library.as_ref().unwrap().thumbnail(id).map(|t| t.id())
}
/// A photo with a saved edit that the grid shows rendered by Develop.
fn edited_in_grid(e: &mut Editor, id: PhotoId) {
    save_edit(e, id, 0.5);
    let ctx = e.context.clone();
    e.library
        .as_mut()
        .unwrap()
        .update_edited(&ctx, id, image::RgbImage::new(4, 4), "{}".into());
}

#[test]
fn a_build_shows_in_the_grid_while_it_is_the_latest() {
    let (_dir, mut e, ids, _) = editor(&["photo.ARW"]);
    edited_in_grid(&mut e, ids[0]);
    let before = shown(&e, ids[0]);
    e.build_previews(&ids, PreviewKind::Standard).unwrap();
    settle(&mut e);
    assert_ne!(shown(&e, ids[0]), before);
}

#[test]
fn a_render_by_develop_during_the_build_wins_over_it() {
    let (_dir, mut e, ids, _) = editor(&["slow-develop.ARW"]);
    edited_in_grid(&mut e, ids[0]);
    e.build_previews(&ids, PreviewKind::Standard).unwrap();
    let path = path_of(&e, ids[0]);
    wait_started(&path);
    // Develop renders the photo again: a newer ticket.
    edited_in_grid(&mut e, ids[0]);
    let newer = shown(&e, ids[0]);
    release(&path);
    settle(&mut e);
    assert_eq!(shown(&e, ids[0]), newer);
}

#[test]
fn an_edit_saved_during_the_build_keeps_the_build_off_the_grid() {
    let (_dir, mut e, ids, _) = editor(&["slow-edit.ARW"]);
    edited_in_grid(&mut e, ids[0]);
    let before = shown(&e, ids[0]);
    e.build_previews(&ids, PreviewKind::Standard).unwrap();
    let path = path_of(&e, ids[0]);
    wait_started(&path);
    // Saved by Sync, say: the ticket stays, the edit does not.
    save_edit(&mut e, ids[0], 2.);
    release(&path);
    settle(&mut e);
    assert_eq!(shown(&e, ids[0]), before);
}

#[test]
fn a_removed_copy_takes_its_requests_and_waiting_builds_along() {
    let (_dir, mut e, ids, cache) = editor(&["slow-copy.ARW"]);
    let master = ids[0];
    let copy = e
        .library
        .as_mut()
        .unwrap()
        .create_virtual_copy(master)
        .unwrap()
        .value;
    let catalog = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog
        .location()
        .clone();
    let path = path_of(&e, master);
    // The master builds first, so the copy is still waiting when it goes.
    e.build_previews(&[master, copy], PreviewKind::Standard)
        .unwrap();
    wait_started(&path);
    e.remove_virtual_copy(copy);
    release(&path);
    settle(&mut e);
    assert_eq!(renders_of(&path), 1);
    let cache = PreviewCache::open(&cache).unwrap();
    assert!(
        cache
            .has_intent(&catalog, master, &path, PreviewKind::Standard)
            .unwrap()
    );
    assert!(
        !cache
            .has_intent(&catalog, copy, &path, PreviewKind::Standard)
            .unwrap()
    );
    // A new copy given the removed one's id starts without its request.
    let again = e
        .library
        .as_mut()
        .unwrap()
        .create_virtual_copy(master)
        .unwrap()
        .value;
    assert!(
        !cache
            .has_intent(&catalog, again, &path, PreviewKind::Standard)
            .unwrap()
    );
}

#[test]
fn discarding_drops_rows_and_requests_of_every_kind() {
    let (_dir, mut e, ids, cache) = editor(&["photo.ARW"]);
    let catalog = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog
        .location()
        .clone();
    e.build_previews(&ids, PreviewKind::Standard).unwrap();
    settle(&mut e);
    assert!(fresh(&cache, &e, ids[0]));
    e.discard_previews(&ids, &PreviewKind::ALL);
    // Upkeep runs on the worker.
    let until = Instant::now() + Duration::from_secs(10);
    while fresh(&cache, &e, ids[0]) {
        assert!(Instant::now() < until, "never discarded");
        std::thread::sleep(Duration::from_millis(5));
    }
    let path = path_of(&e, ids[0]);
    let cache = PreviewCache::open(&cache).unwrap();
    assert!(
        !cache
            .has_intent(&catalog, ids[0], &path, PreviewKind::Standard)
            .unwrap()
    );
}

#[test]
fn identity_follows_the_edit_the_defaults_and_the_demosaic() {
    let defaults = DevelopDefaults::default();
    let none = EditRecord::default();
    let saved = EditRecord {
        recipe: Some("{\"exposure\":1}".into()),
        ..Default::default()
    };
    let base = identity(&none, &defaults, Demosaic::Rawmakase);
    assert_eq!(base, identity(&none, &defaults, Demosaic::Rawmakase));
    assert_ne!(base, identity(&none, &defaults, Demosaic::Libraw));
    assert_ne!(base, identity(&saved, &defaults, Demosaic::Rawmakase));
    let local = EditRecord {
        local: Some("[]".into()),
        ..saved.clone()
    };
    assert_ne!(
        identity(&saved, &defaults, Demosaic::Rawmakase),
        identity(&local, &defaults, Demosaic::Rawmakase)
    );
    let lightroom = EditRecord {
        lightroom: Some("s = {}".into()),
        ..Default::default()
    };
    assert_ne!(base, identity(&lightroom, &defaults, Demosaic::Rawmakase));
    // Only a photo without an edit develops with the raw defaults.
    let other = DevelopDefaults::load(crate::raw_defaults::RawDefaults {
        master: crate::raw_defaults::DefaultChoice::CameraSettings,
        ..Default::default()
    });
    assert_ne!(base, identity(&none, &other, Demosaic::Rawmakase));
    assert_eq!(
        identity(&saved, &defaults, Demosaic::Rawmakase),
        identity(&saved, &other, Demosaic::Rawmakase)
    );
}

#[test]
fn the_full_decode_is_used_when_the_half_size_one_falls_short() {
    let m = Metadata {
        width: 6000,
        height: 4000,
        ..Default::default()
    };
    let whole = Recipe::default();
    assert!(!needs_full(&whole, &m, 2048));
    assert!(needs_full(&whole, &m, 0));
    let cropped = Recipe {
        crop: [0.25, 0.25, 0.75, 0.75],
        ..Default::default()
    };
    assert!(needs_full(&cropped, &m, 2048));
    assert!(!needs_full(&cropped, &m, 1440));
    let mut upright = Recipe::default();
    upright.upright.mode = crate::model::transform::UprightMode::Auto;
    assert!(upright.upright.needs_analysis());
    assert!(needs_full(&upright, &m, 1440));
}

fn synthetic(width: u32, height: u32) -> crate::camera_data::CameraImage {
    let pixels = (0..width * height)
        .map(|i| {
            let (x, y) = (i % width, i / width);
            // Slanted bars, for Upright to find lines in.
            let v = if (x + y / 3) % 12 < 6 { 0.6 } else { 0.1 };
            [v, v, v]
        })
        .collect();
    crate::camera_data::CameraImage {
        recovered: Default::default(),
        width,
        height,
        pixels,
        metadata: Metadata {
            width,
            height,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    }
}

#[test]
fn upright_is_completed_before_rendering() {
    let image = synthetic(96, 64);
    let mut recipe = Recipe::default();
    recipe.upright.mode = crate::model::transform::UprightMode::Auto;
    let out = develop(&mut recipe, &image, 48, &AtomicBool::new(false)).unwrap();
    assert!(!recipe.upright.needs_analysis());
    assert!(out.width().max(out.height()) <= 48);
}

#[test]
fn a_real_render_stops_once_cancelled() {
    let image = synthetic(96, 64);
    let mut recipe = Recipe::default();
    assert!(develop(&mut recipe, &image, 48, &AtomicBool::new(true)).is_err());
}

#[test]
fn the_command_builds_what_the_menu_would() {
    use crate::app::commands::{Command, Operation, Outcome as Reply};
    let (_dir, mut e, ids, _) = editor(&["a.ARW", "b.ARW", "c.JPG"]);
    let library = e.library.as_mut().unwrap();
    library.select(Some(ids[0]));
    library.select_range_to(ids[2]);
    assert_eq!(e.preview_command_scope(), ids);
    let ctx = e.context.clone();
    let reply = e
        .execute_command(
            Command::new(Operation::BuildPreviews(PreviewKind::Standard)),
            &ctx,
        )
        .unwrap();
    assert!(matches!(
        reply,
        Reply::Previews {
            queued: 2,
            skipped: 1
        }
    ));
    let state = serde_json::to_value(e.command_state()).unwrap();
    assert!(state.get("building_previews").is_some());
    settle(&mut e);
}

#[test]
fn quitting_during_a_build_ends_in_time() {
    let (_dir, mut e, ids, _) = editor(&["slow-exit.ARW"]);
    e.build_previews(&ids, PreviewKind::Standard).unwrap();
    wait_started(&path_of(&e, ids[0]));
    let waited = e.exit_within(Duration::from_secs(3));
    assert_eq!(waited.detached, 0);
}

#[test]
fn upkeep_asked_for_before_exit_still_happens() {
    let (_dir, mut e, ids, cache) = editor(&["upkeep.ARW"]);
    let path = path_of(&e, ids[0]);
    e.build_previews(&ids, PreviewKind::Standard).unwrap();
    settle(&mut e);
    assert!(fresh(&cache, &e, ids[0]));
    // Rebuilding, then discarding and quitting at once: whatever the worker
    // has got to, the discard comes after the build.
    save_edit(&mut e, ids[0], 1.);
    e.build_previews(&ids, PreviewKind::Standard).unwrap();
    e.discard_previews(&ids, &PreviewKind::ALL);
    let waited = e.exit_within(Duration::from_secs(3));
    assert_eq!(waited.detached, 0);
    let catalog = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog
        .location()
        .clone();
    let cache = PreviewCache::open(&cache).unwrap();
    assert!(
        !cache
            .has_intent(&catalog, ids[0], &path, PreviewKind::Standard)
            .unwrap()
    );
    assert_eq!(cache.usage().unwrap().standard, 0);
}

/// Waits until `path` was rendered `n` times, then a little longer to see no more.
fn wait_renders(path: &Path, n: usize) {
    let until = Instant::now() + Duration::from_secs(10);
    while renders_of(path) < n {
        assert!(
            Instant::now() < until,
            "rendered {} times",
            renders_of(path)
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(renders_of(path), n);
}
/// Opens `id` as Develop or the Loupe would, leaving the photo open before.
fn open(e: &mut Editor, id: PhotoId) {
    let path = path_of(e, id);
    assert!(e.load_raw(path, Some(id)));
}

#[test]
fn leaving_a_photo_saved_with_another_edit_builds_it_again() {
    let (_dir, mut e, ids, _) = editor(&["a.ARW", "b.ARW"]);
    let (a, b) = (ids[0], ids[1]);
    e.build_previews(&[a], PreviewKind::Standard).unwrap();
    settle(&mut e);
    let path = path_of(&e, a);
    open(&mut e, a);
    // Left unchanged: nothing to do.
    open(&mut e, b);
    open(&mut e, a);
    // Saved with another edit, as autosave does, then left.
    save_edit(&mut e, a, 1.);
    open(&mut e, b);
    wait_renders(&path, 2);
    settle(&mut e);
    assert_eq!(renders_of(&path_of(&e, b)), 0);
}

#[test]
fn an_edit_saved_while_the_first_build_runs_is_built_after_it() {
    let (_dir, mut e, ids, _) = editor(&["slow-first.ARW", "z.ARW"]);
    let (a, b) = (ids[0], ids[1]);
    let path = path_of(&e, a);
    e.build_previews(&[a], PreviewKind::Standard).unwrap();
    wait_started(&path);
    open(&mut e, a);
    save_edit(&mut e, a, 1.);
    open(&mut e, b);
    release(&path);
    wait_renders(&path, 2);
}

#[test]
fn edits_written_elsewhere_rebuild_only_photos_asked_for() {
    let (_dir, mut e, ids, _) = editor(&["a.ARW", "b.ARW"]);
    let (a, b) = (ids[0], ids[1]);
    e.build_previews(&[a], PreviewKind::Standard).unwrap();
    settle(&mut e);
    // As Sync and its Undo do.
    save_edit(&mut e, a, 1.);
    save_edit(&mut e, b, 1.);
    e.refresh_previews(&[a, b]);
    wait_renders(&path_of(&e, a), 2);
    assert_eq!(renders_of(&path_of(&e, b)), 0);
    // Unchanged since: nothing.
    e.refresh_previews(&[a]);
    wait_renders(&path_of(&e, a), 2);
}

#[test]
fn another_copy_or_catalog_of_the_same_file_does_not_inherit_requests() {
    let (dir, mut e, ids, _) = editor(&["a.ARW"]);
    let master = ids[0];
    let path = path_of(&e, master);
    let copy = e
        .library
        .as_mut()
        .unwrap()
        .create_virtual_copy(master)
        .unwrap()
        .value;
    e.build_previews(&[master], PreviewKind::Standard).unwrap();
    settle(&mut e);
    open(&mut e, copy);
    save_edit(&mut e, copy, 1.);
    open(&mut e, master);
    // The same file in a second catalog.
    let other = dir.path().join("other.rawmakase");
    crate::catalog::Catalog::create(&other)
        .unwrap()
        .add_folder(path.parent().unwrap())
        .unwrap();
    let library = crate::app::library::Library::load(&other, e.context.clone()).unwrap();
    let there = library.session.photos[0].id;
    e.document.reset(None);
    e.library = Some(Box::new(library));
    open(&mut e, there);
    save_edit(&mut e, there, 2.);
    e.document.catalog_photo = Some(there);
    e.refresh_previews(&[there]);
    wait_renders(&path, 1);
}

#[test]
fn opening_a_photo_whose_preview_went_stale_builds_it_again() {
    let (_dir, mut e, ids, _) = editor(&["a.ARW"]);
    let path = path_of(&e, ids[0]);
    e.build_previews(&ids, PreviewKind::Standard).unwrap();
    settle(&mut e);
    // Saved as the app quit, say: no photo was left to notice.
    save_edit(&mut e, ids[0], 1.);
    open(&mut e, ids[0]);
    wait_renders(&path, 2);
}

#[test]
fn cancel_drops_refreshes_not_yet_looked_at() {
    let (_dir, mut e, ids, _) = editor(&["slow-refresh.ARW", "b.ARW"]);
    let named = |e: &Editor, name: &str| {
        ids.iter()
            .copied()
            .find(|id| path_of(e, *id).ends_with(name))
            .unwrap()
    };
    let (a, b) = (named(&e, "slow-refresh.ARW"), named(&e, "b.ARW"));
    let path = path_of(&e, a);
    e.build_previews(&[b], PreviewKind::Standard).unwrap();
    settle(&mut e);
    e.build_previews(&[a], PreviewKind::Standard).unwrap();
    wait_started(&path);
    // While a builds, b's edit changes; then everything is cancelled.
    save_edit(&mut e, b, 1.);
    e.refresh_previews(&[b]);
    e.preview_builds.builder.as_ref().unwrap().cancel();
    settle(&mut e);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(renders_of(&path_of(&e, b)), 1);
}

#[test]
fn one_to_one_previews_build_at_full_size_and_discard_on_their_own() {
    let (_dir, mut e, ids, cache) = editor(&["a.ARW"]);
    let path = path_of(&e, ids[0]);
    let identity = identity_of(&e, ids[0]);
    e.build_previews(&ids, PreviewKind::Standard).unwrap();
    e.build_previews(&ids, PreviewKind::OneToOne).unwrap();
    settle(&mut e);
    let has = |kind| {
        PreviewCache::open(&cache)
            .unwrap()
            .sized_fresh(&path, &identity, kind, 0)
            .unwrap()
    };
    assert!(has(PreviewKind::Standard) && has(PreviewKind::OneToOne));
    e.discard_previews(&ids, &[PreviewKind::OneToOne]);
    let until = Instant::now() + Duration::from_secs(10);
    while has(PreviewKind::OneToOne) {
        assert!(Instant::now() < until, "never discarded");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(has(PreviewKind::Standard));
    let catalog = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog
        .location()
        .clone();
    let cache = PreviewCache::open(&cache).unwrap();
    assert!(
        cache
            .has_intent(&catalog, ids[0], &path, PreviewKind::Standard)
            .unwrap()
    );
    assert!(
        !cache
            .has_intent(&catalog, ids[0], &path, PreviewKind::OneToOne)
            .unwrap()
    );
}

#[test]
fn a_photo_too_large_for_a_jpeg_fails_with_why() {
    fn panorama(_: &Item, _: &AtomicBool) -> anyhow::Result<image::RgbImage> {
        Ok(image::RgbImage::new(JPEG_LIMIT + 1, 1))
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pano.ARW");
    std::fs::write(&path, b"raw").unwrap();
    let mut cache = PreviewCache::open(&dir.path().join("previews.sqlite3")).unwrap();
    let item = Item {
        catalog: CatalogLocation::File(dir.path().join("c.rawmakase")),
        photo: PhotoId(1),
        name: "pano.ARW".into(),
        path,
        record: EditRecord::default(),
        defaults: Default::default(),
        demosaic: Demosaic::default(),
        identity: "a".into(),
        kind: PreviewKind::OneToOne,
        edge: 0,
        ticket: None,
    };
    let outcome = build_one(&item, Some(&mut cache), panorama, &AtomicBool::new(false));
    assert!(matches!(outcome, Outcome::Failed(why) if why.contains("65535")));
}

#[test]
fn a_one_to_one_preview_asked_for_is_built_again_after_an_edit() {
    let (_dir, mut e, ids, _) = editor(&["one.ARW"]);
    let path = path_of(&e, ids[0]);
    e.build_previews(&ids, PreviewKind::OneToOne).unwrap();
    settle(&mut e);
    save_edit(&mut e, ids[0], 1.);
    e.refresh_previews(&ids);
    // The 1:1 preview only: no Standard one was asked for.
    wait_renders(&path, 2);
}

#[test]
fn the_command_builds_one_to_one_previews_too() {
    use crate::app::commands::{Command, Operation, Outcome as Reply};
    let (_dir, mut e, ids, _) = editor(&["a.ARW"]);
    e.library.as_mut().unwrap().select(Some(ids[0]));
    let ctx = e.context.clone();
    let reply = e
        .execute_command(
            Command::new(Operation::BuildPreviews(PreviewKind::OneToOne)),
            &ctx,
        )
        .unwrap();
    assert!(matches!(
        reply,
        Reply::Previews {
            queued: 1,
            skipped: 0
        }
    ));
    settle(&mut e);
}

#[test]
fn a_new_discard_setting_replaces_the_expiry_not_run_yet() {
    let (_dir, mut e, ids, _) = editor(&["slow-expiry.ARW"]);
    let path = path_of(&e, ids[0]);
    e.build_previews(&ids, PreviewKind::Standard).unwrap();
    wait_started(&path);
    let expiries = |e: &Editor| {
        let state = e
            .preview_builds
            .builder
            .as_ref()
            .unwrap()
            .shared
            .state
            .lock()
            .unwrap();
        state
            .ops
            .iter()
            .filter(|op| matches!(op, Op::Expire(..)))
            .count()
    };
    e.preview_builds.discard_one_to_one_after = Some(1);
    e.expire_previews();
    e.preview_builds.discard_one_to_one_after = Some(30);
    e.expire_previews();
    assert_eq!(expiries(&e), 1);
    e.preview_builds.discard_one_to_one_after = None;
    e.expire_previews();
    assert_eq!(expiries(&e), 0);
    release(&path);
    settle(&mut e);
}

#[test]
fn clearing_a_kind_stops_its_build_under_way() {
    let (_dir, mut e, ids, cache) = editor(&["slow-clear.ARW"]);
    let path = path_of(&e, ids[0]);
    e.build_previews(&ids, PreviewKind::OneToOne).unwrap();
    wait_started(&path);
    e.clear_previews(PreviewKind::OneToOne);
    settle(&mut e);
    let identity = identity_of(&e, ids[0]);
    assert!(
        !PreviewCache::open(&cache)
            .unwrap()
            .sized_fresh(&path, &identity, PreviewKind::OneToOne, 0)
            .unwrap()
    );
}

#[test]
fn a_build_keeps_its_request_through_an_expiry_while_it_waited() {
    let (_dir, mut e, ids, cache) = editor(&["a.ARW"]);
    let path = path_of(&e, ids[0]);
    let catalog = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog
        .location()
        .clone();
    e.build_previews(&ids, PreviewKind::OneToOne).unwrap();
    // As if the setting changed before the build ran: the request is forgotten.
    e.preview_builds.discard_one_to_one_after = Some(1);
    e.expire_previews();
    settle(&mut e);
    let until = Instant::now() + Duration::from_secs(10);
    let cache = loop {
        let cache = PreviewCache::open(&cache).unwrap();
        if cache
            .has_intent(&catalog, ids[0], &path, PreviewKind::OneToOne)
            .unwrap()
        {
            break cache;
        }
        assert!(Instant::now() < until, "the request was lost");
        std::thread::sleep(Duration::from_millis(5));
    };
    drop(cache);
}

#[test]
fn discarding_a_photo_stops_its_build_under_way() {
    let (_dir, mut e, ids, cache) = editor(&["slow-discard.ARW"]);
    let path = path_of(&e, ids[0]);
    e.build_previews(&ids, PreviewKind::OneToOne).unwrap();
    wait_started(&path);
    e.discard_previews(&ids, &[PreviewKind::OneToOne]);
    settle(&mut e);
    let identity = identity_of(&e, ids[0]);
    assert!(
        !PreviewCache::open(&cache)
            .unwrap()
            .sized_fresh(&path, &identity, PreviewKind::OneToOne, 0)
            .unwrap()
    );
}
