//! Editor tests of `ui_snapshot`: the whole editor driven through its frames.
use crate::app::*;

#[test]
#[ignore = "writes an image to look at"]
fn render_the_library_loupe_toolbar() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    image::RgbImage::new(300, 200).save(folder.join("a.jpg"))?;
    let path = directory.path().join("library.rawmakase");
    crate::catalog::Catalog::create(&path)?.add_folder(&folder)?;
    let ctx = egui::Context::default();
    crate::app::install_fonts(&ctx, &fastframe_text::TextRendering::platform_default());
    crate::app::ui::icons::install(&ctx);
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let mut library = library::Library::load(&path, ctx.clone())?;
    library.select(Some(library.session.photos()[0].id));
    library.open_loupe();
    e.library = Some(Box::new(library));
    e.module = Module::Library;
    let mut textures = Default::default();
    let image = crate::app::ui::ui_snapshot::render(
        &ctx,
        &mut textures,
        Vec2::new(900., 60.),
        2.,
        3,
        |ui| {
            e.library_loupe_toolbar(ui);
        },
    );
    let out =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/library-loupe-toolbar.png");
    image.save(&out)?;
    Ok(())
}

#[test]
#[ignore = "writes an image to look at"]
fn render_the_develop_right_panel() -> anyhow::Result<()> {
    let ctx = egui::Context::default();
    crate::app::install_fonts(&ctx, &fastframe_text::TextRendering::platform_default());
    crate::app::ui::icons::install(&ctx);
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.module = Module::Develop;
    let mut textures = Default::default();
    let image = crate::app::ui::ui_snapshot::render(
        &ctx,
        &mut textures,
        Vec2::new(340., 420.),
        2.,
        3,
        |ui| {
            let frame = e.begin_edit_frame();
            e.develop_right_panel(ui);
            e.finish_edit_frame(frame, &ctx);
        },
    );
    let out =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/develop-right-panel.png");
    image.save(&out)?;
    Ok(())
}

#[test]
#[ignore = "writes an image to look at"]
fn render_the_develop_toolbar() -> anyhow::Result<()> {
    for width in [1400., 900., 760.] {
        let ctx = egui::Context::default();
        crate::app::install_fonts(&ctx, &fastframe_text::TextRendering::platform_default());
        crate::app::ui::icons::install(&ctx);
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        e.module = Module::Develop;
        let mut textures = Default::default();
        let image = crate::app::ui::ui_snapshot::render(
            &ctx,
            &mut textures,
            Vec2::new(width, 60.),
            2.,
            3,
            |ui| {
                e.toolbar(ui);
            },
        );
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("target/develop-toolbar-{width}.png"));
        image.save(&out)?;
        eprintln!("{}", out.display());
    }
    Ok(())
}

#[test]
#[ignore = "writes an image to look at"]
fn render_the_crop_drawer() -> anyhow::Result<()> {
    let ctx = egui::Context::default();
    crate::app::install_fonts(&ctx, &fastframe_text::TextRendering::platform_default());
    crate::app::ui::icons::install(&ctx);
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.view.tool = crate::app::develop::state::Tool::Crop;
    let mut textures = Default::default();
    let image = crate::app::ui::ui_snapshot::render(
        &ctx,
        &mut textures,
        Vec2::new(320., 300.),
        2.,
        3,
        |ui| {
            let _labels = crate::app::ui::widgets::LabelColumn::enter(
                ui,
                crate::app::develop::panels::inspector::DRAWER_LABEL_COLUMN,
            );
            let palette = crate::app::ui::theme::palette(ui.ctx());
            ui.painter().rect_filled(
                ui.max_rect(),
                0.,
                palette.tone(crate::app::ui::design::Tone::CARD),
            );
            let frame = e.begin_edit_frame();
            e.tool_drawer(ui);
            e.finish_edit_frame(frame, &ctx);
        },
    );
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/crop-drawer.png");
    image.save(&out)?;
    eprintln!("{}", out.display());
    Ok(())
}
