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
        pixels: vec![[0.1; 3]; 96].into(),
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
        pixels: vec![[0.1; 3]; 1_000_000].into(),
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
    // From Fit, Cmd+- steps below Fit down to 6.25%, and Cmd+= stops back at Fit.
    let (on, level) = press(egui::Key::Minus, egui::Key::Minus, cmd);
    assert!(on && (level - 0.2 / std::f32::consts::SQRT_2).abs() < 1e-4);
    for _ in 0..8 {
        press(egui::Key::Minus, egui::Key::Minus, cmd);
    }
    assert_eq!(
        press(egui::Key::Minus, egui::Key::Minus, cmd),
        (true, 0.0625)
    );
    // Stepping back up from 6.25% reaches Fit and stops there.
    let mut hit_fit = false;
    for _ in 0..8 {
        if !press(egui::Key::Equals, egui::Key::Equals, cmd).0 {
            hit_fit = true;
            break;
        }
    }
    assert!(hit_fit, "Cmd+= from below Fit stops at Fit");
}

/// Crop stays open while zoomed out below Fit, and closes once zoomed in past Fit.
#[test]
fn crop_stays_open_zoomed_out_and_closes_when_zoomed_in() {
    use crate::app::develop::state::Tool;
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
        pixels: vec![[0.1; 3]; 1_000_000].into(),
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
    editor.view.viewport = Vec2::splat(400.); // Fit is 0.4.
    editor.view.last_fit = Some(0.4);
    editor.view.toggle(Tool::Crop);
    assert!(editor.view.is(Tool::Crop));
    // Zooming out below Fit keeps Crop open.
    editor.step_zoom(-1);
    assert!(editor.view.zoom.on);
    assert!(editor.view.zoom.level < 0.4);
    assert!(editor.view.is(Tool::Crop));
    // Re-opening Crop while zoomed out keeps the zoomed-out view.
    editor.view.toggle(Tool::Crop);
    assert!(!editor.view.is(Tool::Crop));
    editor.view.toggle(Tool::Crop);
    assert!(editor.view.is(Tool::Crop));
    assert!(editor.view.zoom.on && editor.view.zoom.level < 0.4);
    // Zooming in past Fit closes Crop.
    editor.set_zoom(1.);
    assert!(!editor.view.is(Tool::Crop));
    // Opening Crop while zoomed in returns to Fit.
    editor.view.toggle(Tool::Crop);
    assert!(editor.view.is(Tool::Crop));
    assert!(!editor.view.zoom.on);
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
        pixels: vec![[0.1; 3]; 160000].into(),
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
        pixels: vec![[0.2; 3]; 120_000].into(),
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

/// Visualize HDR's overlay is due over an HDR edit whose master renders, whatever
/// the display: on an SDR display, under Preview for SDR display and beside a
/// clipping warning too, keyed without the display's headroom; the whole photo in
/// Fit, a 100% region over it zoomed in. It isn't while it is off, HDR is off, the
/// master is refused or the camera's thumbnail shows.
#[test]
fn the_hdr_overlay_is_due_wherever_the_master_renders() {
    use crate::app::develop::before_after::Compare;
    use crate::app::develop::hdr_preview::{Key, Part};
    use crate::app::shell::display::HdrDisplay;
    let ctx = egui::Context::default();
    let (mut e, _) = crate::app::testing::editor_with_blue_photo(&ctx, Default::default(), true);
    e.preview.last_fit_edge = 64;
    e.view.viewport = egui::Vec2::new(16., 12.);
    e.document.edit.setup_mut().hdr.enabled = true;
    assert_eq!(e.hdr_overlay_due().unwrap_err(), "Visualize HDR is off");
    e.view.visualize_hdr = true;
    let key = |e: &crate::app::Editor, part| {
        Key::new(
            e.document.full().unwrap(),
            &e.effective_recipe(),
            None,
            part,
            1.,
        )
    };
    // An SDR display: no master shows, the overlay does.
    assert!(e.hdr_preview_due().is_err());
    let due = e
        .hdr_overlay_due()
        .expect("over an HDR edit on an SDR display");
    assert_eq!(due.key, key(&e, Part::Whole(16)));
    e.view.hdr_display = HdrDisplay {
        headroom: 4.,
        potential: 16.,
        float_surface: true,
        ..HdrDisplay::default()
    };
    assert_eq!(
        e.hdr_overlay_due().unwrap().key,
        due.key,
        "the display changes nothing"
    );
    e.view.sdr_preview = true;
    assert!(e.hdr_overlay_due().is_ok(), "Preview for SDR display");
    e.view.sdr_preview = false;
    e.view.zoom.set(1.);
    let region = e.region().expect("a 100% region");
    let due = e.hdr_overlay_due().expect("at 100%");
    assert_eq!(due.key, key(&e, Part::Region(region)));
    assert_eq!(due.under, Some(key(&e, Part::Whole(16))));
    e.view.zoom.set(0.);

    e.view.compare = Compare::CameraThumbnail;
    assert!(e.hdr_overlay_due().is_err(), "the camera's thumbnail");
    e.view.compare = Compare::Off;
    e.document.edit.setup_mut().highlights = 20.;
    assert!(e.hdr_overlay_due().is_err(), "refused in HDR");
    e.document.edit.setup_mut().highlights = 0.;
    e.document.edit.setup_mut().hdr.enabled = false;
    assert_eq!(e.hdr_overlay_due().unwrap_err(), "HDR is off");
}

/// Develop is due to show an HDR edit's master on a display with headroom: the whole
/// photo in Fit and zoomed out, a 100% region over it zoomed in, of Before alone, and
/// of each side of Before/After. The SDR rendition shows where the display has none,
/// the edit is SDR or refused in HDR, the camera's thumbnail shows, or a visualization
/// is drawn into the render.
#[test]
fn the_hdr_master_is_due_in_every_view_of_an_hdr_edit() {
    use crate::app::develop::before_after::{Axis, Compare};
    use crate::app::develop::clipping::ClipSide;
    use crate::app::develop::hdr_preview::{Key, Part, Presentation};
    use crate::app::shell::display::HdrDisplay;
    let ctx = egui::Context::default();
    let (mut e, _) = crate::app::testing::editor_with_blue_photo(&ctx, Default::default(), true);
    e.preview.last_fit_edge = 64;
    e.view.viewport = egui::Vec2::new(16., 12.);
    let hdr = HdrDisplay {
        headroom: 4.,
        potential: 16.,
        float_surface: true,
        ..HdrDisplay::default()
    };
    e.view.hdr_display = hdr;
    assert_eq!(e.hdr_preview_due().unwrap_err(), "HDR is off");
    e.document.edit.setup_mut().hdr.enabled = true;
    let key = |e: &crate::app::Editor, part| {
        Key::new(
            e.document.full().unwrap(),
            &e.effective_recipe(),
            None,
            part,
            4.,
        )
    };
    let due = e.hdr_preview_due().expect("an HDR edit in Fit");
    assert_eq!(e.hdr_presentation(), Presentation::Rendering);
    assert_eq!(due.key, key(&e, Part::Whole(16)));
    assert!(due.under.is_none());

    // Zoomed out, the whole photo at the zoomed size; at 100%, the region rendered
    // over the whole photo at its Fit size.
    e.view.zoom.set(0.75);
    assert_eq!(e.hdr_preview_due().unwrap().key, key(&e, Part::Whole(24)));
    e.view.zoom.set(1.);
    let region = e.region().expect("a 100% region");
    let due = e.hdr_preview_due().expect("an HDR edit at 100%");
    assert_eq!(due.key, key(&e, Part::Region(region)));
    assert_eq!(due.under, Some(key(&e, Part::Whole(16))));
    e.view.zoom.set(0.);

    e.view.hdr_display = HdrDisplay {
        potential: 1.,
        ..hdr
    };
    assert!(e.hdr_preview_due().is_err(), "an SDR display");
    e.view.hdr_display = hdr;

    // Before alone is Before's own master, or its SDR rendition.
    let mut before = e.document.edit.recipe().clone();
    before.hdr.enabled = false;
    e.document.before = Some(before.clone());
    e.view.compare = Compare::BeforeOnly;
    assert_eq!(e.hdr_preview_due().unwrap_err(), "HDR is off");
    before.hdr.enabled = true;
    e.document.before = Some(before.clone());
    assert!(e.hdr_preview_due().is_ok(), "Before in HDR");

    // Beside the edit, each side is its own.
    e.view.compare = Compare::SideBySide(Axis::LeftRight);
    assert!(e.hdr_preview_due().is_ok(), "After beside Before");
    assert!(e.hdr_second_side_due().is_err(), "Before not yet asked for");
    e.schedule_before();
    let side = e.hdr_second_side_due().expect("Before beside the edit");
    assert_eq!(side.key.part(), Part::Whole(16));
    before.hdr.enabled = false;
    e.document.before = Some(before);
    e.schedule_before();
    assert_eq!(e.hdr_second_side_due().unwrap_err(), "HDR is off");
    assert!(e.hdr_preview_due().is_ok(), "After stays HDR");
    // The Basic panel names the side showing its SDR rendition, and why.
    assert_eq!(
        e.hdr_second_side_presentation(),
        Some(("Before", Presentation::Sdr("HDR is off")))
    );
    e.view.compare = Compare::BeforeOnly;
    assert_eq!(e.hdr_second_side_presentation(), None);

    e.view.compare = Compare::CameraThumbnail;
    assert!(e.hdr_preview_due().is_err(), "the camera's thumbnail");
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

    // AI Denoise previews in HDR, keyed by the result the SDR rendition shows: each
    // result, partial or final, renders the master again.
    e.document.edit.setup_mut().ai_denoise.enabled = true;
    let plain = e.hdr_preview_due().expect("AI Denoise").key;
    let image = e.document.full().unwrap().clone();
    let source =
        crate::develop::AiSource::recover((*image).clone(), rawmakase_cancel::Cancel::NEVER)
            .unwrap();
    e.ai_denoise_update(crate::ai_denoise::Update {
        plain: std::sync::Arc::downgrade(&image),
        state: crate::ai_denoise::State::Partial(source),
    });
    let partial = e.hdr_preview_due().expect("a partial result").key;
    assert_ne!(partial, plain, "a result is a new render");
    assert_eq!(
        e.hdr_histogram_due().map(|(k, _)| k),
        Ok(crate::app::develop::hdr_histogram::Key::new(
            &image,
            &e.effective_recipe(),
            e.document.ai.source(),
        )),
        "the histogram follows the result too"
    );
    e.document.edit.setup_mut().ai_denoise.enabled = false;
    assert_eq!(e.hdr_preview_due().unwrap().key, key(&e, Part::Whole(16)));

    // Contrast renders in HDR; Highlights does not yet.
    e.document.edit.setup_mut().contrast = 20.;
    assert!(e.hdr_preview_due().is_ok(), "Contrast");
    e.document.edit.setup_mut().contrast = 0.;
    e.document.edit.setup_mut().highlights = 20.;
    assert!(e.hdr_preview_due().is_err(), "refused in HDR");
    e.document.edit.setup_mut().highlights = 0.;
    assert!(e.hdr_preview_due().is_ok());
}

/// The HDR readout reads a 100% region where the HDR preview draws it, by its own
/// key's region, not where the SDR rendition's last region sits: the two workers
/// finish in either order, so after a pan the HDR region can land first.
#[test]
fn the_hdr_readout_reads_the_region_where_the_hdr_preview_draws_it() {
    use crate::app::develop::hdr_preview::{Part, Slot};
    use crate::app::develop::state::TextureMode;
    use crate::app::shell::display::HdrDisplay;
    let ctx = egui::Context::default();
    let (mut e, _) = crate::app::testing::editor_with_blue_photo(&ctx, Default::default(), true);
    e.preview.last_fit_edge = 64;
    e.view.viewport = egui::Vec2::new(16., 12.);
    e.view.hdr_display = HdrDisplay {
        headroom: 4.,
        potential: 16.,
        float_surface: true,
        ..HdrDisplay::default()
    };
    e.document.edit.setup_mut().hdr.enabled = true;
    e.view.zoom.set(1.);
    let region = e.region().expect("a 100% region");
    assert_ne!(region, [0, 0, 4, 4]);
    // The SDR rendition's last region is elsewhere: it hasn't caught up with a pan.
    e.preview.mode = TextureMode::Region([0, 0, 4, 4]);
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(64., 48.));
    let g = crate::develop::Geometry::new(e.document.full().unwrap(), &e.effective_recipe(), 0);
    let reads = e.hdr_preview_reads(rect);
    assert_eq!(reads.len(), 1, "the whole photo only with the region shown");
    let (slot, key, at) = &reads[0];
    assert_eq!((*slot, key.part()), (Slot::Region, Part::Region(region)));
    assert_eq!(
        *at,
        crate::app::develop::before_after::region_on(rect, [g.width, g.height], region)
    );
    // In Fit, the whole photo where the photo is.
    e.view.zoom.set(0.);
    let reads = e.hdr_preview_reads(rect);
    assert_eq!(reads.len(), 1);
    assert_eq!((reads[0].0, reads[0].2), (Slot::Whole, rect));
}

