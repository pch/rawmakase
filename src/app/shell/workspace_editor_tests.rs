//! Editor tests of `workspace`: the whole editor driven through its frames.
use crate::app::testing::*;
use crate::app::*;
use crate::camera_data::Metadata;
use eframe::egui::{Pos2, Rect};

#[test]
fn a_tools_drawer_takes_the_adjustments_place_beside_the_tabs() {
    // The rail's first group (Edit and the tools) switches the adjustments
    // panel; Presets and History keep their panel beside it.
    use crate::app::develop::panels::side_tabs::{edit_id, rail_id};
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
    // Room for the tab panel beside the photo.
    assert!(e.change_panels(PanelChange::Toggle(WorkspacePanel::Left)));
    let tabs_drawn = |ctx: &egui::Context| {
        ctx.read_response(egui::Id::new("presets").with("__resize"))
            .is_some()
    };
    let click = |e: &mut Editor, id: egui::Id| {
        let at = ctx
            .read_response(id)
            .expect("the rail's button")
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
        panel_frame(e, vec![], FocusedButton::No);
    };
    for _ in 0..3 {
        panel_frame(&mut e, vec![], FocusedButton::No);
    }
    assert!(tabs_drawn(&ctx));
    // Crop's drawer leaves the Presets tab open.
    e.view.tool = crate::app::develop::state::Tool::Crop;
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(tabs_drawn(&ctx));
    // Switching the tab keeps the tool.
    click(&mut e, rail_id(DevelopTab::History));
    assert_eq!(e.panels.develop_tab, DevelopTab::History);
    assert_eq!(e.view.tool, crate::app::develop::state::Tool::Crop);
    assert!(tabs_drawn(&ctx));
    // Edit closes the drawer, the tab panel stays.
    click(&mut e, edit_id());
    assert_eq!(e.view.tool, crate::app::develop::state::Tool::None);
    assert!(tabs_drawn(&ctx));
    // Folding the tabs leaves a tool open.
    e.view.tool = crate::app::develop::state::Tool::Mask;
    click(&mut e, rail_id(DevelopTab::History));
    assert!(!tabs_drawn(&ctx));
    assert_eq!(e.view.tool, crate::app::develop::state::Tool::Mask);
}

#[test]
fn a_narrow_window_folds_develops_tabs_until_one_is_asked_for() {
    use crate::app::develop::panels::side_tabs::rail_id;
    use crate::app::shell::panels::{DevelopTab, WorkspacePanel};
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
    for _ in 0..3 {
        panel_frame(&mut e, vec![], FocusedButton::No);
    }
    // 1200 points leave no room for it beside the sidebar and adjustments:
    // folded to its rail, the photo keeps its width.
    assert!(e.panel_shown(WorkspacePanel::Presets));
    assert!(!drawn(&ctx));
    assert!(e.view.viewport.x > 400.);
    let at = ctx
        .read_response(rail_id(DevelopTab::History))
        .expect("the rail's History button")
        .rect
        .center();
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    panel_frame(
        &mut e,
        vec![egui::Event::PointerMoved(at)],
        FocusedButton::No,
    );
    panel_frame(&mut e, vec![button(true)], FocusedButton::No);
    panel_frame(&mut e, vec![button(false)], FocusedButton::No);
    panel_frame(&mut e, vec![], FocusedButton::No);
    // Asked for, it opens on that tab; the layout saved is unchanged.
    assert_eq!(e.panels.develop_tab, DevelopTab::History);
    assert!(drawn(&ctx));
    assert!(e.panel_shown(WorkspacePanel::Presets));
}

