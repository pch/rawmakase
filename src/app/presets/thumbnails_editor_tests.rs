//! Editor tests of the Presets panel's Grid view: the panel drawn through its
//! frames, with the UI's clock given.
use crate::app::*;
use std::sync::Arc;

fn preset(id: &str, group: &str) -> crate::xmp::Preset {
    crate::xmp::Preset {
        id: id.into(),
        name: format!("Preset {id}"),
        group: group.into(),
        path: Default::default(),
        settings: Default::default(),
        curves: Default::default(),
        look: String::new(),
        blockers: Vec::new(),
        notes: Vec::new(),
        photo_settings: false,
        local: Default::default(),
        builtin: false,
    }
}

/// An editor with a decoded photo and a library of group A (4 presets) and group
/// B (40), whose tiles never render: the worker is stopped.
fn editor() -> (egui::Context, Editor) {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    ctx.all_styles_mut(|s| s.animation_time = 0.);
    let (mut editor, _) =
        crate::app::testing::editor_with_blue_photo(&ctx, Default::default(), true);
    let _ = editor.preset_thumbnails.stop();
    let mut presets: Vec<_> = (0..4).map(|i| preset(&format!("a{i}"), "A")).collect();
    presets.extend((0..40).map(|i| preset(&format!("b{i:02}"), "B")));
    editor.presets.library = Arc::new(crate::presets::Library {
        presets,
        errors: Vec::new(),
    });
    editor.refresh_preset_support();
    (ctx, editor)
}

/// One frame of the panel, 245 by 600 points, at `time` seconds, as the app
/// draws it: the tiles swept after.
fn frame(
    ctx: &egui::Context,
    editor: &mut Editor,
    time: f64,
    events: Vec<egui::Event>,
) -> egui::accesskit::TreeUpdate {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(245., 600.),
            )),
            time: Some(time),
            events,
            ..Default::default()
        },
        |ui| {
            ui.allocate_ui(egui::vec2(245., 600.), |ui| editor.presets_ui(ui));
        },
    );
    output.textures_delta.clear();
    editor.preset_thumbnails.sweep();
    output
        .platform_output
        .accesskit_update
        .expect("accessibility is on")
}

/// Clicks the control named `label`.
fn click(ctx: &egui::Context, editor: &mut Editor, time: f64, label: &str) {
    let tree = frame(ctx, editor, time, vec![]);
    let node = crate::app::testing::labelled(&tree, label)
        .unwrap_or_else(|| panic!("no {label}"))
        .clone();
    let [down, up] = crate::app::testing::press_and_release(crate::app::testing::centre(&node));
    frame(ctx, editor, time, down);
    frame(ctx, editor, time, up);
}

/// Only the tiles on screen, and a row either side, are asked for, in the order
/// shown; a collapsed group asks for none.
#[test]
fn only_the_tiles_on_screen_are_asked_for() {
    let (ctx, mut editor) = editor();
    assert_eq!(editor.presets.view, crate::app::presets::PresetView::Grid);
    frame(&ctx, &mut editor, 0., vec![]);
    assert!(
        editor.preset_thumbnails.wanted_ids().is_empty(),
        "both groups closed"
    );
    click(&ctx, &mut editor, 0.1, "B");
    frame(&ctx, &mut editor, 0.2, vec![]);
    let wanted = editor.preset_thumbnails.wanted_ids();
    assert!(wanted.iter().all(|id| id.starts_with('b')), "{wanted:?}");
    assert_eq!(wanted.first().map(String::as_str), Some("b00"));
    // A 600-point panel shows about four rows of two: never the whole group.
    assert!((6..=14).contains(&wanted.len()), "{wanted:?}");
    let mut sorted = wanted.clone();
    sorted.sort();
    assert_eq!(sorted, wanted, "in display order");
}

