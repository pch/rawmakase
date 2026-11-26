//! Thumbnails of photos whose files have no embedded preview the grid can show:
//! a film scanner's linear DNG, whose previews are uncompressed 16-bit grey, or a
//! DNG with no preview at all. Such a photo without an edit is developed with the
//! defaults, as Develop would open it; a file with a JPEG preview still shows that.
use super::*;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// One TIFF directory: its entries (tag, type, count, value), sorted by tag
/// once written, and its one strip, whose offset goes in entry 273.
struct Directory {
    entries: Vec<(u16, u16, u32, Vec<u8>)>,
    strip: Vec<u8>,
}

fn short(v: u16) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}
fn long(v: u32) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

/// An uncompressed 16-bit image of one sample: `photometric` 1 (grey) or
/// 34892 (linear raw), a reduced image when `reduced`, its samples rising from
/// left to right up to `top`.
fn image(width: u32, height: u32, photometric: u16, reduced: bool, top: u16) -> Directory {
    let samples: Vec<u8> = (0..height)
        .flat_map(|_| (0..width).map(move |x| (u32::from(top) * x / (width - 1)) as u16))
        .flat_map(u16::to_le_bytes)
        .collect();
    Directory {
        entries: vec![
            (254, 4, 1, long(u32::from(reduced))),
            (256, 4, 1, long(width)),
            (257, 4, 1, long(height)),
            (258, 3, 1, short(16)),
            (259, 3, 1, short(1)),
            (262, 3, 1, short(photometric)),
            (273, 4, 1, long(0)),
            (277, 3, 1, short(1)),
            (278, 4, 1, long(height)),
            (279, 4, 1, long(samples.len() as u32)),
        ],
        strip: samples,
    }
}

/// The tags that make IFD0 a DNG's, of a film scanner without color data.
fn dng_tags() -> Vec<(u16, u16, u32, Vec<u8>)> {
    vec![
        (271, 2, 8, b"Scanner\0".to_vec()),
        (272, 2, 10, b"Film scan\0".to_vec()),
        (50706, 1, 4, vec![1, 4, 0, 0]),
        (50708, 2, 18, b"Scanner Film scan\0".to_vec()),
    ]
}

/// Appends `directory` to `out`, its strip and its values longer than four
/// bytes first, and returns where it starts.
fn put(out: &mut Vec<u8>, mut directory: Directory, sub: Option<u32>, next: u32) -> u32 {
    let strip = out.len() as u32;
    out.extend(&directory.strip);
    if let Some(sub) = sub {
        directory.entries.push((330, 4, 1, long(sub)));
    }
    directory.entries.sort_by_key(|e| e.0);
    let mut fields = Vec::new();
    for (tag, kind, count, value) in &directory.entries {
        let value = if *tag == 273 {
            long(strip)
        } else {
            value.clone()
        };
        let field = if value.len() <= 4 {
            value.iter().copied().chain([0; 4]).take(4).collect()
        } else {
            out.resize(out.len() + out.len() % 2, 0);
            let at = out.len() as u32;
            out.extend(&value);
            long(at)
        };
        fields.push((*tag, *kind, *count, field));
    }
    out.resize(out.len() + out.len() % 2, 0);
    let at = out.len() as u32;
    out.extend(short(fields.len() as u16));
    for (tag, kind, count, field) in fields {
        out.extend(short(tag));
        out.extend(short(kind));
        out.extend(long(count));
        out.extend(field);
    }
    out.extend(long(next));
    at
}

/// A DNG as a film scanner writes one: a 16-bit grey preview in IFD0, whose
/// SubIFD holds the linear raw of one sample, and a larger grey preview in
/// IFD1; neither preview is JPEG, and nothing says how they are encoded.
fn scanner_dng(folder: &Path) -> PathBuf {
    let mut out = b"II*\0".to_vec();
    out.extend(long(0));
    let raw = put(&mut out, image(64, 48, 34892, false, 40000), None, 0);
    let ifd1 = put(&mut out, image(48, 36, 1, true, 40000), None, 0);
    let mut ifd0 = image(32, 24, 1, true, 40000);
    ifd0.entries.extend(dng_tags());
    let ifd0 = put(&mut out, ifd0, Some(raw), ifd1);
    out[4..8].copy_from_slice(&ifd0.to_le_bytes());
    let path = folder.join("scan.dng");
    std::fs::write(&path, out).unwrap();
    path
}

