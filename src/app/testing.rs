//! What the app's tests share: editors with a catalog or a photo, frames run
//! through them, and the clicks and keys they send.
use crate::app::*;
use crate::camera_data::{CameraImage, Metadata};
use crate::catalog::PhotoId;
use eframe::egui::{Pos2, Rect};
use std::sync::Arc;

/// Delivers worker events until no catalog work is under way.
pub(in crate::app) fn settle_catalog(editor: &mut Editor, ctx: &egui::Context) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while editor.activity.is_busy() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
        editor.events(ctx);
    }
    assert!(
        !editor.activity.is_busy(),
        "the catalog work never finished"
    );
}

/// An editor with a catalog of `names`, its Library open.
pub(in crate::app) fn editor_with_catalog(
    names: &[&str],
) -> anyhow::Result<(tempfile::TempDir, Editor, Vec<PhotoId>)> {
    let d = tempfile::tempdir()?;
    let photos = d.path().join("photos");
    std::fs::create_dir(&photos)?;
    for name in names {
        std::fs::write(photos.join(name), b"fixture")?;
    }
    let path = d.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&path)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let library = crate::app::library::Library::load(&path, ctx)?;
    let ids = library.session.photos().iter().map(|p| p.id).collect();
    e.library = Some(Box::new(library));
    e.module = Module::Library;
    Ok((d, e, ids))
}

pub(in crate::app) fn press(e: &mut Editor, key: egui::Key, modifiers: egui::Modifiers) {
    let ctx = e.context.clone();
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
            // Pressed and released, as a real key is, so the next press of
            // the same key is not a repeat.
            events: [true, false]
                .map(|pressed| egui::Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed,
                    repeat: false,
                    modifiers,
                })
                .into(),
            ..Default::default()
        },
        |ui| e.draw(ui),
    );
    output.textures_delta.clear();
}

/// A small photo of colors spread along blue, with its metadata; decoded unless
/// `decoded` is false.
pub(in crate::app) fn editor_with_blue_photo(
    ctx: &egui::Context,
    session: crate::app::preferences::session::Session,
    decoded: bool,
) -> (Editor, Arc<CameraImage>) {
    let mut editor = Editor::with_context(ctx, None, session, None);
    editor.module = Module::Develop;
    let (width, height) = (32u32, 24u32);
    let metadata = Metadata {
        width,
        height,
        wb: [1.; 3],
        daylight_wb: [1.; 3],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        cam_xyz: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    editor.document.metadata = Some(metadata.clone());
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width,
        height,
        pixels: (0..width * height)
            .map(|i| {
                let v = 0.05 + 0.3 * (i % width) as f32 / width as f32;
                [v, v, 2. * v]
            })
            .collect(),
        metadata,
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    });
    if decoded {
        editor.document.set_image(image.clone());
    }
    (editor, image)
}

/// Runs `action` in an edit frame, as the panels and shortcuts do.
pub(in crate::app) fn in_edit_frame(
    ctx: &egui::Context,
    editor: &mut Editor,
    action: impl FnOnce(&mut Editor),
) {
    let frame = editor.begin_edit_frame();
    action(editor);
    editor.finish_edit_frame(frame, ctx);
}

/// A catalog of two photos, A and B, the latter rated 2, in an editor.
pub(in crate::app) fn two_photo_editor()
-> anyhow::Result<(tempfile::TempDir, egui::Context, Editor, [PhotoId; 2])> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    std::fs::write(photos.join("a.ARW"), b"identity fixture a")?;
    std::fs::write(photos.join("b.ARW"), b"identity fixture b")?;
    let catalog = dir.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&catalog)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let mut l = library::Library::load(&catalog, ctx.clone())?;
    let id = |name: &str, l: &library::Library| {
        l.session
            .photos()
            .iter()
            .find(|p| p.filename.ends_with(name))
            .unwrap()
            .id
    };
    let (a, b) = (id("a.ARW", &l), id("b.ARW", &l));
    l.edit_metadata(
        b,
        crate::app::library::photo_metadata::Edit::Rating(2),
        false,
    )?;
    l.take_done();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.library = Some(Box::new(l));
    editor.module = Module::Library;
    Ok((dir, ctx, editor, [a, b]))
}

