//! Editor tests of `peek`: the whole editor driven through its frames.
use crate::app::testing::*;
use crate::app::ui::widgets::LABEL_CLICK;
use crate::app::*;
use crate::model::params::ParameterId;
use eframe::egui::{Pos2, Rect, Vec2};

struct Window {
    ctx: egui::Context,
    e: Editor,
    time: f64,
    at: Pos2,
}
impl Window {
    fn new(setup: impl FnOnce(&mut Editor)) -> Self {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        ctx.all_styles_mut(|s| s.animation_time = 0.);
        let (mut e, _) = editor_with_blue_photo(&ctx, Default::default(), true);
        e.module = Module::Develop;
        e.onboarding.visible = false;
        setup(&mut e);
        let mut w = Self {
            ctx,
            e,
            time: 0.,
            at: Pos2::ZERO,
        };
        w.wait(0.1);
        w.wait(0.1);
        w
    }
    /// A frame of the whole window, `after` seconds after the last.
    fn frame(&mut self, after: f64, events: Vec<egui::Event>) -> egui::accesskit::TreeUpdate {
        self.time += after;
        let mut output = self.ctx.clone().run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 4000.))),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |ui| self.e.draw(ui),
        );
        output.textures_delta.clear();
        output.platform_output.accesskit_update.unwrap()
    }
    fn wait(&mut self, after: f64) -> egui::accesskit::TreeUpdate {
        self.frame(after, vec![])
    }
    /// Points at the label of slider `name`.
    fn at_label(&mut self, name: &str) {
        let tree = self.wait(0.1);
        let node = tree
            .nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.role() == egui::accesskit::Role::SpinButton && n.label() == Some(name))
            .unwrap_or_else(|| panic!("no slider {name}"));
        self.at = centre(node) - Vec2::new(180., 0.);
    }
    fn button(&mut self, pressed: bool) {
        let [down, up] = press_and_release(self.at);
        self.frame(0.01, if pressed { down } else { up });
    }
    /// Presses the label and holds it too long for a click.
    fn hold(&mut self) {
        self.button(true);
        self.wait(LABEL_CLICK + 0.05);
    }
    fn exposure(&self) -> f32 {
        self.e.document.edit.recipe().exposure
    }
    fn shown(&self) -> Recipe {
        self.e.effective_recipe()
    }
    fn rendering(&self) -> f32 {
        self.e.preview.pending.recipe.as_ref().unwrap().exposure
    }
}
fn exposed() -> Window {
    let mut w = Window::new(|e| e.document.edit.setup_mut().exposure = 1.);
    w.at_label("Exposure");
    w
}

#[test]
fn holding_a_label_shows_the_default_until_released_and_edits_nothing() {
    let mut w = exposed();
    w.button(true);
    assert_eq!(w.shown().exposure, 1., "not while it may still be a click");
    w.wait(LABEL_CLICK + 0.05);
    assert_eq!(w.shown().exposure, 0., "once it is too long for one");
    // Past the time egui still takes a press for a click, too.
    for _ in 0..4 {
        assert_eq!((w.shown().exposure, w.rendering()), (0., 0.));
        assert_eq!(w.exposure(), 1., "the edit stays");
        w.wait(0.5);
    }
    w.button(false);
    assert_eq!((w.shown().exposure, w.rendering()), (1., 1.));
    assert!(!w.e.document.edit.save_state().needs_save());
    assert!(!w.e.document.edit.history().can_undo());
    assert!(!w.e.document.edit.history().in_gesture());
    assert_eq!(w.e.view.selected_setting, None, "a hold is no click");
}

#[test]
fn a_label_still_selects_on_a_click_and_resets_on_a_double_click() {
    let mut w = exposed();
    w.button(true);
    assert_eq!(w.shown().exposure, 1., "a click never flashes the default");
    w.button(false);
    assert_eq!(w.shown().exposure, 1.);
    assert_eq!(w.e.view.selected_setting, Some(ParameterId::Exposure));
    assert_eq!(w.exposure(), 1.);
    w.button(true);
    w.button(false);
    assert_eq!(w.exposure(), 0.);
    let (steps, _) = w.e.document.edit.history().steps();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].name, "Exposure");
}

#[test]
fn holding_a_label_again_and_again_never_resets_it() {
    let mut w = exposed();
    for _ in 0..4 {
        w.hold();
        assert_eq!(w.shown().exposure, 0.);
        w.button(false);
        assert_eq!((w.exposure(), w.shown().exposure), (1., 1.));
    }
    // Nor does a click right after one.
    w.wait(2.);
    w.hold();
    w.button(false);
    w.button(true);
    w.button(false);
    assert_eq!(w.exposure(), 1.);
    // Later, a double-click does.
    w.wait(2.);
    for _ in 0..2 {
        w.button(true);
        w.button(false);
    }
    assert_eq!(w.exposure(), 0.);
}

#[test]
fn a_peek_keeps_showing_off_the_label_and_escape_ends_it() {
    let mut w = exposed();
    w.hold();
    let away = egui::Event::PointerMoved(Pos2::new(300., 600.));
    w.frame(0.1, vec![away]);
    w.wait(1.);
    assert_eq!(w.shown().exposure, 0.);
    w.frame(0.1, key_down(egui::Key::Escape, egui::Modifiers::NONE));
    w.wait(0.1);
    assert_eq!((w.shown().exposure, w.exposure()), (1., 1.));
}