/// An HDR edit shown as its SDR rendition because of the display waits for the
/// display: a Wayland display that turns HDR on wakes no idle window, so the window
/// keeps rechecking while one is open, and only then.
#[test]
fn an_hdr_edit_refused_by_the_display_waits_for_it() {
    use crate::app::shell::display::HdrDisplay;
    let ctx = egui::Context::default();
    let (mut e, _) = crate::app::testing::editor_with_blue_photo(&ctx, Default::default(), true);
    e.preview.last_fit_edge = 64;
    e.view.hdr_display = HdrDisplay::default();
    assert!(!e.hdr_awaits_display(), "HDR is off");
    e.document.edit.setup_mut().hdr.enabled = true;
    assert!(e.hdr_awaits_display(), "an HDR edit on an SDR display");
    e.view.hdr_display = HdrDisplay {
        headroom: 4.,
        potential: 4.,
        float_surface: true,
        ..HdrDisplay::default()
    };
    assert!(!e.hdr_awaits_display(), "shown in HDR");
}

/// An editor showing an HDR edit in Fit on a display with headroom 4.
fn hdr_editor(ctx: &egui::Context) -> crate::app::Editor {
    use crate::app::shell::display::HdrDisplay;
    let (mut e, _) = crate::app::testing::editor_with_blue_photo(ctx, Default::default(), true);
    e.preview.last_fit_edge = 64;
    e.view.viewport = egui::Vec2::new(16., 12.);
    e.view.hdr_display = HdrDisplay {
        headroom: 4.,
        potential: 16.,
        float_surface: true,
        ..HdrDisplay::default()
    };
    e.document.edit.setup_mut().hdr.enabled = true;
    e
}

