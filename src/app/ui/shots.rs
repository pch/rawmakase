//! Windows and dialogs drawn to PNG files, to look at before they ship.
//!
//! `RAWMAKASE_UI_SHOTS=<dir> cargo test -p rawmakase --lib shots_` writes one
//! PNG per shot into `<dir>`, drawn by wgpu with the app's theme, interface
//! face and icons at 2×, as on a Retina display. Without the variable a shot draws
//! nothing, so CI needs no GPU. The images are for looking at, not compared
//! with references: fonts and rasterizers differ between machines.
use crate::app::Editor;
use crate::app::preferences::session::Session;
use eframe::egui;
use std::path::PathBuf;

/// The folder shots are written to, when asked for.
fn folder() -> Option<PathBuf> {
    std::env::var_os("RAWMAKASE_UI_SHOTS").map(PathBuf::from)
}

/// Draws `draw` in a `size`-point window over a new editor, which `setup`
/// prepares first, and writes it as `<name>.png`. Six frames are drawn,
/// enough for the theme, the fonts and a modal's size to take effect; nothing
/// is clicked or typed, so `setup` puts the editor in the state to draw.
pub(in crate::app) fn shot(
    name: &str,
    size: egui::Vec2,
    setup: impl FnOnce(&mut Editor, &egui::Context),
    mut draw: impl FnMut(&mut Editor, &egui::Context),
) {
    let Some(dir) = folder() else {
        return;
    };
    let mut setup = Some(setup);
    let mut harness = egui_kittest::Harness::builder()
        .with_size(size)
        .with_pixels_per_point(2.)
        .wgpu()
        .build_ui_state(
            move |ui, editor: &mut Option<Editor>| {
                let editor = editor.get_or_insert_with(|| {
                    // As the app starts: the desktop's text rendering, the
                    // interface face and the icon font.
                    crate::app::install_fonts(ui.ctx(), &fastframe_text::detect());
                    crate::app::ui::icons::install(ui.ctx());
                    let mut editor = Editor::with_context(ui.ctx(), None, Session::default(), None);
                    if let Some(setup) = setup.take() {
                        setup(&mut editor, ui.ctx());
                    }
                    editor
                });
                draw(editor, ui.ctx());
            },
            None,
        );
    // The theme and fonts take effect on the frame after the first.
    harness.run_steps(6);
    let image = harness
        .render()
        .unwrap_or_else(|e| panic!("{name}: wgpu could not draw the shot: {e}"));
    std::fs::create_dir_all(&dir).expect("the shots folder");
    let file = dir.join(format!("{name}.png"));
    image
        .save(&file)
        .unwrap_or_else(|e| panic!("{}: {e}", file.display()));
}

/// Draws `add` on a panel `width` points wide at the shot's top left, for a
/// shot of something drawn into a [`egui::Ui`] rather than its own window.
pub(in crate::app) fn panel(ctx: &egui::Context, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    egui::Area::new(egui::Id::new("shot-panel"))
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            egui::Frame::central_panel(ui.style()).show(ui, |ui| {
                ui.set_width(width);
                add(ui);
            });
        });
}
