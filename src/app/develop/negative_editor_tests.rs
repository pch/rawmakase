//! Editor tests of the Negative panel, its film base picker and its Auto: the
//! whole editor driven through its frames, the panel clicked where it is drawn.
use crate::app::testing::{centre, controls_frame, in_edit_frame, press_and_release};
use crate::app::*;
use crate::camera_data::{CameraImage, Metadata};
use crate::model::negative::{FilmType, NegativeConversion};
use std::sync::Arc;

/// The clear film edge of [`negative_scan`], in camera values.
const BASE: [f32; 3] = [0.8, 0.5, 0.3];

/// A camera scan of a colour negative, 64 × 48: its top 8 rows clipped (the light
/// around the film), the left quarter below them the clear film edge, and the rest
/// a negative of a ramp; `flat` makes it all one value.
fn negative_scan(flat: bool) -> Arc<CameraImage> {
    let (width, height) = (64u32, 48u32);
    let gamma = [0.62, 0.58, 0.6];
    Arc::new(CameraImage {
        recovered: Default::default(),
        width,
        height,
        pixels: (0..width * height)
            .map(|i| {
                let (x, y) = (i % width, i / width);
                if flat {
                    [0.3, 0.2, 0.1]
                } else if y < 8 {
                    [1.; 3]
                } else if x < 16 {
                    BASE
                } else {
                    let t = (x - 16) as f32 / 47. * 0.8 + (y - 8) as f32 / 39. * 0.2;
                    std::array::from_fn(|c| BASE[c] * 10f32.powf(-gamma[c] * (0.2 + 2. * t)))
                }
            })
            .collect(),
        metadata: Metadata {
            width,
            height,
            wb: [1.; 3],
            daylight_wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            cam_xyz: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    })
}

/// An editor in Develop with [`negative_scan`] decoded (unless `decoded` is false)
/// and the Negative panel expanded so its controls are reachable.
fn editor_with_scan(ctx: &egui::Context, flat: bool, decoded: bool) -> Editor {
    let editor = editor_with_scan_collapsed(ctx, flat, decoded);
    expand_negative(ctx);
    editor
}

/// As [`editor_with_scan`], leaving the Negative panel collapsed at its default and
/// folding the panels above Calibration so the bottom of Develop sits inside
/// `controls_frame`'s viewport.
fn editor_with_scan_collapsed(ctx: &egui::Context, flat: bool, decoded: bool) -> Editor {
    ctx.enable_accesskit();
    ctx.all_styles_mut(|s| s.animation_time = 0.);
    let mut session = crate::app::preferences::session::Session::default();
    for title in [
        "Basic",
        "Tone Curve",
        "Color Mixer",
        "Color Grading",
        "Detail",
        "Lens Corrections",
        "Transform",
        "Effects",
    ] {
        session.collapsed.insert(title.into());
    }
    let mut editor = Editor::with_context(ctx, None, session, None);
    editor.module = Module::Develop;
    let image = negative_scan(flat);
    editor.document.metadata = Some(image.metadata.clone());
    if decoded {
        editor.document.set_image(image);
    }
    editor
}

fn expand_negative(ctx: &egui::Context) {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<std::collections::BTreeSet<String>>(
            crate::app::ui::widgets::expanded_sections_id(),
        )
        .insert("Negative".to_string());
    });
}

/// The bottommost node called `label` in `update`: the Negative panel's, which sits
/// last in Develop (below Calibration), where another panel has a control of the same
/// name.
fn node(update: &egui::accesskit::TreeUpdate, label: &str) -> egui::accesskit::Node {
    update
        .nodes
        .iter()
        .map(|(_, n)| n)
        .filter(|n| n.label() == Some(label) && n.bounds().is_some())
        .max_by(|a, b| a.bounds().unwrap().y0.total_cmp(&b.bounds().unwrap().y0))
        .cloned()
        .unwrap_or_else(|| panic!("no {label}"))
}
fn has(update: &egui::accesskit::TreeUpdate, label: &str) -> bool {
    update.nodes.iter().any(|(_, n)| n.label() == Some(label))
}

/// Clicks the control called `label` in the Develop panels, as a pointer does.
fn click(ctx: &egui::Context, editor: &mut Editor, label: &str) {
    let shown = controls_frame(ctx, editor, vec![]);
    let [down, up] = press_and_release(centre(&node(&shown, label)));
    controls_frame(ctx, editor, down);
    controls_frame(ctx, editor, up);
}

