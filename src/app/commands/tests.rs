use super::*;
use crate::app::Module;
use crate::model::params::ParameterId;
use serde_json::Value;
fn json(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).unwrap()
}
use std::sync::Arc;
fn editor() -> (Editor, egui::Context) {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.onboarding.visible = false;
    e.module = Module::Develop;
    let metadata = crate::camera_data::Metadata {
        width: 12,
        height: 8,
        wb: [1.; 3],
        ..Default::default()
    };
    e.document.metadata = Some(metadata.clone());
    e.document
        .set_image(Arc::new(crate::camera_data::CameraImage {
            recovered: Default::default(),
            width: 12,
            height: 8,
            pixels: vec![[0.2, 0.1, 0.05]; 96],
            metadata,
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
        }));
    (e, ctx)
}
fn set(e: &mut Editor, ctx: &egui::Context, value: f32) -> Result<Outcome> {
    e.execute_command(
        Command::new(Operation::Set(Param::Setting(ParameterId::Exposure), value)),
        ctx,
    )
}
#[test]
fn command_edit_records_history_and_undo_returns_post_action_state() {
    let (mut e, ctx) = editor();
    let initial = e.document.edit.recipe().exposure;
    set(&mut e, &ctx, 1.25).unwrap();
    assert_eq!(json(e.command_state())["values"]["exposure"], 1.25);
    assert!(e.document.edit.save_state().needs_save());
    assert!(e.automation.revision > 0);
    e.execute_command(Command::new(Operation::Action(Action::Undo)), &ctx)
        .unwrap();
    assert_eq!(e.document.edit.recipe().exposure, initial);
    e.execute_command(Command::new(Operation::Action(Action::Redo)), &ctx)
        .unwrap();
    assert_eq!(e.document.edit.recipe().exposure, 1.25);
}
#[test]
fn straighten_is_a_global_parameter_in_degrees() {
    let (mut e, ctx) = editor();
    e.execute_command(
        Command::new(Operation::Set(Param::Setting(ParameterId::Straighten), 1.5)),
        &ctx,
    )
    .unwrap();
    assert_eq!(e.document.edit.recipe().straighten, 1.5);
    assert_eq!(json(e.command_state())["values"]["straighten"], 1.5);
    e.execute_command(Command::new(Operation::Action(Action::Undo)), &ctx)
        .unwrap();
    assert_eq!(e.document.edit.recipe().straighten, 0.);
}
#[test]
fn presets_are_listed_and_applied_by_name_as_one_history_step() {
    let (mut e, ctx) = editor();
    let xmp = |exposure: &str| {
        format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:PresetType="Normal" crs:HasSettings="True" crs:Exposure2012="{exposure}"/></rdf:RDF></x:xmpmeta>"#
        )
    };
    let preset = |name: &str, exposure: &str| crate::xmp::Preset {
        name: name.into(),
        group: "Mine".into(),
        ..crate::xmp::parse(std::path::Path::new(&format!("{name}.xmp")), &xmp(exposure)).unwrap()
    };
    assert_eq!(
        e.execute_command(Command::new(Operation::Presets { group: None }), &ctx)
            .unwrap_err()
            .code,
        "not_ready"
    );
    e.presets.library = Arc::new(crate::presets::Library {
        presets: vec![preset("Bright", "+1.00"), preset("Brighter", "+2.00")],
        errors: Vec::new(),
    });
    e.presets.scanned = true;
    let listed = json(
        e.execute_command(Command::new(Operation::Presets { group: None }), &ctx)
            .unwrap(),
    );
    assert_eq!(listed["presets"][1]["name"], "Brighter");
    assert_eq!(listed["presets"][1]["group"], "Mine");
    let named = |name: &str| {
        Command::new(Operation::Preset(PresetTarget::Name {
            name: name.into(),
            group: None,
        }))
    };
    let applied = json(e.execute_command(named("bright"), &ctx).unwrap());
    assert_eq!(applied["applied"]["name"], "Bright");
    assert_eq!(e.document.edit.recipe().exposure, 1.);
    assert_eq!(json(e.command_state())["preset"], "Bright");
    let (steps, applied) = e.document.edit.history().steps();
    assert_eq!(steps[applied - 1].name, "Preset");
    // Applied again after another edit, it is still named for the preset.
    set(&mut e, &ctx, 0.5).unwrap();
    e.execute_command(named("bright"), &ctx).unwrap();
    let (steps, applied) = e.document.edit.history().steps();
    assert_eq!(steps[applied - 1].name, "Preset");
    assert_eq!(steps[applied - 1].value, "Bright");
    // Applied once more, it changes nothing and leaves no name for the next edit.
    e.execute_command(named("bright"), &ctx).unwrap();
    set(&mut e, &ctx, 0.25).unwrap();
    let (steps, applied) = e.document.edit.history().steps();
    assert_ne!(steps[applied - 1].name, "Preset");
    // An edit no listed preset made names none.
    e.document.edit.setup_mut().preset_name = "Imported Lightroom edit".into();
    assert_eq!(json(e.command_state())["preset"], Value::Null);
    assert_eq!(
        e.execute_command(named("Bri"), &ctx).unwrap_err().code,
        "ambiguous"
    );
    e.module = Module::Library;
    assert_eq!(
        e.execute_command(named("Brighter"), &ctx).unwrap_err().code,
        "no_document"
    );
    assert_eq!(e.document.edit.recipe().exposure, 0.25);
}
#[test]
fn edits_reject_library_loading_modal_and_stale_targets() {
    let (mut e, ctx) = editor();
    let original = e.document.edit.recipe().clone();
    e.module = Module::Library;
    assert_eq!(set(&mut e, &ctx, 1.).unwrap_err().code, "no_document");
    e.module = Module::Develop;
    e.preferences.open = true;
    assert_eq!(set(&mut e, &ctx, 1.).unwrap_err().code, "busy");
    e.preferences.open = false;
    let (generation, _) = e.load.start();
    assert_eq!(set(&mut e, &ctx, 1.).unwrap_err().code, "not_ready");
    e.load.finish(generation);
    let mut command = Command::new(Operation::Set(Param::Setting(ParameterId::Exposure), 1.));
    command.target.generation = Some(generation + 1);
    assert_eq!(
        e.execute_command(command, &ctx).unwrap_err().code,
        "stale_target"
    );
    assert_eq!(*e.document.edit.recipe(), original);
}
#[test]
fn explicit_mask_has_local_units_and_rejects_stale_index() {
    let (mut e, ctx) = editor();
    e.document.edit.setup_mut().masks.push(Default::default());
    let target = Target {
        mask: Some(0),
        generation: Some(e.load.id()),
        revision: Some(e.automation.revision),
        ..Default::default()
    };
    let command = Command {
        operation: Operation::Set(Param::Setting(ParameterId::Temperature), 35.),
        target: target.clone(),
    };
    let global = e.document.edit.recipe().temperature;
    e.execute_command(command.clone(), &ctx).unwrap();
    assert_eq!(e.document.edit.recipe().temperature, global);
    assert_eq!(e.document.edit.recipe().masks[0].adjust.temperature, 0.35);
    assert_eq!(
        e.execute_command(command, &ctx).unwrap_err().code,
        "stale_target"
    );
    let target = Target {
        revision: Some(e.automation.revision),
        ..target
    };
    let before = e.document.edit.recipe().clone();
    assert_eq!(
        e.execute_command(
            Command {
                operation: Operation::Set(Param::Setting(ParameterId::Vibrance), 20.),
                target
            },
            &ctx
        )
        .unwrap_err()
        .code,
        "unsupported_parameter"
    );
    assert_eq!(*e.document.edit.recipe(), before);
}
#[test]
fn black_white_action_matches_existing_treatment_workflow() {
    let (mut e, ctx) = editor();
    let (mut reference, reference_ctx) = editor();
    crate::app::tests::in_edit_frame(&reference_ctx, &mut reference, |r| r.toggle_treatment());
    e.execute_command(Command::new(Operation::Action(Action::ToggleMono)), &ctx)
        .unwrap();
    assert_eq!(e.document.edit.recipe(), reference.document.edit.recipe());
}
#[test]
fn named_actions_are_discoverable_and_parameter_values_are_finite() {
    for name in Action::names() {
        assert_eq!(Action::parse(name).unwrap().name(), name);
    }
    let (mut e, ctx) = editor();
    assert_eq!(
        set(&mut e, &ctx, f32::NAN).unwrap_err().code,
        "invalid_value"
    );
    let capabilities = e
        .execute_command(Command::new(Operation::Capabilities), &ctx)
        .unwrap();
    let capabilities = json(capabilities);
    assert_eq!(capabilities["protocol"], PROTOCOL);
    assert!(
        capabilities["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["name"] == "temperature"
                && v["unit"] == "kelvin"
                && v["mask_unit"] == "percent")
    );
    assert!(
        capabilities["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["name"] == "straighten"
                && v["unit"] == "degrees"
                && v["min"] == -45.
                && v["max"] == 45.
                && v["mask"] == false)
    );
}
#[test]
fn refused_open_does_not_claim_another_photo_opened() -> anyhow::Result<()> {
    let (mut e, ctx) = editor();
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    let missing = photos.join("missing.DNG");
    std::fs::write(&missing, b"fixture")?;
    let db = dir.path().join("catalog.rawmakase");
    let mut catalog = crate::catalog::Catalog::create(&db)?;
    catalog.add_folder(&photos)?;
    drop(catalog);
    e.library = Some(Box::new(super::super::library::Library::load(
        &db,
        ctx.clone(),
    )?));
    let id = e.library.as_ref().unwrap().session.photos[0].id;
    std::fs::remove_file(missing)?;
    e.document.catalog_photo = Some(PhotoId(999));
    let error = e
        .execute_command(Command::new(Operation::Open(PhotoTarget::Id(id))), &ctx)
        .unwrap_err();
    assert_eq!(error.code, "not_editable");
    assert_eq!(e.document.catalog_photo, Some(PhotoId(999)));
    assert!(e.module == Module::Develop);
    Ok(())
}
#[test]
fn output_job_publishes_the_captured_revision_and_never_clobbers() -> anyhow::Result<()> {
    let (mut e, ctx) = editor();
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("synthetic.dng");
    // The decoded synthetic pixels are supplied directly; no private RAW fixture.
    std::fs::write(&source, b"synthetic")?;
    e.document.path = Some(source);
    let path = dir.path().join("preview.jpg");
    let result = e
        .execute_command(
            Command::new(Operation::Output {
                path: path.clone(),
                max_edge: 16,
            }),
            &ctx,
        )
        .unwrap();
    let result = json(result);
    let id = result["job_id"].as_u64().unwrap();
    let revision = result["revision"].clone();
    set(&mut e, &ctx, 1.).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let job = loop {
        let job = e
            .execute_command(Command::new(Operation::Job(id)), &ctx)
            .unwrap();
        let job = json(job);
        if job["status"] != "running" {
            break job;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "output never completed"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert_eq!(job["status"], "completed", "{job}");
    assert_eq!(job["revision"], revision);
    assert!(path.is_file());
    assert_eq!(
        e.execute_command(Command::new(Operation::Output { path, max_edge: 16 }), &ctx)
            .unwrap_err()
            .code,
        "already_exists"
    );
    Ok(())
}

#[test]
fn save_rejects_protected_and_uncataloged_edits() {
    let (mut e, ctx) = editor();
    set(&mut e, &ctx, 1.).unwrap();
    assert_eq!(
        e.execute_command(Command::new(Operation::Save), &ctx)
            .unwrap_err()
            .code,
        "no_catalog"
    );
    assert!(e.document.edit.save_state().needs_save());
    e.document
        .edit
        .save_state_mut()
        .protect("Conflicting source identity".into());
    assert_eq!(
        e.execute_command(Command::new(Operation::Save), &ctx)
            .unwrap_err()
            .code,
        "protected"
    );
    assert!(e.document.edit.save_state().is_protected());
}

#[test]
fn save_success_means_the_catalog_contains_the_current_edit() -> anyhow::Result<()> {
    let (mut e, ctx) = editor();
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    let source = photos.join("synthetic.DNG");
    std::fs::write(&source, b"fixture")?;
    let db = dir.path().join("catalog.rawmakase");
    let mut catalog = crate::catalog::Catalog::create(&db)?;
    catalog.add_folder(&photos)?;
    drop(catalog);
    e.library = Some(Box::new(super::super::library::Library::load(
        &db,
        ctx.clone(),
    )?));
    let id = e.library.as_ref().unwrap().session.photos[0].id;
    e.document.catalog_photo = Some(id);
    e.document.path = Some(source.clone());
    set(&mut e, &ctx, 1.25).unwrap();
    let result = e
        .execute_command(Command::new(Operation::Save), &ctx)
        .unwrap();
    assert_eq!(json(result)["saved"], true);
    assert!(!e.document.edit.save_state().needs_save());
    assert_eq!(
        e.library
            .as_ref()
            .unwrap()
            .session
            .catalog
            .load_edit(id, &source)?
            .unwrap()
            .recipe
            .exposure,
        1.25
    );
    Ok(())
}

#[test]
fn curve_save_is_modal_for_commands_and_state() {
    let (mut e, ctx) = editor();
    crate::app::tests::in_edit_frame(&ctx, &mut e, |e| {
        e.choose_point_curve(super::super::curve_menu::CurveChoice::Save)
    });
    assert_eq!(json(e.command_state())["modal"], true);
    assert_eq!(set(&mut e, &ctx, 1.).unwrap_err().code, "busy");
    assert_eq!(
        e.execute_command(Command::new(Operation::Action(Action::Reset)), &ctx)
            .unwrap_err()
            .code,
        "busy"
    );
    assert!(
        e.execute_command(Command::new(Operation::State), &ctx)
            .is_ok()
    );
}

#[test]
fn asynchronous_edits_and_out_of_frame_undo_invalidate_guards() {
    let (mut e, ctx) = editor();
    let initial = json(e.command_state())["revision"].as_u64().unwrap();
    let mut auto = e.document.edit.recipe().clone();
    auto.exposure = 1.25;
    e.auto_ready(super::super::worker::AutoKind::Settings, Ok(Box::new(auto)));
    let mut command = Command::new(Operation::Set(Param::Setting(ParameterId::Contrast), 10.));
    command.target.revision = Some(initial);
    assert_eq!(
        e.execute_command(command, &ctx).unwrap_err().code,
        "stale_target"
    );
    let after_auto = json(e.command_state())["revision"].as_u64().unwrap();
    assert!(after_auto > initial);
    e.undo();
    let after_undo = json(e.command_state())["revision"].as_u64().unwrap();
    assert!(after_undo > after_auto);
    // Direct worker-style recipe changes must also be caught before a frame snapshot.
    e.document.edit.setup_mut().straighten = 1.;
    let frame = e.begin_edit_frame();
    e.finish_edit_frame(frame, &ctx);
    assert!(json(e.command_state())["revision"].as_u64().unwrap() > after_undo);
}

#[test]
fn no_op_parameters_do_not_name_the_next_unrelated_edit() {
    let (mut e, ctx) = editor();
    let exposure = e.document.edit.recipe().exposure;
    set(&mut e, &ctx, exposure).unwrap();
    let old = e.document.edit.recipe().clone();
    e.document.edit.setup_mut().preset_name = "Example".into();
    e.commit_edit(old, None);
    assert_eq!(
        e.document.edit.history().steps().0.last().unwrap().name,
        "Preset"
    );
    set(&mut e, &ctx, 100.).unwrap();
    e.execute_command(
        Command::new(Operation::Adjust(Param::Setting(ParameterId::Exposure), 1)),
        &ctx,
    )
    .unwrap();
    e.automation.end_turn();
    e.finish_gesture();
    let old = e.document.edit.recipe().clone();
    e.document.edit.setup_mut().preset_name = "Another".into();
    e.commit_edit(old, None);
    assert_eq!(
        e.document.edit.history().steps().0.last().unwrap().name,
        "Preset"
    );
}

#[test]
fn turns_group_only_the_same_parameter_and_scope() {
    let (mut e, ctx) = editor();
    for _ in 0..2 {
        e.execute_command(
            Command::new(Operation::Adjust(Param::Setting(ParameterId::Exposure), 1)),
            &ctx,
        )
        .unwrap();
    }
    let exposure = e.document.edit.recipe().exposure;
    e.execute_command(
        Command::new(Operation::Adjust(Param::Setting(ParameterId::Contrast), 1)),
        &ctx,
    )
    .unwrap();
    e.execute_command(Command::new(Operation::Action(Action::Undo)), &ctx)
        .unwrap();
    assert_eq!(e.document.edit.recipe().contrast, 0.);
    assert_eq!(e.document.edit.recipe().exposure, exposure);
    e.execute_command(Command::new(Operation::Action(Action::Undo)), &ctx)
        .unwrap();
    assert_eq!(e.document.edit.recipe().exposure, 0.);
    e.document.edit.setup_mut().masks.push(Default::default());
    e.command_state();
    e.execute_command(
        Command::new(Operation::Adjust(Param::Setting(ParameterId::Exposure), 1)),
        &ctx,
    )
    .unwrap();
    let global = e.document.edit.recipe().exposure;
    let mut command = Command::new(Operation::Adjust(Param::Setting(ParameterId::Exposure), 1));
    command.target = Target {
        mask: Some(0),
        generation: Some(e.load.id()),
        revision: Some(e.automation.revision),
        ..Default::default()
    };
    e.execute_command(command, &ctx).unwrap();
    e.execute_command(Command::new(Operation::Action(Action::Undo)), &ctx)
        .unwrap();
    assert_eq!(e.document.edit.recipe().masks[0].adjust.exposure, 0.);
    assert_eq!(e.document.edit.recipe().exposure, global);
}

#[test]
fn point_curve_edits_validate_preserve_channels_and_undo() {
    let (mut e, ctx) = editor();
    let original = e.document.edit.recipe().clone();
    let points = vec![[0., 0.], [0.25, 0.2], [0.75, 0.8], [1., 1.]];
    e.execute_command(
        Command::new(Operation::Curve(CurveChannel::Red, points.clone())),
        &ctx,
    )
    .unwrap();
    assert_eq!(e.document.edit.recipe().effects.channels[0].points, points);
    assert_eq!(e.document.edit.recipe().curve, original.curve);
    assert_eq!(
        json(e.command_state())["tone_curve"]["red"]["points"][1][0],
        0.25
    );
    let before = e.document.edit.recipe().clone();
    for points in [
        vec![[0., 0.]],
        vec![[0.5, 0.], [0.5, 1.]],
        vec![[0., -1.], [1., 1.]],
        vec![[0., f32::NAN], [1., 1.]],
    ] {
        assert_eq!(
            e.execute_command(
                Command::new(Operation::Curve(CurveChannel::Rgb, points)),
                &ctx
            )
            .unwrap_err()
            .code,
            "invalid_curve"
        );
        assert_eq!(*e.document.edit.recipe(), before);
    }
    e.execute_command(Command::new(Operation::Action(Action::Undo)), &ctx)
        .unwrap();
    assert_eq!(*e.document.edit.recipe(), original);
    e.execute_command(Command::new(Operation::Action(Action::CurveMedium)), &ctx)
        .unwrap();
    assert_eq!(
        e.document.edit.recipe().curve,
        crate::presets::curves::BuiltinCurve::MediumContrast.curve()
    );
    assert_eq!(
        e.document.edit.recipe().effects.channels,
        original.effects.channels
    );
}

#[test]
fn auto_rejects_an_already_running_job() {
    let (mut e, ctx) = editor();
    let (id, _) = e.document.auto.start();
    for action in [Action::AutoTone, Action::AutoWhiteBalance] {
        assert_eq!(
            e.execute_command(Command::new(Operation::Action(action)), &ctx)
                .unwrap_err()
                .code,
            "busy"
        );
        assert_eq!(e.document.auto.id(), id);
        assert!(e.document.auto.is_running());
    }
}

#[test]
fn ui_edit_during_dial_gesture_has_its_own_undo_step() {
    let (mut e, ctx) = editor();
    e.execute_command(
        Command::new(Operation::Adjust(Param::Setting(ParameterId::Exposure), 3)),
        &ctx,
    )
    .unwrap();
    let exposure = e.document.edit.recipe().exposure;
    let frame = e.begin_edit_frame();
    e.toggle_treatment();
    e.finish_edit_frame(frame, &ctx);
    assert!(e.document.edit.recipe().effects.monochrome);
    e.execute_command(Command::new(Operation::Action(Action::Undo)), &ctx)
        .unwrap();
    assert!(!e.document.edit.recipe().effects.monochrome);
    assert_eq!(e.document.edit.recipe().exposure, exposure);
    e.execute_command(Command::new(Operation::Action(Action::Undo)), &ctx)
        .unwrap();
    assert_eq!(e.document.edit.recipe().exposure, 0.);
}

#[test]
fn idle_frames_keep_turns_grouped_but_transport_changes_split_them() {
    let (mut e, ctx) = editor();
    for _ in 0..2 {
        e.execute_command_from(
            Command::new(Operation::Adjust(Param::Setting(ParameterId::Exposure), 1)),
            Source::Midi(1, 0),
            &ctx,
        )
        .unwrap();
        let frame = e.begin_edit_frame();
        e.finish_edit_frame(frame, &ctx);
    }
    let midi_exposure = e.document.edit.recipe().exposure;
    e.execute_command(
        Command::new(Operation::Adjust(Param::Setting(ParameterId::Exposure), 1)),
        &ctx,
    )
    .unwrap();
    e.execute_command(Command::new(Operation::Action(Action::Undo)), &ctx)
        .unwrap();
    assert_eq!(e.document.edit.recipe().exposure, midi_exposure);
    e.execute_command(Command::new(Operation::Action(Action::Undo)), &ctx)
        .unwrap();
    assert_eq!(e.document.edit.recipe().exposure, 0.);
}

#[test]
fn library_metadata_requires_stable_id_despite_selection_changes() -> anyhow::Result<()> {
    let (mut e, ctx) = editor();
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    for name in ["a.DNG", "b.DNG"] {
        std::fs::write(photos.join(name), b"synthetic")?;
    }
    let db = dir.path().join("catalog.rawmakase");
    let mut catalog = crate::catalog::Catalog::create(&db)?;
    catalog.add_folder(&photos)?;
    drop(catalog);
    e.library = Some(Box::new(super::super::library::Library::load(
        &db,
        ctx.clone(),
    )?));
    e.module = Module::Library;
    let library = e.library.as_mut().unwrap();
    let first = library.session.photos[0].id;
    let second = library.session.photos[1].id;
    library.make_active(first);
    let state = e.command_state();
    e.library.as_mut().unwrap().make_active(second);
    let mut command = Command::new(Operation::Action(Action::Rating(4)));
    command.target.generation = Some(state.generation);
    command.target.revision = Some(state.revision);
    assert_eq!(
        e.execute_command(command.clone(), &ctx).unwrap_err().code,
        "target_required"
    );
    // A previously open Develop photo must not override an explicit Library ID.
    e.document.catalog_photo = Some(second);
    command.target.photo_id = Some(first);
    e.execute_command(command, &ctx).unwrap();
    let library = e.library.as_ref().unwrap();
    assert_eq!(library.photo(first).unwrap().rating, 4);
    assert_eq!(library.photo(second).unwrap().rating, 0);
    Ok(())
}

#[test]
fn absolute_controls_use_parameter_ranges_and_devices_have_separate_undo() {
    let (mut e, ctx) = editor();
    e.execute_command_from(
        Command::new(Operation::ControlValue(
            Param::Setting(ParameterId::Exposure),
            127,
        )),
        Source::Midi(1, 0),
        &ctx,
    )
    .unwrap();
    assert_eq!(e.document.edit.recipe().exposure, 5.);
    e.execute_command_from(
        Command::new(Operation::ControlValue(
            Param::Setting(ParameterId::Exposure),
            0,
        )),
        Source::Midi(2, 0),
        &ctx,
    )
    .unwrap();
    assert_eq!(e.document.edit.recipe().exposure, -5.);
    e.execute_command(Command::new(Operation::Action(Action::Undo)), &ctx)
        .unwrap();
    assert_eq!(e.document.edit.recipe().exposure, 5.);
    e.execute_command(Command::new(Operation::Action(Action::Undo)), &ctx)
        .unwrap();
    assert_eq!(e.document.edit.recipe().exposure, 0.);
    e.document.edit.setup_mut().masks.push(Default::default());
    let state = e.command_state();
    let mut command = Command::new(Operation::ControlValue(
        Param::Setting(ParameterId::Exposure),
        127,
    ));
    command.target = Target {
        mask: Some(0),
        generation: Some(state.generation),
        revision: Some(state.revision),
        ..Default::default()
    };
    e.execute_command_from(command, Source::Midi(1, 1), &ctx)
        .unwrap();
    assert_eq!(e.document.edit.recipe().masks[0].adjust.exposure, 4.);
    assert_eq!(e.document.edit.recipe().exposure, 0.);
}

#[test]
fn absolute_controls_preserve_endpoints_neutral_and_monotonicity() {
    for (_, param) in Param::all() {
        for mask in [false, true] {
            let (min, max) = param.range(mask);
            assert_eq!(param.control_value(0, mask), min);
            assert_eq!(param.control_value(127, mask), max);
            if min < 0. && max > 0. {
                assert_eq!(param.control_value(64, mask), 0.);
            } else {
                assert_eq!(
                    param.control_value(64, mask),
                    min + (max - min) * 64. / 127.
                );
            }
            for value in 1..=127 {
                assert!(param.control_value(value, mask) > param.control_value(value - 1, mask));
            }
        }
    }
}
#[test]
fn the_protocol_states_the_curve_limits_the_app_enforces() {
    use crate::color::curve::{MAX_POINTS, MIN_INPUT_SPACING};
    use rawmakase_protocol::curve::{MINIMUM_INPUT_SPACING, POINT_COUNT};
    assert_eq!(POINT_COUNT, [2, MAX_POINTS]);
    assert_eq!(MINIMUM_INPUT_SPACING, MIN_INPUT_SPACING);
}
#[test]
fn panel_actions_hide_the_open_modules_panels_and_state_reports_them() {
    let (mut e, ctx) = editor();
    let run = |e: &mut Editor, name: &str| {
        e.execute_command(
            Command::new(Operation::Action(Action::parse(name).unwrap())),
            &ctx,
        )
        .unwrap();
    };
    run(&mut e, "panels:sides");
    let state = json(e.command_state());
    assert_eq!(state["panels"]["left"], "hidden");
    assert_eq!(state["panels"]["right"], "hidden");
    assert_eq!(state["panels"]["filmstrip"], "shown");
    run(&mut e, "panels:filmstrip");
    run(&mut e, "panels:left");
    assert_eq!(json(e.command_state())["panels"]["left"], "shown");
    run(&mut e, "panels:all");
    let state = json(e.command_state());
    assert!(
        ["left", "right", "filmstrip"]
            .iter()
            .all(|p| state["panels"][p] == "hidden")
    );
    // The Library's are its own.
    e.module = Module::Library;
    assert_eq!(json(e.command_state())["panels"]["right"], "shown");
    assert!(Action::names().contains(&"panels:right"));
}
