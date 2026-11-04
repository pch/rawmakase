//! Editor tests of `camera_look`: the whole editor driven through its frames.
use crate::app::*;

/// Profile → Fit Camera Look: a refusal (or a panic, which arrives as one) says why
/// and leaves nothing running; a superseded fit is dropped; the fit is one History
/// step Undo takes back; a profile chosen while it ran wins over it.
#[test]
fn a_fitted_camera_look_is_one_step_and_yields_to_later_choices() {
    use crate::camera_profiles::fitted;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let m = crate::camera_data::Metadata {
        make: "Test".into(),
        model: "Camera".into(),
        cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
        ..Default::default()
    };
    let fit = fitted::with_look(
        &fitted::standard_base(&m).unwrap(),
        &fitted::FittedLook::identity(),
    )
    .unwrap();
    e.document.metadata = Some(m.clone());
    let before = e.document.edit.recipe().profile.clone();
    let (g, _) = e.document.camera_look.start();
    e.camera_look_ready(g, before.clone(), Err("Stopped unexpectedly: boom".into()));
    assert!(!e.document.camera_look.is_running());
    assert!(e.messages.has_entry_with("boom"));
    assert!(e.running_work().is_empty());
    assert_eq!(e.document.edit.history().steps().1, 0);
    // A fit asked for again: the first one's result no longer counts.
    let (first, _) = e.document.camera_look.start();
    let (second, _) = e.document.camera_look.start();
    e.camera_look_ready(first, before.clone(), Ok(Box::new(fit.clone())));
    assert_eq!(e.document.edit.recipe().profile, before);
    assert!(e.document.camera_look.is_running());
    assert_eq!(e.running_work()[0].key, "camera look");
    e.camera_look_ready(second, before.clone(), Ok(Box::new(fit.clone())));
    assert_eq!(e.document.edit.recipe().profile.as_deref(), Some(&fit));
    assert_eq!(e.document.edit.recipe().profile_amount, 1.);
    // Temp and Tint follow the new profile, as after a menu choice.
    let mut synced = e.document.edit.recipe().clone();
    synced.profile_changed(&m);
    assert_eq!(
        (synced.temperature, synced.tint, synced.wb),
        (
            e.document.edit.recipe().temperature,
            e.document.edit.recipe().tint,
            e.document.edit.recipe().wb
        )
    );
    let (steps, applied) = e.document.edit.history().steps();
    assert_eq!((applied, steps[0].name.as_str()), (1, "Fit Camera Look"));
    e.undo();
    assert_eq!(e.document.edit.recipe().profile, before);
    // Another profile chosen while it ran: that choice stays.
    let (g, _) = e.document.camera_look.start();
    let chosen = Some(std::sync::Arc::new(fitted::standard_base(&m).unwrap()));
    e.document.edit.setup_mut().profile = chosen.clone();
    e.camera_look_ready(g, before, Ok(Box::new(fit)));
    assert_eq!(e.document.edit.recipe().profile, chosen);
    assert!(e.messages.has_entry_with("changed"));
    assert!(!e.document.camera_look.is_running());
}

/// Listing a photo's profiles again after an import keeps the fitted default of a
/// photo still without colour data, and drops it once an imported profile fits.
#[test]
fn an_import_keeps_a_fitted_default_until_a_profile_fits() {
    use crate::camera_profiles::fitted;
    let m = crate::camera_data::Metadata {
        make: "Test".into(),
        model: "Bare".into(),
        ..Default::default()
    };
    let with_matrix = crate::camera_data::Metadata {
        cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
        ..m.clone()
    };
    let fit = std::sync::Arc::new(
        fitted::with_look(
            &fitted::standard_base(&with_matrix).unwrap(),
            &fitted::FittedLook::identity(),
        )
        .unwrap(),
    );
    let had = vec![fit];
    assert_eq!(
        crate::app::develop::camera_look::keep_fitted_default(&had, &m, vec![]),
        had
    );
    let mut dcp = fitted::standard_base(&with_matrix).unwrap();
    dcp.name = "Mine".into();
    let imported = vec![std::sync::Arc::new(dcp)];
    assert_eq!(
        crate::app::develop::camera_look::keep_fitted_default(&had, &m, imported.clone()),
        imported
    );
}
