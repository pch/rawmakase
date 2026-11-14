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
//! - `crash`: exits.
use rawmakase_extension::protocol::method::RetouchGenerated;
use rawmakase_extension::{
    Context, ErrorCode, Extension, RemoteError, RetouchAsk, RetouchGenerator,
};
use std::path::Path;
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

/// The extension, ready to run.
pub fn extension() -> Extension {
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
}
