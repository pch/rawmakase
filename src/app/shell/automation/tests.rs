use super::*;
use crate::{app::Module, model::params::ParameterId};
/// The command a translated action names, before the editor's device semantics.
fn operation(op: commands::Result<Option<DeviceOp>>) -> commands::Operation {
    match op.unwrap().unwrap() {
        DeviceOp::Action { spec, advance } => action_operation(&spec, advance).unwrap(),
        op => panic!("not an action: {op:?}"),
    }
}
mod mapping_tests {
    use super::*;
    use crate::model::recipe::{Recipe, TEMPERATURE_MAX, TEMPERATURE_MIN, TINT_LIMIT};
    #[test]
    fn dials_clamp_and_label_like_sliders() {
        let mut r = Recipe::default();
        assert_eq!(
            Param::Setting(ParameterId::Contrast).turn(&mut r, 3, 0),
            "+3"
        );
        assert_eq!(
            Param::Setting(ParameterId::Contrast).turn(&mut r, -5, 0),
            "-2"
        );
        assert_eq!(
            Param::Setting(ParameterId::Exposure).turn(&mut r, 5, 0),
            "+0.10"
        );
        Param::Setting(ParameterId::Highlights).turn(&mut r, 1000, 0);
        assert_eq!(r.highlights, 1.);
        Param::Setting(ParameterId::Tint).turn(&mut r, -1000, 0);
        assert_eq!(r.tint, -TINT_LIMIT);
    }
    #[test]
    fn a_dial_never_clamps_an_imported_exposure_beyond_the_slider() {
        let mut r = Recipe {
            exposure: 6.,
            ..Recipe::default()
        };
        // Clockwise would move it further out: it stays.
        assert_eq!(
            Param::Setting(ParameterId::Exposure).turn(&mut r, 1, 0),
            "+6.00"
        );
        // Anticlockwise moves one step toward the slider, not to its end.
        assert_eq!(
            Param::Setting(ParameterId::Exposure).turn(&mut r, -1, 0),
            "+5.98"
        );
        // An exact value may be anything a recipe holds.
        assert_eq!(
            Param::Setting(ParameterId::Exposure).set(&mut r, 7.5, 0),
            "+7.50"
        );
        assert_eq!(
            Param::Setting(ParameterId::Exposure).set(&mut r, 9., 0),
            "+8.00"
        );
        assert_eq!(
            Param::Setting(ParameterId::Contrast).set(&mut r, 150., 0),
            "+100"
        );
    }
    #[test]
    fn temperature_clockwise_is_warmer_and_stays_in_range() {
        let mut r = Recipe {
            temperature: 5000.,
            ..Recipe::default()
        };
        Param::Setting(ParameterId::Temperature).turn(&mut r, 1, 0);
        assert!(r.temperature > 5000.);
        Param::Setting(ParameterId::Temperature).turn(&mut r, 100_000, 0);
        assert_eq!(r.temperature, TEMPERATURE_MAX);
        Param::Setting(ParameterId::Temperature).turn(&mut r, -100_000, 0);
        assert_eq!(r.temperature, TEMPERATURE_MIN);
    }
    #[test]
    fn faders_turn_the_selected_mixer_channel() {
        let mut r = Recipe::default();
        assert_eq!(Param::Band(2).turn(&mut r, 5, 1), "+5");
        assert!((r.hsl[2][1] - 0.05).abs() < 1e-6);
        assert_eq!((r.hsl[2][0], r.hsl[2][2]), (0., 0.));
        assert_eq!(Param::Band(2).label(1), "Yellow Saturation");
        r.effects.monochrome = true;
        Param::Band(7).turn(&mut r, -3, 1);
        assert!((r.effects.gray_mix[7] + 0.03).abs() < 1e-6);
        assert_eq!(r.hsl[7], [0.; 3]);
    }
    #[test]
    fn sliders_are_set_in_the_units_they_show() {
        let mut r = Recipe::default();
        assert_eq!(
            Param::Setting(ParameterId::Exposure).set(&mut r, 1.5, 0),
            "+1.50"
        );
        assert_eq!(Param::Setting(ParameterId::Exposure).shown(&mut r, 0), 1.5);
        assert_eq!(
            Param::Setting(ParameterId::Contrast).set(&mut r, 35., 0),
            "+35"
        );
        assert!((r.contrast - 0.35).abs() < 1e-6);
        assert_eq!(Param::Setting(ParameterId::Contrast).shown(&mut r, 0), 35.);
        Param::Setting(ParameterId::Exposure).set(&mut r, 99., 0);
        assert_eq!(r.exposure, crate::model::recipe::EXPOSURE_LIMIT);
        Param::Setting(ParameterId::Temperature).set(&mut r, 1., 0);
        assert_eq!(r.temperature, TEMPERATURE_MIN);
        // A band addressed by channel ignores the Mixer's selector and B&W.
        r.effects.monochrome = true;
        Param::Hsl(2, 1).set(&mut r, -40., 0);
        Param::Gray(2).set(&mut r, 10., 0);
        assert!((r.hsl[2][1] + 0.4).abs() < 1e-6);
        assert!((r.effects.gray_mix[2] - 0.1).abs() < 1e-6);
        assert_eq!(Param::Gray(2).label(0), "Yellow Gray");
        assert_eq!(Param::Hsl(2, 1).label(0), "Yellow Saturation");
    }
    #[test]
    fn straighten_is_set_and_turned_in_degrees() {
        let mut r = Recipe::default();
        assert_eq!(
            Param::Setting(ParameterId::Straighten).set(&mut r, -2.5, 0),
            "-2.50"
        );
        assert_eq!(r.straighten, -2.5);
        assert_eq!(
            Param::Setting(ParameterId::Straighten).shown(&mut r, 0),
            -2.5
        );
        assert_eq!(
            Param::Setting(ParameterId::Straighten).turn(&mut r, 5, 0),
            "-2.00"
        );
        assert_eq!(Param::Setting(ParameterId::Straighten).label(0), "Angle");
        Param::Setting(ParameterId::Straighten).set(&mut r, 90., 0);
        assert_eq!(r.straighten, 45.);
        Param::Setting(ParameterId::Straighten).turn(&mut r, -1000, 0);
        assert_eq!(r.straighten, -45.);
        assert!(r.validate().is_ok());
        assert_eq!(
            Param::Setting(ParameterId::Straighten).range(false),
            (-45., 45.)
        );
        let mask = crate::model::masks::LocalAdjust::default();
        assert_eq!(
            Param::Setting(ParameterId::Straighten).local_shown(&mask),
            None
        );
    }
    #[test]
    fn slider_names_parse_with_bands_and_channels() {
        assert_eq!(
            Param::parse("Temp"),
            Some(Param::Setting(ParameterId::Temperature))
        );
        assert_eq!(
            Param::parse("straighten"),
            Some(Param::Setting(ParameterId::Straighten))
        );
        assert_eq!(
            Param::parse("Angle"),
            Some(Param::Setting(ParameterId::Straighten))
        );
        assert_eq!(Param::parse("band3.sat"), Some(Param::Hsl(2, 1)));
        assert_eq!(Param::parse("band8.gray"), Some(Param::Gray(7)));
        assert_eq!(Param::parse("band1"), Some(Param::Band(0)));
        assert_eq!(Param::parse("band1.alpha"), None);
        assert_eq!(Param::parse("band0.hue"), None);
        for (name, param) in Param::NAMED {
            assert_eq!(Param::parse(name), Some(param));
        }
    }
    #[test]
    fn default_keys_parse_the_way_the_names_say() {
        assert_eq!(
            parse_action("Cmd+Shift+Z"),
            Some(Action::Key(Key::Z, command() | SHIFT))
        );
        assert_eq!(
            parse_action("backslash"),
            Some(Action::Key(Key::Backslash, Modifiers::NONE))
        );
        assert_eq!(parse_action("hold:alt"), Some(Action::Hold(Modifiers::ALT)));
        assert_eq!(
            parse_action("alt+arrowleft"),
            Some(Action::Key(Key::ArrowLeft, Modifiers::ALT))
        );
        assert_eq!(
            parse_action("Left"),
            Some(Action::Key(Key::ArrowLeft, Modifiers::NONE))
        );
        assert_eq!(parse_action("nonsense"), None);
    }
    #[test]
    fn panel_keys_on_a_button_hide_panels_as_on_the_keyboard() {
        use crate::app::shell::panels::{PanelChange, WorkspacePanel};
        let run = |spec| match supported_action(spec) {
            Some(Action::Key(key, modifiers)) => shortcut_action(key, modifiers),
            _ => None,
        };
        assert_eq!(
            run("tab"),
            Some(commands::Action::Panels(PanelChange::Sides))
        );
        assert_eq!(
            run("shift+tab"),
            Some(commands::Action::Panels(PanelChange::All))
        );
        assert_eq!(
            run("f8"),
            Some(commands::Action::Panels(PanelChange::Toggle(
                WorkspacePanel::Right
            )))
        );
        assert_eq!(
            run("shift+f8"),
            Some(commands::Action::Panels(PanelChange::Toggle(
                WorkspacePanel::Presets
            )))
        );
        assert_eq!(
            parse_action("panels:filmstrip"),
            Some(Action::Named(commands::Action::Panels(
                PanelChange::Toggle(WorkspacePanel::Filmstrip)
            )))
        );
    }
    /// What the MIDI extension sends is the app's notation: every binding of
    /// its Loupedeck+ profile and every choice its page offers is a slider or
    /// an action the app runs, and spells back to itself.
    #[test]
    fn the_midi_extensions_bindings_are_actions_the_app_runs() {
        let profile = rawmakase_midi::profiles::Profile::Loupedeck.mapping();
        for (cc, name) in &profile.dials {
            let param = Param::parse(name).unwrap_or_else(|| panic!("CC {cc}: {name}"));
            assert_eq!(param.spec(), *name, "CC {cc}");
        }
        for (note, spec) in &profile.buttons {
            let action = supported_action(spec).unwrap_or_else(|| panic!("note {note}: {spec}"));
            assert_eq!(
                parse_action(&action_spec(action)),
                Some(action),
                "note {note}"
            );
        }
        for (label, spec) in rawmakase_midi::PRESETS {
            assert!(
                spec.is_empty() || supported_action(spec).is_some(),
                "{label}: {spec}"
            );
            assert!(spec.is_empty() || action_supported(spec), "{label}: {spec}");
        }
        assert!(!action_supported("nonsense"));
        assert!(!action_supported("cmd+q"), "a shortcut with no action");
    }
    #[test]
    fn held_modifiers_join_and_leave() {
        let held = Modifiers::NONE | SHIFT;
        assert!(held.shift);
        assert_eq!(without(held, SHIFT), Modifiers::NONE);
        let both = held | command();
        assert!(both.shift && both.command);
        assert_eq!(without(both, command()), SHIFT);
    }
}