/// The last History step applied: its name, value, and how many are applied.
fn last_step(editor: &Editor) -> (String, String, usize) {
    let (steps, applied) = editor.document.edit.history().steps();
    match applied {
        0 => (String::new(), String::new(), 0),
        _ => {
            let step = steps[applied - 1];
            (step.name.clone(), step.value.clone(), applied)
        }
    }
}

/// Runs the events until no Auto is running.
fn finish_auto(ctx: &egui::Context, editor: &mut Editor) {
    let start = std::time::Instant::now();
    while editor.document.auto.is_running() {
        assert!(start.elapsed().as_secs() < 60, "Auto did not finish");
        std::thread::sleep(std::time::Duration::from_millis(5));
        editor.events(ctx);
    }
}

/// Each of the panel's actions is one History step, named "Negative" with what it
/// did; the same film again is none; Reset is one; Undo and Redo step through them.
#[test]
fn each_negative_panel_action_is_one_named_step() {
    let ctx = egui::Context::default();
    let mut editor = editor_with_scan(&ctx, false, true);
    // Off, only the film switch shows.
    let off = controls_frame(&ctx, &mut editor, vec![]);
    assert!(has(&off, "Color") && !has(&off, "Film base picker") && !has(&off, "Auto levels"));
    click(&ctx, &mut editor, "Color");
    let colour = NegativeConversion::new(FilmType::Colour);
    assert_eq!(editor.document.edit.recipe().negative, Some(colour));
    assert_eq!(last_step(&editor), ("Negative".into(), "Color".into(), 1));
    // The picker: one step, the base read as scanned, and the picker put away.
    click(&ctx, &mut editor, "Film base picker");
    assert!(editor.view.is(crate::app::develop::state::Tool::FilmBase));
    assert_eq!(editor.view.loupe_prompt(), "Click the clear film edge");
    in_edit_frame(&ctx, &mut editor, |e| e.pick_film_base(8. / 64., 0.6));
    let picked = editor.document.edit.recipe().negative.clone().unwrap();
    assert!(
        (0..3).all(|c| (picked.base[c] / BASE[c] - 1.).abs() < 1e-5),
        "{picked:?}"
    );
    assert_eq!(
        last_step(&editor),
        ("Negative".into(), "Film Base".into(), 2)
    );
    assert!(editor.view.is(crate::app::develop::state::Tool::None));
    // Black & white keeps the base.
    click(&ctx, &mut editor, "B&W");
    let bw = picked.with_film(FilmType::BlackWhite);
    assert_eq!(editor.document.edit.recipe().negative, Some(bw.clone()));
    assert_eq!(last_step(&editor), ("Negative".into(), "B&W".into(), 3));
    click(&ctx, &mut editor, "B&W");
    assert_eq!(last_step(&editor).2, 3);
    // Auto: the levels alone.
    click(&ctx, &mut editor, "Auto levels");
    assert!(editor.document.auto.is_running());
    finish_auto(&ctx, &mut editor);
    let levelled = editor.document.edit.recipe().negative.clone().unwrap();
    assert_eq!(
        NegativeConversion {
            dmin: bw.dmin,
            dmax: bw.dmax,
            ..levelled
        },
        bw
    );
    assert_ne!((levelled.dmin, levelled.dmax), (bw.dmin, bw.dmax));
    assert_eq!(last_step(&editor), ("Negative".into(), "Auto".into(), 4));
    click(&ctx, &mut editor, "Off");
    assert_eq!(editor.document.edit.recipe().negative, None);
    assert_eq!(last_step(&editor), ("Negative".into(), "Off".into(), 5));
    editor.undo();
    assert_eq!(
        editor.document.edit.recipe().negative,
        Some(levelled.clone())
    );
    editor.undo();
    assert_eq!(editor.document.edit.recipe().negative, Some(bw));
    editor.redo();
    assert_eq!(editor.document.edit.recipe().negative, Some(levelled));
    // Reset turns it off, as one step of its own, which Undo takes back.
    click(&ctx, &mut editor, "Reset Negative");
    assert_eq!(editor.document.edit.recipe().negative, None);
    assert_eq!(
        last_step(&editor),
        ("Reset Negative".into(), String::new(), 5)
    );
    editor.undo();
    assert!(editor.document.edit.recipe().negative.is_some());
}