/// The tiles' layout: two columns of square tiles inside the panel, the left
/// edges and tops aligned, each tile named after its preset.
#[test]
fn tiles_sit_in_two_aligned_columns() {
    let (ctx, mut editor) = editor();
    frame(&ctx, &mut editor, 0., vec![]);
    click(&ctx, &mut editor, 0.1, "B");
    let tree = frame(&ctx, &mut editor, 0.2, vec![]);
    let bounds = |label: &str| {
        let b = crate::app::testing::labelled(&tree, label)
            .unwrap_or_else(|| panic!("{label}"))
            .bounds()
            .unwrap();
        (b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32)
    };
    let (l0, t0, r0, b0) = bounds("Preset b00");
    let (l1, t1, r1, b1) = bounds("Preset b01");
    let (l2, t2, _, _) = bounds("Preset b02");
    assert!(((r0 - l0) - (b0 - t0)).abs() < 0.5, "square");
    assert_eq!((t0, b0), (t1, b1), "a row shares its top and bottom");
    assert!(((r0 - l0) - (r1 - l1)).abs() < 0.5, "columns of one width");
    assert_eq!(l0, l2, "left edges line up down the grid");
    assert!(l1 > r0 && t2 > b0, "no overlap");
    assert!(r1 <= 245., "inside the panel");
}

/// A favorite toggled changes the list, not the tiles: they stay as they are.
#[test]
fn a_favorite_toggle_renders_nothing_again() {
    let (ctx, mut editor) = editor();
    frame(&ctx, &mut editor, 0., vec![]);
    let generation = editor.preset_thumbnails.generation();
    editor.presets.favorites.insert("b01".into());
    editor.presets.revision += 1;
    frame(&ctx, &mut editor, 0.1, vec![]);
    assert_eq!(editor.preset_thumbnails.generation(), generation);
    // An edit does.
    editor.document.edit.replace(Recipe {
        exposure: 1.,
        ..Default::default()
    });
    frame(&ctx, &mut editor, 0.2, vec![]);
    assert_eq!(editor.preset_thumbnails.generation(), generation + 1);
}

/// List view asks for no tiles, and the choice is kept for the next launch.
#[test]
fn list_view_asks_for_no_tiles_and_is_remembered() {
    let (ctx, mut editor) = editor();
    let dir = tempfile::tempdir().unwrap();
    let session = dir.path().join("session.json");
    editor.session.set_file(Some(session.clone()));
    frame(&ctx, &mut editor, 0., vec![]);
    click(&ctx, &mut editor, 0.1, "B");
    frame(&ctx, &mut editor, 0.2, vec![]);
    assert!(!editor.preset_thumbnails.wanted_ids().is_empty());
    click(&ctx, &mut editor, 0.3, "List");
    assert_eq!(editor.presets.view, crate::app::presets::PresetView::List);
    frame(&ctx, &mut editor, 0.4, vec![]);
    assert!(editor.preset_thumbnails.wanted_ids().is_empty());
    let saved: crate::app::preferences::session::Session =
        serde_json::from_slice(&std::fs::read(&session).unwrap()).unwrap();
    assert!(saved.preset_list);
    let relaunched = Editor::with_context(&ctx, None, saved, None);
    assert_eq!(
        relaunched.presets.view,
        crate::app::presets::PresetView::List
    );
    // Grid again, and the session says so by leaving the field out.
    click(&ctx, &mut editor, 0.5, "Grid");
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&session).unwrap()).unwrap();
    assert!(saved.get("preset_list").is_none());
}

