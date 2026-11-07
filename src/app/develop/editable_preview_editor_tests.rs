//! Editing an offline photo from its editable preview, through the editor as
//! Develop, the arrow keys and automation reach it. The editable previews are
//! real files built from a small synthetic decode, recorded in the catalog as a
//! build records them; the originals are synthetic files taken away and put back.
use crate::app::shell::commands::{Command, Operation, PhotoTarget};
use crate::app::testing::editor_with_catalog;
use crate::app::worker::OpenedFrom;
use crate::app::{Editor, Module};
use crate::camera_data::{CameraImage, Metadata};
use crate::camera_profiles::fitted;
use crate::catalog::{BuiltEditablePreview, Catalog, PhotoId, editable_preview_file_name};
use crate::editable_preview;
use crate::storage::Identity;
use eframe::egui;
use rawmakase_cancel::Cancel;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The camera of the stand-in decode; without colour data when `bare`, so its
/// default is a Camera Look (fitted).
fn metadata(width: u32, height: u32, bare: bool) -> Metadata {
    let mut m = Metadata {
        make: "Testmaker".into(),
        model: "T-1".into(),
        width,
        height,
        wb: [2., 1., 1.5],
        ..Default::default()
    };
    if !bare {
        m.matrix_source = crate::camera_data::MatrixSource::File;
        m.cam_xyz = [[0.8, -0.2, -0.1], [-0.4, 1.2, 0.2], [-0.05, 0.2, 0.6]];
    }
    m
}

/// A fitted look for a bare camera, as a build stores it.
fn fitted_look(m: &Metadata) -> crate::camera_profiles::CameraProfile {
    let base = fitted::matrix_base(m, [[0.4, 0.4, 0.2], [0.2, 0.7, 0.1], [0., 0.1, 0.8]]).unwrap();
    fitted::with_look(&base, &fitted::FittedLook::identity()).unwrap()
}

struct Fx {
    _dir: tempfile::TempDir,
    ctx: egui::Context,
    e: Editor,
    ids: Vec<PhotoId>,
}

/// An editor with a catalog of `names`, each a copy of a synthetic DNG.
fn fixture(names: &[&str]) -> Fx {
    let (dir, mut e, ids) = editor_with_catalog(names).unwrap();
    // A real (synthetic) DNG, so a photo opens from its original too.
    let chart = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/charts/synthetic-d65.dng");
    for id in &ids {
        std::fs::copy(&chart, path_of(&e, *id)).unwrap();
    }
    let ctx = e.context.clone();
    e.module = Module::Develop;
    Fx {
        _dir: dir,
        ctx,
        e,
        ids,
    }
}
fn path_of(e: &Editor, id: PhotoId) -> PathBuf {
    e.library.as_ref().unwrap().photo(id).unwrap().path.clone()
}
fn catalog(e: &Editor) -> &Catalog {
    e.library.as_ref().unwrap().session.catalog()
}

impl Fx {
    fn id(&self, name: &str) -> PhotoId {
        *self
            .ids
            .iter()
            .find(|id| path_of(&self.e, **id).ends_with(name))
            .unwrap()
    }
    fn path(&self, id: PhotoId) -> PathBuf {
        path_of(&self.e, id)
    }
    /// Builds and records an editable preview of `id` from its file as it is
    /// now, as Build Editable Previews does; with a stored Camera Look for a
    /// `bare` camera. Returns the original's identity.
    fn build(&mut self, id: PhotoId, bare: bool) -> Identity {
        let path = self.path(id);
        let identity = Identity::read(&path).unwrap();
        let (w, h) = (48, 32);
        let image = CameraImage {
            width: w,
            height: h,
            recovered: Default::default(),
            pixels: (0..w * h).map(|i| [i as f32 / 900., 0.5, 0.25]).collect(),
            metadata: metadata(w, h, bare),
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        };
        // Reduced, so that it stands for a larger original as a real one does.
        let mut preview = editable_preview::build(
            &image,
            &identity,
            None,
            24,
            editable_preview::Precision::Lossless,
            Cancel::NEVER,
        )
        .unwrap();
        if bare {
            preview
                .set_camera_look(&fitted_look(&image.metadata))
                .unwrap();
        }
        let location = catalog(&self.e).location().clone();
        let folder = catalog(&self.e).editable_preview_folder();
        std::fs::create_dir_all(&folder).unwrap();
        let file = editable_preview_file_name(id, &identity);
        editable_preview::write(&folder.join(&file), &preview).unwrap();
        let header = editable_preview::read_header(&folder.join(&file)).unwrap();
        Catalog::open(&location)
            .unwrap()
            .record_editable_preview(
                id,
                &path,
                &BuiltEditablePreview {
                    file,
                    format_version: header.version,
                    original: header.identity,
                    original_size: (header.original[0], header.original[1]),
                    size: (header.preview[0], header.preview[1]),
                },
            )
            .unwrap();
        self.e.editable_previews_changed();
        identity
    }
    /// Takes `id`'s original away, keeping its bytes to put back.
    fn take_offline(&mut self, id: PhotoId) -> Vec<u8> {
        let path = self.path(id);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        bytes
    }
    /// Delivers events until the open photo's first image is in, or its load failed.
    fn wait_loaded(&mut self) {
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            self.e.events(&self.ctx);
            if (self.e.document.full().is_some() && !self.e.document.loading())
                || self.e.document.failed.is_some()
            {
                return;
            }
            assert!(Instant::now() < until, "the photo never loaded");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn open(&mut self, id: PhotoId) {
        self.e.develop_catalog_photo(id);
        assert_eq!(self.e.document.catalog_photo, Some(id), "not opened");
        self.wait_loaded();
    }
    /// The stored edit's exposure, read against `original`.
    fn saved_exposure<'a>(
        &self,
        id: PhotoId,
        original: impl Into<crate::edits::Original<'a>>,
    ) -> f32 {
        catalog(&self.e)
            .load_edit(id, original)
            .unwrap()
            .expect("saved")
            .recipe
            .exposure
    }
    fn stored_identity(&self, id: PhotoId) -> Identity {
        serde_json::from_str(&catalog(&self.e).edit_record(id).unwrap().identity.unwrap()).unwrap()
    }
    fn refusal(&self, id: PhotoId) -> Option<crate::app::library::Refusal> {
        let l = self.e.library.as_ref().unwrap();
        let photo = l.photo(id).unwrap();
        l.develop_from(photo, photo.path.is_file()).err()
    }
}