/// A clipped patch holds no film base: nothing changes, the picker stays out, and
/// the panel and the status line say why. Turning the conversion off, or undoing
/// it, puts the picker away.
#[test]
fn the_film_base_picker_refuses_a_clipped_patch_and_goes_with_the_conversion() {
    let ctx = egui::Context::default();
    let mut editor = editor_with_scan(&ctx, false, true);
    click(&ctx, &mut editor, "Color");
    click(&ctx, &mut editor, "Film base picker");
    let before = editor.document.edit.recipe().clone();
    in_edit_frame(&ctx, &mut editor, |e| e.pick_film_base(8. / 64., 2. / 48.));
    assert_eq!(*editor.document.edit.recipe(), before);
    assert_eq!(last_step(&editor).2, 1);
    assert!(editor.view.is(crate::app::develop::state::Tool::FilmBase));
    assert!(
        editor.messages.line().contains("clipped"),
        "{}",
        editor.messages.line()
    );
    let shown = controls_frame(&ctx, &mut editor, vec![]);
    let text =
        crate::app::develop::negative::refusal_text(crate::develop::FilmBaseRefusal::Clipped);
    assert!(
        shown
            .nodes
            .iter()
            .any(|(_, n)| n.value() == Some(text) || n.label() == Some(text)),
        "the panel says why"
    );
    // Off: the picker goes with the conversion.
    click(&ctx, &mut editor, "Off");
    assert!(editor.view.is(crate::app::develop::state::Tool::None));
    // Undone back on and off again: the picker taken up, then the conversion undone.
    editor.undo();
    click(&ctx, &mut editor, "Film base picker");
    assert!(editor.view.is(crate::app::develop::state::Tool::FilmBase));
    editor.undo();
    in_edit_frame(&ctx, &mut editor, |_| {});
    assert_eq!(editor.document.edit.recipe().negative, None);
    assert!(editor.view.is(crate::app::develop::state::Tool::None));
}

/// Auto measures the frame off the UI thread and keeps edits made while it ran; a
/// failure says why and leaves nothing running; a result for an edit whose
/// conversion changed meanwhile is measured again; one for another photo is
/// dropped. Without a decoded photo, the picker and Auto are off and the panel
/// says why.
#[test]
fn negative_auto_clears_its_busy_state_whatever_its_end() {
    let ctx = egui::Context::default();
    // Not decoded yet: off, and said so.
    let mut editor = editor_with_scan(&ctx, false, false);
    in_edit_frame(&ctx, &mut editor, |e| {
        e.document.edit.recipe_mut().negative = Some(NegativeConversion::new(FilmType::Colour))
    });
    let shown = controls_frame(&ctx, &mut editor, vec![]);
    assert!(
        shown.nodes.iter().any(|(_, n)| {
            n.value()
                .or(n.label())
                .is_some_and(|t| t.contains("once the photo has loaded"))
        }),
        "the panel says why the picker and Auto are off"
    );
    editor.start_auto(worker::AutoKind::Negative);
    assert!(!editor.document.auto.is_running());

    // A flat frame: the error, and nothing left running or changed.
    let mut editor = editor_with_scan(&ctx, true, true);
    in_edit_frame(&ctx, &mut editor, |e| {
        e.document.edit.recipe_mut().negative = Some(NegativeConversion::new(FilmType::Colour))
    });
    let before = editor.document.edit.recipe().clone();
    editor.start_auto(worker::AutoKind::Negative);
    assert!(editor.document.auto.is_running());
    finish_auto(&ctx, &mut editor);
    assert_eq!(*editor.document.edit.recipe(), before);
    assert!(
        editor.messages.line().contains("too flat"),
        "{}",
        editor.messages.line()
    );

    // The base changed while it ran: measured again with it, then applied, with an
    // edit made meanwhile kept.
    let mut editor = editor_with_scan(&ctx, false, true);
    in_edit_frame(&ctx, &mut editor, |e| {
        e.document.edit.recipe_mut().negative = Some(NegativeConversion::new(FilmType::Colour))
    });
    editor.start_auto(worker::AutoKind::Negative);
    in_edit_frame(&ctx, &mut editor, |e| e.pick_film_base(8. / 64., 0.6));
    editor.document.edit.setup_mut().effects.clarity = 0.25;
    finish_auto(&ctx, &mut editor);
    let r = editor.document.edit.recipe().clone();
    let n = r.negative.clone().unwrap();
    assert!((0..3).all(|c| (n.base[c] / BASE[c] - 1.).abs() < 1e-5));
    let image = editor.document.full().unwrap().clone();
    let want = crate::develop::auto_negative(&image, &r, rawmakase_cancel::Cancel::NEVER)
        .unwrap()
        .conversion;
    assert_eq!((n.dmin, n.dmax), (want.dmin, want.dmax));
    assert_eq!(r.effects.clarity, 0.25);
    // A result for another photo is dropped.
    editor
        .tx
        .send(worker::Event::Auto {
            id: editor.document.load_id() + 1,
            kind: worker::AutoKind::Negative,
            result: Ok(Box::new(crate::model::recipe::Recipe {
                negative: Some(NegativeConversion::new(FilmType::Colour)),
                ..r.clone()
            })),
            holder_in_crop: false,
        })
        .unwrap();
    editor.events(&ctx);
    assert_eq!(*editor.document.edit.recipe(), r);
    assert!(!editor.document.auto.is_running());
}

