//! Editor tests of `point_color_panel`: the whole editor driven through its frames.
use crate::app::testing::*;
use crate::app::*;
use eframe::egui::{Pos2, Rect};

#[test]
fn a_swatch_added_while_visualize_range_is_on_is_visualized_at_once() {
    let ctx = egui::Context::default();
    let (mut editor, _) = editor_with_blue_photo(
        &ctx,
        crate::app::preferences::session::Session::default(),
        true,
    );
    editor.view.mixer_tab = crate::app::develop::state::MixerTab::PointColor;
    editor.view.point_color.visualize = true;
    editor
        .view
        .toggle(crate::app::develop::state::Tool::PointColor);
    let sampled = editor.document.edit.recipe().clone();
    editor.point_color_sample_ready(&sampled, Ok([2., 0.6, 0.3]));
    assert_eq!(editor.view.point_color.selected, Some(0));
    // The render scheduled with the swatch shows its range, with no other change.
    let pending = editor.preview.pending.recipe.as_ref().unwrap();
    assert_eq!(
        pending.point_colors[0].view,
        crate::model::point_color::SwatchView::VisualizeRange
    );
}

#[test]
fn point_colors_dropper_adds_a_selected_swatch_as_one_step_and_visualizes_it() {
    use crate::develop::point_color::SampleRefusal;
    let ctx = egui::Context::default();
    let (mut editor, _) = editor_with_blue_photo(
        &ctx,
        crate::app::preferences::session::Session::default(),
        true,
    );
    editor.view.mixer_tab = crate::app::develop::state::MixerTab::PointColor;
    editor
        .view
        .toggle(crate::app::develop::state::Tool::PointColor);
    assert!(editor.view.picks_color());
    // Sampled off the UI thread; a second click while it runs is ignored.
    let sample = |editor: &mut Editor| {
        editor.start_point_color_sample(0.7, 0.5);
        assert!(editor.document.point_color_pick.is_running());
        editor.start_point_color_sample(0.1, 0.5);
        let start = std::time::Instant::now();
        while editor.document.point_color_pick.is_running() {
            assert!(start.elapsed().as_secs() < 60, "sampling did not finish");
            std::thread::sleep(std::time::Duration::from_millis(5));
            editor.events(&ctx);
        }
    };
    sample(&mut editor);
    let (steps, applied) = editor.document.edit.history().steps();
    assert_eq!(applied, 1);
    assert_eq!(
        (steps[0].name.as_str(), steps[0].value.as_str()),
        ("Point Color", "Add Swatch")
    );
    let swatch = editor.document.edit.recipe().point_colors[0];
    // The photo is blue: a hue near 4 sixths of a turn, sampled with default ranges.
    assert!((swatch.source[0] - 4.).abs() < 0.5, "{swatch:?}");
    assert!(swatch.is_valid());
    assert_eq!(editor.view.point_color.selected, Some(0));
    assert_eq!(editor.view.tool, crate::app::develop::state::Tool::None);
    // The same color again is refused, and changes nothing.
    editor
        .view
        .toggle(crate::app::develop::state::Tool::PointColor);
    sample(&mut editor);
    assert_eq!(
        editor.messages.line(),
        SampleRefusal::AlreadySampled.message()
    );
    assert_eq!(editor.document.edit.recipe().point_colors.len(), 1);
    // A sample still being taken when the dropper is put away is dropped.
    assert_eq!(
        editor.view.tool,
        crate::app::develop::state::Tool::PointColor
    );
    in_edit_frame(&ctx, &mut editor, |e| {
        e.start_point_color_sample(0.2, 0.5);
        e.view.toggle(crate::app::develop::state::Tool::PointColor);
    });
    assert!(!editor.document.point_color_pick.is_running());
    std::thread::sleep(std::time::Duration::from_millis(200));
    editor.events(&ctx);
    assert_eq!(editor.document.edit.recipe().point_colors.len(), 1);
    editor
        .view
        .toggle(crate::app::develop::state::Tool::PointColor);
    // Nor when the Library or Before opens meanwhile: the sample stops at once.
    for leave in [
        (|e: &mut Editor| e.module = Module::Library) as fn(&mut Editor),
        |e: &mut Editor| e.view.compare = crate::app::develop::before_after::Compare::BeforeOnly,
    ] {
        in_edit_frame(&ctx, &mut editor, |e| e.start_point_color_sample(0.2, 0.5));
        leave(&mut editor);
        editor.events(&ctx);
        assert!(!editor.document.point_color_pick.is_running());
        std::thread::sleep(std::time::Duration::from_millis(200));
        editor.events(&ctx);
        assert_eq!(editor.document.edit.recipe().point_colors.len(), 1);
        editor.module = Module::Develop;
        editor.view.compare = crate::app::develop::before_after::Compare::Off;
        editor.view.tool = crate::app::develop::state::Tool::PointColor;
    }
    // A sample of a photo edited meanwhile is dropped.
    let mut changed = editor.document.edit.recipe().clone();
    changed.exposure = 1.;
    editor.point_color_sample_ready(&changed, Ok([2., 0.6, 0.3]));
    assert_eq!(editor.document.edit.recipe().point_colors.len(), 1);
    editor.view.tool = crate::app::develop::state::Tool::None;
    editor.view.mixer_tab = crate::app::develop::state::MixerTab::Mixer;
    // Visualize Range shows the selected swatch while the tab is open, on a color photo.
    assert_eq!(editor.visualized_swatch(), None);
    editor.view.mixer_tab = crate::app::develop::state::MixerTab::PointColor;
    editor.view.point_color.visualize = true;
    editor.view.point_color.ranges = true;
    assert_eq!(editor.visualized_swatch(), Some(0));
    // The whole panel draws, ranges open.
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 1600.))),
            ..Default::default()
        },
        |ui| editor.draw(ui),
    );
    output.textures_delta.clear();
    editor.document.edit.setup_mut().effects.monochrome = true;
    assert_eq!(editor.visualized_swatch(), None);
    editor.document.edit.setup_mut().effects.monochrome = false;
    // The preview's identity includes it, so a picker never takes it for the photo.
    editor.schedule();
    let pending = editor.preview.pending.recipe.as_ref().unwrap();
    assert_eq!(
        pending.point_colors[0].view,
        crate::model::point_color::SwatchView::VisualizeRange
    );
    assert_ne!(Some(pending), Some(&editor.effective_recipe()));
    // Not in Before, which shows the photo's defaults.
    editor.view.compare = crate::app::develop::before_after::Compare::BeforeOnly;
    assert_eq!(editor.visualized_swatch(), None);
    editor.view.compare = crate::app::develop::before_after::Compare::Off;
    // Not while an eyedropper is out, which samples the photo as it renders.
    editor
        .view
        .toggle(crate::app::develop::state::Tool::Defringe);
    assert_eq!(editor.visualized_swatch(), None);
    assert_eq!(editor.view.loupe_prompt(), "Pick a purple or green fringe");
    editor
        .view
        .toggle(crate::app::develop::state::Tool::PointColor);
    assert_eq!(editor.visualized_swatch(), None);
    assert_eq!(editor.view.loupe_prompt(), "Pick a color to adjust");
    // The dropper goes with the tab: on the Mixer tab a click adds no hidden swatch.
    in_edit_frame(&ctx, &mut editor, |e| {
        e.view.mixer_tab = crate::app::develop::state::MixerTab::Mixer
    });
    assert_eq!(editor.view.tool, crate::app::develop::state::Tool::None);
    editor.view.mixer_tab = crate::app::develop::state::MixerTab::PointColor;
    // Nor in the Library.
    editor.module = Module::Library;
    assert_eq!(editor.visualized_swatch(), None);
    editor.module = Module::Develop;
    // One History step, which Undo takes back.
    let mut recipe = editor.document.edit.recipe().clone();
    assert!(editor.document.edit.history_mut().undo(&mut recipe));
    assert!(recipe.point_colors.is_empty());
    assert_eq!(editor.visualized_swatch(), Some(0));
}

