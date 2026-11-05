//! Editor tests of `session`: the whole editor driven through its frames.
use crate::app::*;

#[test]
fn session_preferences_use_the_injected_store() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("session.json");
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        Some(path.clone()),
    );
    editor.document.opened_at(dir.path().join("photo.ARW"));
    editor
        .tx
        .send(Event::Monitor(dir.path().join("display.icc")))
        .unwrap();
    editor.events(&ctx);
    let saved: crate::app::preferences::session::Session =
        serde_json::from_slice(&std::fs::read(path)?)?;
    assert_eq!(saved.last_path, editor.document.path);
    assert_eq!(saved.monitor, Some(dir.path().join("display.icc")));
    assert_eq!(
        editor.view.monitor,
        crate::app::preferences::monitor::MonitorProfile::Custom(dir.path().join("display.icc"))
    );
    Ok(())
}

/// Pinned panels are gone: a session saved with some still opens, the rest of it
/// kept.
#[test]
fn a_session_with_pinned_panels_still_opens() {
    let saved: crate::app::preferences::session::Session = serde_json::from_str(
        r#"{"pinned": ["Histogram", "Navigator"], "solo": ["develop-right"]}"#,
    )
    .expect("an older session");
    assert!(saved.solo.contains("develop-right"));
}

/// A frame with the window maximized or not, at `time`: the session's value
/// afterwards, and whether the frame asked to maximize the window.
fn window_frame(
    e: &mut Editor,
    ctx: &egui::Context,
    time: f64,
    maximized: bool,
    minimized: bool,
) -> (bool, bool) {
    let mut input = egui::RawInput {
        time: Some(time),
        ..Default::default()
    };
    input.viewports.insert(
        egui::ViewportId::ROOT,
        egui::ViewportInfo {
            maximized: Some(maximized),
            minimized: Some(minimized),
            ..Default::default()
        },
    );
    let mut output = ctx.run_ui(input, |ui| e.remember_maximized(ui.ctx()));
    output.textures_delta.clear();
    let asked = output.viewport_output[&egui::ViewportId::ROOT]
        .commands
        .contains(&egui::ViewportCommand::Maximized(true));
    let saved: crate::app::preferences::session::Session = e
        .session
        .file()
        .and_then(|file| std::fs::read(file).ok())
        .map_or_else(Default::default, |json| {
            serde_json::from_slice(&json).unwrap()
        });
    (saved.maximized, asked)
}

#[test]
fn the_window_reopens_maximized_as_it_was_left() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        Some(dir.path().join("session.json")),
    );
    assert_eq!(window_frame(&mut e, &ctx, 0., true, false), (true, false));
    // Minimized from maximized, Windows reports it not maximized.
    assert_eq!(window_frame(&mut e, &ctx, 1., false, true), (true, false));
    assert_eq!(window_frame(&mut e, &ctx, 2., true, false), (true, false));
    // Restored to its size.
    assert_eq!(window_frame(&mut e, &ctx, 3., false, false), (false, false));
    Ok(())
}

#[test]
fn a_window_reopened_maximized_keeps_that_until_the_window_manager_maximizes_it()
-> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("session.json");
    let session = || crate::app::preferences::session::Session {
        maximized: true,
        ..Default::default()
    };
    crate::storage::atomic_json(&file, &session())?;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, session(), Some(file.clone()));
    // An X11 window manager maximizes the window only once it is shown: the first
    // frame asks again, and the frames before it is maximized don't count.
    assert_eq!(window_frame(&mut e, &ctx, 0., false, false), (true, true));
    assert_eq!(window_frame(&mut e, &ctx, 0.5, false, false), (true, false));
    assert_eq!(window_frame(&mut e, &ctx, 1., true, false), (true, false));
    assert_eq!(
        window_frame(&mut e, &ctx, 1.5, false, false),
        (false, false)
    );

    // One that never maximizes it: after a while, the window is saved as it is.
    crate::storage::atomic_json(&file, &session())?;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, session(), Some(file.clone()));
    assert_eq!(window_frame(&mut e, &ctx, 0., false, false), (true, true));
    assert_eq!(window_frame(&mut e, &ctx, 1.9, false, false), (true, false));
    assert_eq!(
        window_frame(&mut e, &ctx, 2.1, false, false),
        (false, false)
    );

    // Maximized already (Windows, Wayland), or started minimized: nothing to ask.
    for minimized in [false, true] {
        crate::storage::atomic_json(&file, &session())?;
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(&ctx, None, session(), Some(file.clone()));
        assert_eq!(
            window_frame(&mut e, &ctx, 0., true, minimized),
            (true, false)
        );
        assert_eq!(window_frame(&mut e, &ctx, 0.1, true, false), (true, false));
    }
    Ok(())
}