/// Auto black & white measures the photo through its conversion, and the colors kept
/// for it are measured again when the conversion changes, never taken from before.
#[test]
fn the_auto_mix_colors_follow_the_conversion() {
    let ctx = egui::Context::default();
    let mut editor = editor_with_scan(&ctx, false, true);
    let image = editor.document.full().unwrap().clone();
    let plain = editor.document.color_spread().unwrap();
    assert_eq!(plain, crate::develop::ColorSpread::measure(&image, None));
    for film in [FilmType::Colour, FilmType::BlackWhite] {
        let n = NegativeConversion {
            base: BASE,
            ..NegativeConversion::new(film)
        };
        in_edit_frame(&ctx, &mut editor, |e| {
            e.document.edit.recipe_mut().negative = Some(n.clone())
        });
        let converted = editor.document.color_spread().unwrap();
        assert_eq!(
            converted,
            crate::develop::ColorSpread::for_recipe(&image, editor.document.edit.recipe())
        );
        assert_ne!(converted, plain);
        // Changing the crop re-measures the spread for a converted negative.
        in_edit_frame(&ctx, &mut editor, |e| {
            e.document.edit.recipe_mut().crop = [0.25, 0.2, 0.9, 0.9];
        });
        let cropped = editor.document.color_spread().unwrap();
        assert_eq!(
            cropped,
            crate::develop::ColorSpread::for_recipe(&image, editor.document.edit.recipe())
        );
        assert_ne!(cropped, converted);
        in_edit_frame(&ctx, &mut editor, |e| {
            e.document.edit.recipe_mut().crop = [0., 0., 1., 1.];
        });
    }
    in_edit_frame(&ctx, &mut editor, |e| {
        e.document.edit.recipe_mut().negative = None
    });
    assert_eq!(editor.document.color_spread().unwrap(), plain);
}

/// An HDR file's HDR master doesn't render a conversion: the panel says so, where
/// the conversion is set, while HDR is on.
#[test]
fn the_negative_panel_says_an_hdr_files_master_has_no_conversion() {
    let ctx = egui::Context::default();
    let mut editor = editor_with_scan(&ctx, false, true);
    if let Some(m) = editor.document.metadata.as_mut() {
        m.source = crate::camera_data::Source::Rendered;
        m.hdr_source = Some(crate::camera_data::HdrSource {
            transfer: crate::color::hdr_output::HdrTransfer::Pq,
            gain_map: false,
            peak: 4.,
        });
    }
    in_edit_frame(&ctx, &mut editor, |e| {
        let r = e.document.edit.recipe_mut();
        r.negative = Some(NegativeConversion::new(FilmType::BlackWhite));
        r.hdr.enabled = true;
    });
    let says = |shown: &egui::accesskit::TreeUpdate| {
        shown.nodes.iter().any(|(_, n)| {
            n.label()
                .or(n.value())
                .is_some_and(|t| t.contains("HDR master"))
        })
    };
    assert!(says(&controls_frame(&ctx, &mut editor, vec![])));
    in_edit_frame(&ctx, &mut editor, |e| {
        e.document.edit.recipe_mut().hdr.enabled = false
    });
    assert!(!says(&controls_frame(&ctx, &mut editor, vec![])));
}

/// The Negative panel's note when Auto finds part of the film holder in the crop.
const HOLDER_NOTE: &str = "The crop holds part of the film holder";

/// Whether the panel shows a note starting with `start`.
fn says(shown: &egui::accesskit::TreeUpdate, start: &str) -> bool {
    shown.nodes.iter().any(|(_, n)| {
        n.label()
            .or(n.value())
            .is_some_and(|t| t.starts_with(start))
    })
}

/// Runs the Negative panel's Auto to its end.
fn run_negative_auto(ctx: &egui::Context, editor: &mut Editor) {
    editor.start_auto(worker::AutoKind::Negative);
    assert!(editor.document.auto.is_running());
    finish_auto(ctx, editor);
}