#[test]
fn an_assistive_edit_made_during_a_peek_is_kept() {
    use crate::model::recipe::Treatment;
    let mut w = exposed();
    let tree = w.wait(0.1);
    let (switch, _) = tree
        .nodes
        .iter()
        .find(|(_, n)| n.label() == Some("Black & White"))
        .expect("the Black & White switch");
    w.hold();
    let click = egui::accesskit::ActionRequest {
        action: egui::accesskit::Action::Click,
        target_tree: egui::accesskit::TreeId::ROOT,
        target_node: *switch,
        data: None,
    };
    w.frame(0.1, vec![egui::Event::AccessKitActionRequest(click)]);
    w.wait(0.1);
    let treatment = |r: &Recipe| r.treatment() == Treatment::BlackWhite;
    assert!(treatment(w.e.document.edit.recipe()));
    assert_eq!((w.shown().exposure, w.exposure()), (0., 1.));
    w.button(false);
    assert_eq!(w.shown().exposure, 1.);
    assert!(treatment(w.e.document.edit.recipe()));
}

#[test]
fn a_peek_is_never_the_photos_thumbnail_and_names_no_step() -> anyhow::Result<()> {
    let mut w = exposed();
    let (_dir, _ctx, mut other, [a, _]) = two_photo_editor()?;
    w.e.library = other.library.take();
    w.e.document.catalog_photo = Some(a);
    w.e.document.path = Some("a.ARW".into());
    assert!(w.e.shows_library_edit());
    w.hold();
    assert!(!w.e.shows_library_edit());
    w.button(false);
    assert!(w.e.shows_library_edit());
    let before = w.e.document.edit.recipe().clone();
    w.e.document.edit.setup_mut().contrast = 0.5;
    w.e.commit_edit(before, None);
    let (steps, _) = w.e.document.edit.history().steps();
    assert_ne!(steps[0].name, "Exposure");
    Ok(())
}

#[test]
fn a_peek_ends_with_its_sliders_or_its_photo() {
    let mut w = exposed();
    w.hold();
    assert!(w.e.preview.peek.is_peeking());
    w.e.module = Module::Library;
    w.wait(0.1);
    assert!(!w.e.preview.peek.is_peeking());
    let mut w = exposed();
    w.hold();
    w.e.open_raw(std::path::PathBuf::from("missing-peek-fixture.ARW"), None);
    assert!(!w.e.preview.peek.is_peeking());
    w.wait(0.1);
    assert!(!w.e.preview.peek.is_peeking());
    assert!(!w.e.document.edit.save_state().needs_save());
}

#[test]
fn a_peek_at_a_masks_slider_leaves_its_overlay_on_but_out_of_the_way() {
    use crate::app::develop::state::Tool;
    use crate::app::worker::Overlay;
    let mut w = Window::new(|e| {
        use crate::model::masks::{LocalAdjust, MaskComponent, MaskGroup, MaskShape};
        let mask = MaskGroup {
            adjust: LocalAdjust {
                exposure: 1.,
                ..Default::default()
            },
            components: vec![MaskComponent::new(MaskShape::Linear {
                from: [0., 0.],
                to: [1., 1.],
            })],
            ..Default::default()
        };
        e.document.edit.setup_mut().masks.push(mask);
        e.view.set_tool(Tool::Mask);
        e.view.masking.select_after_selection(0, 0);
    });
    assert!(matches!(w.e.overlay(), Overlay::Mask { .. }));
    w.at_label("Exposure");
    w.hold();
    assert_eq!(w.shown().masks[0].adjust.exposure, 0.);
    assert!(matches!(w.e.overlay(), Overlay::None));
    w.button(false);
    assert_eq!(w.shown().masks[0].adjust.exposure, 1.);
    assert!(w.e.view.masking.overlay);
}

#[test]
fn a_tools_own_settings_do_not_peek() {
    let mut w = Window::new(|e| {
        e.view.set_tool(crate::app::develop::state::Tool::Remove);
        e.view.retouch.feather = 0.9;
        e.view.retouch.visualize = true;
        e.view.retouch.threshold = 0.9;
    });
    w.at_label("Feather");
    w.hold();
    assert_eq!(w.e.view.retouch.feather, 0.9);
    w.button(false);
    assert_eq!(w.e.view.retouch.feather, 0.9);
    w.at_label("Threshold");
    w.hold();
    assert_eq!(w.e.view.retouch.threshold, 0.9);
}

#[test]
fn a_long_hold_drawn_only_while_it_begins_is_still_no_click() {
    let mut w = exposed();
    w.button(true);
    // Nothing moves, so no frame is drawn until the release.
    w.time += 0.5;
    w.button(false);
    assert_eq!(w.e.view.selected_setting, None);
}

#[test]
fn a_number_being_typed_is_entered_once_the_label_is_released() {
    let mut w = exposed();
    let label = w.at;
    w.at_label("Vibrance");
    w.at += Vec2::new(180., 0.);
    w.button(true);
    w.button(false);
    for _ in 0..3 {
        w.wait(0.016);
    }
    let all = key_down(egui::Key::A, egui::Modifiers::COMMAND);
    w.frame(0.1, all);
    w.frame(0.1, vec![egui::Event::Text("50".into())]);
    w.at = label;
    w.hold();
    assert_eq!(w.shown().exposure, 0.);
    w.button(false);
    w.wait(0.016);
    assert_eq!(w.exposure(), 1.);
    assert_eq!(w.e.document.edit.recipe().vibrance, 0.5);
}
