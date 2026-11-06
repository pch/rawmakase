//! Editor tests of `presets`: the whole editor driven through its frames.
use crate::app::*;
use crate::camera_data::{CameraImage, Metadata};
use std::sync::Arc;

#[test]
fn refreshing_preset_support_cancels_the_hover_render() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 1,
        height: 1,
        pixels: vec![[0.1; 3]],
        metadata: Metadata {
            width: 1,
            height: 1,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    });
    editor.document.set_image(image);
    editor.presets.preview = Some(Recipe {
        exposure: 1.,
        ..Default::default()
    });
    let (previous, cancelled) = editor.preview.task.start();
    editor.refresh_preset_support();
    assert!(editor.presets.preview.is_none());
    assert!(cancelled.load(std::sync::atomic::Ordering::Relaxed));
    assert!(editor.preview.task.id() > previous);
}

#[test]
fn the_preset_list_is_kept_until_what_it_shows_changes() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let preset = |id: &str, group: &str, name: &str| crate::xmp::Preset {
        id: id.into(),
        name: name.into(),
        group: group.into(),
        path: Default::default(),
        settings: Default::default(),
        curves: Default::default(),
        look: String::new(),
        blockers: Vec::new(),
        notes: Vec::new(),
        photo_settings: false,
        local: Default::default(),
        builtin: false,
    };
    editor.presets.library = std::sync::Arc::new(crate::presets::Library {
        presets: vec![
            preset("a", "Film", "Warm⁺"),
            preset("b", "Film", "Cool"),
            preset("c", "Mono", "Grain"),
        ],
        errors: Vec::new(),
    });
    let names = |editor: &mut Editor| -> Vec<(String, Vec<String>)> {
        editor
            .preset_list()
            .iter()
            .map(|g| {
                (
                    g.name.clone(),
                    g.presets.iter().map(|(_, n)| n.clone()).collect(),
                )
            })
            .collect()
    };
    assert_eq!(
        names(&mut editor),
        [
            ("Film".into(), vec!["Warm+".into(), "Cool".into()]),
            ("Mono".into(), vec!["Grain".into()])
        ]
    );
    // Kept while nothing it depends on changes.
    let first = editor.preset_list();
    assert!(std::sync::Arc::ptr_eq(&first, &editor.preset_list()));
    // Rebuilt for a search, and for a favorite.
    editor.presets.filter = "WARM+".into();
    assert_eq!(names(&mut editor), [("Film".into(), vec!["Warm+".into()])]);
    editor.presets.filter.clear();
    editor.presets.favorites_only = true;
    assert!(names(&mut editor).is_empty());
    editor.presets.favorites.insert("c".into());
    editor.presets.revision += 1;
    assert_eq!(names(&mut editor), [("Mono".into(), vec!["Grain".into()])]);
    // Rebuilt when another photo's compatibility replaces this one's.
    editor.presets.favorites_only = false;
    editor.presets.compatible_only = true;
    editor.presets.issues = vec![Some("no".into()), None, Some("no".into())];
    editor.presets.revision += 1;
    assert_eq!(names(&mut editor), [("Film".into(), vec!["Cool".into()])]);
    editor.presets.clear_document();
    assert_eq!(names(&mut editor).len(), 2);
}

/// A Lightroom preset from its settings, as a file would hold them.
fn preset_from(name: &str, settings: &str) -> crate::xmp::Preset {
    let text = format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:PresetType="Normal" crs:HasSettings="True" {settings}><crs:Name><rdf:Alt><rdf:li xml:lang="x-default">{name}</rdf:li></rdf:Alt></crs:Name></rdf:Description></rdf:RDF></x:xmpmeta>"#
    );
    crate::xmp::parse(std::path::Path::new(&format!("{name}.xmp")), &text).unwrap()
}

