//! Editor tests of `viewport`: the whole editor driven through its frames.
use crate::app::*;
use crate::camera_data::{CameraImage, Metadata};
use eframe::egui::{Pos2, Rect};
use std::sync::Arc;

#[test]
fn keyboard_fit_and_physical_pixel_region() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    // Z turns the zoom on; F belongs to the chrome now (see `chrome`), so it
    // leaves the photo's zoom as it was.
    for (key, expected) in [(egui::Key::Z, true), (egui::Key::F, true)] {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
            events: vec![egui::Event::Key {
                key,
                physical_key: Some(key),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| e.draw(ui));
        output.textures_delta.clear();
        assert_eq!(e.view.zoom.on, expected);
    }
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 12,
        height: 8,
        pixels: vec![[0.1; 3]; 96],
        metadata: Metadata {
            width: 12,
            height: 8,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    e.document.set_image(image);
    e.view.zoom.on = true;
    e.view.viewport = Vec2::new(4., 2.);
    assert_eq!(e.region(), Some([4, 3, 4, 2]));
}

/// Cmd+0 is 100%, Cmd+1 fits, and Cmd+= / Cmd+- zoom by a constant factor from
/// the fitted size, stopping at 100% on the way.
#[test]
fn zoom_keys_go_to_100_percent_fit_and_step_by_a_factor() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.module = Module::Develop;
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 1000,
        height: 1000,
        pixels: vec![[0.1; 3]; 1_000_000],
        metadata: Metadata {
            width: 1000,
            height: 1000,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    editor.document.set_image(image);
    // Fits at 0.2: 200 pixels of view for 1000 of photo.
    editor.view.viewport = Vec2::splat(200.);
    let mut press = |key, physical, modifiers| {
        ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key,
                    physical_key: Some(physical),
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                ..Default::default()
            },
            |_| editor.zoom_keys(&ctx),
        )
        .textures_delta
        .clear();
        (editor.view.zoom.on, editor.view.zoom.level)
    };
    let cmd = egui::Modifiers::COMMAND;
    assert_eq!(press(egui::Key::Num0, egui::Key::Num0, cmd), (true, 1.));
    assert!(!press(egui::Key::Num1, egui::Key::Num1, cmd).0);
    let (on, level) = press(egui::Key::Equals, egui::Key::Equals, cmd);
    assert!(on && (level - 0.2 * std::f32::consts::SQRT_2).abs() < 1e-4);
    // 0.28, 0.4, 0.57, 0.8, then 100% rather than 113%.
    for _ in 0..3 {
        press(egui::Key::Equals, egui::Key::Equals, cmd);
    }
    assert_eq!(press(egui::Key::Equals, egui::Key::Equals, cmd), (true, 1.));
    // Cmd+Option+0 types another character on macOS; the physical key counts.
    press(egui::Key::Num1, egui::Key::Num1, cmd);
    let alt = egui::Modifiers {
        alt: true,
        ..egui::Modifiers::COMMAND
    };
    assert_eq!(press(egui::Key::Equals, egui::Key::Num0, alt), (true, 1.));
    // Fit again from just above it.
    press(egui::Key::Num1, egui::Key::Num1, cmd);
    press(egui::Key::Equals, egui::Key::Equals, cmd);
    assert!(!press(egui::Key::Minus, egui::Key::Minus, cmd).0);
}

#[test]
fn photo_click_zooms_and_drag_pans_without_editing() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 400,
        height: 400,
        pixels: vec![[0.1; 3]; 160000],
        metadata: Metadata {
            width: 400,
            height: 400,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    editor.document.set_image(image);
    editor.preview.texture = Some(
        ctx.load_texture(
            "photo",
            egui::ColorImage::filled([200, 200], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        )
        .into(),
    );
    let recipe = editor.document.edit.recipe().clone();
    let mut frame = |events| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.))),
                events,
                ..Default::default()
            },
            |ui| {
                editor.viewport_ui(ui);
            },
        );
        output.textures_delta.clear();
        (
            editor.view.zoom.on,
            editor.view.zoom.pan,
            editor.document.edit.recipe().clone(),
        )
    };
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let p = Pos2::new(100., 100.);
    frame(vec![]);
    frame(vec![egui::Event::PointerMoved(p), button(p, true)]);
    let (zoom, original_pan, _) = frame(vec![button(p, false)]);
    assert!(zoom);
    frame(vec![button(p, true)]);
    let q = Pos2::new(140., 130.);
    let (zoom, pan, after) = frame(vec![egui::Event::PointerMoved(q)]);
    assert!(zoom);
    assert!(pan[0] < original_pan[0] && pan[1] < original_pan[1]);
    assert_eq!(recipe, after);
    let (zoom, _, _) = frame(vec![button(q, false)]);
    assert!(zoom, "Releasing a pan must not toggle zoom");
    frame(vec![button(q, true)]);
    assert!(!frame(vec![button(q, false)]).0);
}