/// A linear DNG of one sample with no preview at all.
fn bare_dng(folder: &Path) -> PathBuf {
    let mut out = b"II*\0".to_vec();
    out.extend(long(0));
    let mut ifd0 = image(64, 48, 34892, false, 40000);
    ifd0.entries.extend(dng_tags());
    let ifd0 = put(&mut out, ifd0, None, 0);
    out[4..8].copy_from_slice(&ifd0.to_le_bytes());
    let path = folder.join("bare.dng");
    std::fs::write(&path, out).unwrap();
    path
}

/// A Library of the folder `make` writes its photo into, its previews kept in
/// a cache of its own.
fn library_with(make: impl FnOnce(&Path) -> PathBuf) -> Result<(tempfile::TempDir, Library)> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    make(&folder);
    let path = directory.path().join("library.rawmakase");
    Catalog::create(&path)?.add_folder(&folder)?;
    let mut library = Library::load(&path, egui::Context::default())?;
    library.cache =
        textures::PreviewTextures::with_cache(&library.ctx, directory.path().join("previews"));
    Ok((directory, library))
}

/// One frame of a view showing the Library's only photo: what it showed last
/// frame published, its previews asked for, and those that came taken.
fn frame(library: &mut Library) {
    let ctx = library.ctx.clone();
    let photo = library.session.photos()[0].clone();
    library.cache.publish_shown();
    library.request_previews(&photo, &ctx);
    library.cache.poll(&ctx);
}

