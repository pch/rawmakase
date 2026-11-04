//! Editor tests of `chrome`: the whole editor driven through its frames.
use crate::app::testing::*;
use crate::app::*;
use eframe::egui::{Pos2, Rect};

/// A frame of the workspace, and the commands it sent the window.
fn panel_frame_commands(e: &mut Editor, events: Vec<egui::Event>) -> Vec<egui::ViewportCommand> {
    let ctx = e.context.clone();
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
            events,
            ..Default::default()
        },
        |ui| e.draw(ui),
    );
    output.textures_delta.clear();
    output.viewport_output[&egui::ViewportId::ROOT]
        .commands
        .clone()
}

fn key_up(key: egui::Key) -> Vec<egui::Event> {
    vec![egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: false,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }]
}

#[test]
fn tab_hides_develops_side_panels_and_the_photo_takes_their_room() {
    use crate::app::shell::panels::WorkspacePanel;
    let dir = tempfile::tempdir().unwrap();
    let session = dir.path().join("session.json");
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        Some(session.clone()),
    );
    // The first launch's catalog opens first, in the Library.
    settle_catalog(&mut e, &ctx);
    e.module = Module::Develop;
    e.onboarding.visible = false;
    // Panels take their width in the first frames.
    for _ in 0..3 {
        panel_frame(&mut e, vec![], FocusedButton::No);
    }
    let with_panels = e.view.viewport;
    panel_frame(
        &mut e,
        key_down(egui::Key::Tab, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(!e.panel_shown(WorkspacePanel::Left));
    assert!(!e.panel_shown(WorkspacePanel::Right));
    assert!(e.panel_shown(WorkspacePanel::Filmstrip));
    assert!(e.view.viewport.x > with_panels.x + 400.);
    assert_eq!(e.view.viewport.y, with_panels.y);
    // Tab took no focus to the first control on its way.
    assert!(ctx.memory(|m| m.focused().is_none()));
    // Kept for the next launch, for Develop only.
    let saved: crate::app::preferences::session::Session =
        serde_json::from_slice(&std::fs::read(&session).unwrap()).unwrap();
    assert_eq!(saved.panels, e.panels);
    assert!(saved.panels.library.shown(WorkspacePanel::Left));
    // Shift+Tab hides the filmstrip and status bar too; again brings back
    // what it hid, the filmstrip, and Tab then the sides.
    panel_frame(
        &mut e,
        key_down(egui::Key::Tab, egui::Modifiers::SHIFT),
        FocusedButton::No,
    );
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(!e.panel_shown(WorkspacePanel::Filmstrip));
    assert!(e.view.viewport.y > with_panels.y);
    panel_frame(
        &mut e,
        key_down(egui::Key::Tab, egui::Modifiers::SHIFT),
        FocusedButton::No,
    );
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(e.panel_shown(WorkspacePanel::Filmstrip));
    assert!(!e.panel_shown(WorkspacePanel::Left));
    panel_frame(
        &mut e,
        key_down(egui::Key::Tab, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert_eq!(e.view.viewport, with_panels);
    // F8 hides the adjustments alone.
    panel_frame(
        &mut e,
        key_down(egui::Key::F8, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    assert!(e.panel_shown(WorkspacePanel::Left));
    assert!(!e.panel_shown(WorkspacePanel::Right));
}

#[test]
fn f_gives_the_photo_the_window_without_touching_the_window() -> anyhow::Result<()> {
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
    // Panels take their width in the first frames.
    for _ in 0..3 {
        panel_frame(&mut e, vec![], FocusedButton::No);
    }
    let with_chrome = e.view.viewport;
    let layout = e.panels;
    let commands = panel_frame_commands(&mut e, key_down(egui::Key::F, egui::Modifiers::NONE));
    panel_frame(&mut e, vec![], FocusedButton::No);
    // The mode is the app's own: no display is filled and the window keeps its
    // size, as the window's own full screen would change it.
    assert!(
        !commands.iter().any(|command| matches!(
            command,
            egui::ViewportCommand::Fullscreen(_)
                | egui::ViewportCommand::Maximized(_)
                | egui::ViewportCommand::Minimized(_)
                | egui::ViewportCommand::WindowLevel(_)
        )),
        "{commands:?}"
    );
    for panel in [
        WorkspacePanel::Left,
        WorkspacePanel::Right,
        WorkspacePanel::Filmstrip,
        WorkspacePanel::Presets,
    ] {
        assert!(!e.panel_shown(panel), "{panel:?} still shows");
    }
    // The bar, the panels, the filmstrip and Develop's toolbar are the photo's
    // room now, less the margin it is drawn with.
    let margin = crate::app::shell::workspace::PHOTO_MARGIN;
    assert_eq!(
        e.view.viewport,
        Vec2::new(1200., 800.) - Vec2::splat(2. * margin)
    );
    // Only what is drawn changed: the panels keep the layout saved for them.
    assert_eq!(e.panels, layout);
    // F again brings the window back, as it was.
    panel_frame(&mut e, key_up(egui::Key::F), FocusedButton::No);
    panel_frame(
        &mut e,
        key_down(egui::Key::F, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(e.panel_shown(WorkspacePanel::Left));
    assert_eq!(e.view.viewport, with_chrome);
    Ok(())
}

#[test]
fn f_hides_the_chrome_of_the_module_it_was_turned_on_in() {
    use crate::app::shell::panels::WorkspacePanel;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.module = Module::Library;
    e.onboarding.visible = false;
    panel_frame(
        &mut e,
        key_down(egui::Key::F, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(!e.panel_shown(WorkspacePanel::Left));
    // Develop draws its chrome, and the Library's is back when it returns: the
    // mode belongs to the module it was turned on in.
    e.module = Module::Develop;
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(e.panel_shown(WorkspacePanel::Left));
    e.module = Module::Library;
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(e.panel_shown(WorkspacePanel::Left));
    assert!(e.panel_shown(WorkspacePanel::Right));
}

#[test]
fn escape_leaves_the_whole_window_mode() {
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
    panel_frame(&mut e, vec![], FocusedButton::No);
    // With the chrome up, Escape is left to Develop's tools: it neither hides
    // the chrome nor is lost to it.
    panel_frame(
        &mut e,
        key_down(egui::Key::Escape, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    panel_frame(&mut e, key_up(egui::Key::Escape), FocusedButton::No);
    assert!(e.panel_shown(WorkspacePanel::Left));
    panel_frame(
        &mut e,
        key_down(egui::Key::F, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    panel_frame(&mut e, key_up(egui::Key::F), FocusedButton::No);
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(!e.panel_shown(WorkspacePanel::Left));
    // Escape brings the window back, as it does in Lightroom.
    panel_frame(
        &mut e,
        key_down(egui::Key::Escape, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    panel_frame(&mut e, key_up(egui::Key::Escape), FocusedButton::No);
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(e.panel_shown(WorkspacePanel::Left));
    assert!(e.panel_shown(WorkspacePanel::Right));
}

#[test]
fn in_the_library_f_gives_the_window_to_the_grid_then_the_photo() -> anyhow::Result<()> {
    use crate::app::shell::panels::WorkspacePanel;
    let (_d, mut e, _ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    e.onboarding.visible = false;
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(e.panel_shown(WorkspacePanel::Left));
    assert!(!e.library.as_ref().unwrap().loupe_open());
    // The first F gives the window to the grid; the photo is not opened for it.
    panel_frame(
        &mut e,
        key_down(egui::Key::F, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    panel_frame(&mut e, key_up(egui::Key::F), FocusedButton::No);
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(!e.panel_shown(WorkspacePanel::Left));
    assert!(!e.library.as_ref().unwrap().loupe_open());
    // The next F gives it to the photo, in the Loupe, with the chrome still
    // hidden.
    panel_frame(
        &mut e,
        key_down(egui::Key::F, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    panel_frame(&mut e, key_up(egui::Key::F), FocusedButton::No);
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(!e.panel_shown(WorkspacePanel::Left));
    assert!(e.library.as_ref().unwrap().loupe_open());
    // The third brings the chrome back, and the grid with it: the mode opened
    // the Loupe, so leaving the mode closes it again.
    panel_frame(
        &mut e,
        key_down(egui::Key::F, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    panel_frame(&mut e, key_up(egui::Key::F), FocusedButton::No);
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(e.panel_shown(WorkspacePanel::Left));
    assert!(!e.library.as_ref().unwrap().loupe_open());
    Ok(())
}

#[test]
fn the_mode_keeps_a_loupe_it_did_not_open() -> anyhow::Result<()> {
    use crate::app::shell::panels::WorkspacePanel;
    let (_d, mut e, _ids) = editor_with_catalog(&["a.RAF"])?;
    e.onboarding.visible = false;
    // A photo already open is what the mode gives the window to, instead of the
    // grid, and leaving the mode leaves it open.
    e.library.as_mut().unwrap().open_loupe();
    panel_frame(
        &mut e,
        key_down(egui::Key::F, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    panel_frame(&mut e, key_up(egui::Key::F), FocusedButton::No);
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(!e.panel_shown(WorkspacePanel::Left));
    assert!(e.library.as_ref().unwrap().loupe_open());
    panel_frame(
        &mut e,
        key_down(egui::Key::Escape, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    panel_frame(&mut e, key_up(egui::Key::Escape), FocusedButton::No);
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(e.panel_shown(WorkspacePanel::Left));
    assert!(e.library.as_ref().unwrap().loupe_open());
    Ok(())
}

#[test]
fn entering_another_module_closes_a_loupe_the_mode_opened() -> anyhow::Result<()> {
    let (_d, mut e, _ids) = editor_with_catalog(&["a.RAF"])?;
    e.onboarding.visible = false;
    for _ in 0..2 {
        panel_frame(
            &mut e,
            key_down(egui::Key::F, egui::Modifiers::NONE),
            FocusedButton::No,
        );
        panel_frame(&mut e, key_up(egui::Key::F), FocusedButton::No);
        panel_frame(&mut e, vec![], FocusedButton::No);
    }
    assert!(e.library.as_ref().unwrap().loupe_open());
    // D draws Develop's chrome, and the Library is back at its grid: the mode
    // belongs to the module it was turned on in.
    e.module = Module::Develop;
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(!e.library.as_ref().unwrap().loupe_open());
    Ok(())
}

#[test]
fn f_leaves_the_mode_over_a_loupe_opened_in_it() -> anyhow::Result<()> {
    use crate::app::shell::panels::WorkspacePanel;
    let (_d, mut e, _ids) = editor_with_catalog(&["a.RAF"])?;
    e.onboarding.visible = false;
    let f = |e: &mut Editor| {
        panel_frame(
            e,
            key_down(egui::Key::F, egui::Modifiers::NONE),
            FocusedButton::No,
        );
        panel_frame(e, key_up(egui::Key::F), FocusedButton::No);
        panel_frame(e, vec![], FocusedButton::No);
    };
    f(&mut e);
    assert!(!e.panel_shown(WorkspacePanel::Left));
    // The grid has the window, and a photo is opened from it, as a double
    // click does: the photo has the window now, so F leaves the mode, and the
    // Loupe stays, as the mode did not open it.
    e.library.as_mut().unwrap().open_loupe();
    panel_frame(&mut e, vec![], FocusedButton::No);
    f(&mut e);
    assert!(e.panel_shown(WorkspacePanel::Left));
    assert!(e.library.as_ref().unwrap().loupe_open());
    Ok(())
}

#[test]
fn panel_keys_bring_the_hidden_chrome_back_as_it_was() {
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
    panel_frame(&mut e, vec![], FocusedButton::No);
    let layout = e.panels;
    // Each panel key, with every panel hidden by the mode, brings the chrome
    // back with the layout it had, rather than changing one no one sees.
    for (key, modifiers) in [
        (egui::Key::Tab, egui::Modifiers::NONE),
        (egui::Key::Tab, egui::Modifiers::SHIFT),
        (egui::Key::F7, egui::Modifiers::NONE),
        (egui::Key::F8, egui::Modifiers::SHIFT),
    ] {
        panel_frame(
            &mut e,
            key_down(egui::Key::F, egui::Modifiers::NONE),
            FocusedButton::No,
        );
        panel_frame(&mut e, key_up(egui::Key::F), FocusedButton::No);
        panel_frame(&mut e, vec![], FocusedButton::No);
        assert!(!e.panel_shown(WorkspacePanel::Left));
        panel_frame(&mut e, key_down(key, modifiers), FocusedButton::No);
        panel_frame(&mut e, key_up(key), FocusedButton::No);
        panel_frame(&mut e, vec![], FocusedButton::No);
        assert_eq!(e.panels, layout, "{modifiers:?} {key:?}");
        for panel in [
            WorkspacePanel::Left,
            WorkspacePanel::Right,
            WorkspacePanel::Filmstrip,
            WorkspacePanel::Presets,
        ] {
            assert!(
                e.panel_shown(panel),
                "{panel:?} after {modifiers:?} {key:?}"
            );
        }
    }
}

#[test]
fn a_loupe_reopened_by_hand_in_the_mode_stays_on_leaving_it() -> anyhow::Result<()> {
    let (_d, mut e, _ids) = editor_with_catalog(&["a.RAF"])?;
    e.onboarding.visible = false;
    let press = |e: &mut Editor, key| {
        panel_frame(e, key_down(key, egui::Modifiers::NONE), FocusedButton::No);
        panel_frame(e, key_up(key), FocusedButton::No);
        panel_frame(e, vec![], FocusedButton::No);
    };
    // The mode opens the Loupe, and the user goes back to the grid and opens
    // it again: that Loupe is the user's, and leaving the mode keeps it.
    press(&mut e, egui::Key::F);
    press(&mut e, egui::Key::F);
    assert!(e.library.as_ref().unwrap().loupe_open());
    e.library.as_mut().unwrap().close_loupe();
    panel_frame(&mut e, vec![], FocusedButton::No);
    press(&mut e, egui::Key::E);
    assert!(e.library.as_ref().unwrap().loupe_open());
    press(&mut e, egui::Key::F);
    assert!(!e.chrome.hidden(e.module));
    assert!(e.library.as_ref().unwrap().loupe_open());
    Ok(())
}

#[test]
fn hiding_the_chrome_saves_a_library_field_being_typed_first() -> anyhow::Result<()> {
    use crate::app::shell::commands::Action;
    use crate::app::shell::panels::WorkspacePanel;
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    e.onboarding.visible = false;
    let copy = e
        .library
        .as_mut()
        .unwrap()
        .create_virtual_copy(ids[0])?
        .value;
    e.library.as_mut().unwrap().select(Some(copy));
    panel_frame(&mut e, vec![], FocusedButton::No);
    // The info panel goes with the chrome, as hiding it does: what is being
    // typed in it is saved first.
    e.library
        .as_mut()
        .unwrap()
        .set_copy_name_draft(copy, "Typed name");
    e.execute_action(Action::Chrome, None).unwrap();
    assert!(!e.panel_shown(WorkspacePanel::Right));
    assert_eq!(
        e.library.as_ref().unwrap().photo(copy).unwrap().copy_name,
        "Typed name"
    );
    let view = e.chrome.show();
    e.chrome_view(view);
    // A field that cannot be saved keeps the panel, and the command says why.
    let library = e.library.as_mut().unwrap();
    library
        .session
        .catalog_mut_for_tests()
        .remove_virtual_copy(copy)?;
    library.set_copy_name_draft(copy, "Unsaved name");
    let refused = e.execute_action(Action::Chrome, None).unwrap_err();
    assert_eq!(refused.code, "save_failed");
    assert!(e.panel_shown(WorkspacePanel::Right));
    Ok(())
}

#[test]
fn state_reports_the_panels_the_chrome_hides() {
    use crate::app::shell::commands::Action;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.module = Module::Develop;
    e.onboarding.visible = false;
    let state = serde_json::to_value(e.command_state()).unwrap();
    assert_eq!(state["panels"]["left"], "shown");
    assert_eq!(state["chrome"], "shown");
    e.execute_action(Action::Chrome, None).unwrap();
    let state = serde_json::to_value(e.command_state()).unwrap();
    for panel in ["left", "right", "filmstrip", "presets"] {
        assert_eq!(state["panels"][panel], "hidden", "{panel}");
    }
    assert_eq!(state["chrome"], "hidden");
}

#[test]
fn escape_closes_a_menu_before_it_leaves_the_mode() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    e.onboarding.visible = false;
    panel_frame(
        &mut e,
        key_down(egui::Key::F, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    panel_frame(&mut e, key_up(egui::Key::F), FocusedButton::No);
    let ctx = e.context.clone();
    let cell = ctx
        .read_response(egui::Id::new(("catalog-photo-cell", ids[0])))
        .unwrap();
    let pos = cell.rect.center();
    panel_frame(
        &mut e,
        vec![egui::Event::PointerMoved(pos)],
        FocusedButton::No,
    );
    for pressed in [true, false] {
        panel_frame(
            &mut e,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }],
            FocusedButton::No,
        );
    }
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(egui::Popup::is_any_open(&ctx));
    // The photo's menu takes Escape first, and the mode stays.
    panel_frame(
        &mut e,
        key_down(egui::Key::Escape, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    assert!(!egui::Popup::is_any_open(&ctx));
    assert!(e.chrome.hidden(e.module));
    Ok(())
}

#[test]
fn f_leaves_the_mode_over_a_library_view_with_no_photo() -> anyhow::Result<()> {
    let (_d, mut e, _) = editor_with_catalog(&["a.RAF"])?;
    e.onboarding.visible = false;
    let library = e.library.as_mut().unwrap();
    let mut layout = library.layout();
    layout.query = "nothing-matches-this".into();
    layout.filters_off = false;
    library.apply_layout(&layout);
    assert!(library.selected_or_first().is_none());
    // The grid takes the window; with no photo to give it to next, the next
    // F brings the chrome back rather than asking for a Loupe forever.
    let f = |e: &mut Editor| {
        panel_frame(
            e,
            key_down(egui::Key::F, egui::Modifiers::NONE),
            FocusedButton::No,
        );
        panel_frame(e, key_up(egui::Key::F), FocusedButton::No);
        panel_frame(e, vec![], FocusedButton::No);
    };
    f(&mut e);
    assert!(e.chrome.hidden(e.module));
    f(&mut e);
    assert!(!e.chrome.hidden(e.module));
    assert!(!e.library.as_ref().unwrap().loupe_open());
    Ok(())
}

#[test]
fn tab_moves_the_focus_on_while_a_control_has_it() {
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
    panel_frame(&mut e, vec![], FocusedButton::Yes);
    panel_frame(
        &mut e,
        key_down(egui::Key::Tab, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    assert!(e.panel_shown(WorkspacePanel::Left));
    assert!(e.panel_shown(WorkspacePanel::Right));
}

/// With the chrome hidden the Loupe's toolbar goes, and its photo takes the row.
#[test]
fn the_chrome_hidden_gives_the_loupe_its_toolbar_row() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    image::RgbImage::from_fn(300, 200, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
    })
    .save(folder.join("a.jpg"))?;
    let path = directory.path().join("library.rawmakase");
    crate::catalog::Catalog::create(&path)?.add_folder(&folder)?;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let mut library = library::Library::load(&path, ctx.clone())?;
    library.wait_for_availability();
    library.select(Some(library.session.photos()[0].id));
    library.open_loupe();
    e.library = Some(Box::new(library));
    e.module = Module::Library;
    let mut view = |chrome: bool| {
        for _ in 0..2 {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
                    ..Default::default()
                },
                |ui| {
                    e.library_view(ui, chrome);
                },
            )
            .textures_delta
            .clear();
        }
        e.library.as_ref().unwrap().loupe_view()
    };
    let with_toolbar = view(true);
    let without = view(false);
    // The view is taller by the toolbar's row, and no wider for it.
    assert!(
        without.y > with_toolbar.y + 20.,
        "{without:?} against {with_toolbar:?}"
    );
    assert_eq!(without.x, with_toolbar.x);
    Ok(())
}