/// Auto measures a holder inside the crop as part of the frame, and the panel says
/// so under Levels; the note goes when the crop changes, when Auto runs again without
/// finding it, when the conversion goes, and with the photo. It is no part of the
/// recipe or of History.
#[test]
fn negative_auto_says_when_the_crop_holds_the_holder() {
    let ctx = egui::Context::default();
    let mut editor = editor_with_scan(&ctx, false, false);
    // The scan with a holder along its left edge, which records no light.
    let mut image = (*negative_scan(false)).clone();
    for (i, p) in image.pixels.iter_mut().enumerate() {
        if i % 64 < 6 {
            *p = [0.0005; 3];
        }
    }
    editor.document.set_image(Arc::new(image));
    in_edit_frame(&ctx, &mut editor, |e| {
        e.document.edit.recipe_mut().negative = Some(NegativeConversion {
            base: BASE,
            ..NegativeConversion::new(FilmType::BlackWhite)
        })
    });
    assert!(!says(
        &controls_frame(&ctx, &mut editor, vec![]),
        HOLDER_NOTE
    ));
    run_negative_auto(&ctx, &mut editor);
    let r = editor.document.edit.recipe().clone();
    let image = editor.document.full().unwrap().clone();
    let measured =
        crate::develop::auto_negative(&image, &r, rawmakase_cancel::Cancel::NEVER).unwrap();
    assert!(measured.holder_in_crop);
    // The levels are the crop's as it is.
    let n = r.negative.as_ref().unwrap();
    assert_eq!(
        (n.dmin, n.dmax),
        (measured.conversion.dmin, measured.conversion.dmax)
    );
    assert!(says(
        &controls_frame(&ctx, &mut editor, vec![]),
        HOLDER_NOTE
    ));
    let steps = editor.document.edit.history().steps().1;
    // Another crop: gone, with no step of its own.
    in_edit_frame(&ctx, &mut editor, |e| {
        e.document.edit.recipe_mut().crop = [0.2, 0., 1., 1.]
    });
    assert!(!says(
        &controls_frame(&ctx, &mut editor, vec![]),
        HOLDER_NOTE
    ));
    assert_eq!(editor.document.edit.history().steps().1, steps + 1);
    // Auto again, the holder cropped out: no note.
    run_negative_auto(&ctx, &mut editor);
    assert!(!says(
        &controls_frame(&ctx, &mut editor, vec![]),
        HOLDER_NOTE
    ));
    // The holder back in: the note again, until the conversion is turned off.
    in_edit_frame(&ctx, &mut editor, |e| {
        e.document.edit.recipe_mut().crop = [0., 0., 1., 1.]
    });
    run_negative_auto(&ctx, &mut editor);
    assert!(says(
        &controls_frame(&ctx, &mut editor, vec![]),
        HOLDER_NOTE
    ));
    assert!(editor.document.edit.recipe().unknown.is_empty());
    click(&ctx, &mut editor, "Off");
    assert!(!says(
        &controls_frame(&ctx, &mut editor, vec![]),
        HOLDER_NOTE
    ));
    editor.undo();
    assert!(!says(
        &controls_frame(&ctx, &mut editor, vec![]),
        HOLDER_NOTE
    ));
    // And another photo: gone.
    run_negative_auto(&ctx, &mut editor);
    assert!(says(
        &controls_frame(&ctx, &mut editor, vec![]),
        HOLDER_NOTE
    ));
    editor.document.reset(None);
    editor.document.start_load();
    editor.document.metadata = Some(image.metadata.clone());
    editor.document.set_image(image);
    assert!(!says(
        &controls_frame(&ctx, &mut editor, vec![]),
        HOLDER_NOTE
    ));
}

/// The note is of the conversion Auto measured, with the conversion still on: undoing
/// that Auto takes it away, and so does another film base. Redoing the Auto doesn't
/// bring it back; Auto run again does.
#[test]
fn the_holder_note_goes_with_the_conversion_auto_measured() {
    let ctx = egui::Context::default();
    let mut editor = editor_with_scan(&ctx, false, false);
    let mut image = (*negative_scan(false)).clone();
    for (i, p) in image.pixels.iter_mut().enumerate() {
        if i % 64 < 6 {
            *p = [0.0005; 3];
        }
    }
    editor.document.set_image(Arc::new(image));
    in_edit_frame(&ctx, &mut editor, |e| {
        e.document.edit.recipe_mut().negative = Some(NegativeConversion {
            base: BASE,
            ..NegativeConversion::new(FilmType::BlackWhite)
        })
    });
    let shown = |editor: &mut Editor| says(&controls_frame(&ctx, editor, vec![]), HOLDER_NOTE);
    run_negative_auto(&ctx, &mut editor);
    assert!(shown(&mut editor));
    // Undone: the conversion is on, unlevelled.
    editor.undo();
    assert!(editor.document.edit.recipe().negative.is_some());
    assert!(!shown(&mut editor));
    editor.redo();
    assert!(!shown(&mut editor));
    run_negative_auto(&ctx, &mut editor);
    assert!(shown(&mut editor));
    // Another film base, as the picker sets it.
    in_edit_frame(&ctx, &mut editor, |e| {
        e.document.edit.recipe_mut().negative.as_mut().unwrap().base = [0.5, 0.4, 0.3]
    });
    assert!(!shown(&mut editor));
}