/// The GPU's HDR master, presented with the picture the viewport draws, shows while
/// the master is due at the headroom it was presented for, and the Basic panel says
/// so; at another headroom the CPU's renders, and the GPU's texture is kept from
/// being written while it is drawn.
#[test]
fn the_gpus_hdr_master_shows_where_it_was_presented_at_the_headroom_due() {
    use crate::app::develop::hdr_preview::Presentation;
    use crate::app::develop::state::Picture;
    use crate::app::worker::HdrTexture;
    let ctx = egui::Context::default();
    let mut e = hdr_editor(&ctx);
    let headroom = e.hdr_preview_due().unwrap().key.headroom();
    let hdr = HdrTexture {
        id: egui::TextureId::User(2),
        size: [16, 12],
        headroom,
    };
    e.preview.texture = Some(Picture::presented_with(
        egui::TextureId::User(1),
        [16, 12],
        Some(hdr),
    ));
    assert_eq!(e.hdr_presentation(), Presentation::Hdr { stops: 2. });
    let drawn = e.preview.presented();
    assert!(drawn.contains(&egui::TextureId::User(1)) && drawn.contains(&hdr.id));
    // Brighter: the display's headroom moved; until the GPU presents for it, the CPU's.
    e.view.hdr_display.headroom = 8.;
    assert_eq!(e.hdr_presentation(), Presentation::Rendering);
    e.view.hdr_display.headroom = 4.;
    // Not due (the clipping warning): the SDR rendition, whatever was presented.
    e.view
        .clipping
        .toggle(crate::app::develop::clipping::ClipSide::Highlights);
    assert!(matches!(e.hdr_presentation(), Presentation::Sdr(_)));
    e.view
        .clipping
        .toggle(crate::app::develop::clipping::ClipSide::Highlights);
    // An SDR picture alone: the CPU's master.
    e.preview.texture = Some(Picture::presented(egui::TextureId::User(1), [16, 12]));
    assert_eq!(e.hdr_presentation(), Presentation::Rendering);

    // Drawn by the GPU, the master asks for EDR without a CPU render.
    let g = crate::develop::Geometry::new(e.document.full().unwrap(), &e.effective_recipe(), 0);
    let mut output = ctx.run_ui(Default::default(), |ui| {
        let painter = ui.painter().clone();
        let gpu = crate::app::develop::viewport::GpuHdr {
            whole: true,
            region: false,
        };
        let due = e.hdr_preview_due();
        e.hdr_preview_ui(&painter, ui.max_rect(), &g, due, false, gpu);
    });
    output.textures_delta.clear();
    assert!(e.view.hdr_shown);
}

