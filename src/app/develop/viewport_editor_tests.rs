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
        hdr: Default::default(),
    });
    e.document.set_image(image);
    e.view.zoom.on = true;
    e.view.viewport = Vec2::new(4., 2.);
    // The view's own region is in physical pixels; the one rendered adds a
    // margin around it.
    let view = e.view.zoom.region(e.view.viewport, 12, 8);
    assert_eq!(view, Some([4, 3, 4, 2]));
    assert_eq!(e.region(), Some([3, 2, 6, 4]));
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
        hdr: Default::default(),
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
        hdr: Default::default(),
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

/// Zoomed in at 1.5x scaling, a pointer step of one point is a pixel and a
/// half: the photo is drawn on whole screen pixels wherever the pan puts it, or
/// a slow drag softens it every other frame.
#[test]
fn a_zoomed_in_photo_sits_on_whole_screen_pixels() {
    let ctx = egui::Context::default();
    ctx.set_pixels_per_point(1.5);
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: 400,
        height: 300,
        pixels: vec![[0.2; 3]; 120_000],
        metadata: Metadata {
            width: 400,
            height: 300,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    }));
    e.module = Module::Develop;
    e.preview.texture = Some(
        ctx.load_texture(
            "photo",
            egui::ColorImage::filled([40, 30], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        )
        .into(),
    );
    e.view.zoom.on = true;
    e.view.zoom.level = 1.;
    for step in 0..12 {
        e.view.zoom.pan = [0.4 + step as f32 * 0.0137, 0.45 + step as f32 * 0.0091];
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(100., 80.))),
                ..Default::default()
            },
            |ui| {
                e.viewport_ui(ui);
            },
        )
        .textures_delta
        .clear();
        let at = e.view.shown_rect.expect("the photo is drawn").min * 1.5;
        assert!(
            (at.x - at.x.round()).abs() < 1e-3 && (at.y - at.y.round()).abs() < 1e-3,
            "drawn at {at:?} pixels"
        );
    }
}

/// Develop's Fit view is due to show an HDR edit's master on a display with headroom,
/// and the SDR rendition where the display has none, the edit is SDR or refused in
/// HDR, the view is zoomed or compares, or a visualization is drawn into the render.
#[test]
fn the_hdr_master_is_due_only_in_the_plain_fit_view_of_an_hdr_edit() {
    use crate::app::develop::before_after::Compare;
    use crate::app::develop::clipping::ClipSide;
    use crate::app::shell::display::HdrDisplay;
    let ctx = egui::Context::default();
    let (mut e, _) = crate::app::testing::editor_with_blue_photo(&ctx, Default::default(), true);
    e.preview.last_fit_edge = 64;
    let hdr = HdrDisplay {
        headroom: 4.,
        potential: 16.,
        float_surface: true,
    };
    e.view.hdr_display = hdr;
    assert!(e.hdr_preview_due().is_err(), "an SDR edit");
    e.document.edit.setup_mut().hdr.enabled = true;
    let (key, _) = e.hdr_preview_due().expect("an HDR edit in Fit");
    assert_eq!(
        e.hdr_presentation(),
        crate::app::develop::hdr_preview::Presentation::Rendering
    );
    assert_eq!(
        key,
        crate::app::develop::hdr_preview::Key::new(
            e.document.full().unwrap(),
            &e.effective_recipe(),
            64,
            4.
        )
    );

    e.view.hdr_display = HdrDisplay {
        potential: 1.,
        ..hdr
    };
    assert!(e.hdr_preview_due().is_err(), "an SDR display");
    e.view.hdr_display = hdr;

    e.view.zoom.on = true;
    assert!(e.hdr_preview_due().is_err(), "zoomed in");
    e.view.zoom.on = false;

    e.view.compare = Compare::BeforeOnly;
    assert!(e.hdr_preview_due().is_err(), "Before");
    e.view.compare = Compare::Off;

    e.view.clipping.toggle(ClipSide::Highlights);
    assert!(e.hdr_preview_due().is_err(), "the clipping warning");
    e.view.clipping.toggle(ClipSide::Highlights);

    e.view.monitor = crate::app::preferences::monitor::MonitorProfile::Srgb;
    assert!(
        e.hdr_preview_due().is_err(),
        "a monitor profile of RAWmakase's own"
    );
    e.view.monitor = Default::default();

    e.module = Module::Library;
    assert!(e.hdr_preview_due().is_err(), "the Library's Loupe");
    e.module = Module::Develop;

    e.document.edit.setup_mut().ai_denoise.enabled = true;
    assert!(e.hdr_preview_due().is_err(), "AI Denoise");
    e.document.edit.setup_mut().ai_denoise.enabled = false;

    e.document.edit.setup_mut().contrast = 20.;
    assert!(e.hdr_preview_due().is_err(), "refused in HDR");
    e.document.edit.setup_mut().contrast = 0.;
    assert!(e.hdr_preview_due().is_ok());
}