mod integration_tests {
    use super::*;
    #[test]
    fn photo_dial_ignores_grid_preserves_loupe_and_navigates_develop() -> anyhow::Result<()> {
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        e.onboarding.visible = false;
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
        e.library = Some(Box::new(crate::app::library::Library::load(
            &db,
            ctx.clone(),
        )?));
        let library = e.library.as_mut().unwrap();
        let first = library.session.photos()[0].id;
        library.make_active(first);
        let second = library.navigate(first, 1).unwrap();
        e.module = Module::Library;
        e.control_messages(vec![Msg::Legacy(Input::Cc(48, 1))], &ctx)
            .unwrap();
        assert!(e.module == Module::Library);
        assert_eq!(e.library.as_ref().unwrap().selected(), Some(first));
        e.library.as_mut().unwrap().open_loupe();
        e.control_messages(vec![Msg::Legacy(Input::Cc(48, 1))], &ctx)
            .unwrap();
        assert!(e.module == Module::Library);
        assert!(e.library.as_ref().unwrap().loupe_open());
        assert_eq!(e.library.as_ref().unwrap().selected(), Some(second));
        // Semantic API navigation and device arrows preserve Loupe too.
        e.control_messages(
            vec![Msg::Command(Command::new(commands::Operation::Navigate(
                -1,
            )))],
            &ctx,
        )
        .unwrap();
        assert!(e.module == Module::Library && e.library.as_ref().unwrap().loupe_open());
        assert_eq!(e.library.as_ref().unwrap().selected(), Some(first));
        e.control_messages(
            vec![Msg::Command(Command::new(commands::Operation::Action(
                commands::Action::Next,
            )))],
            &ctx,
        )
        .unwrap();
        assert!(e.module == Module::Library && e.library.as_ref().unwrap().loupe_open());
        assert_eq!(e.library.as_ref().unwrap().selected(), Some(second));
        e.library.as_mut().unwrap().show_grid();
        e.control_messages(
            vec![Msg::Command(Command::new(commands::Operation::Action(
                commands::Action::Previous,
            )))],
            &ctx,
        )
        .unwrap();
        assert!(e.module == Module::Library && !e.library.as_ref().unwrap().loupe_open());
        assert_eq!(e.library.as_ref().unwrap().selected(), Some(first));
        e.module = Module::Develop;
        e.document.catalog_photo = Some(second);
        e.control_messages(vec![Msg::Legacy(Input::Cc(48, 127))], &ctx)
            .unwrap();
        assert!(e.module == Module::Develop);
        assert_eq!(e.document.catalog_photo, Some(first));
        Ok(())
    }

