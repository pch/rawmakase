use super::*;
use crate::app::Module;
use crate::app::export::naming::editor as name_editor;
use crate::app::export::naming::presets::Preset;
use crate::catalog::PhotoId;
use crate::export_settings::NameContext;
use crate::export_settings::{Destination, Format};
use name_read::{FileFields, Status};

/// An editor on a catalog of two copies of the synthetic chart DNG and a JPEG.
fn editor() -> anyhow::Result<(tempfile::TempDir, Editor, Vec<PhotoId>, egui::Context)> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    let chart = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/charts/synthetic-d65.dng");
    for name in ["a.dng", "b.dng"] {
        std::fs::copy(&chart, photos.join(name))?;
    }
    image::RgbImage::new(8, 8).save(photos.join("c.jpg"))?;
    let catalog = dir.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&catalog)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.raw_defaults = Arc::new(crate::raw_defaults::DevelopDefaults::with_presets(
        Default::default(),
        |_| None,
    ));
    let library = crate::app::library::Library::load(&catalog, ctx.clone())?;
    let ids = library.session.photos().iter().map(|p| p.id).collect();
    e.library = Some(Box::new(library));
    e.module = Module::Library;
    Ok((dir, e, ids, ctx))
}
/// Small TIFFs in `folder`.
fn settings(folder: &std::path::Path) -> ExportSettings {
    ExportSettings {
        destination: Destination::Folder,
        folder: Some(folder.to_path_buf()),
        format: Format::Tiff,
        resize: true,
        long_edge: 96,
        ..Default::default()
    }
}
/// Exports `scope` as the dialog's Export button does, short of saving the
/// settings as the last export's.
fn export(e: &mut Editor, scope: Scope, settings: ExportSettings) -> anyhow::Result<()> {
    let batch = e.batch_photos(&scope, false)?;
    e.plan_export(
        Pending {
            batch,
            settings,
            watermark: None,
            left_out: scope.left_out,
            conflicts: Vec::new(),
        },
        None,
    );
    Ok(())
}
/// Takes the editor's events until every export has finished.
fn wait(e: &mut Editor, ctx: &egui::Context) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while !e.exports.queued.is_empty() {
        assert!(std::time::Instant::now() < deadline, "export not finished");
        e.events(ctx);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
fn listing(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn export_acts_on_the_selection_and_reports_what_it_left_out() -> anyhow::Result<()> {
    let (dir, mut e, ids, ctx) = editor()?;
    let library = e.library.as_mut().unwrap();
    library.select(Some(ids[0]));
    library.select_range_to(ids[2]);
    // The second RAW goes offline; the JPEG is exported like the first.
    let offline = library.photo(ids[1]).unwrap().path.clone();
    std::fs::remove_file(&offline)?;
    let scope = e.export_scope().expect("photos to export");
    assert_eq!(
        scope
            .photos
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["a.dng", "c.jpg"]
    );
    assert_eq!(scope.left_out.len(), 1, "{:?}", scope.left_out);
    assert_eq!(scope.left_out[0].0, "b.dng");
    let out = dir.path().join("out");
    export(&mut e, scope.clone(), settings(&out))?;
    wait(&mut e, &ctx);
    assert_eq!(listing(&out), ["a.tif", "c.tif"]);
    assert_eq!(e.messages.line(), "Exported 2 of 3 · 1 not exported");
    // Kept in the top bar until dismissed: the offline RAW, and why.
    let failed = "Exported 2 of 3 · 1 not exported";
    let detail = e.messages.entry_detail(failed).expect("the failure");
    assert!(detail.contains("b.dng: "), "{detail}");
    assert!(detail.contains("can't be exported"), "{detail}");

    // Again into the same folder: Ask asks once for both files.
    export(&mut e, scope, settings(&out))?;
    let pending = e.exports.pending.take().expect("the question");
    assert_eq!(pending.conflicts, [out.join("a.tif"), out.join("c.tif")]);
    e.plan_export(pending, Some(Existing::Unique));
    wait(&mut e, &ctx);
    assert_eq!(listing(&out), ["a-2.tif", "a.tif", "c-2.tif", "c.tif"]);
    // A later export never hides an earlier failure: it stays until dismissed.
    assert!(e.messages.has_entry(failed));
    Ok(())
}

#[test]
fn an_open_photo_whose_edit_isnt_final_holds_back_only_its_own_export() -> anyhow::Result<()> {
    let (_dir, mut e, ids, _ctx) = editor()?;
    let path = e
        .library
        .as_ref()
        .unwrap()
        .photo(ids[0])
        .unwrap()
        .path
        .clone();
    // Open in Develop, its Lightroom edit still to be applied.
    e.document.catalog_photo = Some(ids[0]);
    e.document.opened_at(path);
    e.document.metadata = Some(Default::default());
    e.document
        .set_image(Arc::new(crate::camera_data::CameraImage {
            recovered: Default::default(),
            width: 1,
            height: 1,
            pixels: vec![[0.1; 3]].into(),
            metadata: Default::default(),
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        }));
    e.document.pending_lightroom = Some("s = {}".into());
    e.module = Module::Develop;
    let library = e.library.as_mut().unwrap();
    library.select(Some(ids[0]));
    assert!(e.export_scope().is_none());
    assert!(
        e.messages
            .line()
            .starts_with("Export waits for the open photo"),
        "{}",
        e.messages.line()
    );
    // A Library selection without it goes ahead.
    e.module = Module::Library;
    e.library.as_mut().unwrap().select(Some(ids[1]));
    let scope = e.export_scope().expect("the other photo");
    assert_eq!(scope.photos.len(), 1);
    assert!(!scope.photos[0].open);
    // Once final, the open photo is exported as shown.
    e.document.pending_lightroom = None;
    e.module = Module::Develop;
    e.library.as_mut().unwrap().select(Some(ids[0]));
    let scope = e.export_scope().expect("the open photo");
    assert!(scope.photos[0].open);
    let batch = e.batch_photos(&scope, true)?;
    assert!(matches!(&batch[0].edit, Edit::Shown { unsaved: true, .. }));
    // Its file gone offline since it was opened: said before the dialog, as it is
    // decoded again from the file.
    std::fs::remove_file(e.document.path.clone().unwrap())?;
    assert!(e.export_scope().is_none());
    assert!(
        e.messages.line().contains("can't be exported"),
        "{}",
        e.messages.line()
    );
    Ok(())
}

#[test]
fn a_photo_opened_without_a_catalog_is_exported_only_while_its_file_is_there() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("a.dng");
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/corpus/charts/synthetic-d65.dng"),
        &path,
    )?;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.document.opened_at(path.clone());
    e.document.metadata = Some(Default::default());
    e.document
        .set_image(Arc::new(crate::camera_data::CameraImage {
            recovered: Default::default(),
            width: 1,
            height: 1,
            pixels: vec![[0.1; 3]].into(),
            metadata: Default::default(),
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        }));
    let scope = e.export_scope().expect("the open photo");
    assert!(scope.photos[0].open && scope.photos[0].id.is_none());
    std::fs::remove_file(&path)?;
    assert!(e.export_scope().is_none());
    assert!(
        e.messages.line().contains("Offline"),
        "{}",
        e.messages.line()
    );
    Ok(())
}

#[test]
fn show_in_finder_opens_one_window_for_each_folder_exported_to() {
    let exported: Vec<PathBuf> = ["/a/1.jpg", "/a/2.jpg", "/b/3.jpg", "/a/4.jpg"]
        .iter()
        .map(PathBuf::from)
        .collect();
    assert_eq!(
        to_show(&exported),
        [PathBuf::from("/a/1.jpg"), PathBuf::from("/b/3.jpg")]
    );
    // However many folders, a handful of windows.
    let many: Vec<PathBuf> = (0..20)
        .map(|i| PathBuf::from(format!("/{i}/x.jpg")))
        .collect();
    assert_eq!(to_show(&many).len(), 5);
    assert_eq!(folders(&many), 20);
}

