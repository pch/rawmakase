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
    use crate::app::ui::widgets::{SLIDER_GAP, SLIDER_LABEL, SLIDER_ROW};
    let row = SLIDER_GAP + SLIDER_ROW + ctx.global_style().spacing.item_spacing.y;
    // The middle of a row's rail, under its label line.
    let rail = SLIDER_GAP + SLIDER_LABEL + (SLIDER_ROW - SLIDER_LABEL) / 2.;
    // Either end resets the whole range: Purple's high end, Green's low end.
    for i in [2, 4] {
        let at = Pos2::new(
            40.,
            ctx.global_style().spacing.window_margin.top as f32 + row * i as f32 + rail,
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
    e.view.set_tool(crate::app::develop::state::Tool::Crop);
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
        // A drop-down shows the choice it holds beside its name.
        .flat_map(|(_, n)| [n.label(), n.value()])
        .flatten()
        .map(String::from)
        .collect();
    let count = |labels: &[String], label: &str| labels.iter().filter(|l| *l == label).count();
    // Open: the menu lists the profile beside the box showing it.
    assert_eq!(count(&labels, "Embedded"), 2, "{labels:?}");
    for absent in [
        "Default (camera matrix)",
        "Import Adobe profiles for this camera",
        "Import profiles…",
        "Import profiles from folder…",
        "Import LUTs…",
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
        // A drop-down shows the choice it holds beside its name.
        .flat_map(|(_, n)| [n.label(), n.value()])
        .flatten()
        .map(String::from)
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
                pixels: vec![[0.1; 3]; 60000].into(),
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
            pixels: vec![[0.1; 3]; 60000].into(),
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

/// At the adjustments panel's narrowest, the Tone Curve's toolbar in Point mode
/// fits its column: the channel dots never pass the contents' edge, which keeps
/// them clear of the scroll bar.
#[test]
fn the_tone_curve_toolbar_fits_the_narrowest_panel() {
    let ctx = egui::Context::default();
    // The panel at its minimum, less its frame's margins on both sides.
    let width = crate::app::shell::workspace::ADJUSTMENTS_MIN - 2. * 8.;
    let mut view = crate::app::develop::state::ViewState {
        parametric_curve: false,
        ..Default::default()
    };
    let mut recipe = crate::model::recipe::Recipe::default();
    let mut step = crate::app::develop::step_name::StepName::default();
    let mut used = Rect::NOTHING;
    for _ in 0..2 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800., 1200.))),
                ..Default::default()
            },
            |ui| {
                // In the adjustments, whose sections have the wider inset.
                let _side = crate::app::ui::widgets::SectionSide::enter(
                    ui,
                    crate::app::ui::widgets::SectionGroup::DevelopRight,
                );
                let column = Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 1200.));
                used = ui
                    .scope_builder(egui::UiBuilder::new().max_rect(column), |ui| {
                        super::panels::tone_curve::show(
                            ui,
                            &mut step,
                            &mut recipe,
                            &mut view,
                            || [[0; 256]; 3],
                            None,
                            &[],
                        );
                    })
                    .response
                    .rect;
            },
        );
        output.textures_delta.clear();
    }
    assert!(
        used.right() <= width + 0.5,
        "the toolbar reaches {} in a {width} pt panel",
        used.right()
    );
}

/// The Crop drawer, its aspect and overlay drop-downs beside its buttons, for
/// `RAWMAKASE_UI_SHOTS` ([`crate::app::ui::shots`]).
#[test]
fn shots_of_the_crop_drawer() {
    crate::app::ui::shots::shot(
        "crop-drawer",
        Vec2::new(340., 330.),
        |e, _| e.view.set_tool(crate::app::develop::state::Tool::Crop),
        |e, ctx| {
            crate::app::ui::shots::panel(ctx, 320., |ui| {
                let frame = e.begin_edit_frame();
                e.tool_drawer(ui);
                e.finish_edit_frame(frame, ctx);
            });
        },
    );
}

/// A photo whose file carries the lens correction `lens`, for the Lens Corrections
/// shots.
fn lens_photo(lens: crate::optics::LensCorrection) -> Metadata {
    Metadata {
        make: "Samsung".into(),
        model: "SM-S911B".into(),
        lens_model: "Samsung Galaxy S23 Rear Main Camera".into(),
        lens: Some(lens),
        width: 4000,
        height: 3000,
        ..Default::default()
    }
}