#[test]
fn long_lens_names_keep_the_develop_panel_on_screen() {
    // Issues 362 and 363: a long lens or lens profile name widened the panel past
    // the window, and the photo was drawn over the panel's left part.
    let viewport = |lens: &str, profile: &str| {
        let ctx = egui::Context::default();
        let mut editor = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        let mut m = Metadata {
            make: "Testcam".into(),
            lens_model: lens.into(),
            focal: 35.,
            aperture: 2.,
            width: 600,
            height: 400,
            ..Default::default()
        };
        let lcp = crate::lens::lcp::test_profile("Testcam", lens, profile, -0.05, -0.5);
        m.lens_profiles =
            crate::lens::lcp::Library::from_texts([("Testcam - RAW.lcp", lcp.as_str())])
                .for_photo(&m);
        assert_eq!(m.lens_profiles.all().len(), 1);
        editor.document.metadata = Some(m);
        editor.document.edit.setup_mut().lens_profile = true;
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1400., 800.));
        for _ in 0..5 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| editor.draw(ui),
            );
            output.textures_delta.clear();
        }
        let panel = egui::containers::panel::PanelState::load(&ctx, egui::Id::new("adjustments"))
            .expect("the Develop panel is shown");
        assert!(
            panel.outer_rect.max.x <= screen.max.x,
            "panel {:?} leaves the window",
            panel.outer_rect
        );
        editor.view.viewport
    };
    let long = "AF-S VR Zoom-Nikkor 70-200mm f/2.8G IF-ED with a converter's long description";
    let short = viewport("35mm F2", "Adobe (35mm F2)");
    assert_eq!(viewport(long, "Adobe (35mm F2)"), short);
    assert_eq!(
        viewport("35mm F2", &format!("Adobe ({long}, Testcam)")),
        short
    );
}

#[test]
fn a_side_panels_edge_shows_the_resize_cursor() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("photos.rawmakase");
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    // Enough photos that the grid's cells reach the panels' edges.
    for i in 0..60 {
        std::fs::write(photos.join(format!("a{i}.ARW")), format!("fixture {i}"))?;
    }
    crate::catalog::Catalog::create(&path)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.library = Some(Box::new(library::Library::load(&path, ctx.clone())?));
    e.onboarding.visible = false;
    let frame = |e: &mut Editor, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
                events,
                ..Default::default()
            },
            |ui| e.draw(ui),
        );
        output.textures_delta.clear();
        output.platform_output.cursor_icon
    };
    // Develop's tab panel last, with room made for it by hiding the sidebar.
    for (module, panels) in [
        (Module::Library, &["library-sidebar", "library-info"][..]),
        (Module::Develop, &["library-sidebar", "adjustments"][..]),
        (Module::Develop, &["presets"][..]),
    ] {
        e.module = module;
        if panels == ["presets"] {
            e.change_panels(crate::app::shell::panels::PanelChange::Toggle(
                crate::app::shell::panels::WorkspacePanel::Left,
            ));
        }
        for _ in 0..3 {
            frame(&mut e, vec![]);
        }
        for &panel in panels {
            let edge = ctx
                .read_response(egui::Id::new(panel).with("__resize"))
                .expect("the panel's edge")
                .rect;
            // Both halves of its grab area: the panel's and the one over the
            // grid or the photo, whose cells are drawn later.
            let half = edge.width() / 2. - 1.;
            for at in [
                edge.center(),
                edge.center() - Vec2::new(half, 0.),
                edge.center() + Vec2::new(half, 0.),
            ] {
                frame(&mut e, vec![egui::Event::PointerMoved(at)]);
                let cursor = frame(&mut e, vec![egui::Event::PointerMoved(at)]);
                assert!(
                    matches!(
                        cursor,
                        egui::CursorIcon::ResizeHorizontal
                            | egui::CursorIcon::ResizeEast
                            | egui::CursorIcon::ResizeWest
                    ),
                    "{panel} at {at:?}: {cursor:?}"
                );
            }
        }
    }
    Ok(())
}