/// Geometry and lens-distortion edits that change which scan pixels Auto sampled
/// (Straighten, Rotation, Flip, Transform, manual Distortion) clear the holder note.
#[test]
fn geometry_and_lens_edits_clear_the_holder_note() {
    type Edit = fn(&mut crate::model::recipe::Recipe);
    let edits: [(&str, Edit); 5] = [
        ("straighten", |r| r.straighten = 3.),
        ("rotation", |r| r.rotation = 1),
        ("flip_x", |r| r.flip_x = true),
        ("transform", |r| r.transform.vertical = 0.2),
        ("manual_distortion", |r| r.lens_manual_distortion = -0.2),
    ];
    for (name, edit) in edits {
        let ctx = egui::Context::default();
        let mut editor = editor_with_scan(&ctx, false, false);
        let mut image = (*negative_scan(false)).clone();
        for (i, p) in image.pixels.iter_mut().enumerate() {
            if i % 64 < 6 {
                *p = [0.0005; 3];
            }
        }
        editor.document.set_image(Arc::new(image));
        in_edit_frame(&ctx, &mut editor, |e| {
            e.document.edit.recipe_mut().negative = Some(NegativeConversion {
                base: BASE,
                ..NegativeConversion::new(FilmType::BlackWhite)
            })
        });
        run_negative_auto(&ctx, &mut editor);
        assert!(
            says(&controls_frame(&ctx, &mut editor, vec![]), HOLDER_NOTE),
            "{name}: note shown after Auto"
        );
        in_edit_frame(&ctx, &mut editor, |e| edit(e.document.edit.recipe_mut()));
        assert!(
            !says(&controls_frame(&ctx, &mut editor, vec![]), HOLDER_NOTE),
            "{name}: note cleared after geometry/lens edit"
        );
    }
}

/// The Negative panel sits at the bottom of Develop (below Calibration) and starts
/// collapsed by default so it doesn't clutter Develop for non-film photos.
#[test]
fn negative_panel_sits_at_the_bottom_and_starts_collapsed() {
    let ctx = egui::Context::default();
    let mut editor = editor_with_scan_collapsed(&ctx, false, true);
    let collapsed = controls_frame(&ctx, &mut editor, vec![]);
    // Negative's Reset button sits below Calibration's Reset button.
    let cal_y = node(&collapsed, "Reset Calibration").bounds().unwrap().y0;
    let neg_y = node(&collapsed, "Reset Negative").bounds().unwrap().y0;
    assert!(
        neg_y > cal_y,
        "Negative ({neg_y}) should sit below Calibration ({cal_y})"
    );
    // Collapsed by default: the Film switch's "Off" option is not shown below Calibration.
    let off_below_neg = collapsed
        .nodes
        .iter()
        .any(|(_, n)| n.label() == Some("Off") && n.bounds().is_some_and(|b| b.y0 > neg_y));
    assert!(!off_below_neg, "Negative should start collapsed");
    // Clicking the Negative header expands it.
    let neg_reset = node(&collapsed, "Reset Negative").bounds().unwrap();
    let header_click = egui::pos2(
        (neg_reset.x0 - 80.) as f32,
        ((neg_reset.y0 + neg_reset.y1) * 0.5) as f32,
    );
    let [down, up] = press_and_release(header_click);
    controls_frame(&ctx, &mut editor, down);
    controls_frame(&ctx, &mut editor, up);
    let expanded = controls_frame(&ctx, &mut editor, vec![]);
    let off_below_neg = expanded
        .nodes
        .iter()
        .any(|(_, n)| n.label() == Some("Off") && n.bounds().is_some_and(|b| b.y0 > neg_y));
    assert!(off_below_neg, "Negative should expand when clicked");
}