/// Draws the Lens Corrections panel for photo `m`, with Enable Profile Corrections
/// `on` and the file's own correction `file`, as `<name>.png`
/// ([`crate::app::ui::shots`]).
fn lens_corrections_shot(name: &str, m: Metadata, on: bool, file: bool) {
    use crate::model::recipe::{ProfileCorrections, Recipe};
    let mut recipe = Recipe {
        lens_file_correction: file,
        ..Recipe::for_metadata(&m)
    };
    let state = if on {
        ProfileCorrections::On
    } else {
        ProfileCorrections::Off
    };
    recipe.set_profile_corrections(&m, state);
    let mut step = crate::app::develop::step_name::StepName::default();
    let mut view = crate::app::develop::state::ViewState::default();
    let lensfun = super::lensfun::View {
        db: None,
        running: None,
        progress: None,
        failure: None,
        suggest: false,
    };
    let width = 340.;
    crate::app::ui::shots::shot(
        name,
        Vec2::new(width + 20., if on { 720. } else { 640. }),
        |_, _| {},
        |_, ctx| {
            let palette = crate::app::ui::theme::palette(ctx);
            crate::app::ui::shots::panel(ctx, width, |ui| {
                let _side = crate::app::ui::widgets::SectionSide::enter(
                    ui,
                    crate::app::ui::widgets::SectionGroup::DevelopRight,
                );
                super::panels::lens_corrections::show(
                    ui,
                    &mut step,
                    &mut recipe,
                    &mut view,
                    Some(&m),
                    &lensfun,
                    &palette,
                );
            });
        },
    );
}

/// Lens Corrections for a phone DNG whose warp the file stores (#1289): the switch
/// for the file's own correction, on and off, with Enable Profile Corrections off
/// and on; a Nikon optional profile, which follows Enable Profile Corrections, gets
/// no switch and no note; Sony's lateral CA is only part of the file's data, and
/// with Enable Profile Corrections on its stand-in renders whatever the switch says,
/// so the switch is disabled.
#[test]
fn shots_of_lens_corrections_stored_in_the_file() {
    use crate::optics::{LensCorrection, Radial};
    let warp = |source: &str, default_on| LensCorrection {
        source: source.into(),
        default_on,
        distortion: Radial::new(vec![0., 1.], vec![1., 1.02]),
        ..Default::default()
    };
    let dng = lens_photo(warp(LensCorrection::DNG_OPCODES, true));
    for (name, on, file) in [
        ("lens-corrections-dng-off", false, true),
        ("lens-corrections-dng-on", true, true),
        ("lens-corrections-dng-file-off", false, false),
        ("lens-corrections-dng-file-off-profile-on", true, false),
    ] {
        lens_corrections_shot(name, dng.clone(), on, file);
    }
    let nikon = Metadata {
        make: "Nikon".into(),
        model: "Z 8".into(),
        lens_model: "NIKKOR Z 14-24mm f/2.8 S".into(),
        ..lens_photo(warp("Nikon built-in", false))
    };
    lens_corrections_shot(
        "lens-corrections-nikon-optional-off",
        nikon.clone(),
        false,
        true,
    );
    lens_corrections_shot("lens-corrections-nikon-optional-on", nikon, true, true);
    let sony = sony_photo();
    lens_corrections_shot("lens-corrections-sony-off", sony.clone(), false, true);
    lens_corrections_shot("lens-corrections-sony-file-off", sony.clone(), false, false);
    lens_corrections_shot("lens-corrections-sony-on", sony, true, true);
}

/// A Sony photo whose lateral CA always applies and whose fuller tables stand in
/// for a profile.
fn sony_photo() -> Metadata {
    use crate::optics::{LensCorrection, Radial};
    let ca = LensCorrection {
        source: LensCorrection::SONY_BUILTIN.into(),
        default_on: true,
        distortion: Radial::new(vec![0., 1.], vec![1., 1.02]),
        ..Default::default()
    };
    Metadata {
        make: "Sony".into(),
        model: "ILCE-7M3".into(),
        lens_model: "FE 55mm F1.8 ZA".into(),
        lens_stand_in: Some(LensCorrection {
            vignetting: Radial::new(vec![0., 1.], vec![1., 1.8]),
            ..ca.clone()
        }),
        ..lens_photo(ca)
    }
}