/// An HDR edit's render asks the GPU for its master at the headroom due, and renders
/// again when the display's headroom moves or the master stops being due; nothing
/// else renders it again.
#[test]
fn an_hdr_edits_render_asks_for_the_master_at_the_headroom_due() {
    let ctx = egui::Context::default();
    let mut e = hdr_editor(&ctx);
    e.schedule();
    let asked = e.preview.last_hdr.expect("the master is due");
    assert!((asked - 4.).abs() < 0.1);
    let id = e.preview.task.id();
    e.follow_hdr_display();
    assert_eq!(e.preview.task.id(), id, "nothing changed");
    e.view.hdr_display.headroom = 4.1;
    e.follow_hdr_display();
    assert_eq!(e.preview.task.id(), id, "within a tenth of a stop");
    e.view.hdr_display.headroom = 8.;
    e.follow_hdr_display();
    assert_ne!(e.preview.task.id(), id);
    assert!((e.preview.last_hdr.unwrap() - 8.).abs() < 0.1);
    e.document.edit.setup_mut().hdr.enabled = false;
    e.follow_hdr_display();
    assert_eq!(e.preview.last_hdr, None, "an SDR edit asks for no master");
}

/// Preview for SDR display shows an HDR edit's SDR rendition on an HDR display, as
/// Lightroom's does: no master is due on either side, the histogram is the SDR
/// rendition's and the window doesn't wait for the display. It is a view choice: the
/// edit is the same with it on and off, and turning it off shows the master again.
#[test]
fn preview_for_sdr_display_shows_the_sdr_rendition_and_leaves_the_edit() {
    use crate::app::develop::before_after::{Axis, Compare};
    use crate::app::develop::hdr_histogram::Source;
    use crate::app::develop::hdr_preview::Presentation;
    use crate::app::develop::viewport::SDR_PREVIEW;
    let ctx = egui::Context::default();
    let mut e = hdr_editor(&ctx);
    assert!(e.hdr_preview_due().is_ok());
    let recipe = e.document.edit.recipe().clone();
    e.view.sdr_preview = true;
    assert_eq!(e.hdr_preview_due().unwrap_err(), SDR_PREVIEW);
    assert_eq!(e.hdr_presentation(), Presentation::Sdr(SDR_PREVIEW));
    assert!(matches!(e.histogram_source(), Source::Sdr(Some(why)) if why == SDR_PREVIEW));
    assert!(e.hdr_histogram_master().is_none());
    e.document.before = Some(recipe.clone());
    e.view.compare = Compare::SideBySide(Axis::LeftRight);
    e.schedule_before();
    assert_eq!(e.hdr_second_side_due().unwrap_err(), SDR_PREVIEW);
    e.view.compare = Compare::Off;
    // An SDR display: nothing to wait for while the SDR rendition is asked for.
    e.view.hdr_display = Default::default();
    assert!(!e.hdr_awaits_display());
    assert_eq!(
        e.document.edit.recipe(),
        &recipe,
        "a view choice, not an edit"
    );
    e.view.sdr_preview = false;
    assert!(e.hdr_awaits_display());
    e.view.hdr_display = hdr_editor(&ctx).view.hdr_display;
    assert!(e.hdr_preview_due().is_ok());
}