fn editor_with_presets(ctx: &egui::Context, presets: Vec<crate::xmp::Preset>) -> Editor {
    let mut editor = Editor::with_context(
        ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.document.metadata = Some(Metadata {
        wb: [2., 1., 1.8],
        daylight_wb: [2., 1., 1.8],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    });
    editor.presets.library = Arc::new(crate::presets::Library {
        presets,
        errors: Vec::new(),
    });
    editor
}

#[test]
fn preset_amount_scales_the_preset_from_the_settings_before_it() {
    let ctx = egui::Context::default();
    let mut editor = editor_with_presets(
        &ctx,
        vec![
            preset_from(
                "Bright",
                r#"crs:SupportsAmount="True" crs:Exposure2012="+1.00" crs:Contrast2012="+40""#,
            ),
            preset_from(
                "Fixed",
                r#"crs:SupportsAmount="False" crs:Exposure2012="+1.00""#,
            ),
        ],
    );
    editor.document.edit.setup_mut().exposure = 0.2;
    let frame = editor.begin_edit_frame();
    editor.apply_preset(0).unwrap();
    editor.finish_edit_frame(frame, &ctx);
    assert_eq!(editor.document.edit.recipe().exposure, 1.);
    // Each drag is one History step, computed again from the settings before the
    // preset, so going back and forth never drifts.
    for amount in [1.7, 0.3, 0.5] {
        let frame = editor.begin_edit_frame();
        editor.set_preset_amount(amount);
        editor.finish_edit_frame(frame, &ctx);
    }
    assert!((editor.document.edit.recipe().exposure - 0.6).abs() < 1e-6);
    assert!((editor.document.edit.recipe().contrast - 0.2).abs() < 1e-6);
    let (steps, applied) = editor.document.edit.history().steps();
    assert_eq!(applied, 4);
    assert_eq!(steps[3].name, "Preset Amount");
    assert_eq!(steps[3].value, "50");
    // An Upright analysis landing meanwhile keeps the Amount, and the Amount keeps it.
    let analysed = vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 2];
    editor.document.edit.setup_mut().upright.corrections = analysed.clone();
    let frame = editor.begin_edit_frame();
    editor.finish_edit_frame(frame, &ctx);
    assert!(editor.presets.amount.is_some());
    let frame = editor.begin_edit_frame();
    editor.set_preset_amount(0.5);
    editor.finish_edit_frame(frame, &ctx);
    assert_eq!(editor.document.edit.recipe().upright.corrections, analysed);
    // A new Upright mode is an edit of its own.
    let frame = editor.begin_edit_frame();
    editor.document.edit.setup_mut().upright.mode = crate::model::transform::UprightMode::Level;
    editor.finish_edit_frame(frame, &ctx);
    assert!(editor.presets.amount.is_none());
    editor.apply_preset(0).unwrap();
    // Any other edit ends it, as Lightroom hides the slider, even with the Presets
    // panel closed.
    let frame = editor.begin_edit_frame();
    editor.document.edit.setup_mut().vibrance = 0.1;
    editor.finish_edit_frame(frame, &ctx);
    assert!(editor.presets.amount.is_none());
    // A preset without an Amount shows none.
    editor.apply_preset(1).unwrap();
    assert!(editor.presets.amount.is_none());
    editor.apply_preset(0).unwrap();
    assert!(editor.presets.amount.is_some());
    // A preset with only choices that aren't numbers looks the same at every Amount
    // above 0: moving it is no step, and leaves no name for the next one.
    editor.presets.library = Arc::new(crate::presets::Library {
        presets: vec![preset_from(
            "Mono",
            r#"crs:SupportsAmount="True" crs:ConvertToGrayscale="True""#,
        )],
        errors: Vec::new(),
    });
    let frame = editor.begin_edit_frame();
    editor.apply_preset(0).unwrap();
    editor.finish_edit_frame(frame, &ctx);
    let frame = editor.begin_edit_frame();
    // As the slider does while it moves.
    editor
        .step_name
        .name("Preset Amount".to_string(), "50".to_string());
    editor.set_preset_amount(0.5);
    editor.finish_edit_frame(frame, &ctx);
    let frame = editor.begin_edit_frame();
    editor.document.edit.setup_mut().exposure = 0.9;
    editor.finish_edit_frame(frame, &ctx);
    assert!(editor.presets.amount.is_none());
    editor.apply_preset(0).unwrap();
    let (steps, applied) = editor.document.edit.history().steps();
    assert_ne!(steps[applied - 1].name, "Preset Amount");
    // Undo ends it too.
    let frame = editor.begin_edit_frame();
    let mut recipe = editor.document.edit.recipe().clone();
    editor.document.edit.history_mut().undo(&mut recipe);
    editor.document.edit.replace(recipe);
    editor.finish_edit_frame(frame, &ctx);
    assert!(editor.presets.amount.is_none());
    // An edit earlier in the frame the slider moves in (a mask's name kept as the
    // slider was clicked) ends it there, rather than being taken back by it.
    editor.apply_preset(0).unwrap();
    let frame = editor.begin_edit_frame();
    editor.document.edit.recipe_mut().vibrance = 0.3;
    assert_eq!(
        editor.set_preset_amount(0.5),
        crate::app::presets::AmountChange::Same
    );
    editor.finish_edit_frame(frame, &ctx);
    assert_eq!(editor.document.edit.recipe().vibrance, 0.3);
    assert!(editor.presets.amount.is_none());
}