/// A 600 × 400 scene to apply presets to: a sky graded to the horizon, a sun,
/// green and brown ground and a red roof.
fn scene() -> Arc<crate::camera_data::CameraImage> {
    let (width, height) = (600u32, 400u32);
    let mut image = (*crate::app::testing::blue_image()).clone();
    image.width = width;
    image.height = height;
    image.metadata.width = width;
    image.metadata.height = height;
    image.pixels = (0..width * height)
        .map(|i| {
            let (x, y) = (
                (i % width) as f32 / width as f32,
                (i / width) as f32 / height as f32,
            );
            let detail = 0.03 * ((x * 180.).sin() * (y * 140.).cos());
            if (x - 0.72).hypot(y - 0.25) < 0.06 {
                [1.4, 1.2, 0.7]
            } else if y < 0.55 {
                let t = y / 0.55;
                [0.12 + 0.3 * t, 0.22 + 0.25 * t, 0.55 + 0.05 * t]
            } else if (0.2..0.4).contains(&x) && (0.5..0.62).contains(&y) {
                [0.45 + detail, 0.08, 0.06]
            } else if x + y * 0.3 < 0.75 {
                [0.08 + detail, 0.22 + detail, 0.05]
            } else {
                [0.3 + detail, 0.2 + detail, 0.1]
            }
        })
        .collect();
    Arc::new(image)
}

/// The Presets panel in a `width`-point column, `height` points tall.
fn panel_of(e: &mut Editor, ctx: &egui::Context, width: f32, height: f32) {
    crate::app::ui::shots::panel(ctx, width, |ui| {
        ui.allocate_ui(egui::vec2(width, height), |ui| e.presets_ui(ui));
    });
}
fn panel(e: &mut Editor, ctx: &egui::Context, width: f32) {
    panel_of(e, ctx, width, 620.);
}

/// Opens the groups named, renders each shown preset's tile as the worker
/// does, except those `failed` names (which fail) and `missing` (left to come),
/// and stops the worker.
fn rendered(
    e: &mut Editor,
    ctx: &egui::Context,
    width: f32,
    open: &[(bool, &str)],
    failed: &[&str],
    missing: &[&str],
) {
    use crate::app::presets::thumbnails::{JobView, Render, Renderer};
    for (imported, group) in open {
        ctx.data_mut(|d| d.insert_temp(crate::app::presets::group_open_id(*imported, group), true));
    }
    // The shot panel's 8 pt margins, then the grid's own inset on each side.
    let inset = crate::app::ui::design::inset::ADJUSTMENT_CONTENT;
    let side = ((width - 2. * (8. + inset) - 4.) / 2.).floor();
    let edge = (side * 2.) as u32;
    let Some(image) = e.document.full().cloned() else {
        return;
    };
    let library = e.presets.library.clone();
    let mut renderer = Renderer::default();
    let job = JobView {
        image: &image,
        recipe: e.document.edit.recipe(),
        metadata: e.document.metadata.as_ref().unwrap(),
        profiles: &e.document.profiles,
        monitor: None,
        edge,
    };
    let tiles = (library.presets.iter())
        .filter(|p| open.iter().any(|(_, g)| *g == p.group))
        .filter(|p| !missing.contains(&p.id.as_str()))
        .map(|p| {
            let result = match failed.contains(&p.id.as_str()) {
                true => Err("The look's profile isn't installed".to_string()),
                false => renderer
                    .tile(&job, p, &std::sync::atomic::AtomicBool::new(false))
                    .map_err(|e| e.to_string()),
            };
            (p.id.clone(), result)
        })
        .collect();
    let monitor = e.view.display_profile();
    let inputs = crate::app::presets::thumbnail_inputs(&e.document, &library, monitor);
    e.preset_thumbnails
        .show_for_test(ctx, &inputs.unwrap(), edge, tiles);
}

fn builtin() -> crate::presets::Library {
    let (presets, errors) = crate::presets::builtin::presets();
    crate::presets::Library { presets, errors }
}

fn with_scene(e: &mut Editor) {
    let image = scene();
    e.module = Module::Develop;
    e.document.metadata = Some(image.metadata.clone());
    e.document.set_image(image);
    e.document.path = Some("scene.dng".into());
    e.presets.library = Arc::new(builtin());
    e.refresh_preset_support();
}