    #[test]
    fn modifier_bindings_become_actions_without_keyboard_state() {
        let mut surface = Legacy::default();
        let command = operation(surface.translate(Input::Note(92, true)));
        assert!(matches!(
            command,
            commands::Operation::Action(commands::Action::Copy)
        ));
        let command = operation(surface.translate(Input::Note(95, true)));
        assert!(matches!(
            command,
            commands::Operation::Action(commands::Action::Undo)
        ));
        surface.translate(Input::Note(66, true)).unwrap();
        let command = operation(surface.translate(Input::Note(95, true)));
        assert!(matches!(
            command,
            commands::Operation::Action(commands::Action::Redo)
        ));
        surface.translate(Input::Note(66, false)).unwrap();
        assert_eq!(surface.held, Modifiers::NONE);
        let command = operation(surface.translate(Input::Note(51, true)));
        assert!(
            matches!(command,commands::Operation::Metadata {edit:crate::app::library::photo_metadata::Edit::ToggleLabel(ref label),advance:false} if label=="Red")
        );
        surface.translate(Input::Note(66, true)).unwrap();
        let command = operation(surface.translate(Input::Note(80, true)));
        assert!(matches!(
            command,
            commands::Operation::Metadata {
                edit: crate::app::library::photo_metadata::Edit::Rating(1),
                advance: true
            }
        ));
    }
    #[test]
    fn each_socket_request_runs_and_replies_before_the_next() {
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        e.onboarding.visible = false;
        let (tx, rx) = mpsc::sync_channel(4);
        e.controls = Hub::new(Settings::default(), rx);
        let (first, r1) = socket::test_request(vec![Msg::Command(Command::new(
            commands::Operation::Set(Param::Setting(ParameterId::Exposure), 1.),
        ))]);
        let (second, r2) =
            socket::test_request(vec![Msg::Command(Command::new(commands::Operation::State))]);
        tx.send(Msg::Request(first)).unwrap();
        tx.send(Msg::Request(second)).unwrap();
        e.control_commands(&ctx);
        assert_eq!(r1.recv().unwrap().unwrap_err().code, "no_document");
        assert!(r2.recv().unwrap().is_ok());
    }
    /// A window macOS stops drawing (behind other apps) gets passes that run only
    /// `App::logic`: control requests and the worker results they wait on are
    /// still taken (#791).
    #[test]
    fn requests_and_worker_results_are_taken_without_drawing() {
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        let (tx, rx) = mpsc::sync_channel(4);
        e.controls = Hub::new(Settings::default(), rx);
        let (state, reply) =
            socket::test_request(vec![Msg::Command(Command::new(commands::Operation::State))]);
        tx.send(Msg::Request(state)).unwrap();
        e.tx.send(crate::app::worker::Event::CatalogWorking("Reading".into()))
            .unwrap();
        let _ = ctx.run_logic(&egui::RawInput::default(), |ctx| {
            eframe::App::logic(&mut e, ctx, &mut eframe::Frame::_new_kittest());
        });
        assert!(reply.try_recv().unwrap().is_ok());
        assert_eq!(e.catalog_work.as_deref(), Some("Reading"));
    }
    #[test]
    fn a_wait_is_answered_when_it_holds_or_when_its_time_is_up() {
        use std::time::Duration;
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        let (tx, rx) = mpsc::sync_channel(4);
        e.controls = Hub::new(Settings::default(), rx);
        let wait = |until, millis| {
            socket::test_request(vec![Msg::Command(Command::new(commands::Operation::Wait(
                until,
                Duration::from_millis(millis),
            )))])
        };
        // A job that does not exist has nothing to wait for.
        let (done, answer) = wait(commands::Until::Job(7), 5000);
        tx.send(Msg::Request(done)).unwrap();
        e.control_commands(&ctx);
        assert_eq!(answer.try_recv().unwrap().unwrap().status, "applied");
        // No photo is opened: the wait stays until its time is up.
        let (pending, answer) = wait(commands::Until::Loaded(crate::catalog::PhotoId(1)), 30);
        tx.send(Msg::Request(pending)).unwrap();
        e.control_commands(&ctx);
        assert!(answer.try_recv().is_err(), "answered before it held");
        std::thread::sleep(Duration::from_millis(40));
        e.control_commands(&ctx);
        assert_eq!(answer.try_recv().unwrap().unwrap().status, "timed_out");
    }
    /// A request naming a control adapter runs only while that adapter's
    /// extension is enabled: checked when it is dispatched and again when a
    /// parked wait is answered. One naming none runs as ever.
    #[test]
    fn a_disabled_adapters_requests_are_refused_even_once_parked() {
        use std::time::Duration;
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        let dir = tempfile::tempdir().unwrap();
        let never = rawmakase_extensions::session::Launch {
            program: dir.path().join("no-such-program"),
            args: Vec::new(),
            dir: std::path::PathBuf::new(),
            env: Vec::new(),
            log: std::path::PathBuf::new(),
        };
        e.extensions = crate::app::extensions::Host::start_for_tests(
            dir.path(),
            never,
            Duration::from_secs(30),
        );
        let (tx, rx) = mpsc::sync_channel(8);
        e.controls = Hub::new(Settings::default(), rx);
        let send = |adapter: Option<&str>, operation| {
            let (request, reply) =
                socket::test_request_from(vec![Msg::Command(Command::new(operation))], adapter);
            tx.send(Msg::Request(request)).unwrap();
            reply
        };
        let code = |reply: mpsc::Receiver<commands::Result<commands::Reply>>| {
            reply.try_recv().expect("answered").err().map(|e| e.code)
        };
        let mcp = Some("rawmakase.mcp/mcp");
        let (from_mcp, unknown, plain) = (
            send(mcp, commands::Operation::State),
            send(Some("dev.nobody/x"), commands::Operation::State),
            send(None, commands::Operation::State),
        );
        // No photo loads here: the wait stays parked.
        let parked = send(
            mcp,
            commands::Operation::Wait(
                commands::Until::Loaded(crate::catalog::PhotoId(1)),
                Duration::from_secs(30),
            ),
        );
        e.control_commands(&ctx);
        assert_eq!(code(from_mcp), None);
        assert_eq!(code(unknown), Some("adapter_disabled"));
        assert_eq!(code(plain), None);
        assert!(parked.try_recv().is_err(), "answered before it held");
        e.set_extension_enabled("rawmakase.mcp", false);
        let (queued, plain) = (
            send(mcp, commands::Operation::State),
            send(None, commands::Operation::State),
        );
        e.control_commands(&ctx);
        assert_eq!(code(parked), Some("adapter_disabled"));
        assert_eq!(code(queued), Some("adapter_disabled"));
        assert_eq!(code(plain), None);
    }
    /// A photo that could not be opened ends a wait for it: it will not load,
    /// and the state says it isn't loaded.
    #[test]
    fn a_wait_for_a_photo_that_failed_to_open_is_answered() {
        use std::time::Duration;
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        let (tx, rx) = mpsc::sync_channel(4);
        e.controls = Hub::new(Settings::default(), rx);
        e.module = Module::Develop;
        let photo = crate::catalog::PhotoId(1);
        e.document.reset(Some(photo));
        let (load, _) = e.document.start_load();
        let (pending, answer) = socket::test_request(vec![Msg::Command(Command::new(
            commands::Operation::Wait(commands::Until::Loaded(photo), Duration::from_secs(30)),
        ))]);
        tx.send(Msg::Request(pending)).unwrap();
        e.control_commands(&ctx);
        assert!(answer.try_recv().is_err(), "answered while loading");
        e.tx.send(crate::app::worker::Event::Failed {
            id: load,
            task: crate::app::worker::TaskKind::Load,
            error: "Could not read the AVIF file".into(),
        })
        .unwrap();
        e.events(&ctx);
        e.control_commands(&ctx);
        let reply = answer.try_recv().expect("answered").unwrap();
        assert_eq!(reply.status, "applied");
        // The state says which photo failed, so a client stops waiting.
        assert_eq!(reply.state.open_failed, Some(photo));
        assert!(!reply.state.loaded);
        // The reason stays the failure's when the status line moves on.
        e.messages.note("Previews built");
        assert_eq!(
            e.command_state().open_error.as_deref(),
            Some("Could not read the AVIF file")
        );
        e.document.reset(Some(photo));
        e.document.start_load();
        assert_eq!(e.command_state().open_failed, None);
        assert_eq!(e.command_state().open_error, None);
    }
}