/// Shows the Library's only photo frame after frame until `done` or 30 s have
/// passed; whether `done` came.
fn show_until(library: &mut Library, done: impl Fn(&Library, &Photo) -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(30) {
        frame(library);
        let photo = library.session.photos()[0].clone();
        if done(library, &photo) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

/// Whether thumbnails or edited previews are still being made for the views.
fn busy(library: &Library) -> bool {
    !library.cache.pending.is_empty()
        || library.cache.edits_pending > 0
        || !library.cache.stand_ins.is_empty()
}

/// Shows the Library's only photo until nothing has been building for three
/// frames in a row (a request is sent in the frame after the one that answers
/// what it waited for), or 30 s have passed; whether it settled.
fn settle(library: &mut Library) -> bool {
    let started = Instant::now();
    let mut idle = 0;
    while started.elapsed() < Duration::from_secs(30) {
        frame(library);
        idle = if busy(library) { 0 } else { idle + 1 };
        if idle == 3 {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

/// `frames` frames more, failing should any of them ask for a preview.
fn stays_settled(library: &mut Library, frames: usize) {
    for _ in 0..frames {
        frame(library);
        assert!(!busy(library), "asked for again");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The pixels of the texture `handle` uploaded, from the next frame's deltas.
fn uploaded(ctx: &egui::Context, handle: &egui::TextureHandle) -> egui::ColorImage {
    let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
    let (_, deltas) = output
        .textures_delta
        .set
        .iter()
        .find(|(id, _)| **id == handle.id())
        .expect("the texture is uploaded");
    let egui::ImageData::Color(image) = &deltas.last().expect("a delta").image;
    let image = (**image).clone();
    output.textures_delta.clear();
    image
}

/// A film scanner's DNG and a DNG with no preview: neither gives an embedded
/// thumbnail, and both get one developed from the photo, grey and rising
/// from left to right as their raws do, made once.
#[test]
fn a_raw_without_a_usable_preview_gets_a_developed_thumbnail() -> Result<()> {
    for make in [scanner_dng, bare_dng] {
        let (_directory, mut library) = library_with(make)?;
        let path = library.session.photos()[0].path.clone();
        assert!(
            thumbnail(&path).is_err(),
            "{} has an embedded preview",
            path.display()
        );
        assert!(
            show_until(&mut library, |l, photo| l.texture(photo).is_some()),
            "{} has no thumbnail",
            path.display()
        );
        let photo = library.session.photos()[0].clone();
        let texture = library.texture(&photo).unwrap().clone();
        let [width, height] = texture.size();
        assert_eq!(width * 3, height * 4, "the photo's shape: {width}×{height}");
        let image = uploaded(&library.ctx, &texture);
        let at = |x: usize| image.pixels[height / 2 * width + x];
        let (dark, light) = (at(1), at(width - 2));
        assert!(
            dark.r() + 50 < light.r() && light.r() > 100,
            "{dark:?} to {light:?}"
        );
        for p in [dark, light] {
            assert!(
                p.r().abs_diff(p.g()) <= 2 && p.b().abs_diff(p.g()) <= 2,
                "{p:?}"
            );
        }
        assert!(settle(&mut library));
        stays_settled(&mut library, 20);
        assert_eq!(library.texture(&photo).map(|t| t.id()), Some(texture.id()));
    }
    Ok(())
}

/// The developed thumbnail is of the defaults the Library previews unedited
/// photos with, and is asked for once the embedded one has failed, not before.
/// Should it fail too, nothing is left building and it is not asked again.
#[test]
fn a_photo_without_an_edit_or_a_thumbnail_is_developed_with_the_defaults() -> Result<()> {
    let (_directory, mut library) = library_with(scanner_dng)?;
    let ctx = library.ctx.clone();
    let photo = library.session.photos()[0].clone();
    let sent = library.detach_edited_previews();
    let (thumbs, asked) = std::sync::mpsc::sync_channel(8);
    library.cache.thumb_tx = thumbs;
    let (results, answered) = std::sync::mpsc::sync_channel(8);
    library.cache.thumb_rx = answered;
    let (renders, rendered) = std::sync::mpsc::channel();
    library.cache.edit_rx = rendered;
    library.request_previews(&photo, &ctx);
    assert_eq!(asked.try_iter().count(), 1);
    assert!(
        sent.try_recv().is_none(),
        "rendered before its thumbnail failed"
    );
    results.send(previews::PreviewResult {
        path: photo.path.clone(),
        image: None,
        display: Default::default(),
        cache_error: None,
        skipped: false,
    })?;
    library.cache.poll(&ctx);
    library.request_previews(&photo, &ctx);
    let Some(previews::EditJob::Render {
        source,
        ahead,
        ticket,
        ..
    }) = sent.try_recv()
    else {
        panic!("no render asked for");
    };
    assert!(!ahead);
    assert_eq!(
        source.tag(),
        previews::EditSource::Defaults(library.defaults.clone()).tag()
    );
    // Asked for once.
    library.request_previews(&photo, &ctx);
    assert!(sent.try_recv().is_none());
    assert_eq!(asked.try_iter().count(), 0);
    assert!(busy(&library));
    renders.send(previews::EditResult::Failed(photo.id, ticket))?;
    library.cache.poll(&ctx);
    assert!(!busy(&library));
    library.request_previews(&photo, &ctx);
    assert!(sent.try_recv().is_none());
    assert_eq!(asked.try_iter().count(), 0);
    assert!(library.texture(&photo).is_none());
    Ok(())
}

/// A RAW with a JPEG preview shows it, as before: nothing is developed.
#[test]
fn a_raw_with_a_jpeg_preview_shows_it_without_developing() -> Result<()> {
    let (_directory, mut library) = library_with(crate::platform::preview_reads::fixture::dng)?;
    let sent = library.detach_edited_previews();
    assert!(show_until(&mut library, |l, photo| {
        l.cache.thumbs.contains_key(&photo.path)
    }));
    stays_settled(&mut library, 20);
    let photo = library.session.photos()[0].clone();
    assert!(!library.cache.has_edited(photo.id));
    assert!(sent.try_recv().is_none(), "developed for its thumbnail");
    Ok(())
}

/// A file that neither has a preview nor develops gets no thumbnail and
/// leaves nothing building: it fails once and is not asked for again.
#[test]
fn a_file_that_neither_shows_nor_develops_fails_once() -> Result<()> {
    let (_directory, mut library) = library_with(|folder| {
        let path = folder.join("broken.dng");
        std::fs::write(&path, b"II*\0not a photo").unwrap();
        path
    })?;
    assert!(settle(&mut library));
    let photo = library.session.photos()[0].clone();
    assert!(library.cache.failed.contains(&photo.path));
    stays_settled(&mut library, 20);
    assert!(library.texture(&photo).is_none());
    Ok(())
}

/// The Library's only photo shown once with its edited previews detached and
/// answered by hand, and its embedded thumbnail failed: what it sends to the
/// edited preview worker, and where that worker's answers go.
fn without_its_thumbnail(
    library: &mut Library,
) -> Result<(
    previews::Sent,
    std::sync::mpsc::Sender<previews::EditResult>,
)> {
    let ctx = library.ctx.clone();
    let photo = library.session.photos()[0].clone();
    let sent = library.detach_edited_previews();
    let (thumbs, _asked) = std::sync::mpsc::sync_channel(8);
    library.cache.thumb_tx = thumbs;
    let (results, answered) = std::sync::mpsc::sync_channel(8);
    library.cache.thumb_rx = answered;
    let (renders, rendered) = std::sync::mpsc::channel();
    library.cache.edit_rx = rendered;
    library.request_previews(&photo, &ctx);
    results.send(previews::PreviewResult {
        path: photo.path.clone(),
        image: None,
        display: Default::default(),
        cache_error: None,
        skipped: false,
    })?;
    library.cache.poll(&ctx);
    Ok((sent, renders))
}

/// Raw Defaults other than the default ones.
fn other_defaults() -> std::sync::Arc<crate::raw_defaults::DevelopDefaults> {
    let settings = crate::raw_defaults::RawDefaults {
        camera_overrides: true,
        ..Default::default()
    };
    std::sync::Arc::new(crate::raw_defaults::DevelopDefaults::with_presets(
        settings,
        |_| None,
    ))
}

/// Raw Defaults changed while a developed thumbnail is still rendering: what
/// comes back was rendered with the old ones and is not shown, and the photo is
/// developed again with the new ones.
#[test]
fn a_defaults_change_drops_a_developed_thumbnail_still_rendering() -> Result<()> {
    let (_directory, mut library) = library_with(scanner_dng)?;
    let ctx = library.ctx.clone();
    let photo = library.session.photos()[0].clone();
    let (sent, renders) = without_its_thumbnail(&mut library)?;
    library.request_previews(&photo, &ctx);
    let Some(previews::EditJob::Render { ticket, .. }) = sent.try_recv() else {
        panic!("no render asked for");
    };
    let defaults = other_defaults();
    library.set_defaults(defaults.clone());
    renders.send(previews::EditResult::Ready(
        photo.id,
        ticket,
        image::RgbImage::new(4, 3),
    ))?;
    library.cache.poll(&ctx);
    assert!(
        library.texture(&photo).is_none(),
        "shown with the old defaults"
    );
    library.request_previews(&photo, &ctx);
    let Some(previews::EditJob::Render { source, .. }) = sent.try_recv() else {
        panic!("not developed again");
    };
    assert_eq!(source.tag(), previews::EditSource::Defaults(defaults).tag());
    Ok(())
}

/// What the Library's preview status line says, line by line, as the status
/// row draws it.
fn status(library: &Library) -> Vec<String> {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        if library.cache.progress_active() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                library.cache.show_progress(ui);
            });
        }
    });
    output.textures_delta.clear();
    output
        .platform_output
        .accesskit_update
        .expect("accessibility is on")
        .nodes
        .iter()
        .filter_map(|(_, node)| node.label().or(node.value()).map(str::to_string))
        .fold(Vec::new(), |mut lines, line| {
            // A label's text is its node's and its text run's.
            if !lines.contains(&line) {
                lines.push(line);
            }
            lines
        })
}

/// The status line says a photo is being developed in place of its embedded
/// preview, not that an edited preview is rendering (it has no edit), and once
/// it is shown doesn't count it unavailable. One whose develop fails too is.
#[test]
fn the_status_line_tells_a_developed_thumbnail_from_an_unavailable_one() -> Result<()> {
    for develops in [true, false] {
        let (_directory, mut library) = library_with(scanner_dng)?;
        let ctx = library.ctx.clone();
        let photo = library.session.photos()[0].clone();
        let (sent, renders) = without_its_thumbnail(&mut library)?;
        library.request_previews(&photo, &ctx);
        let Some(previews::EditJob::Render { ticket, .. }) = sent.try_recv() else {
            panic!("no render asked for");
        };
        // Neither an edited preview nor, while it may still come, unavailable.
        assert_eq!(status(&library), ["Rendering 1 preview"]);
        assert!(busy(&library));
        renders.send(if develops {
            previews::EditResult::Ready(photo.id, ticket, image::RgbImage::new(4, 3))
        } else {
            previews::EditResult::Failed(photo.id, ticket)
        })?;
        library.cache.poll(&ctx);
        assert!(!busy(&library));
        assert_eq!(library.texture(&photo).is_some(), develops);
        let shown = status(&library);
        if develops {
            assert!(shown.is_empty(), "{shown:?}");
        } else {
            assert_eq!(shown, ["1 preview unavailable"]);
        }
    }
    Ok(())
}