/// Choosing what to export reads no file: a JPEG Develop can't edit is chosen,
/// and fails as its export, with the reason.
#[test]
fn a_cmyk_jpeg_is_exported_and_fails_with_why() -> anyhow::Result<()> {
    use image::ImageEncoder;
    let (dir, mut e, ids, ctx) = editor()?;
    let library = e.library.as_mut().unwrap();
    let jpeg = library.photo(ids[2]).unwrap().path.clone();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new(std::fs::File::create(&jpeg)?);
    encoder.set_icc_profile(crate::app::worker::cmyk_profile())?;
    encoder.write_image(&[10; 8 * 8 * 3], 8, 8, image::ExtendedColorType::Rgb8)?;
    library.select(Some(ids[2]));
    let scope = e.export_scope().expect("photos to export");
    assert_eq!(scope.photos.len(), 1);
    assert!(scope.left_out.is_empty(), "{:?}", scope.left_out);
    let out = dir.path().join("out");
    export(&mut e, scope, settings(&out))?;
    wait(&mut e, &ctx);
    assert!(listing(&out).is_empty());
    let detail = e
        .messages
        .entry_detail("Exported 0 of 1 · 1 not exported")
        .expect("the failure");
    assert!(
        detail.contains("c.jpg: ") && detail.contains("CMYK"),
        "{detail}"
    );
    Ok(())
}

/// Return is the Export button, as in Lightroom, but not while the Edit
/// Watermark window, opened from the dialog, is on top of it.
#[test]
fn return_exports_from_the_dialog() -> anyhow::Result<()> {
    let (_dir, mut e, ids, ctx) = editor()?;
    e.library.as_mut().unwrap().select(Some(ids[0]));
    let return_key = |e: &mut Editor| {
        let key = |pressed| egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        for events in [vec![key(true), key(false)], vec![]] {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| e.export_windows(ui.ctx()),
            );
            output.textures_delta.clear();
        }
    };
    // No folder chosen: what Export says, and nothing is exported or saved.
    let open = |e: &mut Editor| {
        e.exports.scope = e.export_scope().expect("a photo to export");
        e.exports.draft = ExportSettings {
            destination: Destination::Folder,
            folder: None,
            ..Default::default()
        };
        e.exports.dialog = true;
        e.messages.clear_line();
    };
    open(&mut e);
    e.exports.watermark_editor = Some(super::watermark_editor::WatermarkEditor::new(
        Default::default(),
        &ctx,
    ));
    return_key(&mut e);
    assert_eq!(e.messages.line(), "");
    assert!(e.exports.dialog);
    e.exports.watermark_editor = None;
    open(&mut e);
    return_key(&mut e);
    assert_eq!(e.messages.line(), "Choose a folder to export to");
    assert!(e.exports.dialog);
    // ProPhoto RGB in a JPEG: said, and the dialog stays open with nothing queued.
    open(&mut e);
    e.exports.draft.folder = Some(_dir.path().join("out"));
    e.exports
        .draft
        .choose_color_space(crate::color::OutputSpace::ProPhoto);
    return_key(&mut e);
    assert!(
        e.messages.line().contains("ProPhoto RGB"),
        "{}",
        e.messages.line()
    );
    assert!(e.exports.dialog && e.exports.queued.is_empty());
    Ok(())
}

