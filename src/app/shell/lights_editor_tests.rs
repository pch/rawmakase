//! Editor tests of `lights`: the whole editor driven through its frames.
use super::Lights;
use crate::app::shell::panels::{WorkspacePanel, toggle_id};
use crate::app::testing::*;
use crate::app::*;
use eframe::egui::{Color32, Pos2, Rect, Vec2};

/// A frame of the window given `events`, and the rects of the shade it painted.
fn shade_frame(e: &mut Editor, events: Vec<egui::Event>) -> Vec<Rect> {
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
    let shade = [Color32::from_black_alpha(204), Color32::BLACK];
    output
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Mesh(mesh)
                if !mesh.vertices.is_empty()
                    && mesh.vertices.iter().all(|v| shade.contains(&v.color)) =>
            {
                Some(
                    mesh.vertices.chunks(4).map(|quad| {
                        Rect::from_points(&quad.iter().map(|v| v.pos).collect::<Vec<_>>())
                    }),
                )
            }
            _ => None,
        })
        .flatten()
        .collect()
}

fn shaded_at(shade: &[Rect], at: Pos2) -> bool {
    shade.iter().any(|r| r.contains(at))
}

#[test]
fn the_grid_stays_lit_while_the_panels_dim_and_go_black() -> anyhow::Result<()> {
    let (_dir, mut e, _) = editor_with_catalog(&["one.ARW", "two.ARW"])?;
    e.onboarding.visible = false;
    shade_frame(&mut e, vec![]);
    assert!(shade_frame(&mut e, vec![]).is_empty(), "the lights are on");
    for lights in [Lights::Dim, Lights::Off] {
        e.lights = lights;
        let shade = shade_frame(&mut e, vec![]);
        let view = e.drop_area.expect("the Library's view");
        assert!(!shaded_at(&shade, view.center()), "{lights:?}: the grid");
        assert!(
            shaded_at(&shade, Pos2::new(20., 400.)),
            "{lights:?}: the sidebar"
        );
        assert!(
            shaded_at(&shade, Pos2::new(600., 10.)),
            "{lights:?}: the bar"
        );
        // The grid's toolbar, under the grid, is shaded too.
        assert!(
            shaded_at(&shade, Pos2::new(view.center().x, view.bottom() - 4.)),
            "{lights:?}: the grid's toolbar"
        );
    }
    Ok(())
}

#[test]
fn a_click_under_the_shade_still_reaches_the_panel_buttons() {
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
        shade_frame(e, vec![egui::Event::PointerMoved(at)]);
        shade_frame(e, vec![button(true)]);
        shade_frame(e, vec![button(false)])
    };
    shade_frame(&mut e, vec![]);
    e.lights = Lights::Dim;
    let shade = click(&mut e, WorkspacePanel::Left);
    assert!(
        !e.panel_shown(WorkspacePanel::Left),
        "the click went through"
    );
    let button = ctx.read_response(toggle_id(WorkspacePanel::Left)).unwrap();
    assert!(shaded_at(&shade, button.rect.center()), "under the shade");
    e.lights = Lights::Off;
    click(&mut e, WorkspacePanel::Left);
    assert!(e.panel_shown(WorkspacePanel::Left));
    // With no photo to light, Develop keeps its photo's area lit.
    let shade = shade_frame(&mut e, vec![]);
    let view = e.drop_area.expect("Develop's view");
    assert!(!shaded_at(&shade, view.center()));
    assert_eq!(
        e.lights,
        Lights::Off,
        "a click leaves the lights as they are"
    );
}

/// The whole window in each light: `RAWMAKASE_UI_SHOTS`
/// ([`crate::app::ui::shots`]).
#[test]
fn shots_of_lights_out() -> anyhow::Result<()> {
    let window = |e: &mut Editor, ctx: &egui::Context| {
        let mut ui = egui::Ui::new(
            ctx.clone(),
            egui::Id::new("lights-shot"),
            egui::UiBuilder::new()
                .layer_id(egui::LayerId::background())
                .max_rect(ctx.content_rect()),
        );
        e.draw(&mut ui);
    };
    let (dir, _, _) = editor_with_catalog(&["one.ARW", "two.ARW", "three.ARW"])?;
    let catalog = dir.path().join("test.rawmakase");
    for (name, lights) in [("dim", Lights::Dim), ("off", Lights::Off)] {
        crate::app::ui::shots::shot(
            &format!("lights-out-grid-{name}"),
            egui::vec2(1100., 700.),
            |e, ctx| {
                let library =
                    crate::app::library::Library::load(&catalog, ctx.clone()).expect("the catalog");
                e.library = Some(Box::new(library));
                e.module = Module::Library;
                e.onboarding.visible = false;
                e.messages.done("Messages stay over the shade");
                e.lights = lights;
            },
            window,
        );
        crate::app::ui::shots::shot(
            &format!("lights-out-develop-{name}"),
            egui::vec2(1100., 700.),
            |e, _| {
                e.module = Module::Develop;
                e.onboarding.visible = false;
                e.document.metadata = Some(blue_image().metadata.clone());
                e.document.set_image(blue_image());
                e.lights = lights;
            },
            window,
        );
    }
    Ok(())
}

#[test]
fn while_a_photo_loads_the_panels_stay_shaded() -> anyhow::Result<()> {
    let d = tempfile::tempdir()?;
    let photos = d.path().join("photos");
    std::fs::create_dir(&photos)?;
    let charts = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/charts");
    std::fs::copy(charts.join("canon-eos-r100-d65.dng"), photos.join("a.dng"))?;
    let path = d.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&path)?.add_folder(&photos)?;
    for develop in [false, true] {
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(&ctx, None, Default::default(), None);
        e.library = Some(Box::new(crate::app::library::Library::load(&path, ctx)?));
        e.module = Module::Library;
        e.onboarding.visible = false;
        e.lights = Lights::Dim;
        let id = e.library.as_ref().unwrap().session.photos()[0].id;
        if develop {
            e.develop_catalog_photo(id);
        } else {
            let library = e.library.as_mut().unwrap();
            library.reveal(id);
            library.open_loupe();
        }
        let shade = shade_frame(&mut e, vec![]);
        assert!(e.document.loading(), "develop {develop}: still loading");
        let view = e.drop_area.expect("the view");
        assert!(
            !shaded_at(&shade, view.center()),
            "develop {develop}: the photo's place"
        );
        for (what, at) in [
            ("the sidebar", Pos2::new(20., 400.)),
            ("the bar", Pos2::new(600., 10.)),
            ("the filmstrip", Pos2::new(600., 760.)),
            ("the right panel", Pos2::new(1100., 400.)),
        ] {
            assert!(shaded_at(&shade, at), "develop {develop}: {what}");
        }
        // With a preview to size it by, only the photo's place is lit, not the
        // backdrop around it, with the chrome hidden too.
        e.preview.stand_in = Some(crate::app::develop::state::Picture::presented(
            egui::TextureId::User(1),
            [100, 300],
        ));
        e.toggle_chrome();
        let shade = shade_frame(&mut e, vec![]);
        if develop || e.document.loading() {
            let view = e.drop_area.expect("the view");
            assert!(!shaded_at(&shade, view.center()), "develop {develop}");
            let beside = Pos2::new(view.left() + view.width() * 0.1, view.center().y);
            assert!(shaded_at(&shade, beside), "develop {develop}: the backdrop");
        }
    }
    Ok(())
}