/// A Sony shot at 16:9 embeds a 16:9 JPEG, its aspect-ratio crop of the 3:2
/// sensor (#1256). Until the first render, that JPEG fills the photo's crop
/// rather than being stretched over the whole frame and clipped to the crop;
/// a camera JPEG of the whole frame still sits on the whole frame. So it does
/// while the RAW decodes, its frame known from the metadata alone.
#[test]
fn the_camera_jpeg_sits_on_the_frame_it_shows() {
    // 16:9 of a 3:2 frame: the middle 27/32 of its height.
    let wide = [0., 5. / 64., 1., 59. / 64.];
    let cases = [([160, 90], wide), ([150, 100], [0., 0., 1., 1.])];
    for (decoded, (jpeg, frame)) in [true, false]
        .into_iter()
        .flat_map(|d| cases.map(|c| (d, c)))
    {
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        e.module = Module::Develop;
        let (load, _) = e.document.start_load();
        let metadata = Metadata {
            width: 600,
            height: 400,
            wb: [1.; 3],
            make: "Sony".into(),
            camera_crop: Some(wide),
            ..Default::default()
        };
        if decoded {
            e.document.set_image(Arc::new(CameraImage {
                recovered: Default::default(),
                width: 600,
                height: 400,
                pixels: vec![[0.2; 3]; 240_000].into(),
                metadata,
                fast: false,
                scale_factor: 1.,
                scale_clipped: 0,
                hdr: Default::default(),
            }));
        } else {
            e.document.metadata = Some(metadata);
        }
        e.document.edit.setup_mut().crop = wide;
        e.tx.send(crate::app::worker::Event::Embedded {
            id: load,
            image: image::RgbImage::new(jpeg[0], jpeg[1]),
        })
        .unwrap();
        e.events(&ctx);
        assert!(e.preview.embedded);
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400., 400.))),
                ..Default::default()
            },
            |ui| {
                e.viewport_ui(ui);
            },
        );
        output.textures_delta.clear();
        let texture = e.preview.texture.as_ref().unwrap().id();
        let drawn = output
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::epaint::Shape::Mesh(mesh) if mesh.texture_id == texture => Some(mesh),
                _ => None,
            })
            .expect("the camera JPEG is drawn")
            .calc_bounds();
        let crop = e.view.shown_rect.expect("the photo is drawn");
        let size = crop.size() / Vec2::new(wide[2] - wide[0], wide[3] - wide[1]);
        let whole = crop.min - Vec2::new(wide[0], wide[1]) * size;
        let want = Rect::from_min_max(
            whole + Vec2::new(frame[0], frame[1]) * size,
            whole + Vec2::new(frame[2], frame[3]) * size,
        );
        assert!(
            (drawn.min - want.min).length() < 0.5 && (drawn.max - want.max).length() < 0.5,
            "a {jpeg:?} JPEG drawn at {drawn:?}, not {want:?} (decoded: {decoded})"
        );
    }
}