/// Export with Previous exports in the saved color space, and refuses one its
/// format can't hold or a later release saved, before anything renders.
#[test]
fn export_with_previous_keeps_its_colour_space_or_refuses_it() -> anyhow::Result<()> {
    use crate::color::OutputSpace;
    let (dir, mut e, ids, ctx) = editor()?;
    e.exports.unsaved_settings = true;
    e.library.as_mut().unwrap().select(Some(ids[0]));
    let out = dir.path().join("out");
    let mut jpeg = settings(&out);
    jpeg.choose_format(Format::Jpeg);
    jpeg.choose_color_space(OutputSpace::ProPhoto);
    e.export_previous(Some(jpeg));
    assert!(e.exports.queued.is_empty());
    assert!(
        e.messages.line().contains("ProPhoto RGB"),
        "{}",
        e.messages.line()
    );
    let file = dir.path().join("export.json");
    std::fs::write(&file, r#"{"color_space":"DisplayP3","format":"Tiff"}"#)?;
    let mut later = ExportSettings::load_file(&file).unwrap();
    (later.destination, later.folder) = (Destination::Folder, Some(out.clone()));
    e.messages.clear_line();
    e.export_previous(Some(later));
    assert!(e.exports.queued.is_empty());
    assert!(
        e.messages.line().contains("later version"),
        "{}",
        e.messages.line()
    );
    assert!(listing(&out).is_empty());
    let mut adobe = settings(&out);
    adobe.choose_color_space(OutputSpace::AdobeRgb);
    e.export_previous(Some(adobe));
    // The batch captured its settings: a choice made in the dialog meanwhile
    // doesn't change it.
    e.exports.draft.choose_color_space(OutputSpace::Srgb);
    wait(&mut e, &ctx);
    let exported = out.join("a.tif");
    let facts = crate::exif::colour_facts(&exported).unwrap();
    assert_eq!(
        (facts.color_space, facts.interop_index.as_deref()),
        (Some(0xffff), Some("R03"))
    );
    Ok(())
}

/// One frame of the export windows, with `events` as input.
fn frame(e: &mut Editor, ctx: &egui::Context, events: Vec<egui::Event>) {
    let mut output = ctx.run_ui(
        egui::RawInput {
            events,
            ..Default::default()
        },
        |ui| e.export_windows(ui.ctx()),
    );
    output.textures_delta.clear();
}
/// `key`, pressed and released, then a frame without input.
fn press(e: &mut Editor, ctx: &egui::Context, key: egui::Key) {
    let event = |pressed| egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    frame(e, ctx, vec![event(true), event(false)]);
    frame(e, ctx, vec![]);
}
/// Return, pressed and released, then a frame without input.
fn press_return(e: &mut Editor, ctx: &egui::Context) {
    press(e, ctx, egui::Key::Enter);
}
/// Takes the editor's events until `done`.
fn until(e: &mut Editor, ctx: &egui::Context, done: impl Fn(&Editor) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !done(e) {
        assert!(std::time::Instant::now() < deadline, "never happened");
        e.events(ctx);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}
/// A reader that names every camera "Sony ILCE-7RM6", once `gate` lets each
/// photo through, counting the photos read.
fn gated_reader() -> (
    name_read::Reader,
    std::sync::mpsc::Sender<()>,
    Arc<std::sync::atomic::AtomicUsize>,
) {
    let (open, gate) = std::sync::mpsc::channel::<()>();
    let gate = Mutex::new(gate);
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = count.clone();
    let reader: name_read::Reader = Arc::new(move |_photo: &name_read::ToRead| {
        let _ = gate.lock().unwrap().recv();
        counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(FileFields {
            camera: crate::export_settings::CameraNames::Read {
                make: Some("Sony".into()),
                model: Some("ILCE-7RM6".into()),
            },
            ..Default::default()
        })
    });
    (reader, open, count)
}
/// The Export dialog for the selection, with `settings`.
fn open_dialog(e: &mut Editor, settings: ExportSettings) {
    e.exports.read.cancel();
    e.exports.scope = e.export_scope().expect("photos to export");
    e.exports.draft = settings;
    e.exports.dialog = true;
    e.exports.unsaved_settings = true;
}
const MODEL: &str = "{{com.adobe.model}}-{{image_name}}";

/// The Example and the files are named from what the catalog had when Export
/// was chosen, typing still in the Library included; a change to the catalog
/// afterwards (a backfilled capture time, a title) changes neither.
#[test]
fn names_come_from_the_catalog_as_it_was_when_export_was_chosen() -> anyhow::Result<()> {
    let (dir, mut e, ids, _ctx) = editor()?;
    let library = e.library.as_mut().unwrap();
    library.session.catalog_mut_for_tests().set_text(
        &ids[..1],
        crate::metadata::TextField::Title,
        "Harbour",
    )?;
    let copy = library.create_virtual_copy(ids[0])?.value;
    library.select(Some(copy));
    // Typed, not saved yet.
    library.set_copy_name_draft(copy, "Night");
    let scope = e.export_scope().expect("the copy");
    let fields = &scope.photos[0].fields;
    assert_eq!(
        (fields.title.as_deref(), fields.copy_name.as_str()),
        (Some("Harbour"), "Night")
    );
    assert_eq!(fields.captured, None);
    // The catalog changes after the dialog opened.
    let catalog = e.library.as_mut().unwrap().session.catalog_mut_for_tests();
    catalog.set_text(&[copy], crate::metadata::TextField::Title, "Later")?;
    catalog.fill_capture_times(&[(copy, "2026-04-24T18:43:24.000".into())])?;
    let mut s = settings(&dir.path().join("out"));
    s.choose_tokens("{{com.adobe.title}} {{copy_name}} {{date_YYYY}}");
    let example = s
        .file_name_for(
            &scope.photos[0].source,
            &crate::export_settings::NameContext {
                index: 0,
                total: 1,
                fields: &scope.photos[0].fields,
            },
        )
        .0;
    assert_eq!(example, "Harbour Night.tif");
    let batch = e.batch_photos(&scope, false)?;
    let plan = batch::plan(&batch, &s, None).unwrap();
    assert_eq!(
        plan.entries[0].target.file_name().unwrap(),
        "Harbour Night.tif"
    );
    assert_eq!(plan.entries[0].notes, ["no capture date"]);
    Ok(())
}

/// A template that needs the camera's model reads it once, when it first
/// needs it; Export (and Return) wait for it; a result of another read is
/// dropped; the files are named from what was read.
#[test]
fn the_dialog_reads_the_camera_names_once_a_template_needs_them() -> anyhow::Result<()> {
    let (dir, mut e, ids, ctx) = editor()?;
    let (reader, open, read) = gated_reader();
    e.exports.read.set_reader(reader);
    e.library.as_mut().unwrap().select(Some(ids[0]));
    let out = dir.path().join("out");
    open_dialog(&mut e, settings(&out));
    // Filename needs nothing from the file.
    frame(&mut e, &ctx, vec![]);
    assert_eq!(e.exports.read.status(), Status::Idle);
    // The editor's template needs the model: read now, once.
    e.exports.draft.choose_tokens(MODEL);
    frame(&mut e, &ctx, vec![]);
    frame(&mut e, &ctx, vec![]);
    assert_eq!(
        e.exports.read.status(),
        Status::Reading { done: 0, total: 1 }
    );
    press_return(&mut e, &ctx);
    assert!(
        e.exports.dialog && e.exports.queued.is_empty(),
        "Return waits"
    );
    // A read of an earlier scope is dropped.
    e.names_read(0, Ok(vec![Ok(FileFields::default())]));
    assert!(matches!(e.exports.read.status(), Status::Reading { .. }));
    open.send(())?;
    until(&mut e, &ctx, |e| e.exports.read.status() == Status::Ready);
    frame(&mut e, &ctx, vec![]);
    assert_eq!(read.load(std::sync::atomic::Ordering::SeqCst), 1);
    press_return(&mut e, &ctx);
    assert!(!e.exports.dialog);
    wait(&mut e, &ctx);
    assert_eq!(listing(&out), ["ILCE-7RM6-a.tif"]);
    Ok(())
}

/// Cancel stops the read; its late completion changes nothing, and the next
/// dialog reads again.
#[test]
fn cancelling_the_dialog_drops_its_read() -> anyhow::Result<()> {
    let (dir, mut e, ids, ctx) = editor()?;
    let (reader, open, _read) = gated_reader();
    e.exports.read.set_reader(reader);
    e.library.as_mut().unwrap().select(Some(ids[0]));
    let mut s = settings(&dir.path().join("out"));
    s.choose_tokens(MODEL);
    open_dialog(&mut e, s.clone());
    frame(&mut e, &ctx, vec![]);
    assert!(matches!(e.exports.read.status(), Status::Reading { .. }));
    let escape = |pressed| egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    frame(&mut e, &ctx, vec![escape(true), escape(false)]);
    frame(&mut e, &ctx, vec![]);
    assert!(!e.exports.dialog);
    assert_eq!(e.exports.read.status(), Status::Idle);
    // The cancelled worker stops without a result; any that came is dropped.
    open.send(())?;
    e.names_read(1, Ok(vec![Ok(FileFields::default())]));
    assert_eq!(e.exports.read.status(), Status::Idle);
    open_dialog(&mut e, s);
    frame(&mut e, &ctx, vec![]);
    open.send(())?;
    until(&mut e, &ctx, |e| e.exports.read.status() == Status::Ready);
    assert!(matches!(
        e.exports.scope.photos[0].fields.camera,
        crate::export_settings::CameraNames::Read { .. }
    ));
    Ok(())
}

/// A reader that panics: the read fails, nothing stays busy, and Export says
/// it goes on without the names' fields; the files then say what they lack.
#[test]
fn a_failed_read_is_said_and_exports_without_it() -> anyhow::Result<()> {
    let (dir, mut e, ids, ctx) = editor()?;
    e.exports.read.set_reader(Arc::new(
        |_: &name_read::ToRead| -> Result<FileFields, String> { panic!("disk gone") },
    ));
    e.library.as_mut().unwrap().select(Some(ids[0]));
    let out = dir.path().join("out");
    let mut s = settings(&out);
    s.choose_tokens(MODEL);
    // Export with Previous waits, then opens the dialog to say so.
    e.exports.unsaved_settings = true;
    e.export_previous(Some(s));
    assert!(e.exports.waiting.is_some() && e.export_modal());
    until(&mut e, &ctx, |e| e.exports.dialog);
    assert!(e.exports.waiting.is_none());
    match e.exports.read.status() {
        Status::Failed(why) => assert!(why.contains("disk gone"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(
        e.messages.line().contains("could not be read"),
        "{}",
        e.messages.line()
    );
    press_return(&mut e, &ctx);
    wait(&mut e, &ctx);
    // As Lightroom: the text around a value it lacks stays.
    assert_eq!(listing(&out), ["-a.tif"]);
    let detail = e
        .messages
        .entry_detail("Exported 1 of 1 · 1 with notes")
        .expect("the notes");
    assert!(
        detail.contains("a.dng: the camera's make and model were not read"),
        "{detail}"
    );
    Ok(())
}

/// Export with Previous with a template that needs the files waits for them in
/// a window that can cancel, then exports; one that needs nothing exports at
/// once.
#[test]
fn export_with_previous_waits_for_what_its_names_need() -> anyhow::Result<()> {
    let (dir, mut e, ids, ctx) = editor()?;
    let (reader, open, _read) = gated_reader();
    e.exports.read.set_reader(reader);
    e.exports.unsaved_settings = true;
    e.library.as_mut().unwrap().select(Some(ids[0]));
    let out = dir.path().join("out");
    let mut s = settings(&out);
    s.choose_tokens(MODEL);
    e.export_previous(Some(s.clone()));
    assert!(e.exports.waiting.is_some() && e.exports.queued.is_empty());
    // Cancel in the window.
    frame(&mut e, &ctx, vec![]);
    let escape = |pressed| egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    frame(&mut e, &ctx, vec![escape(true), escape(false)]);
    assert!(e.exports.waiting.is_none() && !e.export_modal());
    assert_eq!(e.messages.line(), "Export cancelled");
    open.send(())?;
    // Again, to the end.
    e.export_previous(Some(s));
    open.send(())?;
    wait_for_queue(&mut e, &ctx);
    assert_eq!(listing(&out), ["ILCE-7RM6-a.tif"]);
    // Nothing to read: at once.
    e.export_previous(Some(settings(&dir.path().join("plain"))));
    assert!(e.exports.waiting.is_none() && !e.exports.queued.is_empty());
    wait(&mut e, &ctx);
    assert_eq!(listing(&dir.path().join("plain")), ["a.tif"]);
    Ok(())
}
/// Takes events until an export was queued and has finished.
fn wait_for_queue(e: &mut Editor, ctx: &egui::Context) {
    until(e, ctx, |e| !e.exports.queued.is_empty());
    wait(e, ctx);
}

/// A photo opened without a catalog has its capture time in its file only:
/// Date - Filename waits for it, which a catalog photo doesn't.
#[test]
fn a_photo_without_a_catalog_reads_its_date_from_its_file() -> anyhow::Result<()> {
    let (_dir, mut e, ids, _ctx) = editor()?;
    let date = ExportSettings {
        naming: Some(crate::export_settings::Naming::DateFilename),
        ..Default::default()
    };
    e.library.as_mut().unwrap().select(Some(ids[0]));
    e.exports.scope = e.export_scope().unwrap();
    assert!(!e.names_wait(&date.name_template()));
    let mut model = ExportSettings::default();
    model.choose_tokens(MODEL);
    e.exports
        .read
        .set_reader(Arc::new(|_: &name_read::ToRead| Ok(FileFields::default())));
    assert!(
        e.names_wait(&model.name_template()),
        "a catalog photo's model"
    );
    e.exports.read.cancel();
    // Standalone.
    e.exports.scope.photos[0].id = None;
    assert!(e.names_wait(&date.name_template()));
    e.exports.read.cancel();
    // What its file gives goes in its fields.
    let mut fields = crate::export_settings::NameFields::default();
    name_read::fill(
        &mut fields,
        FileFields {
            captured: Some("2026-04-24T18:43:24.000".into()),
            info: Some(crate::metadata::PhotoInfo {
                iso: Some(100.),
                ..Default::default()
            }),
            ..Default::default()
        },
        true,
    );
    assert_eq!(fields.captured.as_deref(), Some("2026-04-24T18:43:24.000"));
    assert_eq!(fields.info.iso, Some(100.));
    Ok(())
}

/// The real reader: a DNG's make and model from its EXIF, the maker spelled
/// as RAWmakase spells it; a file with none has none.
#[test]
fn the_reader_takes_the_files_own_names() -> anyhow::Result<()> {
    let (dir, e, ids, _ctx) = editor()?;
    let dng = e
        .library
        .as_ref()
        .unwrap()
        .photo(ids[0])
        .unwrap()
        .path
        .clone();
    let (make, model) = crate::exif::camera_names(&dng).unwrap_or_default();
    let read = name_read::read_path(&dng, true).unwrap();
    // Its info as the catalog reads it, dimensions included.
    assert!(
        read.info.as_ref().is_some_and(|i| i.dimensions.is_some()),
        "{read:?}"
    );
    assert_eq!(
        read.camera,
        crate::export_settings::CameraNames::Read {
            make: Some(crate::raw::maker_name(&make)).filter(|m| !m.is_empty()),
            model: Some(model).filter(|m| !m.is_empty()),
        }
    );
    let text = dir.path().join("note.txt");
    std::fs::write(&text, "not a photo")?;
    assert_eq!(
        name_read::read_path(&text, false).unwrap().camera,
        crate::export_settings::CameraNames::Read {
            make: None,
            model: None
        }
    );
    // A file gone is an error, not a photo without names.
    std::fs::remove_file(&text)?;
    assert!(name_read::read_path(&text, false).is_err());
    Ok(())
}

/// An X3F, which RAWmakase doesn't decode, names its export from its
/// preview's EXIF, else its property list; opened without a catalog, it
/// gets its photo info too, not "can't be read" (#671).
#[test]
fn an_x3f_names_its_export_from_its_exif_or_property_list() -> anyhow::Result<()> {
    use crate::exif::{CameraExif, Field, tag};
    use crate::export::exif::{Container, camera_raw, x3f_with_properties};
    let dir = tempfile::tempdir()?;
    let quattro = dir.path().join("SDIM4388.X3F");
    let camera = CameraExif {
        main: vec![
            Field::ascii(tag::MAKE, "SIGMA"),
            Field::ascii(tag::MODEL, "sd Quattro"),
        ],
        ..Default::default()
    };
    std::fs::write(&quattro, camera_raw(Container::X3f, camera))?;
    let sd10 = dir.path().join("RAW_SIGMA_SD10.X3F");
    std::fs::write(
        &sd10,
        x3f_with_properties(&[("CAMMANUF", "SIGMA"), ("CAMMODEL", "SIGMA SD10")]),
    )?;
    for (path, model, camera) in [
        (&quattro, "sd Quattro", "SIGMA sd Quattro"),
        (&sd10, "SIGMA SD10", "SIGMA SD10"),
    ] {
        let read = name_read::read_path(path, true).unwrap();
        assert_eq!(
            read.camera,
            crate::export_settings::CameraNames::Read {
                make: Some("Sigma".into()),
                model: Some(model.into()),
            },
            "{model}"
        );
        assert_eq!(read.info.and_then(|i| i.camera).as_deref(), Some(camera));
    }
    Ok(())
}

/// A file's own title, caption, copyright and creator: its sidecar's XMP
/// first, then its EXIF's Image Description, Copyright (the photographer's
/// part) and Artist (several people as Lightroom names them); they fill
/// only what the catalog has none of, a cleared caption staying empty.
#[test]
fn a_files_own_descriptive_text_names_what_the_catalog_lacks() -> anyhow::Result<()> {
    use crate::exif::{CameraExif, Field, tag};
    use crate::export::exif::{Container, camera_raw};
    let dir = tempfile::tempdir()?;
    let x3f = dir.path().join("SDIM0001.X3F");
    let mut copyright = b"Ann Example\0Ed Itor\0".to_vec();
    let camera = CameraExif {
        main: vec![
            Field::ascii(tag::MAKE, "SIGMA"),
            Field::ascii(tag::MODEL, "sd Quattro"),
            Field::ascii(tag::IMAGE_DESCRIPTION, "From EXIF"),
            Field::ascii(tag::ARTIST, "A One; B Two"),
            Field {
                tag: tag::COPYRIGHT,
                kind: 2,
                count: copyright.len() as u32,
                bytes: std::mem::take(&mut copyright),
            },
        ],
        exif: vec![Field::short(tag::FOCAL_LENGTH_IN_35MM, 39)],
        ..Default::default()
    };
    std::fs::write(&x3f, camera_raw(Container::X3f, camera))?;
    std::fs::write(
        dir.path().join("SDIM0001.xmp"),
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
        <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/" dc:title="Harbour"/>
        </rdf:RDF></x:xmpmeta>"#,
    )?;
    let read = name_read::read_path(&x3f, false).unwrap();
    assert_eq!(
        read.descriptive,
        name_read::FileText {
            title: Some("Harbour".into()),
            caption: Some("From EXIF".into()),
            copyright: Some("Ann Example".into()),
            creator: Some("A One, B Two".into()),
            artist: Some("A One".into()),
        }
    );
    // A RAW container's EXIF, read for what only the file says.
    assert_eq!(read.facts.focal_35mm, Some(39));
    let mut fields = NameFields {
        caption: Some(String::new()),
        creator: Some("Catalog Person".into()),
        ..Default::default()
    };
    name_read::fill(&mut fields, read, false);
    assert_eq!(fields.title.as_deref(), Some("Harbour"));
    assert_eq!(fields.caption.as_deref(), Some(""), "cleared stays cleared");
    assert_eq!(fields.copyright.as_deref(), Some("Ann Example"));
    assert_eq!(fields.creator.as_deref(), Some("Catalog Person"));
    Ok(())
}

/// What only a file says names its photo: its software (EXIF), and its
/// IPTC headline and place names (its sidecar's XMP), as Lightroom writes
/// them; a file that says none of them names nothing by them.
#[test]
fn a_files_software_and_place_names_name_its_photo() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let tif = dir.path().join("a.tif");
    // A little-endian TIFF whose one directory holds Software.
    let software = b"ProbeSoft 1.2\0";
    let mut t = b"II*\0\x08\0\0\0\x01\0".to_vec();
    t.extend(0x0131u16.to_le_bytes());
    t.extend(2u16.to_le_bytes());
    t.extend((software.len() as u32).to_le_bytes());
    t.extend(26u32.to_le_bytes());
    t.extend([0; 4]);
    t.extend(software);
    std::fs::write(&tif, t)?;
    std::fs::write(
        dir.path().join("a.tif.xmp"),
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
        <rdf:Description rdf:about="" xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/"
          xmlns:Iptc4xmpCore="http://iptc.org/std/Iptc4xmpCore/1.0/xmlns/"
          photoshop:Headline="Probe Headline" photoshop:City="Probe City"
          photoshop:State="Probe State" photoshop:Country="Probe Country"
          Iptc4xmpCore:Location="Probe Place"/>
        </rdf:RDF></x:xmpmeta>"#,
    )?;
    let read = name_read::read_path(&tif, false).unwrap();
    let mut fields = NameFields::default();
    name_read::fill(&mut fields, read, false);
    let template = crate::export_settings::TokenTemplate::parse(
        "{{com.adobe.software}}|{{com.adobe.headline}}|{{com.adobe.city}}|{{com.adobe.state}}|{{com.adobe.country}}|{{com.adobe.location}}",
    );
    let at = NameContext {
        index: 0,
        total: 1,
        fields: &fields,
    };
    assert_eq!(
        template.stem(&tif, &at, "", 1).0,
        "ProbeSoft 1.2-Probe Headline-Probe City-Probe State-Probe Country-Probe Place"
    );
    let bare = dir.path().join("b.txt");
    std::fs::write(&bare, "not a photo")?;
    let read = name_read::read_path(&bare, false).unwrap();
    assert_eq!(read.facts, Default::default());
    Ok(())
}

/// A title only the photo's sidecar has names its export: the Example waits
/// for the file, then has it.
#[test]
fn a_title_only_the_file_has_names_the_export() -> anyhow::Result<()> {
    let (dir, mut e, ids, ctx) = editor()?;
    let source = e
        .library
        .as_ref()
        .unwrap()
        .photo(ids[0])
        .unwrap()
        .path
        .clone();
    std::fs::write(
        source.with_extension("xmp"),
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
        <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/" dc:title="Harbour"/>
        </rdf:RDF></x:xmpmeta>"#,
    )?;
    e.library.as_mut().unwrap().select(Some(ids[0]));
    let mut s = settings(&dir.path().join("out"));
    s.choose_tokens("{{com.adobe.title}}");
    open_dialog(&mut e, s);
    frame(&mut e, &ctx, vec![]);
    assert!(matches!(e.exports.read.status(), Status::Reading { .. }));
    until(&mut e, &ctx, |e| e.exports.read.status() == Status::Ready);
    let photo = &e.exports.scope.photos[0];
    let at = NameContext {
        index: 0,
        total: 1,
        fields: &photo.fields,
    };
    assert_eq!(
        e.exports.draft.file_name_for(&photo.source, &at).0,
        "Harbour.tif"
    );
    Ok(())
}

/// Return belongs to the Filename Template Editor while it is open over the
/// dialog: it is Done, keeping the template, and nothing is exported. With a
/// mistake in the template it does nothing, and never types a line break.
#[test]
fn return_in_the_template_editor_is_done_and_does_not_export() -> anyhow::Result<()> {
    let (dir, mut e, ids, ctx) = editor()?;
    e.library.as_mut().unwrap().select(Some(ids[0]));
    open_dialog(&mut e, settings(&dir.path().join("out")));
    let mut wrong = name_editor::NameEditor::new(MODEL, Preset::Custom);
    wrong.text.push('{');
    let typed = wrong.text.clone();
    e.exports.name_editor = Some(wrong);
    // Typing in the text, as when Return is pressed in the app.
    frame(&mut e, &ctx, vec![]);
    ctx.memory_mut(|m| m.request_focus(name_editor::text_id()));
    frame(&mut e, &ctx, vec![]);
    assert!(ctx.memory(|m| m.has_focus(name_editor::text_id())));
    press_return(&mut e, &ctx);
    assert_eq!(
        e.exports.name_editor.as_ref().map(|n| &n.text),
        Some(&typed)
    );
    assert_eq!(e.exports.draft.custom_tokens(), None);
    e.exports.name_editor = Some(name_editor::NameEditor::new(MODEL, Preset::Custom));
    press_return(&mut e, &ctx);
    assert!(e.exports.name_editor.is_none());
    assert_eq!(e.exports.draft.custom_tokens(), Some(MODEL));
    assert!(e.exports.dialog && e.exports.queued.is_empty());
    Ok(())
}

/// A line break never gets into the template: pasted or typed, it is
/// dropped before the text takes it, so what is typed next lands at the
/// cursor.
#[test]
fn line_breaks_never_get_into_the_template() -> anyhow::Result<()> {
    let (dir, mut e, ids, ctx) = editor()?;
    e.library.as_mut().unwrap().select(Some(ids[0]));
    open_dialog(&mut e, settings(&dir.path().join("out")));
    let mut typing = name_editor::NameEditor::new("abcd", Preset::Custom);
    typing.text = "abcd".into();
    e.exports.name_editor = Some(typing);
    frame(&mut e, &ctx, vec![]);
    ctx.memory_mut(|m| m.request_focus(name_editor::text_id()));
    frame(&mut e, &ctx, vec![]);
    let mut state = egui::TextEdit::load_state(&ctx, name_editor::text_id()).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::one(
            egui::text::CCursor::new(2),
        )));
    state.store(&ctx, name_editor::text_id());
    frame(&mut e, &ctx, vec![egui::Event::Paste("X\r\nY\n".into())]);
    frame(&mut e, &ctx, vec![egui::Event::Text("Z".into())]);
    frame(&mut e, &ctx, vec![egui::Event::Text("V\nW".into())]);
    frame(&mut e, &ctx, vec![egui::Event::Text("U".into())]);
    assert_eq!(
        e.exports.name_editor.as_ref().map(|n| n.text.as_str()),
        Some("abXYZVWUcd")
    );
    // An input method's text, while composed and once committed.
    let mut state = egui::TextEdit::load_state(&ctx, name_editor::text_id()).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::one(
            egui::text::CCursor::new(2),
        )));
    state.store(&ctx, name_editor::text_id());
    let composed = |e: &Editor| e.exports.name_editor.as_ref().map(|n| n.text.clone());
    frame(
        &mut e,
        &ctx,
        vec![egui::Event::Ime(egui::ImeEvent::Preedit {
            text: "日\n本".into(),
            active_range_chars: Some(2..3),
        })],
    );
    assert_eq!(composed(&e).as_deref(), Some("ab日本XYZVWUcd"));
    frame(
        &mut e,
        &ctx,
        vec![egui::Event::Ime(egui::ImeEvent::Commit("日\r\n本".into()))],
    );
    assert_eq!(composed(&e).as_deref(), Some("ab日本XYZVWUcd"));
    Ok(())
}