#[test]
fn side_panels_stop_where_their_contents_stop() -> anyhow::Result<()> {
    // Dragged narrower than its contents, a panel was painted only that wide
    // but laid out as wide as them: the strip between showed the window's
    // black. Each one stops at its minimum, which its contents fit.
    use crate::app::shell::workspace::{
        ADJUSTMENTS_MIN, LIBRARY_INFO_MIN, LIBRARY_SIDEBAR_MIN, PRESETS_MIN,
    };
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("photos.rawmakase");
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    std::fs::write(photos.join("a.ARW"), b"fixture")?;
    crate::catalog::Catalog::create(&path)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.library = Some(Box::new(library::Library::load(&path, ctx.clone())?));
    e.onboarding.visible = false;
    let frame = |e: &mut Editor, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
                events,
                ..Default::default()
            },
            |ui| e.draw(ui),
        );
        output.textures_delta.clear();
    };
    let button = |at: Pos2, pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    for (module, panel, outward, min) in [
        (Module::Develop, "library-sidebar", 0., LIBRARY_SIDEBAR_MIN),
        (Module::Develop, "presets", 1200., PRESETS_MIN),
        (Module::Develop, "adjustments", 1200., ADJUSTMENTS_MIN),
        (Module::Library, "library-sidebar", 0., LIBRARY_SIDEBAR_MIN),
        (Module::Library, "library-info", 1200., LIBRARY_INFO_MIN),
    ] {
        e.module = module;
        // The window has room for Develop's tab panel without the sidebar.
        if panel == "presets" {
            e.change_panels(crate::app::shell::panels::PanelChange::Toggle(
                crate::app::shell::panels::WorkspacePanel::Left,
            ));
        }
        for _ in 0..3 {
            frame(&mut e, vec![]);
        }
        let edge = |ctx: &egui::Context| {
            ctx.read_response(egui::Id::new(panel).with("__resize"))
                .expect("the panel's edge")
                .rect
                .center()
                .x
        };
        let start = Pos2::new(edge(&ctx), 400.);
        frame(&mut e, vec![egui::Event::PointerMoved(start)]);
        frame(&mut e, vec![button(start, true)]);
        // Mid-drag, as far as it goes: laid out at its minimum, as painted.
        let to = Pos2::new(outward, start.y);
        for _ in 0..3 {
            frame(&mut e, vec![egui::Event::PointerMoved(to)]);
        }
        // Develop's tab panel stops at the adjustments, and they at the tab
        // rail, not the window's edge.
        let inner = |id: &str| {
            egui::containers::panel::PanelState::load(&ctx, egui::Id::new(id))
                .expect("the panel beside")
                .outer_rect
                .left()
        };
        let stop = match (module, panel) {
            (Module::Develop, "presets") => inner("adjustments"),
            (Module::Develop, "adjustments") => inner("develop-rail"),
            _ => outward,
        };
        let laid_out = (edge(&ctx) - stop).abs();
        assert!(
            (laid_out - min).abs() <= 1.,
            "{panel}: laid out {laid_out}, painted {min}"
        );
        frame(&mut e, vec![button(to, false)]);
        for _ in 0..2 {
            frame(&mut e, vec![]);
        }
    }
    Ok(())
}

#[test]
fn a_tools_drawer_keeps_the_adjustments_width() {
    let ctx = egui::Context::default();
    crate::app::ui::icons::install(&ctx);
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.module = Module::Develop;
    e.onboarding.visible = false;
    let width = |e: &mut Editor| {
        for _ in 0..3 {
            panel_frame(e, vec![], FocusedButton::No);
        }
        egui::containers::panel::PanelState::load(&ctx, egui::Id::new("adjustments"))
            .unwrap()
            .outer_rect
            .width()
    };
    let edit = width(&mut e);
    for tool in [
        crate::app::develop::state::Tool::Crop,
        crate::app::develop::state::Tool::Remove,
        crate::app::develop::state::Tool::RedEye,
        crate::app::develop::state::Tool::Mask,
    ] {
        e.view.tool = tool;
        assert_eq!(width(&mut e), edit, "{tool:?} widened the panel");
        e.view.tool = crate::app::develop::state::Tool::None;
        assert_eq!(width(&mut e), edit, "after {tool:?}");
    }
}
