//! A test-only extension of retouch generators
//! (`remove/rawmakase-extension.toml`): each answers `retouch.generate` from
//! the region's pixels and coverage alone, so the app's tests can check what
//! was asked and what was kept, end to end through shared memory.
//!
//! - `fill` and `local`: the painted pixels (coverage above 0) a flat grey,
//!   [`GREY`]; the others as given;
//! - `gate`: writes `started` in its working folder, then answers as `fill`
//!   once `release` appears there, or `cancelled` when cancelled first;
//! - `setup`: `needs_setup` while the `key` setting is empty, then as `fill`;
//! - `refuse`: `refused`; `unavailable`: `unavailable`; `unnamed`: a model
//!   with an empty name;
//! - `crash`: exits;
//! - `ask`: fills as `local` with the text its panel `ask` (in the tool's
//!   drawer) holds, which it needs, and says it as its summary.
use rawmakase_extension::protocol::controls::{Control, ControlKind};
use rawmakase_extension::protocol::method::RetouchGenerated;
use rawmakase_extension::{
    Context, ErrorCode, Extension, Panel, RemoteError, RetouchAsk, RetouchGenerator,
};
use serde_json::Value;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The sRGB value `fill` writes over the painted pixels.
pub const GREY: [u8; 3] = [128, 128, 128];

/// One of the generators, by its id.
struct Generator(&'static str);

impl RetouchGenerator for Generator {
    fn generate(
        &mut self,
        cx: &Context,
        mut ask: RetouchAsk<'_>,
    ) -> Result<RetouchGenerated, RemoteError> {
        let model = RetouchGenerated {
            model: format!("example-{}@1", self.0),
            model_key: format!("example-{}/1", self.0),
            summary: None,
        };
        match self.0 {
            "refuse" => Err(RemoteError::new(
                ErrorCode::Refused,
                "the service declined this image",
            )),
            "unavailable" => Err(RemoteError::new(
                ErrorCode::Unavailable,
                "the service can't be reached",
            )),
            "unnamed" => Ok(RetouchGenerated {
                model: String::new(),
                ..model
            }),
            "crash" => std::process::exit(3),
            "setup"
                if cx
                    .settings()
                    .get("key")
                    .and_then(|v| v.as_str())
                    .is_none_or(str::is_empty) =>
            {
                Err(RemoteError::new(
                    ErrorCode::NeedsSetup,
                    "Set the API key in Preferences > Extensions",
                ))
            }
            "gate" => {
                // For a test to know the request is running here.
                let _ = std::fs::write("started", b"");
                if !wait_for("release", cx) {
                    return Err(RemoteError::new(ErrorCode::Cancelled, "cancelled"));
                }
                fill(&mut ask);
                Ok(model)
            }
            _ => {
                fill(&mut ask);
                Ok(model)
            }
        }
    }
}

/// Waits up to 20 s for `file` in the working folder; false when `cx` is
/// cancelled first, or it never appears.
fn wait_for(file: &str, cx: &Context) -> bool {
    let until = Instant::now() + Duration::from_secs(20);
    while Instant::now() < until {
        if Path::new(file).exists() {
            return true;
        }
        if cx.is_cancelled() {
            return false;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    false
}

/// Writes the image into the result, with the painted pixels [`GREY`].
fn fill(ask: &mut RetouchAsk<'_>) {
    let (image, coverage, result) = (
        &ask.request.image,
        &ask.request.coverage,
        &ask.request.result,
    );
    for y in 0..image.height as usize {
        for x in 0..image.width as usize {
            let from = &ask.image[y * image.stride as usize + x * 3..][..3];
            let painted = ask.coverage[y * coverage.stride as usize + x] > 0;
            let to = &mut ask.result[y * result.stride as usize + x * 3..][..3];
            to.copy_from_slice(if painted { &GREY } else { from });
        }
    }
}

/// What the `ask` panel's text field holds, shared with the `ask`
/// generator.
type Asked = Arc<Mutex<String>>;

/// The `ask` panel: one text field.
struct AskPanel(Asked);

impl AskPanel {
    fn controls(&self) -> Vec<Control> {
        use rawmakase_extension::protocol::controls::Choice;
        let choice = |id: &str, title: &str| Choice {
            id: id.into(),
            title: title.into(),
        };
        vec![
            // Drawn only, for how a drawer looks: the generator reads none.
            Control {
                id: Some("mode".into()),
                label: Some("Mode".into()),
                help: None,
                visible_when: None,
                disabled: false,
                kind: ControlKind::Segmented {
                    options: vec![choice("one", "One"), choice("two", "Two")],
                    value: "one".into(),
                },
            },
            Control {
                id: Some("text".into()),
                label: Some("Prompt".into()),
                help: Some("Needed: what to put where you paint".into()),
                visible_when: None,
                disabled: false,
                kind: ControlKind::Text {
                    value: self.0.lock().unwrap().clone(),
                    multiline: true,
                    placeholder: Some("What to put there".into()),
                },
            },
        ]
    }
}

impl Panel for AskPanel {
    fn open(&mut self, _cx: &Context) -> Result<Vec<Control>, RemoteError> {
        Ok(self.controls())
    }
    fn changed(
        &mut self,
        _cx: &Context,
        control: &str,
        value: Value,
    ) -> Result<Vec<Control>, RemoteError> {
        if control == "text" {
            *self.0.lock().unwrap() = value.as_str().unwrap_or_default().to_string();
        }
        Ok(self.controls())
    }
    fn clicked(&mut self, _cx: &Context, _control: &str) -> Result<Vec<Control>, RemoteError> {
        Ok(self.controls())
    }
}

/// The `ask` generator: fills with what the panel holds, which it needs.
struct Ask(Asked);

impl RetouchGenerator for Ask {
    fn generate(
        &mut self,
        _cx: &Context,
        mut ask: RetouchAsk<'_>,
    ) -> Result<RetouchGenerated, RemoteError> {
        let text = self.0.lock().unwrap().trim().to_string();
        if text.is_empty() {
            return Err(RemoteError::new(
                ErrorCode::NeedsInput,
                "Say what to put there first",
            ));
        }
        fill(&mut ask);
        Ok(RetouchGenerated {
            model: "example-ask@1".into(),
            model_key: "example-ask/1".into(),
            summary: Some(text.replace(char::is_control, " ")),
        })
    }
}

/// The extension, ready to run.
pub fn extension() -> Extension {
    let asked = Asked::default();
    [
        "fill",
        "local",
        "gate",
        "setup",
        "refuse",
        "unavailable",
        "unnamed",
        "crash",
    ]
    .into_iter()
    .fold(Extension::new(), |e, id| {
        e.retouch_generator(id, Generator(id))
    })
    .retouch_generator("ask", Ask(asked.clone()))
    .panel("ask", AskPanel(asked))
}