/// Escape closes one window at a time: the Filename Template Editor first,
/// dropping its template, also while its text is being typed in, and then
/// the dialog.
#[test]
fn escape_closes_the_template_editor_then_the_dialog() -> anyhow::Result<()> {
    let (dir, mut e, ids, ctx) = editor()?;
    e.library.as_mut().unwrap().select(Some(ids[0]));
    open_dialog(&mut e, settings(&dir.path().join("out")));
    frame(&mut e, &ctx, vec![]);
    e.exports.name_editor = Some(name_editor::NameEditor::new(MODEL, Preset::Custom));
    frame(&mut e, &ctx, vec![]);
    ctx.memory_mut(|m| m.request_focus(name_editor::text_id()));
    frame(&mut e, &ctx, vec![]);
    assert!(ctx.memory(|m| m.has_focus(name_editor::text_id())));
    press(&mut e, &ctx, egui::Key::Escape);
    assert!(e.exports.name_editor.is_none());
    assert!(e.exports.dialog);
    assert_eq!(e.exports.draft.custom_tokens(), None);
    press(&mut e, &ctx, egui::Key::Escape);
    assert!(!e.exports.dialog);
    // Escape as the editor opens from the dialog never closes the dialog
    // under it.
    open_dialog(&mut e, settings(&dir.path().join("out")));
    frame(&mut e, &ctx, vec![]);
    e.exports.name_editor = Some(name_editor::NameEditor::new(MODEL, Preset::Custom));
    press(&mut e, &ctx, egui::Key::Escape);
    assert!(e.exports.dialog);
    press(&mut e, &ctx, egui::Key::Escape);
    assert!(e.exports.name_editor.is_none());
    assert!(e.exports.dialog && e.exports.queued.is_empty());
    Ok(())
}

