use super::*;
use crate::app::testing::editor_with_catalog;
use crate::catalog::preview_cache::Stamp;
use std::path::Path;
use std::time::{Duration, Instant};

/// An editor with a catalog of `names`, its stored previews in its own folder.
fn editor(names: &[&str]) -> (tempfile::TempDir, Editor, Vec<PhotoId>, PathBuf) {
    let (dir, mut e, ids) = editor_with_catalog(names).unwrap();
    let cache = dir.path().join("previews.sqlite3");
    e.stand_ins = StandIns::new(e.tx.clone(), e.context.clone(), cache.clone());
    (dir, e, ids, cache)
}
/// Stores a Standard preview of `photo` for the edit it has now, `color` throughout.
fn store(e: &Editor, cache: &Path, photo: PhotoId, color: u8) {
    let wanted = e.stand_in_for(photo).unwrap();
    PreviewCache::open(cache)
        .unwrap()
        .store_sized(
            &wanted.path,
            &wanted.identity,
            PreviewKind::Standard,
            &Stamp::read(&wanted.path).unwrap(),
            2048,
            &image::RgbImage::from_pixel(40, 20, image::Rgb([color; 3])),
        )
        .unwrap();
}
/// Starts loading `photo` as `load_raw` does, without decoding anything.
fn open(e: &mut Editor, photo: PhotoId, neighbour: Option<PhotoId>) -> u64 {
    e.document.catalog_photo = Some(photo);
    e.preview.clear_document();
    let (id, _) = e.document.start_load();
    e.request_stand_ins(id, photo, neighbour);
    id
}
/// Takes events until `done`, or fails after a while.
fn until(e: &mut Editor, done: impl Fn(&Editor) -> bool) {
    let ctx = e.context.clone();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(e) {
        assert!(Instant::now() < deadline, "never happened");
        std::thread::sleep(Duration::from_millis(2));
        e.events(&ctx);
    }
}
fn embedded(e: &mut Editor, load: u64) {
    e.tx.send(Event::Embedded {
        id: load,
        image: image::RgbImage::new(30, 20),
    })
    .unwrap();
    let ctx = e.context.clone();
    e.events(&ctx);
}
fn live_render(e: &mut Editor) {
    let ctx = e.context.clone();
    e.set_pixels(&ctx, false, [2, 2], &[0; 12], None);
}

#[test]
fn the_stored_preview_shows_before_or_after_the_embedded_one() {
    let (_dir, mut e, ids, cache) = editor(&["a.ARW"]);
    store(&e, &cache, ids[0], 200);
    // The stored preview first: the embedded JPEG is then ignored.
    let load = open(&mut e, ids[0], None);
    until(&mut e, |e| e.preview.stand_in.is_some());
    embedded(&mut e, load);
    assert!(e.preview.standing_in());
    assert!(e.preview.texture.is_none());
    // The embedded JPEG first: the stored preview replaces it.
    e.preview.clear_document();
    let (load, _) = e.document.start_load();
    embedded(&mut e, load);
    assert!(e.preview.texture.is_some() && e.preview.embedded);
    e.request_stand_ins(load, ids[0], None);
    until(&mut e, |e| e.preview.stand_in.is_some());
    assert!(e.preview.standing_in());
    // Nothing that reads the photo sees it.
    assert!(e.preview.samples.is_none() && e.document.full().is_none());
    // The first render takes over.
    live_render(&mut e);
    assert!(!e.preview.standing_in());
    assert!(e.preview.stand_in.is_none());
}

#[test]
fn a_stored_preview_for_an_old_load_or_after_the_first_render_is_dropped() {
    let (_dir, mut e, ids, cache) = editor(&["a.ARW", "b.ARW"]);
    store(&e, &cache, ids[0], 200);
    let first = open(&mut e, ids[0], None);
    let image = egui::ColorImage::new([4, 4], vec![egui::Color32::WHITE; 16]);
    let wanted = e.stand_in_for(ids[0]).unwrap();
    // Opened another photo meanwhile.
    open(&mut e, ids[1], None);
    let ctx = e.context.clone();
    e.stand_in_ready(
        &ctx,
        StandIn {
            load: Some(first),
            wanted: wanted.clone(),
            image: image.clone(),
        },
    );
    assert!(e.preview.stand_in.is_none());
    // Rendered before it arrived.
    let load = open(&mut e, ids[0], None);
    live_render(&mut e);
    e.stand_in_ready(
        &ctx,
        StandIn {
            load: Some(load),
            wanted,
            image,
        },
    );
    assert!(e.preview.stand_in.is_none());
}

#[test]
fn a_preview_stored_for_another_edit_is_not_shown() {
    let (_dir, mut e, ids, cache) = editor(&["a.ARW", "b.ARW"]);
    store(&e, &cache, ids[0], 200);
    // The edit changes after the preview was built.
    let path = e
        .library
        .as_ref()
        .unwrap()
        .photo(ids[0])
        .unwrap()
        .path
        .clone();
    e.library
        .as_mut()
        .unwrap()
        .session
        .save_edit(
            ids[0],
            &path,
            &crate::storage::Identity::read(&path).unwrap(),
            &Default::default(),
            &Default::default(),
            crate::catalog::HistoryUpdate::Keep,
        )
        .unwrap();
    // The neighbour's, which exists, arrives; the photo's own never does.
    store(&e, &cache, ids[1], 100);
    open(&mut e, ids[0], Some(ids[1]));
    until(&mut e, |e| !e.stand_ins.prepared.is_empty());
    assert!(e.preview.stand_in.is_none());
}

