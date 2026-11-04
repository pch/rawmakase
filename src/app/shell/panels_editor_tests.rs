//! Editor tests of `panels`: the whole editor driven through its frames.
use crate::app::testing::*;
use crate::app::*;

#[test]
fn each_module_keeps_its_own_panels() -> anyhow::Result<()> {
    use crate::app::shell::panels::WorkspacePanel;
    let (_d, mut e, _ids) = editor_with_catalog(&["a.RAF"])?;
    panel_frame(&mut e, vec![], FocusedButton::No);
    panel_frame(
        &mut e,
        key_down(egui::Key::F7, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    assert!(!e.panel_shown(WorkspacePanel::Left));
    e.module = Module::Develop;
    assert!(e.panel_shown(WorkspacePanel::Left));
    e.module = Module::Library;
    // The Library draws with its filmstrip hidden.
    panel_frame(
        &mut e,
        key_down(egui::Key::F6, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    assert!(!e.panel_shown(WorkspacePanel::Filmstrip));
    panel_frame(&mut e, vec![], FocusedButton::No);
    Ok(())
}

#[test]
fn shift_f8_folds_develops_tab_panel_alone() {
    use crate::app::shell::panels::WorkspacePanel;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.module = Module::Develop;
    e.onboarding.visible = false;
    let drawn = |ctx: &egui::Context| {
        ctx.read_response(egui::Id::new("presets").with("__resize"))
            .is_some()
    };
    for _ in 0..2 {
        panel_frame(&mut e, vec![], FocusedButton::No);
    }
    // 1200 points leave it folded by itself; Shift+F8 opens it, as its rail does.
    assert!(!drawn(&ctx));
    let shift_f8 = || key_down(egui::Key::F8, egui::Modifiers::SHIFT);
    panel_frame(&mut e, shift_f8(), FocusedButton::No);
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(drawn(&ctx));
    // F8's plain pattern did not take it: the adjustments stay.
    assert!(e.panel_shown(WorkspacePanel::Right));
    panel_frame(&mut e, shift_f8(), FocusedButton::No);
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(!drawn(&ctx));
    assert!(e.panel_shown(WorkspacePanel::Right));
}

#[test]
fn hiding_develops_right_panel_takes_its_tab_panel_and_rail() {
    use crate::app::develop::panels::side_tabs::rail_id;
    use crate::app::shell::panels::{DevelopTab, PanelChange, WorkspacePanel};
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.module = Module::Develop;
    e.onboarding.visible = false;
    for _ in 0..2 {
        panel_frame(&mut e, vec![], FocusedButton::No);
    }
    assert!(ctx.read_response(rail_id(DevelopTab::Presets)).is_some());
    assert!(e.change_panels(PanelChange::Toggle(WorkspacePanel::Right)));
    for _ in 0..2 {
        panel_frame(&mut e, vec![], FocusedButton::No);
    }
    assert!(ctx.read_response(rail_id(DevelopTab::Presets)).is_none());
    assert!(
        ctx.read_response(egui::Id::new("presets").with("__resize"))
            .is_none()
    );
    // A tool's drawer keeps to the hidden side too.
    e.view.tool = crate::app::develop::state::Tool::Crop;
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(ctx.read_response(rail_id(DevelopTab::Presets)).is_none());
}

#[test]
fn hiding_the_presets_ends_a_preset_preview() {
    use crate::app::shell::panels::PanelChange;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.module = Module::Develop;
    e.presets.preview = Some(Recipe {
        exposure: 1.,
        ..Default::default()
    });
    e.presets.hover = Some((0, std::time::Instant::now()));
    assert!(e.change_panels(PanelChange::Sides));
    assert!(e.presets.preview.is_none());
    assert!(e.presets.hover.is_none());
}

#[test]
fn the_library_info_panel_stays_while_a_field_in_it_cannot_be_saved() -> anyhow::Result<()> {
    use crate::app::shell::panels::{PanelChange, WorkspacePanel};
    let (_d, mut e, ids) = editor_with_catalog(&["a.ARW"])?;
    let library = e.library.as_mut().unwrap();
    let copy = library.create_virtual_copy(ids[0])?.value;
    library
        .session
        .catalog_mut_for_tests()
        .remove_virtual_copy(copy)?;
    library.set_copy_name_draft(copy, "B&W");
    assert!(!e.change_panels(PanelChange::Toggle(WorkspacePanel::Right)));
    assert!(e.panel_shown(WorkspacePanel::Right));
    assert!(e.messages.line().starts_with("Not saved"));
    // The left panel holds no field; it hides.
    assert!(e.change_panels(PanelChange::Toggle(WorkspacePanel::Left)));
    e.library.as_mut().unwrap().discard_drafts();
    assert!(e.change_panels(PanelChange::Sides));
    assert!(!e.panel_shown(WorkspacePanel::Right));
    Ok(())
}

#[test]
fn the_bars_panel_buttons_hide_and_show_their_panels() {
    use crate::app::shell::panels::{WorkspacePanel, toggle_id};
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.module = Module::Develop;
    e.onboarding.visible = false;
    let click = |e: &mut Editor, panel: WorkspacePanel| {
        let at = ctx
            .read_response(toggle_id(panel))
            .expect("the bar draws the panel's button")
            .rect
            .center();
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        panel_frame(e, vec![egui::Event::PointerMoved(at)], FocusedButton::No);
        panel_frame(e, vec![button(true)], FocusedButton::No);
        panel_frame(e, vec![button(false)], FocusedButton::No);
    };
    panel_frame(&mut e, vec![], FocusedButton::No);
    click(&mut e, WorkspacePanel::Left);
    assert!(!e.panel_shown(WorkspacePanel::Left));
    assert!(e.panel_shown(WorkspacePanel::Right));
    click(&mut e, WorkspacePanel::Left);
    assert!(e.panel_shown(WorkspacePanel::Left));
    click(&mut e, WorkspacePanel::Right);
    assert!(!e.panel_shown(WorkspacePanel::Right));
    click(&mut e, WorkspacePanel::Filmstrip);
    assert!(!e.panel_shown(WorkspacePanel::Filmstrip));
    // A button clicked keeps no focus that would take Tab from the panels.
    panel_frame(
        &mut e,
        key_down(egui::Key::Tab, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    assert!(
        !e.panel_shown(WorkspacePanel::Left),
        "Tab hid the left panel"
    );
    // The setup view has no panels, and no buttons for them.
    e.onboarding.visible = true;
    panel_frame(&mut e, vec![], FocusedButton::No);
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(ctx.read_response(toggle_id(WorkspacePanel::Left)).is_none());
}

#[test]
fn edit_on_the_rail_folds_the_adjustments_and_brings_them_back() {
    use crate::app::develop::panels::side_tabs::edit_id;
    use crate::app::shell::panels::WorkspacePanel;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.module = Module::Develop;
    e.onboarding.visible = false;
    let drawn = |ctx: &egui::Context| {
        ctx.read_response(egui::Id::new("adjustments").with("__resize"))
            .is_some()
    };
    let click = |e: &mut Editor| {
        let at = ctx.read_response(edit_id()).expect("Edit").rect.center();
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        panel_frame(e, vec![egui::Event::PointerMoved(at)], FocusedButton::No);
        panel_frame(e, vec![button(true)], FocusedButton::No);
        panel_frame(e, vec![button(false)], FocusedButton::No);
        panel_frame(e, vec![], FocusedButton::No);
    };
    for _ in 0..2 {
        panel_frame(&mut e, vec![], FocusedButton::No);
    }
    assert!(drawn(&ctx));
    // Edit, already showing, folds the adjustments; the rail stays.
    click(&mut e);
    assert!(!drawn(&ctx));
    assert!(!e.panel_shown(WorkspacePanel::Adjustments));
    assert!(e.panel_shown(WorkspacePanel::Right));
    // A tool's drawer shows while they are folded.
    e.view.tool = crate::app::develop::state::Tool::Crop;
    panel_frame(&mut e, vec![], FocusedButton::No);
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(drawn(&ctx));
    // Closing it folds them again; Edit from it brings the adjustments back.
    e.view.tool = crate::app::develop::state::Tool::None;
    panel_frame(&mut e, vec![], FocusedButton::No);
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(!drawn(&ctx));
    e.view.tool = crate::app::develop::state::Tool::Crop;
    panel_frame(&mut e, vec![], FocusedButton::No);
    click(&mut e);
    assert_eq!(e.view.tool, crate::app::develop::state::Tool::None);
    assert!(drawn(&ctx));
    assert!(e.panel_shown(WorkspacePanel::Adjustments));
    // Tab leaves the fold alone: it is the rail's.
    click(&mut e);
    let tab = || key_down(egui::Key::Tab, egui::Modifiers::NONE);
    panel_frame(&mut e, tab(), FocusedButton::No);
    panel_frame(&mut e, tab(), FocusedButton::No);
    assert!(e.panel_shown(WorkspacePanel::Right));
    assert!(!e.panel_shown(WorkspacePanel::Adjustments));
}
