//! Library previews converted to the monitor profile: the grid's and
//! filmstrip's thumbnails and edited previews, the Loupe's preview and zoomed
//! region, and Compare's and Survey's screen previews.
use super::*;
use std::path::{Path, PathBuf};

/// A wide-gamut monitor: sRGB's saturated colors land on other device values.
fn display_p3() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("crates/rawmakase-decode/tests/data/monitor/display-p3-v4.icc")
}

const RED: [u8; 3] = [200, 40, 30];

/// `RED` as the monitor at `profile` is sent it.
fn on_monitor(profile: &Path) -> [u8; 3] {
    let mut rgb = RED.to_vec();
    crate::color_management::display_transform(profile, &mut rgb).unwrap();
    [rgb[0], rgb[1], rgb[2]]
}

/// The first pixel of the texture uploaded as `handle`, read from the next
/// frame's texture deltas.
fn uploaded(ctx: &egui::Context, handle: &egui::TextureHandle) -> [u8; 3] {
    let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
    let (_, deltas) = output
        .textures_delta
        .set
        .iter()
        .find(|(id, _)| **id == handle.id())
        .expect("the texture is uploaded");
    let egui::ImageData::Color(image) = &deltas.last().expect("a delta").image;
    let [r, g, b, _] = image.pixels[0].to_array();
    output.textures_delta.clear();
    [r, g, b]
}

fn close(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(a, b)| a.abs_diff(b) <= 1)
}

/// A catalog of PNGs filled with [`RED`], untagged, so read as sRGB.
fn red_library(names: &[&str]) -> Result<(tempfile::TempDir, Library)> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    for name in names {
        image::RgbImage::from_pixel(96, 64, image::Rgb(RED)).save(folder.join(name))?;
    }
    let path = directory.path().join("library.rawmakase");
    Catalog::create(&path)?.add_folder(&folder)?;
    let library = Library::load(&path, egui::Context::default())?;
    Ok((directory, library))
}

fn wait_for_thumbnail(library: &mut Library, path: &Path) {
    let ctx = library.ctx.clone();
    let started = std::time::Instant::now();
    while !library.cache.thumbs.contains_key(path) {
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
        std::thread::sleep(std::time::Duration::from_millis(5));
        library.cache.poll(&ctx);
    }
}

/// Issue 550 and a chosen profile on macOS: Develop converted its pixels to
/// the monitor, the grid showed sRGB as device RGB.
#[test]
fn grid_thumbnails_are_converted_to_the_monitor_profile() -> Result<()> {
    let (_directory, mut library) = red_library(&["a.png"])?;
    let ctx = library.ctx.clone();
    let photo = library.session.photos()[0].clone();
    let expected = on_monitor(&display_p3());
    assert!(!close(expected, RED), "the profile moves the color");

    library.set_display(Some(display_p3()));
    library.cache.request_thumbnail(&photo.path, &ctx);
    wait_for_thumbnail(&mut library, &photo.path);
    let texture = library.texture(&photo).unwrap().clone();
    let got = uploaded(&ctx, &texture);
    assert!(close(got, expected), "{got:?} is not {expected:?}");

    // Back to sRGB: the converted texture goes, and the next is sRGB again.
    library.set_display(None);
    assert!(library.texture(&photo).is_none());
    library.cache.request_thumbnail(&photo.path, &ctx);
    wait_for_thumbnail(&mut library, &photo.path);
    let texture = library.texture(&photo).unwrap().clone();
    // Within the preview cache's JPEG rounding.
    let got = uploaded(&ctx, &texture);
    assert!(close(got, RED), "{got:?} is not {RED:?}");
    Ok(())
}

fn p3() -> crate::app::preferences::monitor::DisplayProfile {
    crate::app::preferences::monitor::DisplayProfile::new(Some(display_p3()))
}

