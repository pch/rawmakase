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
        crate::app::session::Session::default(),
        Some(path.clone()),
    );
    editor.document.opened_at(dir.path().join("photo.ARW"));
    editor
        .tx
        .send(Event::Monitor(dir.path().join("display.icc")))
        .unwrap();
    editor.events(&ctx);
    let saved: crate::app::session::Session = serde_json::from_slice(&std::fs::read(path)?)?;
    assert_eq!(saved.last_path, editor.document.path);
    assert_eq!(saved.monitor, Some(dir.path().join("display.icc")));
    assert_eq!(
        editor.view.monitor,
        crate::app::monitor::MonitorProfile::Custom(dir.path().join("display.icc"))
    );
    Ok(())
}

/// Pinned panels are gone: a session saved with some still opens, the rest of it
/// kept.
#[test]
fn a_session_with_pinned_panels_still_opens() {
    let saved: crate::app::session::Session = serde_json::from_str(
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
    let saved: crate::app::session::Session = e
        .session_file
        .as_ref()
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
        crate::app::session::Session::default(),
        Some(dir.path().join("session.json")),
    );
    let keeps = workspace::KEEPS_MAXIMIZED;
    assert_eq!(window_frame(&mut e, &ctx, 0., true, false), (keeps, false));
    // Minimized from maximized, Windows reports it not maximized.
    assert_eq!(window_frame(&mut e, &ctx, 1., false, true), (keeps, false));
    assert_eq!(window_frame(&mut e, &ctx, 2., true, false), (keeps, false));
    // Restored to its size.
    assert_eq!(window_frame(&mut e, &ctx, 3., false, false), (false, false));
    Ok(())
}

#[test]
fn a_window_reopened_maximized_keeps_that_until_the_window_manager_maximizes_it()
-> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("session.json");
    let session = || crate::app::session::Session {
        maximized: true,
        ..Default::default()
    };
    crate::storage::atomic_json(&file, &session())?;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, session(), Some(file.clone()));
    let keeps = workspace::KEEPS_MAXIMIZED;
    // An X11 window manager maximizes the window only once it is shown: the first
    // frame asks again, and the frames before it is maximized don't count.
    assert_eq!(window_frame(&mut e, &ctx, 0., false, false), (true, keeps));
    assert_eq!(window_frame(&mut e, &ctx, 0.5, false, false), (true, false));
    assert_eq!(window_frame(&mut e, &ctx, 1., true, false), (true, false));
    assert_eq!(
        window_frame(&mut e, &ctx, 1.5, false, false),
        (!keeps, false)
    );

    // One that never maximizes it: after a while, the window is saved as it is.
    crate::storage::atomic_json(&file, &session())?;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, session(), Some(file.clone()));
    assert_eq!(window_frame(&mut e, &ctx, 0., false, false), (true, keeps));
    assert_eq!(window_frame(&mut e, &ctx, 1.9, false, false), (true, false));
    assert_eq!(
        window_frame(&mut e, &ctx, 2.1, false, false),
        (!keeps, false)
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