/// A photo of `width` × `height` pixels, decoded at half size (a RAW's draft)
/// when `draft`; its metadata gives the full size.
fn photo_of(width: u32, height: u32, draft: bool) -> (Metadata, Arc<CameraImage>) {
    let metadata = Metadata {
        width,
        height,
        ..crate::app::testing::blue_image().metadata.clone()
    };
    let (w, h) = if draft {
        (width / 2, height / 2)
    } else {
        (width, height)
    };
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: w,
        height: h,
        pixels: vec![[0.1; 3]; (w * h) as usize].into(),
        metadata: metadata.clone(),
        fast: draft,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    });
    (metadata, image)
}

/// Where a frame drew `texture`, and the zoom slider's knob.
struct Drawn {
    texture: Option<Rect>,
    knob: f32,
}

fn draw_frame(e: &mut Editor, texture: Option<egui::TextureId>) -> Drawn {
    let ctx = e.context.clone();
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
            ..Default::default()
        },
        |ui| e.draw(ui),
    );
    output.textures_delta.clear();
    let mut drawn = Drawn {
        texture: None,
        knob: f32::NAN,
    };
    let mut knobs = 0;
    for clipped in &output.shapes {
        match &clipped.shape {
            egui::Shape::Mesh(mesh) if Some(mesh.texture_id) == texture => {
                drawn.texture = Some(mesh.calc_bounds());
            }
            egui::Shape::Circle(c) if c.radius == 6. && c.fill == egui::Color32::WHITE => {
                drawn.knob = c.center.x;
                knobs += 1;
            }
            _ => {}
        }
    }
    assert_eq!(knobs, 1, "one zoom slider");
    drawn
}

/// Moving on to the next photo keeps the zoom and where it looks, in Develop as
/// in the Loupe and in Lightroom, rather than going back to Fit.
#[test]
fn moving_on_keeps_the_zoom() -> anyhow::Result<()> {
    let (_dir, _ctx, mut e, [a, b]) = crate::app::testing::two_photo_editor()?;
    e.module = Module::Develop;
    e.develop_catalog_photo(a);
    e.set_zoom(1.);
    e.view.zoom.pan = [0.3, 0.6];
    e.develop_catalog_photo(b);
    assert_eq!(e.document.catalog_photo, Some(b));
    assert!(e.view.zoom.on);
    assert_eq!(e.view.zoom.level, 1.);
    assert_eq!(e.view.zoom.pan, [0.3, 0.6]);
    Ok(())
}

/// The next photo opened zoomed in keeps the slider's fit until its own is
/// known; a photo closed lets go of it.
#[test]
fn the_last_fit_goes_with_the_photo_closed() {
    let mut view = crate::app::develop::state::ViewState {
        last_fit: Some(0.25),
        ..Default::default()
    };
    view.next_document(true);
    assert_eq!(view.last_fit, Some(0.25));
    view.clear_document();
    assert_eq!(view.last_fit, None);
}

/// Crop open in Develop, then zoomed in in the Loupe (the zoom is shared): the
/// next photo opens zoomed in, with Crop closed, as one cannot crop zoomed in.
#[test]
fn the_next_photo_opened_zoomed_in_closes_crop() -> anyhow::Result<()> {
    use crate::app::develop::state::Tool;
    let (_dir, _ctx, mut e, [a, b]) = crate::app::testing::two_photo_editor()?;
    e.module = Module::Develop;
    e.develop_catalog_photo(a);
    e.view.set_tool(Tool::Crop);
    e.view.zoom.set(1.);
    e.develop_catalog_photo(b);
    assert_eq!(e.document.catalog_photo, Some(b));
    assert!(e.view.zoom.on);
    assert!(!e.view.is(Tool::Crop));
    // At Fit, Crop stays open from photo to photo.
    e.view.zoom.set(0.);
    e.view.set_tool(Tool::Crop);
    e.develop_catalog_photo(a);
    assert!(e.view.is(Tool::Crop));
    Ok(())
}

