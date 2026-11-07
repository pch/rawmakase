//! Editor tests of `inspector`: the whole editor driven through its frames.
use crate::app::testing::*;
use crate::app::*;
use crate::camera_data::Metadata;
use eframe::egui::{Pos2, Rect, Vec2};

#[test]
fn double_clicking_a_defringe_hue_resets_it_to_its_colors_default() {
    let ctx = egui::Context::default();
    let mut effects = crate::model::effects::Effects {
        // Green below its default, as the fringe selector can leave it.
        defringe_ranges: [[0.1, 0.95], [0.0, 0.2]],
        ..Default::default()
    };
    let mut time = 0.;
    let mut step = crate::app::develop::step_name::StepName::default();
    let mut frame = |effects: &mut crate::model::effects::Effects,
                     events: Vec<egui::Event>,
                     wait: f64| {
        time += wait;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(320., 400.))),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| crate::app::develop::panels::inspector::defringe_sliders(ui, &mut step, effects),
        );
        output.textures_delta.clear();
    };
    // Six rows, Purple then Green: Amount, Hue (low end), Hue (high end).
    frame(&mut effects, vec![], 0.);
    let row = 24. + ctx.global_style().spacing.item_spacing.y;
    // Either end resets the whole range: Purple's high end, Green's low end.
    for i in [2, 4] {
        let at = Pos2::new(
            40.,
            ctx.global_style().spacing.window_margin.top as f32 + row * i as f32 + 12.,
        );
        let click = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        // Apart in time, so egui never counts a triple click.
        frame(&mut effects, vec![egui::Event::PointerMoved(at)], 1.);
        for pressed in [true, false, true, false] {
            frame(&mut effects, vec![click(pressed)], 0.05);
        }
    }
    assert_eq!(effects.defringe_ranges, [[0.3, 0.7], [0.4, 0.6]]);
    // History names the range reset, not the value the clamped end briefly took.
    assert_eq!(
        step.given(),
        Some(("Defringe Green Hue".to_string(), "40 / 60".to_string()))
    );
}

#[test]
fn the_crop_drawer_keeps_its_layout_whatever_its_buttons_show() {
    let ctx = egui::Context::default();
    crate::app::ui::icons::install(&ctx);
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.view.tool = crate::app::develop::state::Tool::Crop;
    let size = |e: &mut Editor, width: f32| {
        let mut rect = Rect::NOTHING;
        for _ in 0..2 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
                    ..Default::default()
                },
                |ui| {
                    ui.set_width(width);
                    let frame = e.begin_edit_frame();
                    rect = ui.scope(|ui| e.tool_drawer(ui)).response.rect;
                    e.finish_edit_frame(frame, &ctx);
                },
            );
            output.textures_delta.clear();
        }
        rect
    };
    let plain = size(&mut e, 320.);
    e.view.ruler = crate::app::develop::tools::crop_tool::Ruler::Armed;
    let _ = e.document.straighten.start();
    e.view.aspect = 0.8;
    e.view.crop_guides.guide = crate::app::develop::tools::crop_tool::Guide::GoldenSpiral;
    let busy = size(&mut e, 320.);
    assert_eq!(plain.size(), busy.size());
}

