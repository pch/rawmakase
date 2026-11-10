//! A test-only extension, built with the SDK as a third party would build
//! one. The app's tests run it (re-running their own test binary, which links
//! this library), and it shows each kind of contribution:
//!
//! - `brighten`: reads its target (the selection) and proposes a higher
//!   Exposure for each photo, by the `step` setting, during its own
//!   `command.run` request;
//! - `settings`: answers with its settings, as `hello` and `setting-changed`
//!   left them;
//! - `read`: answers with what it reads of the editor;
//! - `wait`: runs until cancelled, saying so in the status line;
//!   `propose-late` then proposes anyway, and reports what the app answered
//!   with a notification;
//! - `refuse`: fails at once, which is no useful work;
//! - `hang` and `crash`: a program that stops answering, and one that exits;
//! - the `note` panel: a text field validated by the program, and a button.
//!
//! A file `crash-on-event` in its working folder makes the program exit at
//! the first event it gets, after its handshake: a start that does nothing
//! useful.
use rawmakase_extension::protocol::controls::{Control, ControlKind};
use rawmakase_extension::protocol::method::{AppEvent, CommandRun, Level, SettingsChange};
use rawmakase_extension::{Context, ErrorCode, Extension, Panel, RemoteError};
use serde_json::{Map, Value, json};
use std::time::Duration;

/// The note panel's text, which the program keeps.
#[derive(Default)]
struct Note {
    text: String,
}

/// The longest note the panel accepts.
const NOTE: usize = 10;

impl Note {
    fn controls(&self, error: Option<String>) -> Vec<Control> {
        vec![
            Control {
                id: Some("text".into()),
                label: Some("Note".into()),
                help: error,
                visible_when: None,
                disabled: false,
                kind: ControlKind::Text {
                    value: self.text.clone(),
                    multiline: false,
                },
            },
            Control {
                id: Some("clear".into()),
                label: None,
                help: None,
                visible_when: None,
                disabled: false,
                kind: ControlKind::Button {
                    title: "Clear".into(),
                },
            },
        ]
    }
}

impl Panel for Note {
    fn open(&mut self, _cx: &Context) -> Result<Vec<Control>, RemoteError> {
        Ok(self.controls(None))
    }
    fn changed(
        &mut self,
        _cx: &Context,
        control: &str,
        value: Value,
    ) -> Result<Vec<Control>, RemoteError> {
        let text = value.as_str().unwrap_or_default();
        if control != "text" {
            return Err(RemoteError::new(
                ErrorCode::InvalidRequest,
                "no such control",
            ));
        }
        if text.chars().count() > NOTE {
            return Ok(self.controls(Some(format!("At most {NOTE} characters"))));
        }
        self.text = text.to_string();
        Ok(self.controls(None))
    }
    fn clicked(&mut self, _cx: &Context, _control: &str) -> Result<Vec<Control>, RemoteError> {
        self.text.clear();
        Ok(self.controls(None))
    }
}

/// Exposure raised by `step` on every photo of the command's target.
fn brighten(cx: &Context, run: CommandRun) -> Result<Value, RemoteError> {
    let target = run
        .target
        .ok_or_else(|| RemoteError::new(ErrorCode::InvalidRequest, "no target"))?;
    let step = cx
        .settings()
        .get("step")
        .and_then(Value::as_f64)
        .unwrap_or(0.5);
    let changes = target
        .photos
        .iter()
        .map(|p| {
            let exposure = p
                .settings
                .get("exposure")
                .and_then(Value::as_f64)
                .unwrap_or(0.);
            let mut settings = Map::new();
            settings.insert("exposure".into(), json!(exposure + step));
            SettingsChange {
                photo: p.id,
                settings,
            }
        })
        .collect();
    let proposed = cx.host().propose_settings(cx.op(), target.token, changes)?;
    Ok(serde_json::to_value(proposed).expect("a reply is JSON"))
}

/// What it reads of the editor: the selection, the open photo and how many
/// parameters the control layer has.
fn read(cx: &Context, _run: CommandRun) -> Result<Value, RemoteError> {
    let selection = cx.host().read_selection()?;
    let open = cx.host().read_open_photo()?;
    let capabilities = cx.host().read_capabilities()?;
    let parameters = capabilities
        .get("parameters")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    Ok(json!({
        "selection": selection.photos.iter().map(|p| p.id).collect::<Vec<_>>(),
        "open": open.photos.iter().map(|p| p.id).collect::<Vec<_>>(),
        "parameters": parameters,
    }))
}

/// Says in the status line that it waits, then waits for its cancellation.
fn wait_until_cancelled(cx: &Context) {
    let _ = cx.host().status(Some("Waiting"));
    while !cx.is_cancelled() {
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The extension, ready to run.
pub fn extension() -> Extension {
    Extension::new()
        .command("brighten", brighten)
        .command("settings", |cx: &Context, _run: CommandRun| {
            Ok(Value::Object(cx.settings().clone()))
        })
        .command("read", read)
        .command("wait", |cx: &Context, _run: CommandRun| {
            wait_until_cancelled(cx);
            cx.host().status(None)?;
            Err(RemoteError::new(ErrorCode::Cancelled, "cancelled"))
        })
        .command("propose-late", |cx: &Context, run: CommandRun| {
            wait_until_cancelled(cx);
            let target =
                run.target
                    .unwrap_or_else(|| rawmakase_extension::protocol::method::Snapshot {
                        token: String::new(),
                        photos: Vec::new(),
                    });
            let changes = target
                .photos
                .iter()
                .map(|p| {
                    let mut settings = Map::new();
                    settings.insert("exposure".into(), json!(1.5));
                    SettingsChange {
                        photo: p.id,
                        settings,
                    }
                })
                .collect();
            let answer = match cx.host().propose_settings(cx.op(), target.token, changes) {
                Ok(_) => "applied".to_string(),
                Err(e) => e.code.to_string(),
            };
            cx.host().status(None)?;
            cx.host()
                .notify(Level::Info, &format!("late proposal: {answer}"))?;
            Err(RemoteError::new(ErrorCode::Cancelled, "cancelled"))
        })
        .command(
            "refuse",
            |_cx: &Context, _run: CommandRun| -> Result<Value, RemoteError> {
                Err(RemoteError::new(ErrorCode::InvalidRequest, "refused"))
            },
        )
        .command(
            "hang",
            |cx: &Context, _run: CommandRun| -> Result<Value, RemoteError> {
                // Answers nothing, cancellation and quitting included, until the
                // app ends it.
                cx.host().status(Some("Hanging"))?;
                std::thread::sleep(Duration::from_secs(120));
                Ok(Value::Null)
            },
        )
        .command(
            "crash",
            |_cx: &Context, _run: CommandRun| -> Result<Value, RemoteError> {
                std::process::exit(3)
            },
        )
        .panel("note", Note::default())
        .on_event(|event, _host| {
            if !matches!(event, AppEvent::Quitting(_))
                && std::path::Path::new("crash-on-event").exists()
            {
                std::process::exit(4);
            }
        })
}