#[test]
fn a_swatch_added_while_color_mixer_is_off_turns_it_on() {
    use crate::model::panels::{Panel, PanelState};
    let ctx = egui::Context::default();
    let (mut editor, _) = editor_with_blue_photo(
        &ctx,
        crate::app::preferences::session::Session::default(),
        true,
    );
    editor
        .document
        .edit
        .setup_mut()
        .panels
        .set(Panel::ColorMixer, PanelState::Off);
    editor.view.mixer_tab = crate::app::develop::state::MixerTab::PointColor;
    editor
        .view
        .toggle(crate::app::develop::state::Tool::PointColor);
    editor.start_point_color_sample(0.7, 0.5);
    let start = std::time::Instant::now();
    while editor.document.point_color_pick.is_running() {
        assert!(start.elapsed().as_secs() < 60, "sampling did not finish");
        std::thread::sleep(std::time::Duration::from_millis(5));
        editor.events(&ctx);
    }
    assert_eq!(editor.document.edit.recipe().point_colors.len(), 1);
    // As a slider moved in a switched-off panel does, so the swatch shows.
    assert_eq!(
        editor
            .document
            .edit
            .recipe()
            .panels
            .state(Panel::ColorMixer),
        PanelState::On
    );
    editor.undo();
    assert_eq!(
        editor
            .document
            .edit
            .recipe()
            .panels
            .state(Panel::ColorMixer),
        PanelState::Off
    );
}
