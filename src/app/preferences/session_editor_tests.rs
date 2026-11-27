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

/// Drains editor events until the catalog finishes opening.
fn settle_catalog(editor: &mut Editor, ctx: &egui::Context) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while editor.activity.is_busy() && std::time::Instant::now() < deadline {
        editor.events(ctx);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    editor.events(ctx);
    assert!(!editor.activity.is_busy(), "catalog did not settle");
}

/// A catalog on disk with two photos inside `photos/trip`, returning the catalog
/// path, both photo ids and the subfolder's source key.
fn two_photo_catalog(
    dir: &tempfile::TempDir,
) -> anyhow::Result<(PathBuf, [PhotoId; 2], [PathBuf; 2], String)> {
    let trip = dir.path().join("photos/trip");
    std::fs::create_dir_all(&trip)?;
    let a_path = trip.join("a.RAF");
    let b_path = trip.join("b.RAF");
    std::fs::write(&a_path, b"fixture a")?;
    std::fs::write(&b_path, b"fixture b")?;
    let catalog = dir.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&catalog)?.add_folder(&trip)?;
    let opened = crate::app::library::Library::load(&catalog, egui::Context::default())?;
    let photos = opened.session.photos();
    let a = photos.iter().find(|p| p.filename == "a.RAF").unwrap().id;
    let b = photos.iter().find(|p| p.filename == "b.RAF").unwrap().id;
    let folder_key = format!("root:{}", opened.session.roots()[0].0.0);
    Ok((catalog, [a, b], [a_path, b_path], folder_key))
}