pub(in crate::app) fn rating(editor: &Editor, id: PhotoId) -> i32 {
    editor.library.as_ref().unwrap().photo(id).unwrap().rating
}

/// A frame of the whole window, with a button holding the keyboard focus when
/// `focus` says so.
pub(in crate::app) fn panel_frame(e: &mut Editor, events: Vec<egui::Event>, focus: FocusedButton) {
    let ctx = e.context.clone();
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
            events,
            ..Default::default()
        },
        |ui| {
            if focus == FocusedButton::Yes {
                egui::Area::new(egui::Id::new("focus-holder"))
                    .show(ui.ctx(), |ui| ui.button("focused").request_focus());
            }
            e.draw(ui)
        },
    );
    output.textures_delta.clear();
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum FocusedButton {
    Yes,
    No,
}

pub(in crate::app) fn key_down(key: egui::Key, modifiers: egui::Modifiers) -> Vec<egui::Event> {
    vec![egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers,
    }]
}

/// The accessibility tree of `editor`'s Develop controls in a frame given `events`.
pub(in crate::app) fn controls_frame(
    ctx: &egui::Context,
    editor: &mut Editor,
    events: Vec<egui::Event>,
) -> egui::accesskit::TreeUpdate {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(320., 1600.))),
            events,
            ..Default::default()
        },
        |ui| {
            let frame = editor.begin_edit_frame();
            egui::CentralPanel::default().show(ui, |ui| editor.controls(ui));
            editor.finish_edit_frame(frame, ui.ctx());
        },
    );
    output.textures_delta.clear();
    output
        .platform_output
        .accesskit_update
        .expect("accessibility is on")
}

pub(in crate::app) fn press_and_release(at: Pos2) -> [Vec<egui::Event>; 2] {
    [true, false].map(|pressed| {
        vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    })
}

pub(in crate::app) fn centre(node: &egui::accesskit::Node) -> Pos2 {
    let b = node.bounds().expect("placed");
    Pos2::new(((b.x0 + b.x1) / 2.) as f32, ((b.y0 + b.y1) / 2.) as f32)
}

/// A Develop panel for a photo of `model`, with Adobe's installation scanned as
/// holding nothing, so the panel never depends on what this machine has installed.
pub(in crate::app) fn community_editor(ctx: &egui::Context, model: &str, rendered: bool) -> Editor {
    ctx.enable_accesskit();
    ctx.all_styles_mut(|s| s.animation_time = 0.);
    let mut editor = Editor::with_context(ctx, None, Default::default(), None);
    editor.document.metadata = Some(Metadata {
        make: "Fujifilm".into(),
        model: model.into(),
        source: if rendered {
            crate::camera_data::Source::Rendered
        } else {
            Default::default()
        },
        ..Default::default()
    });
    ctx.data_mut(|d| {
        d.insert_temp(
            egui::Id::new(("adobe-profiles", "Fujifilm", model)),
            Vec::<std::path::PathBuf>::new(),
        );
    });
    // Nor on the profile library's files.
    editor.assume_catalogue_profiles_available();
    editor
}

pub(in crate::app) fn labelled<'a>(
    tree: &'a egui::accesskit::TreeUpdate,
    label: &str,
) -> Option<&'a egui::accesskit::Node> {
    tree.nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.label() == Some(label) || n.value() == Some(label))
}

pub(in crate::app) fn click(
    ctx: &egui::Context,
    editor: &mut Editor,
    node: &egui::accesskit::Node,
) {
    let [down, up] = press_and_release(centre(node));
    controls_frame(ctx, editor, down);
    controls_frame(ctx, editor, up);
}