/// The switch for the file's own lens correction, as a click reaches it: shown for
/// a correction the file always applies and turning it off and on again; not shown
/// for one that follows Enable Profile Corrections; disabled while a stand-in renders
/// it anyway (#1289).
#[test]
fn the_files_lens_correction_switch_turns_it_off_and_on() {
    use crate::model::recipe::{ProfileCorrections, Recipe};
    use crate::optics::{LensCorrection, Radial};
    use egui_kittest::kittest::Queryable;
    const LABEL: &str = "Lens correction stored in the file";
    let harness = |m: Metadata, profile: bool| {
        let mut recipe = Recipe::for_metadata(&m);
        if profile {
            recipe.set_profile_corrections(&m, ProfileCorrections::On);
        }
        let mut step = crate::app::develop::step_name::StepName::default();
        let mut view = crate::app::develop::state::ViewState::default();
        let lensfun = super::lensfun::View {
            db: None,
            running: None,
            progress: None,
            failure: None,
            suggest: false,
        };
        egui_kittest::Harness::builder()
            .with_size(Vec2::new(360., 720.))
            .build_ui_state(
                move |ui, r: &mut Recipe| {
                    let palette = crate::app::ui::theme::palette(ui.ctx());
                    super::panels::lens_corrections::show(
                        ui,
                        &mut step,
                        r,
                        &mut view,
                        Some(&m),
                        &lensfun,
                        &palette,
                    );
                },
                recipe,
            )
    };
    let warp = |default_on| LensCorrection {
        source: LensCorrection::DNG_OPCODES.into(),
        default_on,
        distortion: Radial::new(vec![0., 1.], vec![1., 1.02]),
        ..Default::default()
    };
    let mut dng = harness(lens_photo(warp(true)), false);
    dng.run();
    assert!(dng.state().lens_file_correction);
    dng.get_by_label(LABEL).click();
    dng.run();
    assert!(!dng.state().lens_file_correction);
    dng.get_by_label(LABEL).click();
    dng.run();
    assert!(dng.state().lens_file_correction);
    let mut optional = harness(lens_photo(warp(false)), true);
    optional.run();
    assert!(optional.query_by_label(LABEL).is_none());
    let mut sony = harness(sony_photo(), true);
    sony.run();
    sony.get_by_label(LABEL).click();
    sony.run();
    assert!(
        sony.state().lens_file_correction,
        "disabled while the stand-in renders"
    );
}

/// The point curve's right-click menu, as Lightroom's: a curve copied from one
/// channel pastes into another as one named step, and Reset All Channels puts
/// every channel back to linear.
#[test]
fn the_point_curve_menu_copies_pastes_and_resets_channels() {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    ctx.all_styles_mut(|s| s.animation_time = 0.);
    let mut view = crate::app::develop::state::ViewState {
        parametric_curve: false,
        ..Default::default()
    };
    let mut recipe = crate::model::recipe::Recipe::default();
    recipe.curve.points = vec![[0., 0.], [0.5, 0.7], [1., 1.]];
    let mut step = crate::app::develop::step_name::StepName::default();
    let frame = |recipe: &mut crate::model::recipe::Recipe,
                 view: &mut crate::app::develop::state::ViewState,
                 step: &mut crate::app::develop::step_name::StepName,
                 events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400., 1200.))),
                events,
                ..Default::default()
            },
            |ui| {
                super::panels::tone_curve::show(
                    ui,
                    step,
                    recipe,
                    view,
                    || [[0; 256]; 3],
                    None,
                    &[],
                );
            },
        );
        output.textures_delta.clear();
        output.platform_output.accesskit_update.expect("on")
    };
    let node = |tree: &egui::accesskit::TreeUpdate, label: &str| {
        tree.nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some(label))
            .cloned()
            .unwrap_or_else(|| panic!("no {label}"))
    };
    let right_click = |at: Pos2| {
        [true, false].map(|pressed| {
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Secondary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        })
    };
    // Right-clicks the well's empty top-left corner and picks `item`.
    let choose = |recipe: &mut _, view: &mut _, step: &mut _, item: &str| {
        let tree = frame(recipe, view, step, vec![]);
        let well = node(&tree, "Point curve").bounds().expect("placed");
        let corner = Pos2::new(well.x0 as f32 + 20., well.y0 as f32 + 20.);
        for events in right_click(corner) {
            frame(recipe, view, step, events);
        }
        let menu = frame(recipe, view, step, vec![]);
        for events in press_and_release(centre(&node(&menu, item))) {
            frame(recipe, view, step, events);
        }
        frame(recipe, view, step, vec![]);
    };
    let points = recipe.curve.points.clone();
    choose(&mut recipe, &mut view, &mut step, "Copy RGB Curve");
    assert_eq!(recipe.curve.points, points, "copying changes nothing");
    assert!(step.given().is_none());
    view.selected_curve = 1;
    choose(&mut recipe, &mut view, &mut step, "Paste RGB Curve");
    assert_eq!(recipe.effects.channels[0], recipe.curve);
    assert_eq!(
        step.given(),
        Some(("Point Curve".into(), "Paste RGB into Red".into()))
    );
    choose(&mut recipe, &mut view, &mut step, "Reset Red Channel");
    assert_eq!(recipe.effects.channels[0], Default::default());
    assert_eq!(recipe.curve.points, points, "only the channel shown resets");
    choose(&mut recipe, &mut view, &mut step, "Reset All Channels");
    assert_eq!(recipe.curve, crate::color::curve::ToneCurve::default());
    assert_eq!(
        step.given(),
        Some(("Point Curve".into(), "Reset All Channels".into()))
    );
}