/// A file that can't be read fails the read (Export says it goes on without
/// it); what the others gave is used.
#[test]
fn a_file_that_cant_be_read_fails_the_read_and_keeps_the_others() -> anyhow::Result<()> {
    let (dir, mut e, ids, ctx) = editor()?;
    e.exports
        .read
        .set_reader(Arc::new(|photo: &name_read::ToRead| {
            if photo.path.ends_with("b.dng") {
                return Err("b.dng can't be read".into());
            }
            Ok(FileFields {
                camera: crate::export_settings::CameraNames::Read {
                    make: None,
                    model: Some("X100".into()),
                },
                ..Default::default()
            })
        }));
    let library = e.library.as_mut().unwrap();
    library.select(Some(ids[0]));
    library.select_range_to(ids[1]);
    let mut s = settings(&dir.path().join("out"));
    s.choose_tokens(MODEL);
    open_dialog(&mut e, s);
    frame(&mut e, &ctx, vec![]);
    until(&mut e, &ctx, |e| {
        matches!(e.exports.read.status(), Status::Failed(_))
    });
    assert_eq!(
        e.exports.read.status(),
        Status::Failed("b.dng can't be read".into())
    );
    let fields: Vec<_> = e
        .exports
        .scope
        .photos
        .iter()
        .map(|p| p.fields.camera.clone())
        .collect();
    assert_eq!(
        fields,
        [
            crate::export_settings::CameraNames::Read {
                make: None,
                model: Some("X100".into())
            },
            crate::export_settings::CameraNames::NotRead,
        ]
    );
    press_return(&mut e, &ctx);
    wait(&mut e, &ctx);
    assert_eq!(listing(&dir.path().join("out")), ["-b.tif", "X100-a.tif"]);
    Ok(())
}