/// A screen reader hears the picker and Auto as buttons, which are off while the
/// photo loads, Auto while it measures, and the picker as pressed while it is out.
#[test]
fn the_picker_and_auto_tell_screen_readers_their_state() {
    use egui::accesskit::{Role, Toggled};
    let ctx = egui::Context::default();
    let state = |editor: &mut Editor| {
        let shown = controls_frame(&ctx, editor, vec![]);
        ["Film base picker", "Auto levels"].map(|label| {
            let n = node(&shown, label);
            assert_eq!(n.role(), Role::Button, "{label}");
            (n.is_disabled(), n.toggled() == Some(Toggled::True))
        })
    };
    let mut editor = editor_with_scan(&ctx, false, false);
    in_edit_frame(&ctx, &mut editor, |e| {
        e.document.edit.recipe_mut().negative = Some(NegativeConversion::new(FilmType::Colour))
    });
    assert_eq!(state(&mut editor), [(true, false), (true, false)]);
    let mut editor = editor_with_scan(&ctx, false, true);
    click(&ctx, &mut editor, "Color");
    assert_eq!(state(&mut editor), [(false, false), (false, false)]);
    click(&ctx, &mut editor, "Film base picker");
    assert_eq!(state(&mut editor), [(false, true), (false, false)]);
    editor.document.auto.start();
    assert_eq!(state(&mut editor), [(false, true), (true, false)]);
}

/// A screen reader hears which film type is set: the switch's options are radio
/// buttons, the set one selected, the others not.
#[test]
fn the_film_switch_tells_screen_readers_which_film_is_set() {
    use egui::accesskit::{Role, Toggled};
    let ctx = egui::Context::default();
    let mut editor = editor_with_scan(&ctx, false, true);
    let state = |shown: &egui::accesskit::TreeUpdate| {
        ["Off", "Color", "B&W"].map(|label| {
            let n = node(shown, label);
            assert_eq!(n.role(), Role::RadioButton, "{label}");
            n.toggled() == Some(Toggled::True)
        })
    };
    assert_eq!(
        state(&controls_frame(&ctx, &mut editor, vec![])),
        [true, false, false]
    );
    click(&ctx, &mut editor, "B&W");
    assert_eq!(
        state(&controls_frame(&ctx, &mut editor, vec![])),
        [false, false, true]
    );
}

/// The film base picker and Auto end where the film switch's track ends (its last
/// segment sits 2 points inside it), and the switch's segments share one row.
#[test]
fn the_negative_panel_lines_up_its_rows() {
    let ctx = egui::Context::default();
    let mut editor = editor_with_scan(&ctx, false, true);
    click(&ctx, &mut editor, "Color");
    let shown = controls_frame(&ctx, &mut editor, vec![]);
    let bounds = |label: &str| node(&shown, label).bounds().expect("placed");
    let right = bounds("B&W").x1 + 2.;
    for label in ["Film base picker", "Auto levels"] {
        assert!(
            (bounds(label).x1 - right).abs() < 0.5,
            "{label} ends at {}, the switch at {right}",
            bounds(label).x1
        );
    }
    // The three segments share one row.
    let middle = |label: &str| centre(&node(&shown, label)).y;
    assert!((middle("Off") - middle("B&W")).abs() < 0.5);
}