/// A thumbnail converted for the profile used before a switch is not shown:
/// it is asked for again, converted for the new one.
#[test]
fn a_thumbnail_converted_for_the_last_profile_is_asked_for_again() -> Result<()> {
    let (_directory, mut library) = red_library(&["a.png"])?;
    let ctx = library.ctx.clone();
    let path = library.session.photos()[0].path.clone();
    let (tx, rx) = std::sync::mpsc::sync_channel(8);
    library.cache.thumb_tx = tx;
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(8);
    library.cache.thumb_rx = result_rx;
    library.set_display(Some(display_p3()));
    library.cache.request_thumbnail(&path, &ctx);
    let asked: Vec<_> = rx.try_iter().collect();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].display, p3());

    library.set_display(None);
    result_tx.try_send(previews::PreviewResult {
        path: path.clone(),
        image: Some(image::RgbImage::new(4, 4)),
        display: p3(),
        cache_error: None,
        skipped: false,
    })?;
    library.cache.poll(&ctx);
    assert!(!library.cache.thumbs.contains_key(&path));
    assert!(!library.cache.pending.contains(&path));
    assert!(!library.cache.failed.contains(&path));
    library.cache.request_thumbnail(&path, &ctx);
    let asked: Vec<_> = rx.try_iter().collect();
    assert_eq!(asked.len(), 1);
    assert!(!asked[0].display.converts());
    Ok(())
}

/// Edited previews capture the profile; a switch drops the ones shown and
/// the results still on their way.
#[test]
fn edited_previews_follow_a_profile_switch() -> Result<()> {
    let (_directory, mut library) = red_library(&["a.png"])?;
    let ctx = library.ctx.clone();
    let photo = library.session.photos()[0].clone();
    let sent = library.detach_edited_previews();
    let (tx, rx) = std::sync::mpsc::channel();
    library.cache.edit_rx = rx;
    library.set_display(Some(display_p3()));
    let edit = || Some(previews::EditSource::saved(Default::default()));
    library.cache.request_edited(&photo, edit);
    let Some(previews::EditJob::Render {
        ticket, display, ..
    }) = sent.try_recv()
    else {
        panic!("a render");
    };
    assert_eq!(display, p3());
    tx.send(previews::EditResult::Ready(
        photo.id,
        ticket,
        image::RgbImage::new(4, 4),
    ))?;
    library.cache.poll(&ctx);
    assert!(library.cache.has_edited(photo.id));

    // A second request on its way when the profile changes.
    library.cache.ask_again(photo.id);
    library.cache.request_edited(&photo, edit);
    let Some(previews::EditJob::Render { ticket, .. }) = sent.try_recv() else {
        panic!("a render");
    };
    library.set_display(None);
    assert!(!library.cache.has_edited(photo.id));
    tx.send(previews::EditResult::Ready(
        photo.id,
        ticket,
        image::RgbImage::new(4, 4),
    ))?;
    library.cache.poll(&ctx);
    assert!(!library.cache.has_edited(photo.id));
    library.cache.request_edited(&photo, edit);
    let Some(previews::EditJob::Render { display, .. }) = sent.try_recv() else {
        panic!("a render");
    };
    assert!(!display.converts());
    Ok(())
}

/// Develop's render, shown as the photo's thumbnail, is converted on the
/// worker when a profile is used, and shown at once as sRGB when none is.
#[test]
fn develops_thumbnail_is_converted_on_the_worker() -> Result<()> {
    let (_directory, mut library) = red_library(&["a.png"])?;
    let ctx = library.ctx.clone();
    let id = library.session.photos()[0].id;
    let sent = library.detach_edited_previews();
    let recipe = crate::model::recipe::Recipe::default();
    let red = || image::RgbImage::from_pixel(4, 4, image::Rgb(RED));

    library.update_edited(&ctx, id, None, PreviewAi::Off, red(), &recipe);
    assert!(library.cache.has_edited(id));
    assert!(sent.try_recv().is_none());

    library.set_display(Some(display_p3()));
    library.update_edited(&ctx, id, None, PreviewAi::Off, red(), &recipe);
    assert!(!library.cache.has_edited(id));
    let Some(previews::EditJob::Convert {
        id: asked,
        ticket,
        display,
        ..
    }) = sent.try_recv()
    else {
        panic!("a conversion");
    };
    assert_eq!((asked, display), (id, p3()));
    assert_eq!(library.cache.edited_ticket(id), Some(ticket));
    Ok(())
}