/// The editor's Preset menu: Save Current Settings as New Preset… asks a
/// name and saves the template under it, which Rename To then lists and
/// shows; Update Preset gives it the template as edited; Delete preset…
/// asks, deletes it, and leaves the template as it was, a preset of none.
/// What the dialog chose keeps its tokens whatever happens to the preset.
#[test]
fn templates_are_saved_updated_and_deleted_from_the_editor() -> anyhow::Result<()> {
    use crate::app::export::naming::presets::{Action, Library};
    use crate::export_settings::SavedTemplates;
    let (dir, mut e, ids, ctx) = editor()?;
    let file = dir.path().join("filename_templates.json");
    e.library.as_mut().unwrap().select(Some(ids[0]));
    open_dialog(&mut e, settings(&dir.path().join("out")));
    e.exports.saved = Library(SavedTemplates::load_from(&file));
    e.exports.name_editor = Some(name_editor::NameEditor::new(MODEL, Preset::Custom));
    frame(&mut e, &ctx, vec![]);
    frame(&mut e, &ctx, vec![]);
    let act = |e: &mut Editor, action| {
        let editor = e.exports.name_editor.as_mut().unwrap();
        let tokens = editor.parsed().ok().map(|t| t.to_lightroom());
        editor
            .presets
            .act(action, tokens.as_deref(), &mut e.exports.saved);
    };
    act(&mut e, Action::New);
    frame(&mut e, &ctx, vec![]);
    frame(&mut e, &ctx, vec![]);
    // The name is typed over "Untitled Preset"; Return creates it, and
    // leaves the editor open.
    frame(&mut e, &ctx, vec![egui::Event::Text("Cameras".into())]);
    press_return(&mut e, &ctx);
    let saved = SavedTemplates::load_from(&file).map_err(anyhow::Error::msg)?;
    assert_eq!(
        saved.get("Cameras").map(|t| t.tokens).as_deref(),
        Some(MODEL)
    );
    let editor = e.exports.name_editor.as_ref().expect("still open");
    assert_eq!(editor.presets.preset(), &Preset::Saved("Cameras".into()));
    // Edited, then updated.
    e.exports
        .name_editor
        .as_mut()
        .unwrap()
        .text
        .push_str("-{Hour}");
    act(&mut e, Action::Update);
    let edited = format!("{MODEL}-{{{{date_Hour}}}}");
    let saved = SavedTemplates::load_from(&file).map_err(anyhow::Error::msg)?;
    assert_eq!(saved.get("Cameras").map(|t| t.tokens), Some(edited.clone()));
    // Done: Rename To shows it by its name.
    press_return(&mut e, &ctx);
    assert!(e.exports.name_editor.is_none());
    assert_eq!(e.exports.draft.custom_tokens(), Some(edited.as_str()));
    let shown = Preset::of(
        e.exports.draft.naming(),
        e.exports.draft.custom_tokens(),
        &e.exports.saved,
    );
    assert_eq!(shown, Preset::Saved("Cameras".into()));
    // Deleted from the editor opened on it: the dialog keeps the tokens.
    e.exports.name_editor = Some(name_editor::NameEditor::new(&edited, shown));
    frame(&mut e, &ctx, vec![]);
    frame(&mut e, &ctx, vec![]);
    act(&mut e, Action::Delete);
    frame(&mut e, &ctx, vec![]);
    frame(&mut e, &ctx, vec![]);
    press_return(&mut e, &ctx);
    let editor = e.exports.name_editor.as_ref().expect("still open");
    assert_eq!(editor.presets.preset(), &Preset::Custom);
    let saved = SavedTemplates::load_from(&file).map_err(anyhow::Error::msg)?;
    assert!(saved.list().is_empty());
    assert_eq!(e.exports.draft.custom_tokens(), Some(edited.as_str()));
    Ok(())
}

/// Escape drops the Preset menu's question first, saving nothing, then the
/// editor, then the dialog.
#[test]
fn escape_closes_the_presets_question_then_the_editor() -> anyhow::Result<()> {
    use crate::app::export::naming::presets::{Action, Library};
    use crate::export_settings::SavedTemplates;
    let (dir, mut e, ids, ctx) = editor()?;
    let file = dir.path().join("filename_templates.json");
    e.library.as_mut().unwrap().select(Some(ids[0]));
    open_dialog(&mut e, settings(&dir.path().join("out")));
    e.exports.saved = Library(SavedTemplates::load_from(&file));
    e.exports.name_editor = Some(name_editor::NameEditor::new(MODEL, Preset::Custom));
    frame(&mut e, &ctx, vec![]);
    frame(&mut e, &ctx, vec![]);
    let editor = e.exports.name_editor.as_mut().unwrap();
    editor
        .presets
        .act(Action::New, Some(MODEL), &mut e.exports.saved);
    frame(&mut e, &ctx, vec![]);
    frame(&mut e, &ctx, vec![]);
    press(&mut e, &ctx, egui::Key::Escape);
    let editor = e.exports.name_editor.as_ref().expect("the editor stays");
    assert!(!editor.presets.asking());
    assert!(!file.exists());
    press(&mut e, &ctx, egui::Key::Escape);
    assert!(e.exports.name_editor.is_none());
    assert!(e.exports.dialog);
    Ok(())
}

/// Export and the windows it opens, for `RAWMAKASE_UI_SHOTS`
/// ([`crate::app::ui::shots`]): the dialog as it opens, all its sections at
/// once with a TIFF's HDR rows and a watermark on, the watermark editor's
/// text options and the Filename Template Editor.
#[test]
fn shots_of_export() {
    fn draft(e: &mut Editor) {
        e.exports.draft = ExportSettings {
            destination: Destination::Folder,
            folder: Some("/Users/someone/Pictures/Exports".into()),
            format: Format::Tiff,
            watermark: true,
            ..Default::default()
        };
        e.exports.draft.choose_hdr(Some(Default::default()));
    }
    crate::app::ui::shots::shot(
        "export-dialog",
        egui::vec2(800., 760.),
        |e, _| {
            draft(e);
            e.exports.dialog = true;
        },
        |e, ctx| e.export_windows(ctx),
    );
    crate::app::ui::shots::shot(
        "export-sections",
        egui::vec2(700., 1190.),
        |e, _| draft(e),
        |e, ctx| {
            crate::app::ui::shots::panel(ctx, 668., |ui| {
                let fields = Default::default();
                let at = NameContext {
                    index: 0,
                    total: 1,
                    fields: &fields,
                };
                e.export_sections(ui, std::path::Path::new(""), &at, None);
            });
        },
    );
    // A JPEG's File Settings, the size limit off and on.
    for (name, limit_size) in [
        ("export-jpeg-quality", false),
        ("export-jpeg-size-limit", true),
    ] {
        crate::app::ui::shots::shot(
            name,
            egui::vec2(700., 1100.),
            |e, _| {
                e.exports.draft = ExportSettings {
                    limit_size,
                    ..Default::default()
                };
            },
            |e, ctx| {
                crate::app::ui::shots::panel(ctx, 668., |ui| {
                    let fields = Default::default();
                    let at = NameContext {
                        index: 0,
                        total: 1,
                        fields: &fields,
                    };
                    e.export_sections(ui, std::path::Path::new(""), &at, None);
                });
            },
        );
    }
    crate::app::ui::shots::shot(
        "export-watermark-editor",
        egui::vec2(900., 760.),
        |e, ctx| {
            draft(e);
            e.exports.dialog = true;
            e.exports.watermark_editor = Some(super::watermark_editor::WatermarkEditor::new(
                Default::default(),
                ctx,
            ));
        },
        |e, ctx| e.export_windows(ctx),
    );
    crate::app::ui::shots::shot(
        "export-filename-template-editor",
        egui::vec2(900., 760.),
        |e, _| {
            draft(e);
            e.exports.dialog = true;
            e.exports.name_editor = Some(name_editor::NameEditor::new(MODEL, Preset::Custom));
        },
        |e, ctx| e.export_windows(ctx),
    );
    // Send To: a destination chosen, one that can't take files now, and one
    // whose extension is gone.
    let offered = [
        crate::app::extensions::Destination {
            key: "dev.jane.gallery/web".into(),
            name: "Web Gallery".into(),
            unavailable: None,
            options: Vec::new(),
        },
        crate::app::extensions::Destination {
            key: "dev.jane.lab/print".into(),
            name: "Print Lab".into(),
            unavailable: Some("Print Lab stopped working".into()),
            options: Vec::new(),
        },
    ];
    for (name, chosen) in [
        ("export-send-to", Some("dev.jane.gallery/web")),
        ("export-send-to-unavailable", Some("dev.jane.lab/print")),
        ("export-send-to-missing", Some("dev.jane.gone/web")),
        ("export-send-to-nowhere", None),
    ] {
        let mut send_to = chosen.map(str::to_string);
        crate::app::ui::shots::shot(
            name,
            egui::vec2(700., 120.),
            |_, _| {},
            |_, ctx| {
                crate::app::ui::shots::panel(ctx, 668., |ui| {
                    ui.horizontal(|ui| {
                        crate::app::ui::widgets::form_row(ui, "After Export", |ui| {
                            ui.label("Do Nothing");
                        });
                    });
                    super::dialog::send_to_row(ui, &mut send_to, &offered);
                });
            },
        );
    }
}