/// The Negative panel in each state it is met in, for `RAWMAKASE_UI_SHOTS`
/// ([`crate::app::ui::shots`]): off; a colour negative nothing has measured yet; a
/// black & white one picked and levelled; the picker out; the picker after a
/// clipped click; Auto off before the photo decodes; while Auto runs; after an Auto
/// that found part of the film holder in the crop; and an HDR
/// file with HDR on, whose master renders no conversion.
#[test]
fn shots_of_the_negative_panel() {
    use crate::app::develop::state::Tool;
    let measured = NegativeConversion {
        base: BASE,
        dmin: [0.12; 3],
        dmax: [0.95; 3],
        ..NegativeConversion::new(FilmType::BlackWhite)
    };
    let colour = Some(NegativeConversion::new(FilmType::Colour));
    type Setup = fn(&mut Editor);
    let states: [(&str, Option<NegativeConversion>, bool, Setup); 9] = [
        ("negative-off", None, true, |_| {}),
        ("negative-colour", colour.clone(), true, |_| {}),
        ("negative-black-white", Some(measured.clone()), true, |_| {}),
        ("negative-picking", colour.clone(), true, |e| {
            e.view.set_tool(Tool::FilmBase)
        }),
        ("negative-picker-refused", colour.clone(), true, |e| {
            e.view.set_tool(Tool::FilmBase);
            e.view.film_base_refusal = Some(crate::develop::FilmBaseRefusal::Clipped);
        }),
        ("negative-not-decoded", colour, false, |_| {}),
        ("negative-auto-running", Some(measured.clone()), true, |e| {
            e.document.auto.start();
        }),
        (
            "negative-holder-in-crop",
            Some(measured.clone()),
            true,
            |e| {
                let r = e.document.edit.recipe();
                e.view.negative_holder = Some(crate::app::develop::state::NegativeHolder::new(
                    e.document.load_id(),
                    r,
                    r.negative.clone().unwrap(),
                ));
            },
        ),
        ("negative-hdr-file", Some(measured), true, |e| {
            if let Some(m) = e.document.metadata.as_mut() {
                m.source = crate::camera_data::Source::Rendered;
                m.hdr_source = Some(crate::camera_data::HdrSource {
                    transfer: crate::color::hdr_output::HdrTransfer::Pq,
                    gain_map: false,
                    peak: 4.,
                });
            }
            e.document.edit.setup_mut().hdr.enabled = true;
        }),
    ];
    let draw_bottom = |e: &mut Editor, ctx: &egui::Context| {
        crate::app::ui::shots::panel(ctx, 320., |ui| {
            let _side = crate::app::ui::widgets::SectionSide::enter(
                ui,
                crate::app::ui::widgets::SectionGroup::DevelopRight,
            );
            egui::ScrollArea::vertical()
                .max_height(244.)
                .show(ui, |ui| {
                    let frame = e.begin_edit_frame();
                    e.controls(ui);
                    e.finish_edit_frame(frame, ctx);
                    ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                });
        });
    };
    for (name, negative, decoded, setup) in states {
        crate::app::ui::shots::shot(
            name,
            egui::Vec2::new(340., 260.),
            move |e, ctx| {
                e.module = Module::Develop;
                let image = negative_scan(false);
                e.document.metadata = Some(image.metadata.clone());
                if decoded {
                    e.document.set_image(image);
                }
                in_edit_frame(ctx, e, |e| e.document.edit.recipe_mut().negative = negative);
                setup(e);
                expand_negative(ctx);
            },
            draw_bottom,
        );
    }
    // Bottom of Develop with Negative collapsed by default (showing Calibration above it
    // and the breathing space below it).
    crate::app::ui::shots::shot(
        "negative-bottom-collapsed",
        egui::Vec2::new(340., 260.),
        move |e, _ctx| {
            e.module = Module::Develop;
            let image = negative_scan(false);
            e.document.metadata = Some(image.metadata.clone());
            e.document.set_image(image);
        },
        draw_bottom,
    );
}

#[test]
fn measure_crop_respects_transform_panel_off_for_constrain_crop() {
    use crate::app::develop::state::Tool;
    use crate::model::panels::{Panel, PanelState};
    let ctx = egui::Context::default();
    let mut editor = editor_with_scan(&ctx, false, true);
    in_edit_frame(&ctx, &mut editor, |e| {
        let r = e.document.edit.recipe_mut();
        r.negative = Some(NegativeConversion::new(FilmType::Colour));
        r.crop = [0.1, 0.1, 0.9, 0.9];
        r.constrain_crop = true;
        r.lens_manual_distortion = -0.2;
    });

    editor.view.set_tool(Tool::Crop);
    let effective = editor.effective_recipe();
    assert_eq!(
        editor.measure_crop(&effective),
        Some(crate::develop::MeasureCrop {
            crop: [0.1, 0.1, 0.9, 0.9],
            constrain_crop: true,
        })
    );

    // Switching the Transform panel off bypasses `constrain_crop` in normal renders,
    // so `measure_crop` must mirror `as_rendered()` and clear `constrain_crop` too.
    in_edit_frame(&ctx, &mut editor, |e| {
        e.document
            .edit
            .recipe_mut()
            .panels
            .set(Panel::Transform, PanelState::Off);
    });
    let effective = editor.effective_recipe();
    assert_eq!(
        editor.measure_crop(&effective),
        Some(crate::develop::MeasureCrop {
            crop: [0.1, 0.1, 0.9, 0.9],
            constrain_crop: false,
        })
    );

    // Without a crop, Transform off leaves `constrain_crop` false so no override is needed.
    in_edit_frame(&ctx, &mut editor, |e| {
        e.document.edit.recipe_mut().crop = [0., 0., 1., 1.];
    });
    let effective = editor.effective_recipe();
    assert_eq!(editor.measure_crop(&effective), None);
}