#[test]
fn a_button_resolves_the_same_through_its_spec() {
    // Devices hand the core a spec; it must name what the action itself does.
    let modifiers = [
        Modifiers::NONE,
        SHIFT,
        Modifiers::CTRL,
        command(),
        Modifiers::ALT,
    ];
    let mut actions: Vec<_> = commands::Action::names()
        .into_iter()
        .filter_map(commands::Action::parse)
        .map(Action::Named)
        .chain([
            Action::ToggleMono,
            Action::Mixer(0),
            Action::Mixer(1),
            Action::Mixer(2),
        ])
        .collect();
    for key in Key::ALL {
        for a in modifiers {
            for b in modifiers {
                actions.push(Action::Key(*key, a | b));
            }
        }
    }
    for action in actions {
        let spec = action_spec(action);
        assert_eq!(
            parse_action(&spec).and_then(resolve),
            resolve(action),
            "{spec}"
        );
    }
}

#[test]
fn each_device_source_keeps_its_own_photo_dial_detent() {
    let mut hub = Hub::inactive();
    let device = |device, epoch| commands::Source::Device {
        session: 1,
        device,
        epoch,
    };
    let (a, b) = (device(1, 1), device(2, 1));
    assert_eq!(hub.photo_turn(a, 1, 2), 1);
    assert_eq!(hub.photo_turn(a, 1, 2), 0);
    // The other device's turn starts afresh and leaves this one's half detent.
    assert_eq!(hub.photo_turn(b, 1, 2), 1);
    assert_eq!(hub.photo_turn(a, 1, 2), 1);
    // So does the socket's legacy photo dial.
    assert_eq!(hub.photo_turn(commands::Source::Socket, 1, 2), 1);
    assert_eq!(hub.photo_turn(commands::Source::Socket, 1, 2), 0);
    // A reconnection, or another start of the program, starts a fresh turn,
    // which moves at once.
    assert_eq!(hub.photo_turn(device(1, 2), 1, 2), 1);
    let restarted = commands::Source::Device {
        session: 2,
        device: 1,
        epoch: 1,
    };
    assert_eq!(hub.photo_turn(restarted, 1, 2), 1);
    // A dial idle past a turn's pause is dropped: it would start afresh.
    for dial in &mut hub.photo_dials {
        dial.last -= PHOTO_IDLE * 2;
    }
    assert_eq!(hub.photo_turn(a, 1, 2), 1);
    assert_eq!(hub.photo_dials.len(), 1);
}