/// The Send To row with a web gallery's options under it, as the Export
/// dialog draws them, for `RAWMAKASE_UI_SHOTS`.
#[test]
fn shots_of_send_to_options() {
    let manifest = rawmakase_extensions::Manifest::parse(
        r#"
[extension]
id = "dev.jane.gallery"
name = "Gallery"
version = "1.0.0"
rawmakase = ">=0.1"
api = 4
[program]
path = "bin/gallery"
[[export-destination]]
id = "web"
name = "Web Gallery"
[[export-destination.option]]
id = "title"
name = "Title"
help = "Empty: the folder's name"
control = "text"
default = ""
[[export-destination.option]]
id = "author"
name = "Author"
control = "text"
default = ""
[[export-destination.option]]
id = "layout"
name = "Layout"
control = { segmented = ["Grid", "Rows", "Masonry"] }
default = "Grid"
[[export-destination.option]]
id = "columns"
name = "Columns"
control = { number = [1, 6] }
default = 3
visible-when = { setting = "layout", equals = "Grid" }
[[export-destination.option]]
id = "theme"
name = "Theme"
control = { choice = ["Light", "Dark"] }
default = "Light"
[[export-destination.option]]
id = "captions"
name = "Captions"
control = "toggle"
default = true
"#,
    )
    .unwrap();
    let declared = &manifest.export_destinations[0];
    let destination = crate::app::extensions::Destination {
        key: "dev.jane.gallery/web".into(),
        name: declared.name.clone(),
        unavailable: None,
        options: declared.options.clone(),
    };
    let offered = [destination.clone()];
    crate::app::ui::shots::shot(
        "export-send-to-options",
        egui::vec2(700., 330.),
        |e, _| {
            e.exports.draft.send_to = Some(destination.key.clone());
            e.exports.draft.send_to_options.insert(
                destination.key.clone(),
                [("title".to_string(), serde_json::json!("Ola & Tomek"))].into(),
            );
        },
        |e, ctx| {
            crate::app::ui::shots::panel(ctx, 668., |ui| {
                super::dialog::send_to_row(ui, &mut e.exports.draft.send_to, &offered);
                e.send_to_options(ui, &destination);
            });
        },
    );
}

/// The test-only export destinations' program, when this binary is started
/// as it; a test that does nothing otherwise.
const DESTINATIONS: &str = "RAWMAKASE_EXAMPLE_DESTINATIONS";

#[test]
fn destinations_program() {
    if std::env::var_os(DESTINATIONS).is_none() {
        return;
    }
    let code = i32::from(rawmakase_example::destinations::extension().run().is_err());
    std::process::exit(code);
}

/// Loads the test-only `dev.rawmakase.destinations` extension from a
/// developer folder in `dir`, its program this test binary started again.
fn load_destinations(e: &mut Editor, dir: &std::path::Path) {
    use rawmakase_extensions::session::Launch;
    let folder = dir.join("destinations");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(
        folder.join(rawmakase_extensions::protocol::manifest::FILE),
        include_str!("../../../extensions/example/destinations/rawmakase-extension.toml"),
    )
    .unwrap();
    let data = dir.join("data");
    rawmakase_extensions::store::Store::new(&data)
        .add_developer(&folder)
        .unwrap();
    let program = Launch {
        program: std::env::current_exe().expect("the test binary"),
        args: [
            "app::export::tests::destinations_program",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ]
        .map(Into::into)
        .to_vec(),
        dir: std::path::PathBuf::new(),
        env: vec![(DESTINATIONS.into(), "1".into())],
        log: std::path::PathBuf::new(),
    };
    e.extensions = crate::app::extensions::Host::start_for_tests(
        &data,
        program,
        std::time::Duration::from_secs(30),
    );
}

/// Polls the extensions, and takes the editor's events, until the top bar
/// says something that starts with `start`; that line.
fn said(e: &mut Editor, ctx: &egui::Context, start: &str) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        e.events(ctx);
        let (outcomes, _) = e.extensions_poll(std::time::Instant::now());
        e.extension_outcomes(outcomes);
        if e.messages.line().starts_with(start) {
            return e.messages.line().to_string();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "waiting for {start:?}; the line says {:?}",
            e.messages.line()
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn send_to_sends_the_written_files_and_says_what_was_exported_then_sent() -> anyhow::Result<()> {
    let (dir, mut e, ids, ctx) = editor()?;
    load_destinations(&mut e, dir.path());
    let offered: Vec<_> = e
        .extensions
        .export_destinations()
        .into_iter()
        .map(|d| (d.key, d.name, d.unavailable, d.options.len()))
        .collect();
    assert_eq!(
        offered,
        [
            (
                "dev.rawmakase.destinations/index".into(),
                "Index File".into(),
                None,
                1
            ),
            (
                "dev.rawmakase.destinations/closed".into(),
                "Closed Lab".into(),
                None,
                0
            ),
        ]
    );
    let out = dir.path().join("out");
    let scope = e.export_scope_of(vec![ids[0], ids[2]]).expect("photos");
    let mut chosen = settings(&out);
    chosen.send_to = Some("dev.rawmakase.destinations/index".into());
    // The heading typed under Send To goes with the files.
    chosen.send_to_options.insert(
        "dev.rawmakase.destinations/index".into(),
        [("heading".to_string(), serde_json::json!("Our Wedding"))].into(),
    );
    export(&mut e, scope.clone(), chosen.clone())?;
    wait(&mut e, &ctx);
    let line = said(&mut e, &ctx, "Exported 2 of 2 · Sent");
    assert_eq!(line, "Exported 2 of 2 · Sent 2 to Index File");
    // The destination's message and where its result is, when opened.
    let index = out.join(rawmakase_example::destinations::INDEX);
    let detail = e.messages.entry_detail(&line).expect("the result");
    assert_eq!(detail, format!("Indexed out\n{}", index.display()));
    // In export order, each with its size as written.
    let text = std::fs::read_to_string(&index)?;
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "Our Wedding");
    let (w, h) = image::image_dimensions(out.join("c.tif"))?;
    assert!(lines[1].starts_with("a.tif "), "{text}");
    assert!(lines[1].ends_with(" unlabelled"), "{text}");
    assert!(lines[2].starts_with(&format!("c.tif {w} x {h} ")), "{text}");

    // A destination that fails leaves the export as it was, and says so.
    chosen.send_to = Some("dev.rawmakase.destinations/closed".into());
    chosen.existing = Existing::Unique;
    export(&mut e, scope, chosen)?;
    wait(&mut e, &ctx);
    let line = said(&mut e, &ctx, "Exported 2 of 2 · Closed Lab");
    assert_eq!(
        line,
        "Exported 2 of 2 · Closed Lab failed: the lab is closed today"
    );
    assert_eq!(
        listing(&out),
        ["a-2.tif", "a.tif", "c-2.tif", "c.tif", "index.txt"]
    );
    Ok(())
}

#[test]
fn send_to_a_destination_that_is_not_installed_still_exports() -> anyhow::Result<()> {
    let (dir, mut e, ids, ctx) = editor()?;
    let out = dir.path().join("out");
    let scope = e.export_scope_of(vec![ids[2]]).expect("a photo");
    let mut chosen = settings(&out);
    chosen.send_to = Some("dev.jane.gallery/web".into());
    export(&mut e, scope, chosen)?;
    wait(&mut e, &ctx);
    assert_eq!(listing(&out), ["c.tif"]);
    assert_eq!(
        e.messages.line(),
        "Exported c.tif · Not sent: dev.jane.gallery isn't installed"
    );
    Ok(())
}

/// A finished batch's outcomes, with the capture times its names were made
/// from.
fn queued_with(captured: Vec<Option<String>>) -> Queued {
    Queued {
        names: (0..captured.len()).map(|i| format!("{i}.dng")).collect(),
        left_out: Vec::new(),
        after: crate::export_settings::AfterExport::Nothing,
        editing: None,
        send_to: Some("dev.jane.gallery/web".into()),
        send_to_options: None,
        facts: captured
            .into_iter()
            .map(|captured| {
                Facts::of(&BatchPhoto {
                    source: "a.dng".into(),
                    name: "a.dng".into(),
                    edit: Edit::Catalog(Default::default()),
                    values: Values::default(),
                    naming: NameFields {
                        captured,
                        ..Default::default()
                    },
                    place: None,
                    flag: None,
                })
            })
            .collect(),
    }
}