/// Zoomed in, a photo that couldn't be opened before its size was read shows
/// its preview behind the message, at Fit, rather than nothing.
#[test]
fn a_photo_that_fails_to_open_zoomed_in_shows_its_preview() -> anyhow::Result<()> {
    let (_dir, ctx, mut e, [a, b]) = crate::app::testing::two_photo_editor()?;
    e.module = Module::Develop;
    e.develop_catalog_photo(a);
    e.set_zoom(1.);
    e.develop_catalog_photo(b);
    e.document.invalidate_load();
    let preview = egui::ColorImage::new([16, 12], vec![egui::Color32::GRAY; 16 * 12]);
    crate::app::develop::state::Picture::upload(&mut e.preview.stand_in, &ctx, "stand-in", preview);
    let preview = e.preview.stand_in.as_ref().map(|p| p.id());
    assert_eq!(draw_frame(&mut e, preview).texture, None, "still opening");
    e.document.failed = Some("unreadable".into());
    let drawn = draw_frame(&mut e, preview).texture.expect("the preview");
    assert!(
        drawn.width() <= 1200. && drawn.height() <= 800.,
        "at Fit: {drawn:?}"
    );
    Ok(())
}

/// Zoomed in, the camera's JPEG, which a photo on a network volume brings before
/// its size is read, isn't shown at Fit meanwhile: it shows once it can be
/// placed at the zoom level.
#[test]
fn zoomed_in_the_camera_jpeg_waits_for_the_photo_size() -> anyhow::Result<()> {
    let (_dir, ctx, mut e, [a, b]) = crate::app::testing::two_photo_editor()?;
    e.module = Module::Develop;
    e.develop_catalog_photo(a);
    e.set_zoom(1.);
    e.develop_catalog_photo(b);
    let (load, _) = e.document.start_load();
    e.tx.send(Event::Embedded {
        id: load,
        image: image::RgbImage::new(160, 120),
    })
    .unwrap();
    e.events(&ctx);
    let jpeg = e.preview.texture.as_ref().map(|t| t.id());
    assert!(jpeg.is_some() && e.preview.embedded);
    assert_eq!(draw_frame(&mut e, jpeg).texture, None, "the JPEG at Fit");
    let (metadata, _) = photo_of(1600, 1200, false);
    e.document.metadata = Some(metadata);
    let drawn = draw_frame(&mut e, jpeg).texture.expect("the JPEG");
    assert_eq!(drawn.size(), Vec2::new(1600., 1200.));
    Ok(())
}

/// Zoomed in, a 100% region rendered from a RAW's half-size draft stays where
/// it shows when the full-size image replaces the draft, until a render of
/// that lands: its pixels are the draft's, not the full-size image's.
#[test]
fn a_draft_region_stays_in_place_when_the_full_size_image_arrives() -> anyhow::Result<()> {
    let (_dir, ctx, mut e, [a, _]) = crate::app::testing::two_photo_editor()?;
    e.module = Module::Develop;
    // Only the results sent here arrive: the renderer takes a job and holds it.
    crate::app::task::wait_for(vec![e.renderer.stop()], std::time::Duration::from_secs(5));
    let (taken_tx, taken) = std::sync::mpsc::channel();
    let (_release, release) = std::sync::mpsc::channel();
    e.renderer = crate::app::worker::holding(taken_tx, release);
    e.develop_catalog_photo(a);
    e.document.invalidate_load();
    let (metadata, draft) = photo_of(1600, 1200, true);
    e.document.metadata = Some(metadata);
    e.document.set_image(draft);
    e.set_zoom(1.);
    draw_frame(&mut e, None);
    draw_frame(&mut e, None);
    let id = e.preview.task.id();
    assert!(matches!(
        e.preview.pending.mode,
        crate::app::develop::state::TextureMode::Region(_)
    ));
    while taken.recv_timeout(std::time::Duration::from_secs(5))? != id {}
    let [w, h] = match e.preview.pending.mode {
        crate::app::develop::state::TextureMode::Region([_, _, w, h]) => [w, h],
        _ => unreachable!(),
    };
    e.tx.send(Event::Rendered {
        id,
        pane: crate::app::worker::Pane::After,
        preview: crate::app::worker::Preview::Pixels {
            image: crate::rendered::Rendered {
                width: w,
                height: h,
                pixels: vec![[0.5; 3]; (w * h) as usize],
            },
            display_rgb: vec![128; (w * h * 3) as usize],
            navigator: None,
        },
        histogram: Box::new(crate::rendered::Histogram::EMPTY),
        thumbnail: None,
        samples: None,
        stage: crate::app::worker::RenderStage::Fit,
        status: "draft".into(),
    })
    .unwrap();
    e.events(&ctx);
    let region = e.preview.region.as_ref().map(|p| p.id());
    let drafted = draw_frame(&mut e, region).texture.expect("the region");
    let (_, full) = photo_of(1600, 1200, false);
    e.document.set_image(full);
    let replaced = draw_frame(&mut e, region).texture.expect("the region");
    assert_eq!(replaced, drafted);
    Ok(())
}