/// Right-clicking a point still removes it, without opening the menu.
#[test]
fn right_clicking_a_point_curve_point_removes_it_without_the_menu() {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let mut view = crate::app::develop::state::ViewState {
        parametric_curve: false,
        ..Default::default()
    };
    let mut recipe = crate::model::recipe::Recipe::default();
    recipe.curve.points = vec![[0., 0.], [0.5, 0.7], [1., 1.]];
    let mut step = crate::app::develop::step_name::StepName::default();
    let mut frame = |recipe: &mut crate::model::recipe::Recipe, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400., 1200.))),
                events,
                ..Default::default()
            },
            |ui| {
                super::panels::tone_curve::show(
                    ui,
                    &mut step,
                    recipe,
                    &mut view,
                    || [[0; 256]; 3],
                    None,
                    &[],
                );
            },
        );
        output.textures_delta.clear();
        output.platform_output.accesskit_update.expect("on")
    };
    let tree = frame(&mut recipe, vec![]);
    let well = tree
        .nodes
        .iter()
        .find(|(_, n)| n.label() == Some("Point curve"))
        .and_then(|(_, n)| n.bounds())
        .expect("the well");
    // The well is square, with the curve 5 points in from its edges.
    let inner = (well.x1 - well.x0) as f32 - 10.;
    let point = Pos2::new(
        well.x0 as f32 + 5. + 0.5 * inner,
        well.y1 as f32 - 5. - 0.7 * inner,
    );
    for pressed in [true, false] {
        frame(
            &mut recipe,
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Secondary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    let menu = frame(&mut recipe, vec![]);
    assert_eq!(recipe.curve.points.len(), 2);
    assert!(
        !menu
            .nodes
            .iter()
            .any(|(_, n)| n.label() == Some("Reset All Channels")),
        "no menu"
    );
}

/// The Point curve with a point selected, and its right-click menu with a Red
/// curve copied, for `RAWMAKASE_UI_SHOTS` ([`crate::app::ui::shots`]).
#[test]
fn shots_of_the_point_curve_and_its_menu() {
    use crate::app::ui::widgets::{CurveMenu, curve_menu_items};
    crate::app::ui::shots::shot(
        "point-curve",
        Vec2::new(320., 560.),
        |e, _| {
            e.view.parametric_curve = false;
            e.view.selected_curve = 2;
        },
        |e, ctx| {
            let mut recipe = crate::model::recipe::Recipe::default();
            recipe.effects.channels[1].points = vec![[0., 0.], [0.3, 0.22], [0.7, 0.8], [1., 1.]];
            crate::app::ui::shots::panel(ctx, 300., |ui| {
                super::panels::tone_curve::show(
                    ui,
                    &mut Default::default(),
                    &mut recipe,
                    &mut e.view,
                    || [[0; 256]; 3],
                    None,
                    &[],
                );
            });
        },
    );
    for (name, menu) in [
        (
            "point-curve-menu",
            CurveMenu {
                any_changed: true,
                copied: Some(1),
            },
        ),
        ("point-curve-menu-nothing-copied", CurveMenu::default()),
    ] {
        crate::app::ui::shots::shot(
            name,
            Vec2::new(220., 150.),
            |_, _| {},
            |_, ctx| {
                egui::Area::new(egui::Id::new("menu"))
                    .fixed_pos(Pos2::new(10., 10.))
                    .show(ctx, |ui| {
                        egui::Frame::menu(ui.style()).show(ui, |ui| {
                            ui.set_width(180.);
                            curve_menu_items(ui, 2, menu.any_changed, menu);
                        });
                    });
            },
        );
    }
}