/// An extension's device operations run in order, each only while its
/// connection is the device's current one, read just before it runs.
#[test]
fn device_operations_run_only_in_their_current_epoch() {
    use crate::app::extensions::Devices;
    use rawmakase_extensions::protocol::method::{AdapterDevice, DeviceState};
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.onboarding.visible = false;
    let devices = Devices::default();
    let report = |state, epoch| AdapterDevice {
        adapter: "midi".into(),
        device: "1".into(),
        state,
        epoch,
    };
    let operate = |epoch, ops: Vec<Operation>| AdapterOperate {
        adapter: "midi".into(),
        device: "1".into(),
        epoch,
        ops,
    };
    let exposure = || Operation::Adjust {
        param: "exposure".into(),
        ticks: 5,
    };
    // Not connected: refused whole.
    let refused = e
        .device_operations(&devices, 1, operate(1, vec![exposure()]), "MIDI")
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::StaleTarget);
    devices.apply(&report(DeviceState::Connected, 1)).unwrap();
    // Connected, an operation that runs and fails (no photo is open) is said
    // and counts for nothing; one dropped in the Library grid ran.
    e.module = Module::Library;
    let photo = Operation::Photo {
        ticks: 1,
        detent: 2,
    };
    let unknown = Operation::Value {
        param: "sparkle".into(),
        value: 3,
    };
    let ran = e
        .device_operations(
            &devices,
            1,
            operate(1, vec![exposure(), photo, unknown]),
            "MIDI",
        )
        .unwrap();
    assert_eq!(ran.applied, 1);
    assert!(
        e.messages.line().starts_with("MIDI: "),
        "{}",
        e.messages.line()
    );
    // A new connection refuses the old one's.
    devices
        .apply(&report(DeviceState::Disconnected, 1))
        .unwrap();
    devices.apply(&report(DeviceState::Connected, 2)).unwrap();
    let stale = e
        .device_operations(&devices, 1, operate(1, vec![exposure()]), "MIDI")
        .unwrap_err();
    assert_eq!(stale.code, ErrorCode::StaleTarget);
}