/// Zoomed in, the next photo shows zoomed from its first frame: its stored
/// preview while it opens, then a RAW's half-size draft, enlarged to the level
/// chosen, never at Fit or at twice the scale; and the zoom slider holds still
/// throughout, 100% counted in the full-size photo's pixels.
#[test]
fn the_next_photo_shows_at_the_zoom_from_its_first_frame() -> anyhow::Result<()> {
    let (_dir, ctx, mut e, [a, b]) = crate::app::testing::two_photo_editor()?;
    e.module = Module::Develop;
    // What the loads bring (the fixtures don't decode) is set by hand instead.
    e.develop_catalog_photo(a);
    e.document.invalidate_load();
    let (metadata, image) = photo_of(1600, 1200, false);
    e.document.metadata = Some(metadata);
    e.document.set_image(image);
    e.set_pixels(&ctx, false, [16, 12], &[90; 16 * 12 * 3], None, None);
    e.set_zoom(1.);
    // The second frame's toolbar knows the size of the view the first laid out.
    draw_frame(&mut e, None);
    let photo = draw_frame(&mut e, None);
    assert_eq!(e.view.shown_rect.unwrap().size(), Vec2::new(1600., 1200.));
    let knob = photo.knob;

    e.develop_catalog_photo(b);
    e.document.invalidate_load();
    let stand_in = egui::ColorImage::new([16, 12], vec![egui::Color32::GRAY; 16 * 12]);
    crate::app::develop::state::Picture::upload(
        &mut e.preview.stand_in,
        &ctx,
        "stand-in",
        stand_in,
    );
    let stand_in = e.preview.stand_in.as_ref().map(|p| p.id());
    // Its size not yet read: no preview at Fit, and the knob stays.
    let opening = draw_frame(&mut e, stand_in);
    assert_eq!(opening.texture, None, "the preview at Fit");
    assert_eq!(opening.knob, knob);
    // Its metadata read: the preview at 100% of the full-size photo.
    let (metadata, full) = photo_of(1600, 1200, false);
    e.document.metadata = Some(metadata);
    let read = draw_frame(&mut e, stand_in);
    assert_eq!(read.texture.unwrap().size(), Vec2::new(1600., 1200.));
    assert_eq!(read.knob, knob);
    // The half-size draft, then the full-size image: the same.
    let (_, draft) = photo_of(1600, 1200, true);
    e.document.set_image(draft);
    e.set_pixels(&ctx, false, [16, 12], &[90; 16 * 12 * 3], None, None);
    let shown = draw_frame(&mut e, None);
    assert_eq!(e.view.shown_rect.unwrap().size(), Vec2::new(1600., 1200.));
    assert_eq!(shown.knob, knob);
    let viewport = e.view.viewport;
    assert_eq!(
        e.fit_level(),
        Some((viewport.x / 1600.).min(viewport.y / 1200.))
    );
    e.document.set_image(full);
    let full = draw_frame(&mut e, None);
    assert_eq!(e.view.shown_rect.unwrap().size(), Vec2::new(1600., 1200.));
    assert_eq!(full.knob, knob);
    Ok(())
}

/// The turns between two orientations are counted as the photo is shown: under a
/// single mirror, Rotate Right lowers the recorded turns and still shows clockwise.
#[test]
fn turns_between_orientations_count_as_shown() {
    use crate::app::develop::state::Orientation;
    use crate::develop::{QuarterTurn, turn};
    for flips in [(false, false), (true, false), (false, true), (true, true)] {
        let mut r = crate::model::recipe::Recipe {
            flip_x: flips.0,
            flip_y: flips.1,
            ..Default::default()
        };
        let then = Orientation::of(&r);
        turn(&mut r, QuarterTurn::Right);
        assert_eq!(then.turns_to(Orientation::of(&r)), Some(1), "{flips:?}");
        turn(&mut r, QuarterTurn::Right);
        turn(&mut r, QuarterTurn::Right);
        assert_eq!(then.turns_to(Orientation::of(&r)), Some(3), "{flips:?}");
        r.flip_x = !r.flip_x;
        assert_eq!(then.turns_to(Orientation::of(&r)), None, "{flips:?}");
    }
}