#[test]
fn the_neighbour_is_prepared_beside_the_photo_and_shown_at_once() {
    let (_dir, mut e, ids, cache) = editor(&["a.ARW", "b.ARW"]);
    store(&e, &cache, ids[0], 200);
    store(&e, &cache, ids[1], 100);
    // Asking for the neighbour never replaces the photo's own request.
    open(&mut e, ids[0], Some(ids[1]));
    until(&mut e, |e| {
        e.preview.stand_in.is_some() && e.stand_ins.prepared.len() == 1
    });
    // Moving on shows the prepared one in the first frame.
    open(&mut e, ids[1], None);
    assert!(e.preview.standing_in());
    assert!(e.stand_ins.prepared.is_empty());
}

#[test]
fn an_offline_raw_in_the_loupe_gets_its_stored_preview_identity() {
    let (_dir, mut e, ids, _) = editor(&["a.ARW"]);
    let wanted = e.stand_in_for(ids[0]).unwrap();
    std::fs::remove_file(&wanted.path).unwrap();
    let library = e.library.as_mut().unwrap();
    library.refresh().unwrap();
    library.wait_for_availability();
    library.select(Some(ids[0]));
    library.open_loupe();
    e.loupe_stored_preview();
    assert_eq!(
        e.library.as_ref().unwrap().loupe_stored(),
        Some(&(ids[0], wanted.identity))
    );
}

/// An edited JPEG is developed, so it has a stored preview as a RAW does; an
/// unedited one, shown as its file is, has none.
#[test]
fn an_edited_jpeg_has_a_stored_preview_and_an_unedited_one_does_not() {
    let (_dir, mut e, ids, _) = editor(&["a.jpg"]);
    assert!(e.stand_in_for(ids[0]).is_none());
    let library = e.library.as_mut().unwrap();
    let path = library.photo(ids[0]).unwrap().path.clone();
    library
        .session
        .save_edit(
            ids[0],
            &path,
            &crate::storage::Identity::read(&path).unwrap(),
            &Default::default(),
            &Default::default(),
            crate::catalog::HistoryUpdate::Keep,
        )
        .unwrap();
    let wanted = e.stand_in_for(ids[0]).expect("a stored preview");
    // Offline in the Loupe, it is shown.
    std::fs::remove_file(&path).unwrap();
    let library = e.library.as_mut().unwrap();
    library.refresh().unwrap();
    library.wait_for_availability();
    library.select(Some(ids[0]));
    library.open_loupe();
    e.loupe_stored_preview();
    assert_eq!(
        e.library.as_ref().unwrap().loupe_stored(),
        Some(&(ids[0], wanted.identity))
    );
}

#[test]
fn a_load_that_fails_lets_go_of_the_stored_preview() {
    let (_dir, mut e, ids, cache) = editor(&["a.ARW"]);
    store(&e, &cache, ids[0], 200);
    let load = open(&mut e, ids[0], None);
    until(&mut e, |e| e.preview.stand_in.is_some());
    e.tx.send(Event::Failed {
        id: load,
        task: crate::app::worker::TaskKind::Load,
        error: "unreadable".into(),
    })
    .unwrap();
    let ctx = e.context.clone();
    e.events(&ctx);
    assert!(!e.preview.standing_in());
}

#[test]
fn a_first_render_of_a_region_takes_over_too() {
    let (_dir, mut e, ids, cache) = editor(&["a.ARW"]);
    store(&e, &cache, ids[0], 200);
    open(&mut e, ids[0], None);
    until(&mut e, |e| e.preview.stand_in.is_some());
    let ctx = e.context.clone();
    e.set_pixels(&ctx, true, [2, 2], &[0; 12], None);
    assert!(!e.preview.standing_in());
}

#[test]
fn a_stored_preview_arriving_after_a_region_render_is_dropped() {
    let (_dir, mut e, ids, _) = editor(&["a.ARW"]);
    let load = open(&mut e, ids[0], None);
    let ctx = e.context.clone();
    e.set_pixels(&ctx, true, [2, 2], &[0; 12], None);
    let wanted = e.stand_in_for(ids[0]).unwrap();
    e.stand_in_ready(
        &ctx,
        StandIn {
            load: Some(load),
            wanted,
            image: egui::ColorImage::new([4, 4], vec![egui::Color32::WHITE; 16]),
        },
    );
    assert!(e.preview.stand_in.is_none());
}

#[test]
fn stepping_onto_a_neighbour_prepared_ahead_does_not_hang() {
    let (dir, mut e, ids, cache) = editor(&["a.ARW", "b.ARW"]);
    store(&e, &cache, ids[1], 200);
    open(&mut e, ids[0], Some(ids[1]));
    until(&mut e, |e| !e.stand_ins.prepared.is_empty());
    e.module = crate::app::Module::Develop;
    // → in Develop, as a key: showing the prepared preview uploads a texture,
    // which must not happen while the frame's input is locked (issue 455).
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let ctx = e.context.clone();
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: [true, false]
                    .map(|pressed| egui::Event::Key {
                        key: egui::Key::ArrowRight,
                        physical_key: Some(egui::Key::ArrowRight),
                        pressed,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    })
                    .into(),
                ..Default::default()
            },
            |ui| e.draw(ui),
        );
        output.textures_delta.clear();
        let _ = done_tx.send((e.document.catalog_photo, e.preview.stand_in.is_some(), dir));
    });
    let (photo, standing_in, _dir) = done_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the frame never finished");
    assert_eq!(photo, Some(ids[1]));
    assert!(standing_in);
}