#[test]
fn an_offline_photo_opens_from_its_editable_preview_saves_and_loads_from_the_original_again() {
    let mut fx = fixture(&["a.dng"]);
    let a = fx.id("a.dng");
    let original = fx.build(a, false);
    let bytes = fx.take_offline(a);
    fx.open(a);
    let d = &fx.e.document;
    assert!(d.failed.is_none(), "{:?}", d.failed);
    assert!(d.opened_from_preview());
    assert_eq!(d.from.label(), "Editable Preview");
    assert_eq!(
        fx.e.opened_from().map(|(label, _)| label),
        Some("Editable Preview")
    );
    // The original's size and facts, its identity the one recorded.
    let image = d.full().unwrap().clone();
    assert!(image.fast);
    assert_eq!((image.metadata.width, image.metadata.height), (48, 32));
    assert!(image.photo_scale() < 1.);
    assert_eq!(d.file.as_ref(), Some(&original));
    // Final: no full-size stage follows to replace it.
    for _ in 0..50 {
        fx.e.events(&fx.ctx);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(Arc::ptr_eq(fx.e.document.full().unwrap(), &image));

    fx.e.document.edit.setup_mut().exposure = 0.6;
    fx.e.document.edit.save_state_mut().mark_changed();
    assert!(
        fx.e.flush(),
        "{:?}",
        fx.e.document.edit.save_state().message()
    );
    assert_eq!(fx.stored_identity(a), original);
    let path = fx.path(a);
    assert_eq!(
        fx.saved_exposure(a, crate::edits::Original::recorded(&path, &original)),
        0.6
    );

    // The same file back: the edit loads from it, and the next open uses it.
    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(fx.saved_exposure(a, &path), 0.6);
    fx.e.document.close();
    fx.open(a);
    assert_eq!(fx.e.document.from, OpenedFrom::Original);
    assert_eq!(fx.e.opened_from().map(|(label, _)| label), Some("Original"));
}

#[test]
fn a_replaced_original_protects_the_edit_made_offline() {
    let mut fx = fixture(&["a.dng"]);
    let a = fx.id("a.dng");
    fx.build(a, false);
    let mut bytes = fx.take_offline(a);
    fx.open(a);
    fx.e.document.edit.setup_mut().exposure = 0.6;
    fx.e.document.edit.save_state_mut().mark_changed();
    assert!(fx.e.flush());
    fx.e.document.close();
    // Another photo in its place (here a longer file), which still opens.
    let path = fx.path(a);
    bytes.extend_from_slice(b"another photo");
    std::fs::write(&path, &bytes).unwrap();
    assert!(catalog(&fx.e).load_edit(a, &path).is_err());
    fx.open(a);
    assert_eq!(fx.e.document.from, OpenedFrom::Original);
    assert!(fx.e.document.edit.save_state().is_protected());
}

#[test]
fn the_original_returning_while_open_keeps_the_preview_and_checks_saves_against_the_file() {
    let mut fx = fixture(&["a.dng"]);
    let a = fx.id("a.dng");
    let original = fx.build(a, false);
    let bytes = fx.take_offline(a);
    fx.open(a);
    let path = fx.path(a);
    // Back as it was: still edited from its preview, and saved against the file.
    std::fs::write(&path, &bytes).unwrap();
    fx.e.document.edit.setup_mut().exposure = 0.3;
    fx.e.document.edit.save_state_mut().mark_changed();
    assert!(fx.e.flush());
    assert!(fx.e.document.opened_from_preview());
    assert_eq!(fx.saved_exposure(a, &path), 0.3);
    assert!(fx.stored_identity(a).same_photo(&original));
    // Replaced while open: the file is read, and gets none of the edit.
    std::fs::write(&path, b"another photo entirely").unwrap();
    fx.e.document.edit.setup_mut().exposure = 0.9;
    fx.e.document.edit.save_state_mut().mark_changed();
    assert!(!fx.e.flush());
    assert!(fx.e.document.edit.save_state().needs_save());
    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(fx.saved_exposure(a, &path), 0.3);
}

#[test]
fn a_failed_offline_save_stays_pending_and_keeps_the_photo_open() {
    let mut fx = fixture(&["a.dng", "b.dng"]);
    let (a, b) = (fx.id("a.dng"), fx.id("b.dng"));
    let original = fx.build(a, false);
    fx.take_offline(a);
    fx.open(a);
    // Another window saves an edit for another version of the photo meanwhile:
    // this preview is no longer of the edit's photo, so its save is refused.
    let path = fx.path(a);
    let other = Identity {
        size: original.size + 1,
        ..original
    };
    Catalog::open(catalog(&fx.e).location())
        .unwrap()
        .save_edit(
            a,
            crate::edits::Original::recorded(&path, &other),
            &other,
            &Default::default(),
            &Default::default(),
            crate::catalog::HistoryUpdate::Keep,
        )
        .unwrap();
    fx.e.document.edit.setup_mut().exposure = 0.6;
    fx.e.document.edit.save_state_mut().mark_changed();
    // Autosave fails, and the edit stays pending.
    let until = Instant::now() + Duration::from_secs(10);
    while fx.e.document.edit.save_state().message().is_none() {
        fx.e.autosave(&fx.ctx);
        fx.e.events(&fx.ctx);
        assert!(Instant::now() < until, "the save never ended");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(fx.e.document.edit.save_state().needs_save());
    // Refused by the catalog's check of the stored edit, not for the missing
    // file: the save went through with the recorded identity.
    let why =
        fx.e.document
            .edit
            .save_state()
            .message()
            .unwrap()
            .to_string();
    assert!(why.contains("catalog edit protected"), "{why}");
    // Navigation saves first, fails again, and stays on the photo.
    fx.e.develop_catalog_photo(b);
    assert_eq!(fx.e.document.catalog_photo, Some(a));
    assert!(fx.e.document.edit.save_state().needs_save());
    assert_eq!(fx.e.document.edit.recipe().exposure, 0.6);
}

#[test]
fn a_preview_deleted_while_open_still_saves_and_then_is_refused_with_its_reason() {
    let mut fx = fixture(&["a.dng"]);
    let a = fx.id("a.dng");
    fx.build(a, false);
    fx.take_offline(a);
    fx.open(a);
    let file = fx.e.document.from.editable_preview().unwrap().file.clone();
    std::fs::remove_file(&file).unwrap();
    // The edit is the photo's, saved for the identity recorded at opening.
    fx.e.document.edit.setup_mut().exposure = 0.2;
    fx.e.document.edit.save_state_mut().mark_changed();
    assert!(fx.e.flush());
    fx.e.document.close();
    let refusal = fx.refusal(a).expect("refused");
    assert!(
        refusal.detail().contains("its file is missing"),
        "{}",
        refusal.detail()
    );
    assert_eq!(refusal.label(), "Offline");
    fx.e.develop_catalog_photo(a);
    assert_eq!(fx.e.document.catalog_photo, None);
}

#[test]
fn a_stale_or_damaged_preview_is_refused_with_its_reason() {
    let mut fx = fixture(&["a.dng", "b.dng"]);
    let (a, b) = (fx.id("a.dng"), fx.id("b.dng"));
    fx.build(a, false);
    fx.build(b, false);
    // Edited online for a later version of a, then taken offline.
    let path = fx.path(a);
    std::fs::write(&path, b"a later version of a").unwrap();
    let later = Identity::read(&path).unwrap();
    Catalog::open(catalog(&fx.e).location())
        .unwrap()
        .save_edit(
            a,
            &path,
            &later,
            &Default::default(),
            &Default::default(),
            crate::catalog::HistoryUpdate::Keep,
        )
        .unwrap();
    fx.take_offline(a);
    let refusal = fx.refusal(a).expect("refused");
    assert!(
        refusal.detail().contains("another version"),
        "{}",
        refusal.detail()
    );
    // A damaged file.
    fx.take_offline(b);
    let file = catalog(&fx.e).editable_preview(b).unwrap().unwrap().path;
    std::fs::write(&file, b"RMKEDPV\0damaged").unwrap();
    let refusal = fx.refusal(b).expect("refused");
    assert!(matches!(
        refusal,
        crate::app::library::Refusal::EditablePreview(_)
    ));
}

#[test]
fn ai_masks_ai_denoise_fit_camera_look_and_export_need_the_original() {
    let mut fx = fixture(&["a.dng"]);
    let a = fx.id("a.dng");
    fx.build(a, false);
    fx.take_offline(a);
    fx.open(a);
    assert_eq!(
        fx.e.selection_unavailable(),
        Some("Connect the original to use AI masks")
    );
    assert_eq!(
        fx.e.camera_look_unavailable(),
        Some("Connect the original to fit a Camera Look")
    );
    assert!(!fx.e.can_fit_camera_look());
    // AI Denoise on in the edit: nothing is asked of the service, and the photo
    // renders plain.
    fx.e.document.edit.setup_mut().ai_denoise.enabled = true;
    let image = fx.e.document.full().unwrap().clone();
    let geometry = crate::develop::Geometry::new(&image, fx.e.document.edit.recipe(), 0);
    fx.e.request_ai_denoise(&image, &geometry, None);
    assert!(fx.e.document.ai.status().is_none());
    assert!(
        fx.e.open_photo_unready()
            .is_some_and(|why| why.contains("editable preview"))
    );
}

#[test]
fn an_offline_photo_without_colour_data_opens_with_its_stored_camera_look() {
    let mut fx = fixture(&["a.dng"]);
    let a = fx.id("a.dng");
    fx.build(a, true);
    fx.take_offline(a);
    fx.open(a);
    let d = &fx.e.document;
    let stored = fitted_look(&metadata(48, 32, true));
    let fitted: Vec<_> = d.profiles.iter().filter(|p| fitted::is_fitted(p)).collect();
    assert_eq!(fitted.len(), 1);
    assert_eq!(fitted[0].as_ref(), &stored);
    // It is the photo's default, as online.
    let profile = d.edit.recipe().profile.as_deref().expect("a profile");
    assert_eq!(profile, &stored);
}

#[test]
fn stepping_and_automation_open_take_a_preview_and_pass_over_or_refuse_photos_without() {
    let mut fx = fixture(&["a.dng", "b.dng", "c.dng"]);
    let (a, b, c) = (fx.id("a.dng"), fx.id("b.dng"), fx.id("c.dng"));
    fx.build(c, false);
    fx.take_offline(b);
    fx.take_offline(c);
    fx.open(a);
    // b is offline without one: passed over; c is offline with one: stopped on.
    let order: Vec<PhotoId> = {
        let l = fx.e.library.as_ref().unwrap();
        let mut order = vec![a];
        while let Some(next) = l.navigate(*order.last().unwrap(), 1) {
            if order.contains(&next) {
                break;
            }
            order.push(next);
        }
        order
    };
    assert_eq!(order.len(), 3);
    let next = fx.e.develop_step(order[0], 1).unwrap();
    assert!(
        next != b,
        "stepped onto the photo without an editable preview"
    );
    // Automation: refused without one, opened with one.
    let ctx = fx.ctx.clone();
    let refused =
        fx.e.execute_command(Command::new(Operation::Open(PhotoTarget::Id(b))), &ctx)
            .expect_err("refused");
    assert_eq!(refused.code, "not_editable");
    fx.e.execute_command(Command::new(Operation::Open(PhotoTarget::Id(c))), &ctx)
        .unwrap();
    fx.wait_loaded();
    assert!(fx.e.document.opened_from_preview());
    assert_eq!(fx.e.command_state().opened_from, Some("editable_preview"));
    // What Quick Develop and the menu are told, offline.
    let l = fx.e.library.as_ref().unwrap();
    assert!(l.develop_from(l.photo(c).unwrap(), false).is_ok());
    assert_eq!(
        l.develop_from(l.photo(b).unwrap(), false).err(),
        Some(crate::app::library::Refusal::Offline)
    );
}

#[test]
fn the_library_filters_photos_with_an_editable_preview() {
    let mut fx = fixture(&["a.dng", "b.dng"]);
    let (a, b) = (fx.id("a.dng"), fx.id("b.dng"));
    fx.build(a, false);
    let l = fx.e.library.as_mut().unwrap();
    assert!(l.has_editable_preview(a) && !l.has_editable_preview(b));
    assert_eq!(l.editable_preview_count(), 1);
    l.show_only_editable_previews(true);
    assert_eq!(l.visible_ids(), vec![a]);
    l.show_only_editable_previews(false);
    assert_eq!(l.visible_ids().len(), 2);
}

#[test]
fn autosave_saves_an_offline_edit_against_the_recorded_identity() {
    let mut fx = fixture(&["a.dng"]);
    let a = fx.id("a.dng");
    let original = fx.build(a, false);
    fx.take_offline(a);
    fx.open(a);
    fx.e.document.edit.setup_mut().exposure = 0.45;
    fx.e.document.edit.save_state_mut().mark_changed();
    let until = Instant::now() + Duration::from_secs(10);
    while fx.e.document.edit.save_state().needs_save() {
        fx.e.autosave(&fx.ctx);
        fx.e.events(&fx.ctx);
        assert!(
            fx.e.document.edit.save_state().message().is_none(),
            "{:?}",
            fx.e.document.edit.save_state().message()
        );
        assert!(Instant::now() < until, "the save never ended");
        std::thread::sleep(Duration::from_millis(2));
    }
    let path = fx.path(a);
    assert_eq!(
        fx.saved_exposure(a, crate::edits::Original::recorded(&path, &original)),
        0.45
    );
    assert_eq!(fx.stored_identity(a), original);
}

#[test]
fn a_photo_moved_while_offline_keeps_saving_against_the_recorded_identity() {
    let mut fx = fixture(&["a.dng"]);
    let a = fx.id("a.dng");
    let original = fx.build(a, false);
    fx.take_offline(a);
    fx.open(a);
    // Its folder renamed, say: the original still not there at the new path.
    let moved = fx.path(a).with_file_name("moved.dng");
    fx.e.document.relink(moved);
    assert_eq!(fx.e.document.file.as_ref(), Some(&original));
    fx.e.document.edit.setup_mut().exposure = 0.25;
    fx.e.document.edit.save_state_mut().mark_changed();
    assert!(
        fx.e.flush(),
        "{:?}",
        fx.e.document.edit.save_state().message()
    );
    assert_eq!(fx.stored_identity(a), original);
}

impl Fx {
    /// Runs the automation request `request` (its JSON, as the control socket
    /// reads it) on the open photo.
    fn automate(
        &mut self,
        request: serde_json::Value,
    ) -> crate::app::shell::commands::Result<serde_json::Value> {
        let command = crate::app::shell::automation::command_of(request);
        let ctx = self.ctx.clone();
        self.e
            .execute_command(command, &ctx)
            .map(|outcome| serde_json::to_value(outcome).unwrap())
    }
    /// Waits for the output job `started` began; its last state.
    fn finished(&mut self, started: &serde_json::Value) -> serde_json::Value {
        let id = started["job_id"].as_u64().expect("a job");
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            let job = self
                .automate(serde_json::json!({"cmd": "job", "job_id": id}))
                .unwrap();
            if job["status"] != "running" {
                return job;
            }
            assert!(Instant::now() < until, "the job never finished");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// The pixel size of the JPEG at `path`.
fn jpeg_size(path: &std::path::Path) -> (u32, u32) {
    let image = image::ImageReader::open(path)
        .unwrap()
        .with_guessed_format()
        .unwrap()
        .decode()
        .unwrap();
    (image.width(), image.height())
}

/// Automation's `preview` of a photo open from its editable preview renders the
/// editable preview with the edit, says so, and never a decode of the original in
/// its place, even once the original is back; an edit with AI Denoise renders
/// plain, as Develop shows it.
#[test]
fn automation_previews_a_photo_open_from_its_editable_preview() {
    let mut fx = fixture(&["a.dng"]);
    let a = fx.id("a.dng");
    fx.build(a, false);
    let bytes = fx.take_offline(a);
    fx.open(a);
    assert!(fx.e.document.opened_from_preview());
    let preview_size = {
        let full = fx.e.document.full().unwrap();
        (full.width, full.height)
    };
    let mut ai = fx.e.document.edit.recipe().ai_denoise;
    ai.enabled = true;
    ai.amount = 1.;
    fx.e.document.edit.setup_mut().ai_denoise = ai;
    let out = fx._dir.path().join("offline.jpg");
    let started = fx
        .automate(serde_json::json!({"cmd": "preview", "path": out, "max_edge": 1600}))
        .expect("previewed");
    assert_eq!(started["opened_from"], "editable_preview");
    let job = fx.finished(&started);
    assert_eq!(job["status"], "completed", "{job}");
    assert_eq!(job["opened_from"], "editable_preview");
    // At most the editable preview's size: the original is far larger.
    assert_eq!(jpeg_size(&out), preview_size);
    // The original back, the photo still open from its editable preview.
    std::fs::write(fx.path(a), bytes).unwrap();
    let back = fx._dir.path().join("back.jpg");
    let started = fx
        .automate(serde_json::json!({"cmd": "preview", "path": back}))
        .expect("previewed");
    let job = fx.finished(&started);
    assert_eq!(job["status"], "completed", "{job}");
    assert_eq!(job["opened_from"], "editable_preview");
    assert_eq!(jpeg_size(&back), preview_size);
}

/// A `preview` of a photo open from its editable preview is a JPEG only: a TIFF,
/// PNG or JPEG XL path is refused and nothing is written, while a photo open from
/// its original is still previewed as TIFF.
#[test]
fn automation_previews_an_editable_preview_only_as_jpeg() {
    let mut fx = fixture(&["a.dng", "b.dng"]);
    let (a, b) = (fx.id("a.dng"), fx.id("b.dng"));
    fx.build(a, false);
    fx.take_offline(a);
    fx.open(a);
    for name in ["p.tif", "p.tiff", "p.png", "p.jxl", "p.TIF"] {
        let out = fx._dir.path().join(name);
        let refused = fx
            .automate(serde_json::json!({"cmd": "preview", "path": out, "max_edge": 2560}))
            .expect_err(name);
        assert_eq!(refused.code, "invalid_request", "{name}");
        assert!(
            refused.message.contains("only as JPEG"),
            "{}",
            refused.message
        );
        assert!(!out.exists(), "{name}");
    }
    let out = fx._dir.path().join("p.JPEG");
    let job = {
        let started = fx
            .automate(serde_json::json!({"cmd": "preview", "path": out}))
            .expect("a JPEG");
        fx.finished(&started)
    };
    assert_eq!(job["status"], "completed", "{job}");
    // A photo open from its original is previewed as TIFF as before.
    fx.open(b);
    assert!(!fx.e.document.opened_from_preview());
    let tiff = fx._dir.path().join("original.tif");
    let started = fx
        .automate(serde_json::json!({"cmd": "preview", "path": tiff}))
        .expect("a TIFF");
    assert_eq!(fx.finished(&started)["status"], "completed");
    assert!(tiff.is_file());
}

/// `export` stays refused for a photo open from its editable preview, offline or
/// with the original back (decision 5), and writes nothing.
#[test]
fn automation_export_is_refused_for_a_photo_open_from_its_editable_preview() {
    let mut fx = fixture(&["a.dng"]);
    let a = fx.id("a.dng");
    fx.build(a, false);
    let bytes = fx.take_offline(a);
    fx.open(a);
    for (back, name) in [(false, "offline"), (true, "back")] {
        if back {
            std::fs::write(fx.path(a), &bytes).unwrap();
        }
        for request in [
            serde_json::json!({"cmd": "export", "path": fx._dir.path().join(format!("{name}.jpg"))}),
            serde_json::json!({"cmd": "export", "path": fx._dir.path().join(format!("{name}.tif")), "max_edge": 64, "color_space": "prophoto"}),
        ] {
            let refused = fx.automate(request.clone()).expect_err("refused");
            assert_eq!(refused.code, "not_ready", "{request}");
            assert!(
                refused.message.contains("editable preview"),
                "{}",
                refused.message
            );
        }
        assert!(!fx._dir.path().join(format!("{name}.jpg")).exists());
        assert!(!fx._dir.path().join(format!("{name}.tif")).exists());
    }
}

/// Opened from its original, a photo's `preview` and `export` render from the
/// original, as before.
#[test]
fn automation_previews_and_exports_a_photo_open_from_its_original() {
    let mut fx = fixture(&["a.dng"]);
    let a = fx.id("a.dng");
    fx.build(a, false);
    fx.open(a);
    assert!(!fx.e.document.opened_from_preview());
    for cmd in ["preview", "export"] {
        let out = fx._dir.path().join(format!("{cmd}.jpg"));
        let started = fx
            .automate(serde_json::json!({"cmd": cmd, "path": out}))
            .expect(cmd);
        assert_eq!(started["opened_from"], "original");
        let job = fx.finished(&started);
        assert_eq!(job["status"], "completed", "{cmd}: {job}");
        assert!(out.is_file(), "{cmd}");
    }
}

/// A path that is there but can't be read (here a link to itself, which the
/// file system can't resolve) is never taken for an offline original.
#[cfg(unix)]
#[test]
fn an_original_that_is_there_but_unreadable_is_not_opened_from_its_preview() {
    let mut fx = fixture(&["a.dng"]);
    let a = fx.id("a.dng");
    fx.build(a, false);
    let path = fx.path(a);
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&path, &path).unwrap();
    assert!(!path.is_file());
    let refusal = fx.refusal(a).expect("refused");
    assert!(
        refusal.detail().contains("can't be read"),
        "{}",
        refusal.detail()
    );
}

#[test]
fn the_loupe_shows_an_unusable_preview_s_photo_as_before() {
    let mut fx = fixture(&["a.dng", "b.dng"]);
    let (a, b) = (fx.id("a.dng"), fx.id("b.dng"));
    fx.build(a, false);
    fx.build(b, false);
    fx.take_offline(a);
    fx.take_offline(b);
    let file = catalog(&fx.e).editable_preview(b).unwrap().unwrap().path;
    std::fs::remove_file(file).unwrap();
    fx.e.editable_previews_changed();
    let l = fx.e.library.as_mut().unwrap();
    // The scan of which files are there finds them gone.
    l.refresh().unwrap();
    l.wait_for_availability();
    l.select(Some(a));
    l.open_loupe();
    assert_eq!(l.loupe_develops(), Some(a));
    assert_eq!(l.loupe_offline_developed(), None);
    // A missing preview: the Loupe's own stored preview, as for any offline RAW.
    l.select(Some(b));
    assert_eq!(l.loupe_develops(), None);
    assert_eq!(l.loupe_offline_developed(), Some(b));
}

#[test]
fn undo_after_leaving_an_offline_photo_saves_against_its_recorded_identity() {
    let mut fx = fixture(&["a.dng", "b.dng"]);
    let (a, b) = (fx.id("a.dng"), fx.id("b.dng"));
    let original = fx.build(a, false);
    fx.take_offline(a);
    fx.open(a);
    let before = fx.e.document.edit.recipe().clone();
    fx.e.document.edit.setup_mut().exposure = 1.;
    fx.e.commit_edit(before, None);
    assert!(fx.e.flush());
    fx.e.sync_undo();
    fx.open(b);
    assert_eq!(fx.e.document.catalog_photo, Some(b));
    fx.e.undo();
    let path = fx.path(a);
    assert_eq!(
        fx.saved_exposure(a, crate::edits::Original::recorded(&path, &original)),
        0.
    );
    assert_eq!(fx.stored_identity(a), original);
}

#[test]
fn find_on_disk_saves_a_change_made_offline_rather_than_offer_to_discard_it() {
    let mut fx = fixture(&["a.dng"]);
    let a = fx.id("a.dng");
    let original = fx.build(a, false);
    fx.take_offline(a);
    fx.open(a);
    fx.e.document.edit.setup_mut().exposure = 0.35;
    fx.e.document.edit.save_state_mut().mark_changed();
    assert!(!fx.e.unsaved_on_missing_file());
    let path = fx.path(a);
    assert_eq!(
        fx.saved_exposure(a, crate::edits::Original::recorded(&path, &original)),
        0.35
    );
}

#[test]
fn a_synced_photo_that_can_t_be_opened_again_is_closed_not_left_stale() {
    let mut fx = fixture(&["a.dng"]);
    let a = fx.id("a.dng");
    fx.build(a, false);
    fx.open(a);
    assert_eq!(fx.e.document.from, OpenedFrom::Original);
    // A Sync wrote its edit; meanwhile its drive went away, and its editable
    // preview can't be used (deleted).
    fx.take_offline(a);
    let file = catalog(&fx.e).editable_preview(a).unwrap().unwrap().path;
    std::fs::remove_file(file).unwrap();
    fx.e.reopen_synced(&[a]);
    assert_eq!(fx.e.document.catalog_photo, None);
}

#[test]
fn the_loupe_asks_again_after_passing_through_an_online_photo() {
    let mut fx = fixture(&["a.dng", "b.dng"]);
    let (a, b) = (fx.id("a.dng"), fx.id("b.dng"));
    fx.build(a, false);
    fx.take_offline(a);
    let l = fx.e.library.as_mut().unwrap();
    l.refresh().unwrap();
    l.wait_for_availability();
    l.select(Some(a));
    l.open_loupe();
    assert_eq!(l.loupe_develops(), Some(a));
    // Past an online photo and back, with A's editable preview gone meanwhile.
    l.select(Some(b));
    assert_eq!(l.loupe_develops(), Some(b));
    let file = l
        .session
        .catalog()
        .editable_preview(a)
        .unwrap()
        .unwrap()
        .path;
    std::fs::remove_file(file).unwrap();
    l.select(Some(a));
    assert_eq!(l.loupe_develops(), None);
    assert_eq!(l.loupe_offline_developed(), Some(a));
}

impl Fx {
    /// Saves an edit of exposure `exposure` for `id`, against its file as it is.
    fn save_exposure(&self, id: PhotoId, exposure: f32) {
        let path = self.path(id);
        let recipe = crate::model::recipe::Recipe {
            exposure,
            ..Default::default()
        };
        Catalog::open(catalog(&self.e).location())
            .unwrap()
            .save_edit(
                id,
                &path,
                &Identity::read(&path).unwrap(),
                &recipe,
                &Default::default(),
                crate::catalog::HistoryUpdate::Keep,
            )
            .unwrap();
    }
    /// Syncs `source`'s saved settings onto `targets`, as Sync Settings from the
    /// Library does, and hands the result to the editor, which records it for
    /// Undo.
    fn sync(&mut self, source: PhotoId, targets: &[PhotoId]) {
        use crate::app::library::sync::{
            BatchChange, BatchProgress, Decoding, SyncSource, SyncTarget, synchronize,
        };
        let target = |id: PhotoId| SyncTarget {
            id,
            path: self.path(id),
            name: format!("{id:?}"),
        };
        let targets: Vec<_> = targets.iter().map(|id| target(*id)).collect();
        let result = synchronize(
            &mut Catalog::open(catalog(&self.e).location()).unwrap(),
            Some(&SyncSource::Photo(target(source))),
            &BatchChange::Settings(Default::default()),
            &targets,
            &Default::default(),
            &Decoding::default(),
            &BatchProgress::new("Sync", targets.len()),
        );
        assert!(result.failed.is_empty(), "{:?}", result.failed);
        assert_eq!(result.synced.len(), targets.len());
        self.e.synced(result);
    }
    /// The stored edit's exposure as the catalog holds it, whatever is at its
    /// path now.
    fn stored_exposure(&self, id: PhotoId) -> f32 {
        let record = catalog(&self.e).edit_record(id).unwrap();
        serde_json::from_str::<crate::model::recipe::Recipe>(&record.recipe.expect("saved"))
            .unwrap()
            .exposure
    }
}

#[test]
fn a_syncs_undo_and_redo_restore_an_offline_photo_against_its_recorded_identity() {
    let mut fx = fixture(&["a.dng", "b.dng"]);
    let (a, b) = (fx.id("a.dng"), fx.id("b.dng"));
    fx.save_exposure(a, 0.2);
    fx.save_exposure(b, 0.7);
    let original = fx.build(a, false);
    fx.sync(b, &[a]);
    assert_eq!(fx.saved_exposure(a, &fx.path(a)), 0.7);
    fx.take_offline(a);
    let path = fx.path(a);
    let recorded = crate::edits::Original::recorded(&path, &original);
    fx.e.undo();
    assert_eq!(
        fx.saved_exposure(a, recorded),
        0.2,
        "{}",
        fx.e.messages.line()
    );
    assert_eq!(fx.stored_identity(a), original);
    fx.e.redo();
    assert_eq!(
        fx.saved_exposure(a, recorded),
        0.7,
        "{}",
        fx.e.messages.line()
    );
    assert_eq!(fx.stored_identity(a), original);
}

#[test]
fn a_syncs_undo_refuses_every_photo_when_one_is_offline_without_a_usable_preview() {
    let mut fx = fixture(&["a.dng", "b.dng", "c.dng"]);
    let (a, b, c) = (fx.id("a.dng"), fx.id("b.dng"), fx.id("c.dng"));
    fx.save_exposure(a, 0.2);
    fx.save_exposure(b, 0.7);
    fx.save_exposure(c, 0.3);
    fx.sync(b, &[a, c]);
    let bytes = fx.take_offline(a);
    // One transaction: C, online, is not restored either.
    fx.e.undo();
    let line = fx.e.messages.line().to_owned();
    assert!(line.starts_with("Undo failed"), "{line}");
    assert!(line.contains("has no editable preview"), "{line}");
    assert_eq!(fx.stored_exposure(a), 0.7);
    assert_eq!(fx.saved_exposure(c, &fx.path(c)), 0.7);
    // The command stays: with the original back, unchanged, Undo works as before.
    std::fs::write(fx.path(a), &bytes).unwrap();
    fx.e.undo();
    assert_eq!(
        fx.saved_exposure(a, &fx.path(a)),
        0.2,
        "{}",
        fx.e.messages.line()
    );
    assert_eq!(fx.saved_exposure(c, &fx.path(c)), 0.3);
}

#[test]
fn a_syncs_undo_refuses_an_offline_photo_whose_preview_is_of_another_version() {
    // Its edit, and the Sync, were made for another version of the photo than
    // its editable preview, which was not built again.
    let mut fx = fixture(&["a.dng", "b.dng"]);
    let (a, b) = (fx.id("a.dng"), fx.id("b.dng"));
    fx.build(a, false);
    let path = fx.path(a);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.extend_from_slice(b"another version");
    std::fs::write(&path, &bytes).unwrap();
    fx.save_exposure(a, 0.2);
    fx.save_exposure(b, 0.7);
    fx.sync(b, &[a]);
    fx.take_offline(a);
    fx.e.undo();
    let line = fx.e.messages.line().to_owned();
    assert!(line.contains("of another version of the photo"), "{line}");
    assert_eq!(fx.stored_exposure(a), 0.7);
}

#[test]
fn a_syncs_redo_refuses_an_offline_photo_without_an_edit_whose_preview_is_of_another_version() {
    // With no edit, nothing in the catalog says the preview is stale: the
    // identity the Sync read does.
    let mut fx = fixture(&["a.dng", "b.dng"]);
    let (a, b) = (fx.id("a.dng"), fx.id("b.dng"));
    fx.build(a, false);
    let path = fx.path(a);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.extend_from_slice(b"another version");
    std::fs::write(&path, &bytes).unwrap();
    fx.save_exposure(b, 0.7);
    fx.sync(b, &[a]);
    // Undo clears an edit that wasn't there, which needs no file.
    fx.e.undo();
    assert!(catalog(&fx.e).edit_record(a).unwrap().recipe.is_none());
    fx.take_offline(a);
    fx.e.redo();
    let line = fx.e.messages.line().to_owned();
    assert!(line.contains("changed since the Sync"), "{line}");
    assert!(catalog(&fx.e).edit_record(a).unwrap().recipe.is_none());
}

#[test]
fn a_syncs_undo_protects_a_replaced_original() {
    let mut fx = fixture(&["a.dng", "b.dng"]);
    let (a, b) = (fx.id("a.dng"), fx.id("b.dng"));
    fx.save_exposure(a, 0.2);
    fx.save_exposure(b, 0.7);
    let original = fx.build(a, false);
    fx.sync(b, &[a]);
    // Another photo in its place: read, never stood in for by the preview.
    let mut bytes = fx.take_offline(a);
    bytes.extend_from_slice(b"another photo");
    std::fs::write(fx.path(a), &bytes).unwrap();
    fx.e.undo();
    assert!(fx.e.messages.line().starts_with("Undo failed"));
    assert_eq!(fx.stored_identity(a), original);
    assert_eq!(fx.stored_exposure(a), 0.7);
}

#[test]
fn a_syncs_undo_never_stands_in_for_an_original_that_is_there_but_cant_be_read() {
    let mut fx = fixture(&["a.dng", "b.dng"]);
    let (a, b) = (fx.id("a.dng"), fx.id("b.dng"));
    fx.save_exposure(a, 0.2);
    fx.save_exposure(b, 0.7);
    fx.build(a, false);
    fx.sync(b, &[a]);
    // Something is at its path that isn't its file.
    fx.take_offline(a);
    std::fs::create_dir(fx.path(a)).unwrap();
    let l = fx.e.library.as_ref().unwrap();
    assert!(l.stand_in(l.photo(a).unwrap()).is_err());
    fx.e.undo();
    assert!(fx.e.messages.line().starts_with("Undo failed"));
    std::fs::remove_dir(fx.path(a)).unwrap();
    assert_eq!(fx.stored_exposure(a), 0.7);
}

#[test]
fn a_syncs_undo_and_redo_reopen_the_photo_open_from_its_editable_preview() {
    let mut fx = fixture(&["a.dng", "b.dng"]);
    let (a, b) = (fx.id("a.dng"), fx.id("b.dng"));
    fx.save_exposure(a, 0.2);
    fx.save_exposure(b, 0.7);
    let original = fx.build(a, false);
    fx.sync(b, &[a]);
    fx.take_offline(a);
    fx.open(a);
    assert!(fx.e.document.opened_from_preview());
    assert_eq!(fx.e.document.edit.recipe().exposure, 0.7);
    // Opened again from its editable preview, with the edit Undo wrote.
    fx.e.undo();
    fx.wait_loaded();
    assert_eq!(fx.e.document.catalog_photo, Some(a));
    assert!(fx.e.document.opened_from_preview());
    assert_eq!(fx.e.document.edit.recipe().exposure, 0.2);
    assert!(!fx.e.document.edit.save_state().needs_save());
    assert_eq!(fx.stored_identity(a), original);
    // Its editable preview gone meanwhile: Redo writes nothing and the photo
    // stays open as it was; with the preview back, Redo works.
    let file = catalog(&fx.e).editable_preview(a).unwrap().unwrap().path;
    let kept = std::fs::read(&file).unwrap();
    std::fs::remove_file(&file).unwrap();
    fx.e.redo();
    let line = fx.e.messages.line().to_owned();
    assert!(line.starts_with("Redo failed"), "{line}");
    assert_eq!(
        fx.e.document.catalog_photo,
        Some(a),
        "nothing written, still open"
    );
    std::fs::write(&file, &kept).unwrap();
    fx.e.redo();
    fx.wait_loaded();
    assert_eq!(fx.stored_exposure(a), 0.7);
    assert_eq!(fx.e.document.edit.recipe().exposure, 0.7);
}