/// Long names, presets that don't fit this camera, a failed tile and tiles still
/// to come, in an imported group.
fn imported_looks(e: &mut Editor) {
    let mut presets = (*e.presets.library).clone();
    for (id, name) in [
        (
            "long1",
            "Kodachrome 64 Daylight, Pushed One Stop, Faded Shadows",
        ),
        ("long2", "Cinematic Teal and Orange (Strong)"),
        ("odd", "Made for Another Camera"),
        ("fail", "Classic Chrome Look"),
        ("wait1", "Coming Next"),
        ("wait2", "Coming After"),
    ] {
        let mut p = preset(id, "Imported Looks");
        p.name = name.into();
        presets.presets.push(p);
    }
    e.presets.library = Arc::new(presets);
    e.refresh_preset_support();
    let odd = e
        .presets
        .library
        .presets
        .iter()
        .position(|p| p.id == "odd")
        .unwrap();
    e.presets.issues[odd] = Some("The profile Fujifilm Velvia is for another camera".into());
}

#[test]
fn shots_of_preset_thumbnails() {
    use crate::app::ui::shots::shot;
    let size = egui::vec2(261., 640.);
    let basic = (false, "Color");
    shot(
        "presets-grid-typical",
        size,
        |e, ctx| {
            with_scene(e);
            let first = e
                .presets
                .library
                .presets
                .iter()
                .position(|p| p.group == basic.1)
                .unwrap();
            let _ = e.apply_preset(first);
            e.presets
                .favorites
                .insert(e.presets.library.presets[first + 1].id.clone());
            let groups = [basic, (false, "B&W")];
            rendered(e, ctx, 245., &groups, &[], &[]);
        },
        |e, ctx| panel(e, ctx, 245.),
    );
    shot(
        "presets-grid-long-names-failed-waiting",
        egui::vec2(261., 800.),
        |e, ctx| {
            with_scene(e);
            imported_looks(e);
            rendered(
                e,
                ctx,
                245.,
                &[(true, "Imported Looks")],
                &["fail"],
                &["wait1", "wait2"],
            );
        },
        |e, ctx| panel_of(e, ctx, 245., 780.),
    );
    shot(
        "presets-grid-collapsed",
        size,
        |e, ctx| {
            with_scene(e);
            rendered(e, ctx, 245., &[], &[], &[]);
        },
        |e, ctx| panel(e, ctx, 245.),
    );
    shot(
        "presets-grid-favorites",
        size,
        |e, ctx| {
            with_scene(e);
            let ids: Vec<String> = (e.presets.library.presets.iter())
                .filter(|p| p.group == basic.1)
                .take(3)
                .map(|p| p.id.clone())
                .collect();
            e.presets.favorites.extend(ids);
            e.presets.favorites_only = true;
            rendered(e, ctx, 245., &[basic], &[], &[]);
        },
        |e, ctx| panel(e, ctx, 245.),
    );
    shot(
        "presets-grid-no-photo",
        size,
        |e, ctx| {
            e.module = Module::Develop;
            e.presets.library = Arc::new(builtin());
            e.refresh_preset_support();
            ctx.data_mut(|d| {
                d.insert_temp(crate::app::presets::group_open_id(basic.0, basic.1), true)
            });
        },
        |e, ctx| panel(e, ctx, 245.),
    );
    shot(
        "presets-list",
        size,
        |e, ctx| {
            with_scene(e);
            e.presets.view = crate::app::presets::PresetView::List;
            rendered(e, ctx, 245., &[basic], &[], &[]);
        },
        |e, ctx| panel(e, ctx, 245.),
    );
    let narrow = crate::app::shell::workspace::PRESETS_MIN;
    shot(
        "presets-grid-narrow",
        egui::vec2(narrow + 16., 640.),
        |e, ctx| {
            with_scene(e);
            imported_looks(e);
            rendered(
                e,
                ctx,
                narrow,
                &[(true, "Imported Looks"), basic],
                &["fail"],
                &[],
            );
        },
        move |e, ctx| panel(e, ctx, narrow),
    );
}