#[test]
fn the_basic_panel_offers_relative_white_balance_for_a_jpeg() {
    use egui::accesskit::Role;
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let m = Metadata {
        source: crate::camera_data::Source::Rendered,
        width: 64,
        height: 48,
        ..Default::default()
    };
    let (profiles, _) = crate::camera_profiles::installed(&m);
    editor.document.reset(None);
    let (generation, _) = editor.document.start_load();
    for event in [
        Event::Header(Box::new(worker::LoadedHeader {
            from: Default::default(),
            id: generation,
            file: None,
            path: "photo.jpg".into(),
            metadata: m.clone(),
            recipe: Recipe::for_rendered(&m),
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
    let nodes = || update.nodes.iter().map(|(_, node)| node);
    let combo = |value: &str| {
        nodes()
            .find(|n| n.role() == Role::ComboBox && n.value() == Some(value))
            .cloned()
            .unwrap_or_else(|| panic!("no {value} menu"))
    };
    let white_balance = combo("As Shot");
    let below = centre(&white_balance).y;
    // The first slider under the WB menu is relative Temp, then Tint: ±100 each.
    let mut numbers: Vec<_> = nodes()
        .filter(|n| n.role() == Role::SpinButton && centre(n).y > below)
        .collect();
    numbers.sort_by(|a, b| centre(a).y.total_cmp(&centre(b).y));
    for number in &numbers[..2] {
        assert_eq!(
            (number.min_numeric_value(), number.max_numeric_value()),
            (Some(-100.), Some(100.))
        );
    }
    // The profile is the file's own; nothing to import.
    let profile = combo("Embedded");
    let [down, up] = press_and_release(centre(&profile));
    controls_frame(&ctx, &mut editor, down);
    controls_frame(&ctx, &mut editor, up);
    let menu = controls_frame(&ctx, &mut editor, vec![]);
    let labels: Vec<String> = menu
        .nodes
        .iter()
        .filter_map(|(_, n)| n.label().or(n.value()).map(String::from))
        .collect();
    let count = |labels: &[String], label: &str| labels.iter().filter(|l| *l == label).count();
    // Open: the menu lists the profile beside the box showing it.
    assert_eq!(count(&labels, "Embedded"), 2, "{labels:?}");
    for absent in [
        "Default (camera matrix)",
        "Import Adobe profiles for this camera",
        "Import profiles…",
        "Import profiles from folder…",
        "Import .cube LUTs…",
    ] {
        assert_eq!(count(&labels, absent), 0, "{absent}: {labels:?}");
    }
    // Close it, then open the WB menu: no Kelvin presets.
    controls_frame(
        &ctx,
        &mut editor,
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    let [down, up] = press_and_release(centre(&white_balance));
    controls_frame(&ctx, &mut editor, down);
    controls_frame(&ctx, &mut editor, up);
    let menu = controls_frame(&ctx, &mut editor, vec![]);
    let labels: Vec<String> = menu
        .nodes
        .iter()
        .filter_map(|(_, n)| n.label().or(n.value()).map(String::from))
        .collect();
    // The box shows As Shot, and the menu offers it with Auto and Custom.
    assert_eq!(count(&labels, "As Shot"), 2, "{labels:?}");
    for offered in ["Auto", "Custom"] {
        assert_eq!(count(&labels, offered), 1, "{offered}: {labels:?}");
    }
    assert_eq!(count(&labels, "Daylight"), 0, "{labels:?}");
}

#[test]
fn the_crop_aspect_names_the_crops_ratio_beside_its_padlock() {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    crate::app::ui::icons::install(&ctx);
    let drawer = |crop_aspect: &str, crop: [f32; 4]| {
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session {
                crop_aspect: crop_aspect.into(),
                ..Default::default()
            },
            None,
        );
        e.document
            .set_image(std::sync::Arc::new(crate::camera_data::CameraImage {
                recovered: Default::default(),
                width: 300,
                height: 200,
                pixels: vec![[0.1; 3]; 60000],
                metadata: Metadata {
                    width: 300,
                    height: 200,
                    ..Default::default()
                },
                fast: false,
                scale_factor: 1.,
                scale_clipped: 0,
                hdr: Default::default(),
            }));
        e.document.edit.setup_mut().crop = crop;
        e.view.toggle(crate::app::develop::state::Tool::Crop);
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400., 800.))),
                ..Default::default()
            },
            |ui| {
                let frame = e.begin_edit_frame();
                e.tool_drawer(ui);
                e.finish_edit_frame(frame, &ctx);
            },
        );
        output.textures_delta.clear();
        output
            .platform_output
            .accesskit_update
            .expect("accessibility is on")
    };
    let custom = [0., 0., 0.75, 1.];
    // Free: Custom, as Lightroom shows it, beside an open padlock (measured).
    let free = drawer("free", custom);
    assert!(labelled(&free, "Custom").is_some());
    assert!(labelled(&free, "Aspect unlocked").is_some());
    assert!(labelled(&free, "Aspect locked").is_none());
    // Unlocked and uncropped, the photo's own ratio is named.
    let whole = drawer("free", [0., 0., 1., 1.]);
    assert!(labelled(&whole, "Original").is_some());
    assert!(labelled(&whole, "Aspect unlocked").is_some());
    // A ratio chosen locks the Custom crop as read.
    let locked = drawer("original", custom);
    assert!(labelled(&locked, "Custom").is_some());
    assert!(labelled(&locked, "Aspect locked").is_some());
}

#[test]
fn clicking_the_crop_padlock_locks_and_unlocks_the_aspect() {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    crate::app::ui::icons::install(&ctx);
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session {
            crop_aspect: "free".into(),
            ..Default::default()
        },
        None,
    );
    e.document
        .set_image(std::sync::Arc::new(crate::camera_data::CameraImage {
            recovered: Default::default(),
            width: 300,
            height: 200,
            pixels: vec![[0.1; 3]; 60000],
            metadata: Metadata {
                width: 300,
                height: 200,
                ..Default::default()
            },
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        }));
    e.document.edit.setup_mut().crop = [0., 0., 0.75, 1.];
    e.view.toggle(crate::app::develop::state::Tool::Crop);
    let frame = |e: &mut Editor, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400., 800.))),
                events,
                ..Default::default()
            },
            |ui| {
                let frame = e.begin_edit_frame();
                e.tool_drawer(ui);
                e.finish_edit_frame(frame, &ctx);
            },
        );
        output.textures_delta.clear();
        output
            .platform_output
            .accesskit_update
            .expect("accessibility is on")
    };
    let click = |e: &mut Editor, label: &str| {
        let tree = frame(e, vec![]);
        let at = centre(labelled(&tree, label).expect(label));
        let [down, up] = press_and_release(at);
        frame(e, down);
        frame(e, up);
        frame(e, vec![])
    };
    let tree = click(&mut e, "Aspect unlocked");
    assert!(labelled(&tree, "Aspect locked").is_some());
    assert!(e.crop_constraint().is_some());
    let tree = click(&mut e, "Aspect locked");
    assert!(labelled(&tree, "Aspect unlocked").is_some());
    assert_eq!(e.crop_constraint(), None);
}