/// What the session file says, as last written.
fn saved(path: &std::path::Path) -> crate::app::preferences::session::Session {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// The path saved is the catalog open, else the photo open, else the one the session
/// was opened with.
#[test]
fn the_session_saves_the_catalog_then_the_photo_then_the_path_it_opened_with() -> anyhow::Result<()>
{
    let (d, mut e, _) = crate::app::testing::editor_with_catalog(&["a.RAF"])?;
    let session = d.path().join("session.json");
    e.session.set_file(Some(session.clone()));
    let fallback = d.path().join("before.rawmakase");
    e.session.set_fallback_path(Some(fallback.clone()));
    e.document.opened_at(d.path().join("photos/a.RAF"));
    e.save_session()?;
    assert_eq!(
        saved(&session).last_path,
        Some(d.path().join("test.rawmakase"))
    );
    e.library = None;
    e.save_session()?;
    assert_eq!(
        saved(&session).last_path,
        Some(d.path().join("photos/a.RAF"))
    );
    e.document = Default::default();
    e.save_session()?;
    assert_eq!(saved(&session).last_path, Some(fallback));
    Ok(())
}

/// A setting another part of the app keeps reaches the file with the next write, and
/// fields a newer release wrote survive every write.
#[test]
fn every_write_holds_the_other_owners_settings_and_the_unknown_fields() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("session.json");
    std::fs::write(&path, r#"{"a_newer_field": {"kept": true}}"#)?;
    let loaded: crate::app::preferences::session::Session =
        serde_json::from_slice(&std::fs::read(&path)?)?;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, loaded, Some(path.clone()));
    e.updates.automatic = false;
    e.save_session()?;
    e.session.auto_advance = !e.session.auto_advance;
    e.save_session()?;
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    assert_eq!(json["a_newer_field"]["kept"], true);
    assert_eq!(json["no_update_checks"], true);
    assert_eq!(saved(&path).auto_advance, e.session.auto_advance);
    Ok(())
}

/// A write that fails says so, and the setting changed stays changed for the next one.
#[test]
fn a_failed_write_is_an_error_and_keeps_the_setting() {
    let dir = tempfile::tempdir().unwrap();
    // A file where the session's folder would be: the write cannot make it.
    std::fs::write(dir.path().join("blocked"), b"").unwrap();
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        Some(dir.path().join("blocked/session.json")),
    );
    e.session.collapsed.insert("Tone Curve".into());
    assert!(e.save_session().is_err());
    assert!(e.session.collapsed.contains("Tone Curve"));
}

/// Confirming Copy Settings saves its groups even when they are the ones already kept.
#[test]
fn confirming_copy_settings_saves_its_groups_even_unchanged() -> anyhow::Result<()> {
    use crate::app::develop::settings_transfer::{CopyChoice, Transfer};
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("session.json");
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        Some(path.clone()),
    );
    e.open_copy_dialog(Transfer::Copy);
    assert!(!path.exists());
    e.close_copy_dialog(CopyChoice::Confirm);
    assert!(path.exists());
    Ok(())
}