/// The Loupe's own preview of a JPEG, TIFF or PNG, and its zoomed region,
/// are converted; a switch drops both and asks for the photo again.
#[test]
fn the_loupe_and_its_zoomed_region_are_converted() -> Result<()> {
    let (_directory, mut library) = red_library(&["a.png"])?;
    let ctx = library.ctx.clone();
    library.wait_for_availability();
    let photo = library.session.photos()[0].clone();
    library.select(Some(photo.id));
    library.open_loupe();
    assert!(library.loupe_develops().is_none());
    library.set_display(Some(display_p3()));
    let mut zoom = crate::app::view::navigator::Zoom::default();
    let mut frame = |library: &mut Library| {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200., 800.),
                )),
                ..Default::default()
            },
            |ui| {
                library.grid(ui, &mut zoom, true, None, Default::default());
            },
        )
        .textures_delta
        .clear();
    };
    frame(&mut library);
    library.loupe.wait(&ctx);
    let texture = library
        .loupe
        .texture()
        .cloned()
        .expect("the Loupe's preview");
    let expected = on_monitor(&display_p3());
    let got = uploaded(&ctx, &texture);
    assert!(close(got, expected), "{got:?} is not {expected:?}");

    library
        .loupe
        .regions
        .request(photo.id, &photo.path, None, [0.5, 0.5], [32, 32], 1.);
    library.loupe.regions.wait(&ctx);
    let (region, _) = library.loupe.regions.region.clone().expect("a region");
    let got = uploaded(&ctx, &region);
    assert!(close(got, expected), "{got:?} is not {expected:?}");

    library.set_display(None);
    assert!(library.loupe.texture().is_none());
    assert!(library.loupe.regions.region.is_none());
    frame(&mut library);
    library.loupe.wait(&ctx);
    let texture = library.loupe.texture().cloned().expect("asked for again");
    let got = uploaded(&ctx, &texture);
    assert!(close(got, RED), "{got:?} is not {RED:?}");
    Ok(())
}

/// The workspace hands the Library the profile Develop converts to, each
/// frame.
#[test]
fn the_library_converts_to_the_profile_develop_uses() -> Result<()> {
    let (_directory, library) = red_library(&["a.png"])?;
    let ctx = library.ctx.clone();
    let mut editor = crate::app::Editor::with_context(&ctx, None, Default::default(), None);
    editor.library = Some(Box::new(library));
    editor.view.monitor = crate::app::preferences::monitor::MonitorProfile::Custom(display_p3());
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| editor.draw(ui));
    output.textures_delta.clear();
    assert_eq!(editor.library.as_ref().unwrap().cache.display, p3());
    editor.view.monitor = crate::app::preferences::monitor::MonitorProfile::Srgb;
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| editor.draw(ui));
    output.textures_delta.clear();
    assert!(!editor.library.as_ref().unwrap().cache.display.converts());
    Ok(())
}

/// Build Standard-Sized Previews shows what it built as the photo's
/// thumbnail: converted on the worker too, never uploaded as sRGB.
#[test]
fn a_built_preview_is_converted_on_the_worker() -> Result<()> {
    let (_directory, mut library) = red_library(&["a.png"])?;
    let ctx = library.ctx.clone();
    let photo = library.session.photos()[0].clone();
    let sent = library.detach_edited_previews();
    library.set_display(Some(display_p3()));
    library.cache.request_edited(&photo, || {
        Some(previews::EditSource::saved(Default::default()))
    });
    let Some(previews::EditJob::Render { ticket, .. }) = sent.try_recv() else {
        panic!("a render");
    };
    let built = image::RgbImage::from_pixel(4, 4, image::Rgb(RED));
    library.show_built_preview(&ctx, photo.id, &built);
    assert!(!library.cache.has_edited(photo.id));
    let Some(previews::EditJob::Convert {
        ticket: asked,
        display,
        ..
    }) = sent.try_recv()
    else {
        panic!("a conversion");
    };
    assert_eq!((asked, display), (ticket, p3()));
    // Without a profile it is shown at once, as before.
    library.set_display(None);
    library.show_built_preview(&ctx, photo.id, &built);
    assert!(library.cache.has_edited(photo.id));
    Ok(())
}