#[test]
fn a_delivery_names_the_files_exported_in_order_in_their_deepest_common_folder() {
    let root = std::path::Path::new(if cfg!(windows) { "C:\\shoot" } else { "/shoot" });
    let exported = |path: std::path::PathBuf| Outcome::Exported {
        path,
        notes: Vec::new(),
        size: (300, 200),
    };
    let queued = queued_with(vec![
        Some("2026:05:04 10:21:33".into()),
        None,
        Some("2026-05-04 10:21:35.000".into()),
    ]);
    let outcomes = [
        exported(root.join("day 1/out/a.jpg")),
        Outcome::Failed("unreadable".into()),
        exported(root.join("day 2/out/c.jpg")),
    ];
    let deliver = delivery(&queued, &outcomes).expect("files to send");
    assert_eq!(deliver.folder, root);
    assert_eq!(deliver.label, "shoot");
    let names: Vec<_> = deliver.files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["a.jpg", "c.jpg"]);
    assert_eq!(
        deliver.files[0].captured.as_deref(),
        Some("2026-05-04T10:21:33")
    );
    assert_eq!(
        deliver.files[1].captured.as_deref(),
        Some("2026-05-04T10:21:35")
    );
    assert_eq!(
        (deliver.files[1].width, deliver.files[1].height),
        (Some(300), Some(200))
    );
    // One folder: its own name is the label.
    let one = delivery(&queued, &outcomes[..1]).unwrap();
    assert_eq!(
        (one.folder.as_path(), one.label.as_str()),
        (root.join("day 1/out").as_path(), "out")
    );
    // Nothing exported, nothing sent.
    assert!(delivery(&queued, &[Outcome::Cancelled]).is_none());
}

#[test]
fn a_delivery_tells_each_photos_label_rating_and_flag_in_export_order() {
    let root = std::path::Path::new(if cfg!(windows) { "C:\\out" } else { "/out" });
    let photo = |label: &str, rating: i32, flag: Option<i32>| BatchPhoto {
        source: "a.dng".into(),
        name: "a.dng".into(),
        edit: Edit::Catalog(Default::default()),
        values: Values {
            label: label.into(),
            rating,
            ..Default::default()
        },
        naming: NameFields::default(),
        place: None,
        flag,
    };
    let mut queued = queued_with(vec![None; 4]);
    queued.facts = [
        photo("Yellow", 4, Some(1)),
        photo("", 0, Some(-1)),
        // A label of another name, as Lightroom's custom label sets keep.
        photo("Approved", 2, Some(0)),
        // Outside the catalog: nothing of the catalog's.
        photo("", 0, None),
    ]
    .iter()
    .map(Facts::of)
    .collect();
    let outcomes: Vec<_> = ["a", "b", "c", "d"]
        .iter()
        .map(|n| Outcome::Exported {
            path: root.join(format!("{n}.jpg")),
            notes: Vec::new(),
            size: (10, 10),
        })
        .collect();
    let deliver = delivery(&queued, &outcomes).unwrap();
    let marks: Vec<_> = deliver
        .files
        .iter()
        .map(|f| (f.label.as_deref(), f.rating, f.flag.as_deref()))
        .collect();
    assert_eq!(
        marks,
        [
            (Some("yellow"), Some(4), Some("pick")),
            (None, Some(0), Some("reject")),
            (None, Some(2), None),
            (None, None, None),
        ]
    );
    let deliver = ExportDeliver {
        destination: "web".into(),
        ..deliver
    };
    deliver.validate().unwrap();
}

#[test]
fn a_capture_time_is_sent_in_one_form_or_not_at_all() {
    for (kept, sent) in [
        ("2026-05-04 10:21:33.000", Some("2026-05-04T10:21:33")),
        ("2026:05:04 10:21:33", Some("2026-05-04T10:21:33")),
        ("2026-04-24T18:43:24.000", Some("2026-04-24T18:43:24")),
        ("2026-05-04", None),
        ("May 4, 2026 10:21:33", None),
        ("", None),
    ] {
        assert_eq!(capture_time(kept).as_deref(), sent, "{kept}");
    }
}

#[test]
fn a_deliverys_line_says_what_was_exported_then_what_was_sent() {
    use crate::app::shell::messages::Level;
    let exported = "Exported 12 of 12";
    let sent = Ok(Delivered {
        sent: 12,
        message: Some("Published Wedding".into()),
        location: Some("https://example.com/wedding".into()),
    });
    assert_eq!(
        delivered_line("Web Gallery", exported, &sent),
        (
            Level::Done,
            "Exported 12 of 12 · Sent 12 to Web Gallery".into(),
            Some("Published Wedding\nhttps://example.com/wedding".into())
        )
    );
    let plain = Ok(Delivered {
        sent: 3,
        ..Default::default()
    });
    assert_eq!(delivered_line("Web Gallery", exported, &plain).2, None);
    let failed = Err(RemoteError::new(ErrorCode::Unavailable, "offline"));
    assert_eq!(
        delivered_line("Web Gallery", exported, &failed),
        (
            Level::Failure,
            "Exported 12 of 12 · Web Gallery failed: offline".into(),
            None
        )
    );
    let stopped = Err(RemoteError::new(ErrorCode::Cancelled, "stopped"));
    assert_eq!(
        delivered_line("Web Gallery", exported, &stopped).1,
        "Exported 12 of 12 · Sending to Web Gallery stopped"
    );
}

#[test]
fn stop_on_the_extensions_page_cancels_a_delivery() {
    let (dir, mut e, _, _) = editor().unwrap();
    load_destinations(&mut e, dir.path());
    let file = dir.path().join("a.jpg");
    let deliver = ExportDeliver {
        destination: String::new(),
        options: serde_json::Map::new(),
        folder: dir.path().to_path_buf(),
        label: "out".into(),
        files: vec![ExportedFile {
            name: "a.jpg".into(),
            path: file,
            width: None,
            height: None,
            captured: None,
            label: None,
            rating: None,
            flag: None,
        }],
    };
    let key = "dev.rawmakase.destinations/index";
    let name = e
        .extensions
        .deliver(key, deliver, None, "Exported a.jpg".into())
        .unwrap();
    assert_eq!(name, "Index File");
    let id = "dev.rawmakase.destinations";
    assert!(e.extensions.command_running(id));
    assert_eq!(
        e.extensions.status(),
        ["Example Destinations: Sending to Index File…"]
    );
    let outcomes = e.extensions.cancel_commands(id);
    assert!(
        matches!(
            &outcomes[..],
            [crate::app::extensions::Outcome::Delivered { result: Err(err), .. }]
                if err.code == ErrorCode::Cancelled
        ),
        "{outcomes:?}"
    );
    assert!(!e.extensions.command_running(id));
    e.extension_outcomes(outcomes);
    assert_eq!(
        e.messages.line(),
        "Exported a.jpg · Sending to Index File stopped"
    );
}

#[test]
fn a_destinations_options_are_their_defaults_until_chosen_and_while_they_fit() {
    use rawmakase_extensions::protocol::manifest::{Setting, SettingControl};
    let option = |id: &str, control: SettingControl, default: serde_json::Value| Setting {
        id: id.into(),
        name: None,
        help: None,
        control,
        default,
        step: None,
        unit: None,
        visible_when: None,
    };
    let options = [
        option("title", SettingControl::Text, "".into()),
        option(
            "layout",
            SettingControl::Choice(vec!["Grid".into(), "Rows".into()]),
            "Grid".into(),
        ),
        option("captions", SettingControl::Toggle, false.into()),
    ];
    let values = crate::app::extensions::option_values(&options, None);
    assert_eq!(
        serde_json::Value::Object(values),
        serde_json::json!({"title": "", "layout": "Grid", "captions": false})
    );
    // A value the extension's options no longer take is its default again,
    // and one for an option it no longer has is left out.
    let chosen = [
        ("title".to_string(), serde_json::json!("Wedding")),
        ("layout".to_string(), serde_json::json!("Masonry")),
        ("gone".to_string(), serde_json::json!(1)),
    ]
    .into();
    let values = crate::app::extensions::option_values(&options, Some(&chosen));
    assert_eq!(
        serde_json::Value::Object(values),
        serde_json::json!({"title": "Wedding", "layout": "Grid", "captions": false})
    );
    // The settings keep only what differs from the default.
    let mut settings = ExportSettings::default();
    let key = "dev.jane.gallery/web";
    super::dialog::choose_option(&mut settings, key, &options[2], true.into());
    assert_eq!(settings.send_to_options[key]["captions"], true);
    super::dialog::choose_option(&mut settings, key, &options[2], false.into());
    assert!(settings.send_to_options.is_empty());
}