/// A clean shutdown restores the last view (Loupe or Develop) and selected photo
/// on the next launch without any crash notice.
#[test]
fn a_clean_shutdown_restores_loupe_and_develop_normally() -> anyhow::Result<()> {
    use crate::app::preferences::session::{LibraryLayout, Session};
    for develop in [false, true] {
        let dir = tempfile::tempdir()?;
        let (catalog, [_, b], _, folder_key) = two_photo_catalog(&dir)?;
        let session_file = dir.path().join("session.json");
        let saved_state = Session {
            last_path: Some(catalog),
            library_source: folder_key,
            selected_photo: Some(b),
            develop,
            library_layout: LibraryLayout {
                view: if develop { "grid" } else { "loupe" }.into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let mut first = Editor::with_context(&ctx, None, saved_state, Some(session_file.clone()));
        settle_catalog(&mut first, &ctx);
        first.exit_within(std::time::Duration::from_secs(5));
        drop(first);

        let relaunched_session = saved(&session_file);
        let mut second = Editor::with_context(&ctx, None, relaunched_session, Some(session_file));
        settle_catalog(&mut second, &ctx);
        assert!(
            !second.messages.has_entry_with("did not close cleanly"),
            "clean exit must not trigger crash notice"
        );
        if develop {
            assert_eq!(second.module, Module::Develop);
            assert_eq!(second.document.catalog_photo, Some(b));
        } else {
            assert_eq!(second.module, Module::Library);
            let library = second.library.as_ref().unwrap();
            assert!(library.loupe_open());
            assert_eq!(library.selected(), Some(b));
        }
        second.exit_within(std::time::Duration::from_secs(5));
    }
    Ok(())
}

/// After an unclean exit (crash in Loupe, Compare or Develop), the next launch
/// opens the catalog in Library Grid with no photo selected, preserves the
/// folder source, layout settings and preferences, shows a brief notice, and
/// returns to normal restore once that recovered session exits cleanly.
#[test]
fn an_unclean_exit_opens_in_grid_without_selection_and_recovers_after_clean_exit()
-> anyhow::Result<()> {
    use crate::app::preferences::session::{LibraryLayout, Session};
    use crate::app::shell::workspace::LayoutEdit;

    for (develop, view) in [(false, "loupe"), (false, "compare"), (true, "grid")] {
        let dir = tempfile::tempdir()?;
        let (catalog, [a, b], _, folder_key) = two_photo_catalog(&dir)?;
        let session_file = dir.path().join("session.json");
        let crashed_session = Session {
            last_path: Some(catalog.clone()),
            library_source: folder_key.clone(),
            selected_photo: Some(b),
            develop,
            auto_advance: true,
            theme: Some("nord.toml".into()),
            theme_chosen: true,
            decoded_cache_size_gib: Some(16),
            library_layout: LibraryLayout {
                view: view.into(),
                sort: "file_name".into(),
                reverse: true,
                ..Default::default()
            },
            ..Default::default()
        };
        crate::storage::atomic_json(&session_file, &crashed_session)?;

        // Launch 1 starts and dies without calling exit().
        let ctx = egui::Context::default();
        let first = Editor::with_context(&ctx, None, crashed_session, Some(session_file.clone()));
        drop(first);

        // Launch 2 detects the unclean exit and guards session restore.
        let mut second =
            Editor::with_context(&ctx, None, saved(&session_file), Some(session_file.clone()));
        settle_catalog(&mut second, &ctx);
        assert_eq!(second.module, Module::Library);
        assert!(second.document.catalog_photo.is_none());
        let library = second.library.as_ref().unwrap();
        assert!(!library.loupe_open());
        assert_eq!(library.layout().view, "grid");
        assert_eq!(library.selected(), None);
        assert_eq!(library.source_key(), folder_key);
        assert_eq!(library.layout().sort, "file_name");
        assert!(library.layout().reverse);
        assert!(second.session.auto_advance);
        assert_eq!(second.decode_cache.limit(), 16 << 30);
        assert!(
            second.messages.has_entry_with("did not close cleanly"),
            "expected recovery warning for develop={develop}, view={view}"
        );
        let persisted = saved(&session_file);
        assert_eq!(persisted.last_path.as_ref(), Some(&catalog));
        assert_eq!(persisted.selected_photo, None);
        assert!(!persisted.develop);
        assert_eq!(persisted.library_layout.view, "grid");
        assert!(persisted.auto_advance);
        assert_eq!(persisted.theme.as_deref(), Some("nord.toml"));
        assert_eq!(persisted.decoded_cache_size_gib, Some(16));

        // User picks a photo in Loupe and exits cleanly: Launch 3 restores normally.
        let library = second.library.as_mut().unwrap();
        library.select(Some(a));
        library.open_loupe();
        second.remember_place(LayoutEdit::Settled);
        second.exit_within(std::time::Duration::from_secs(5));
        drop(second);

        let mut third =
            Editor::with_context(&ctx, None, saved(&session_file), Some(session_file.clone()));
        settle_catalog(&mut third, &ctx);
        assert!(!third.messages.has_entry_with("did not close cleanly"));
        let library = third.library.as_ref().unwrap();
        assert!(library.loupe_open());
        assert_eq!(library.selected(), Some(a));
        third.exit_within(std::time::Duration::from_secs(5));
    }
    Ok(())
}

/// If a recovered session also crashes before a clean shutdown, the next launch
/// still guards session restore.
#[test]
fn a_second_crash_during_recovery_still_guards_the_next_launch() -> anyhow::Result<()> {
    use crate::app::preferences::session::{LibraryLayout, Session};
    use crate::app::shell::workspace::LayoutEdit;

    let dir = tempfile::tempdir()?;
    let (catalog, [_, b], _, _) = two_photo_catalog(&dir)?;
    let session_file = dir.path().join("session.json");
    let session = Session {
        last_path: Some(catalog),
        selected_photo: Some(b),
        library_layout: LibraryLayout {
            view: "loupe".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    crate::storage::atomic_json(&session_file, &session)?;

    let ctx = egui::Context::default();
    let first = Editor::with_context(&ctx, None, session, Some(session_file.clone()));
    drop(first);

    // Recovered session opens Loupe again, then also crashes without exit().
    let mut second =
        Editor::with_context(&ctx, None, saved(&session_file), Some(session_file.clone()));
    settle_catalog(&mut second, &ctx);
    let library = second.library.as_mut().unwrap();
    library.select(Some(b));
    library.open_loupe();
    second.remember_place(LayoutEdit::Settled);
    drop(second);

    let mut third =
        Editor::with_context(&ctx, None, saved(&session_file), Some(session_file.clone()));
    settle_catalog(&mut third, &ctx);
    assert!(third.messages.has_entry_with("did not close cleanly"));
    let library = third.library.as_ref().unwrap();
    assert!(!library.loupe_open());
    assert_eq!(library.selected(), None);
    third.exit_within(std::time::Duration::from_secs(5));
    Ok(())
}

/// A second instance started while the first is still running does not mistake
/// it for a crash, and exiting the second instance does not clear the first's
/// launch guard.
#[test]
fn a_concurrent_instance_does_not_trigger_or_clear_crash_recovery() -> anyhow::Result<()> {
    use crate::app::preferences::session::{LibraryLayout, Session};

    let dir = tempfile::tempdir()?;
    let (catalog, [_, b], _, _) = two_photo_catalog(&dir)?;
    let session_file = dir.path().join("session.json");
    let loupe_session = || Session {
        last_path: Some(catalog.clone()),
        selected_photo: Some(b),
        library_layout: LibraryLayout {
            view: "loupe".into(),
            ..Default::default()
        },
        ..Default::default()
    };

    let ctx = egui::Context::default();
    let first = Editor::with_context(&ctx, None, loupe_session(), Some(session_file.clone()));

    let mut second = Editor::with_context(&ctx, None, loupe_session(), Some(session_file.clone()));
    settle_catalog(&mut second, &ctx);
    assert!(!second.messages.has_entry_with("did not close cleanly"));
    assert!(second.library.as_ref().unwrap().loupe_open());
    assert_eq!(second.library.as_ref().unwrap().selected(), Some(b));
    second.exit_within(std::time::Duration::from_secs(5));
    drop(second);

    // First instance now crashes: Launch 3 must still detect the crash.
    drop(first);
    let mut third = Editor::with_context(&ctx, None, loupe_session(), Some(session_file));
    settle_catalog(&mut third, &ctx);
    assert!(third.messages.has_entry_with("did not close cleanly"));
    assert!(!third.library.as_ref().unwrap().loupe_open());
    assert_eq!(third.library.as_ref().unwrap().selected(), None);
    third.exit_within(std::time::Duration::from_secs(5));
    Ok(())
}

/// An unclean exit when nothing was selected or open in Loupe/Develop still
/// clears the Library's default selection, without showing a misleading notice.
#[test]
fn an_unclean_exit_without_saved_photo_or_view_clears_selection_silently() -> anyhow::Result<()> {
    use crate::app::preferences::session::{LibraryLayout, Session};

    let dir = tempfile::tempdir()?;
    let (catalog, _, _, _) = two_photo_catalog(&dir)?;
    let session_file = dir.path().join("session.json");
    let grid_session = || Session {
        last_path: Some(catalog.clone()),
        selected_photo: None,
        develop: false,
        library_layout: LibraryLayout {
            view: "grid".into(),
            ..Default::default()
        },
        ..Default::default()
    };

    let ctx = egui::Context::default();
    let first = Editor::with_context(&ctx, None, grid_session(), Some(session_file.clone()));
    drop(first);

    let mut second = Editor::with_context(&ctx, None, grid_session(), Some(session_file));
    settle_catalog(&mut second, &ctx);
    assert_eq!(second.module, Module::Library);
    assert_eq!(second.library.as_ref().unwrap().selected(), None);
    assert!(!second.messages.has_entry_with("did not close cleanly"));
    second.exit_within(std::time::Duration::from_secs(5));
    Ok(())
}

/// Passing an explicit photo on the command line after a crash still opens the
/// requested photo instead of the crashed one, without a contradictory Grid notice.
#[test]
fn an_explicit_cli_photo_still_opens_after_an_unclean_exit() -> anyhow::Result<()> {
    use crate::app::preferences::session::{LibraryLayout, Session};

    let dir = tempfile::tempdir()?;
    let (catalog, [a, b], [_, b_path], _) = two_photo_catalog(&dir)?;
    let session_file = dir.path().join("session.json");
    let crashed = || Session {
        last_path: Some(catalog.clone()),
        selected_photo: Some(a),
        develop: true,
        library_layout: LibraryLayout {
            view: "loupe".into(),
            ..Default::default()
        },
        ..Default::default()
    };

    let ctx = egui::Context::default();
    let first = Editor::with_context(&ctx, None, crashed(), Some(session_file.clone()));
    drop(first);

    let mut second = Editor::with_context(&ctx, Some(b_path), crashed(), Some(session_file));
    settle_catalog(&mut second, &ctx);
    assert_eq!(second.module, Module::Develop);
    assert_eq!(second.document.catalog_photo, Some(b));
    assert!(!second.messages.has_entry_with("did not close cleanly"));
    second.exit_within(std::time::Duration::from_secs(5));
    Ok(())
}

/// After an unclean exit whose `last_path` named a standalone photo (no catalog),
/// recovery skips it, clears it from `session.json` so it does not return on the
/// next clean launch, and words the notice without claiming Grid view opened.
#[test]
fn an_unclean_exit_with_a_standalone_last_photo_clears_it_and_adjusts_notice() -> anyhow::Result<()>
{
    use crate::app::preferences::session::Session;

    let dir = tempfile::tempdir()?;
    let photo = dir.path().join("bad.RAF");
    std::fs::write(&photo, b"fixture")?;
    let session_file = dir.path().join("session.json");
    let crashed = Session {
        last_path: Some(photo),
        auto_advance: true,
        ..Default::default()
    };
    crate::storage::atomic_json(&session_file, &crashed)?;

    let ctx = egui::Context::default();
    let first = Editor::with_context(&ctx, None, crashed, Some(session_file.clone()));
    drop(first);

    let mut second =
        Editor::with_context(&ctx, None, saved(&session_file), Some(session_file.clone()));
    assert!(second.library.is_none());
    assert!(
        second
            .messages
            .has_entry("RAWmakase did not close cleanly: skipped restoring the last photo")
    );
    let persisted = saved(&session_file);
    assert_eq!(persisted.last_path, None);
    assert!(persisted.auto_advance);

    second.exit_within(std::time::Duration::from_secs(5));
    drop(second);

    // Even after saving again before/during clean exit, the standalone photo does not come back.
    assert_eq!(saved(&session_file).last_path, None);
    Ok(())
}
