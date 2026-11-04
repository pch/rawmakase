use super::*;
use crate::{app::Module, model::params::ParameterId};
mod mapping_tests {
    use super::*;
    use crate::model::recipe::{Recipe, TEMPERATURE_MAX, TEMPERATURE_MIN, TINT_LIMIT};
    #[test]
    fn encoder_values_are_relative() {
        assert_eq!(
            (
                Encoder::TwosComplement.ticks(1),
                Encoder::TwosComplement.ticks(127),
                Encoder::TwosComplement.ticks(3),
                Encoder::TwosComplement.ticks(125)
            ),
            (Some(1), Some(-1), Some(3), Some(-3))
        );
    }
    #[test]
    fn parses_device_messages() {
        assert!(matches!(
            midi::parse(&[0xB0, 48, 1], None),
            Some(Msg::Cc(48, 1))
        ));
        assert!(matches!(
            midi::parse(&[0x90, 68, 64], None),
            Some(Msg::Note(68, true))
        ));
        assert!(matches!(
            midi::parse(&[0x80, 68, 64], None),
            Some(Msg::Note(68, false))
        ));
        assert!(midi::parse(&[0xF8], None).is_none());
    }
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
    fn config_overrides_defaults() {
        let mut config = Config::defaults();
        config.apply(&serde_json::json!({
            "dials": {"41": "contrast", "33": null},
            "buttons": {"98": "cmd+shift+u", "95": null, "99": "hold:shift"}
        }));
        assert_eq!(
            config.dials.get(&41),
            Some(&Param::Setting(ParameterId::Contrast))
        );
        assert!(!config.dials.contains_key(&33));
        assert_eq!(
            config.buttons.get(&98),
            Some(&Action::Key(Key::U, command() | SHIFT))
        );
        assert!(!config.buttons.contains_key(&95));
        assert_eq!(config.buttons.get(&99), Some(&Action::Hold(SHIFT)));
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
    #[test]
    fn saved_mapping_reads_back_without_inheriting_removed_controls() {
        let defaults = Config::defaults();
        let saved = defaults.to_json();
        assert_eq!(saved["dials"]["33"], "exposure");
        assert_eq!(
            parse_action(saved["buttons"]["95"].as_str().unwrap()),
            defaults.buttons.get(&95).copied()
        );
        let mut config = Config::defaults();
        config.dials.insert(41, Param::Hsl(2, 1));
        config.dials.insert(42, Param::Gray(7));
        config.dials.remove(&33);
        config
            .buttons
            .insert(50, Action::Key(Key::U, command() | SHIFT));
        config.buttons.insert(110, Action::Hold(Modifiers::ALT));
        config.buttons.insert(111, Action::Mixer(2));
        config.buttons.remove(&95);
        config.photo_dial = None;
        let read = Config::from_json(&config.to_json()).unwrap();
        assert_eq!(read, config);
    }
    #[test]
    fn every_default_spells_back_to_itself() {
        let config = Config::defaults();
        for (cc, param) in &config.dials {
            assert_eq!(Param::parse(&param.spec()), Some(*param), "CC {cc}");
        }
        for (note, action) in &config.buttons {
            assert_eq!(
                parse_action(&action_spec(*action)),
                Some(*action),
                "note {note}"
            );
        }
        for (label, spec) in settings::PRESETS {
            assert!(
                spec.is_empty() || parse_action(spec).is_some(),
                "{label}: {spec}"
            );
        }
    }
    #[test]
    fn every_device_message_is_listed_for_the_page() {
        // The page lists what the device sends; the defaults must be among it.
        let config = Config::defaults();
        for cc in config.dials.keys().chain(config.photo_dial.iter()) {
            assert!(profiles::DIALS.iter().any(|(_, n)| n == cc), "CC {cc}");
        }
        for note in config.buttons.keys() {
            assert!(
                profiles::BUTTONS.iter().any(|(_, n)| n == note),
                "note {note}"
            );
        }
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
    fn disconnect_clears_modifiers_even_with_old_input_queued() {
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        let (tx, rx) = mpsc::sync_channel(1);
        e.controls = Hub::new(
            Settings {
                devices: vec![DeviceConfig::new(1, Profile::Loupedeck)],
                socket: false,
            },
            rx,
        );
        let old = midi::reset_connection(&e.controls.devices[0].status, &ctx);
        e.control_messages(
            vec![Msg::Midi(
                e.controls.devices[0].status.clone(),
                old,
                Box::new(Msg::Note(66, true)),
            )],
            &ctx,
        )
        .unwrap();
        assert!(e.controls.devices[0].held.shift);
        tx.send(Msg::Midi(
            e.controls.devices[0].status.clone(),
            old,
            Box::new(Msg::Note(68, true)),
        ))
        .unwrap();
        // The reset cannot be lost even though the bounded queue is full.
        let new = midi::reset_connection(&e.controls.devices[0].status, &ctx);
        e.control_commands(&ctx);
        assert_eq!(e.controls.devices[0].held, Modifiers::NONE);
        let undo = e.controls.devices[0]
            .translate(Msg::Note(95, true))
            .unwrap()
            .unwrap();
        assert!(matches!(
            undo.operation,
            commands::Operation::Action(commands::Action::Undo)
        ));
        e.control_messages(
            vec![Msg::Midi(
                e.controls.devices[0].status.clone(),
                new,
                Box::new(Msg::Note(66, true)),
            )],
            &ctx,
        )
        .unwrap();
        assert!(e.controls.devices[0].held.shift);
        midi::reset_connection(&e.controls.devices[0].status, &ctx);
        e.control_commands(&ctx);
        assert_eq!(e.controls.devices[0].held, Modifiers::NONE);
        // Changing the configured port creates a separate listener identity.
        let old_source = e.controls.devices[0].status.clone();
        let old_epoch = locked(&old_source).epoch;
        e.controls.devices[0] = Device::new(DeviceConfig::new(1, Profile::Loupedeck));
        e.control_messages(
            vec![Msg::Midi(
                old_source,
                old_epoch,
                Box::new(Msg::Note(66, true)),
            )],
            &ctx,
        )
        .unwrap();
        assert_eq!(e.controls.devices[0].held, Modifiers::NONE);
    }

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
        e.control_messages(vec![Msg::Cc(48, 1)], &ctx).unwrap();
        assert!(e.module == Module::Library);
        assert_eq!(e.library.as_ref().unwrap().selected(), Some(first));
        e.library.as_mut().unwrap().open_loupe();
        e.control_messages(vec![Msg::Cc(48, 1)], &ctx).unwrap();
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
        e.control_messages(vec![Msg::Cc(48, 127)], &ctx).unwrap();
        assert!(e.module == Module::Develop);
        assert_eq!(e.document.catalog_photo, Some(first));
        Ok(())
    }

    #[test]
    fn modifier_bindings_become_actions_without_keyboard_state() {
        let mut surface = Device::new(DeviceConfig::new(0, Profile::Loupedeck));
        let command = surface.translate(Msg::Note(92, true)).unwrap().unwrap();
        assert!(matches!(
            command.operation,
            commands::Operation::Action(commands::Action::Copy)
        ));
        let command = surface.translate(Msg::Note(95, true)).unwrap().unwrap();
        assert!(matches!(
            command.operation,
            commands::Operation::Action(commands::Action::Undo)
        ));
        surface.translate(Msg::Note(66, true)).unwrap();
        let command = surface.translate(Msg::Note(95, true)).unwrap().unwrap();
        assert!(matches!(
            command.operation,
            commands::Operation::Action(commands::Action::Redo)
        ));
        surface.translate(Msg::Note(66, false)).unwrap();
        assert_eq!(surface.held, Modifiers::NONE);
        let command = surface.translate(Msg::Note(51, true)).unwrap().unwrap();
        assert!(
            matches!(command.operation,commands::Operation::Metadata {edit:crate::app::library::photo_metadata::Edit::ToggleLabel(ref label),advance:false} if label=="Red")
        );
        surface.translate(Msg::Note(66, true)).unwrap();
        let command = surface.translate(Msg::Note(80, true)).unwrap().unwrap();
        assert!(matches!(
            command.operation,
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
        e.controls = Hub::new(
            Settings {
                devices: vec![DeviceConfig::new(1, Profile::Loupedeck)],
                socket: false,
            },
            rx,
        );
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
fn migration_preserves_legacy_overrides_and_custom_devices_are_empty() {
    let legacy = serde_json::json!({"port":"My Deck","socket":true,"dials":{"33":null,"41":"texture"},"buttons":{"95":null}});
    let mut settings = Settings::from_json(&legacy).unwrap();
    assert!(settings.socket);
    assert_eq!(settings.devices[0].port, "My Deck");
    assert!(!settings.devices[0].exact);
    assert!(!settings.devices[0].mapping.dials.contains_key(&33));
    assert_eq!(
        settings.devices[0].mapping.dials.get(&41),
        Some(&Param::Setting(ParameterId::Texture))
    );
    assert!(!settings.devices[0].mapping.buttons.contains_key(&95));
    let id = settings.add(Profile::Custom);
    let custom = settings.devices.iter_mut().find(|d| d.id == id).unwrap();
    assert!(custom.mapping.dials.is_empty() && custom.mapping.buttons.is_empty());
    assert!(custom.port.is_empty());
    custom.port = "Other device".into();
    custom
        .mapping
        .dials
        .insert(11, Param::Setting(ParameterId::Exposure));
    custom.mapping.encoders.insert(11, Encoder::Offset);
    let restored = Settings::from_json(&settings.to_json()).unwrap();
    assert_eq!(restored.devices, settings.devices);
    assert!(
        !restored.socket,
        "MIDI config must not enable script access"
    );
    assert!(Settings::default().devices.is_empty());
    assert!(Settings::from_json(&serde_json::json!({"version":99,"devices":[]})).is_err());
}

#[test]
fn port_selection_requires_an_unambiguous_binding() {
    let ports = vec![
        midi::Port {
            id: "a".into(),
            name: "Controller".into(),
        },
        midi::Port {
            id: "b".into(),
            name: "Controller".into(),
        },
    ];
    let mut binding = DeviceConfig::new(1, Profile::Custom);
    assert_eq!(midi::select_port(&binding, &ports).unwrap(), None);
    binding.port = "Controller".into();
    assert!(midi::select_port(&binding, &ports).is_err());
    binding.port_id = Some("b".into());
    assert_eq!(
        midi::select_port(&binding, &ports).unwrap(),
        Some("b".into())
    );
    assert_eq!(
        midi::select_port(&binding, &ports[..1]).unwrap(),
        None,
        "Do not switch to another identical controller after disconnect"
    );
}

#[test]
fn control_formats_and_midi_channels_are_explicit() {
    assert_eq!(Encoder::TwosComplement.ticks(127), Some(-1));
    assert_eq!(Encoder::Offset.ticks(63), Some(-1));
    assert_eq!(Encoder::Offset.ticks(64), Some(0));
    assert_eq!(Encoder::SignMagnitude.ticks(65), Some(-1));
    assert_eq!(Encoder::Absolute.ticks(127), None);
    assert!(midi::parse(&[0xb1, 12, 127], Some(1)).is_none());
    assert!(matches!(
        midi::parse(&[0xb1, 12, 127], Some(2)),
        Some(Msg::Cc(12, 127))
    ));
    let mut binding = DeviceConfig::new(1, Profile::Custom);
    binding
        .mapping
        .dials
        .insert(12, Param::Setting(ParameterId::Exposure));
    let mut device = Device::new(binding);
    assert!(matches!(
        device
            .translate(Msg::Cc(12, 127))
            .unwrap()
            .unwrap()
            .operation,
        commands::Operation::ControlValue(Param::Setting(ParameterId::Exposure), 127)
    ));
    device.binding.mapping.encoders.insert(12, Encoder::Offset);
    assert!(matches!(
        device
            .translate(Msg::Cc(12, 63))
            .unwrap()
            .unwrap()
            .operation,
        commands::Operation::Adjust(Param::Setting(ParameterId::Exposure), -1)
    ));
}

#[test]
fn editing_one_device_preserves_others_and_removed_input_is_ignored() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let (_, rx) = mpsc::sync_channel(8);
    e.controls = Hub::new(
        Settings {
            devices: vec![
                DeviceConfig::new(1, Profile::Loupedeck),
                DeviceConfig::new(2, Profile::Loupedeck),
            ],
            socket: false,
        },
        rx,
    );
    let source = e.controls.devices[0].status.clone();
    e.control_messages(
        vec![Msg::Midi(source.clone(), 0, Box::new(Msg::Note(66, true)))],
        &ctx,
    )
    .unwrap();
    assert!(e.controls.devices[0].held.shift);
    assert!(!e.controls.devices[1].held.shift);
    assert!(!e.controls.legacy.held.shift);
    let unaffected = e.controls.devices[1].identity;
    let mut updated = e.controls.settings.clone();
    updated.devices[0]
        .mapping
        .dials
        .insert(41, Param::Setting(ParameterId::Texture));
    e.controls.apply(updated.clone());
    assert_eq!(e.controls.devices[1].identity, unaffected);
    assert!(!e.controls.devices[0].held.shift);
    e.control_messages(
        vec![Msg::Midi(source, 0, Box::new(Msg::Note(66, true)))],
        &ctx,
    )
    .unwrap();
    assert!(!e.controls.devices[0].held.shift);
    let removed = e.controls.devices[0].status.clone();
    updated.devices.remove(0);
    e.controls.apply(updated);
    e.control_messages(
        vec![Msg::Midi(removed, 0, Box::new(Msg::Note(66, true)))],
        &ctx,
    )
    .unwrap();
    assert!(!e.controls.devices[0].held.shift);
}
