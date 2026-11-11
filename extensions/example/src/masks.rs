//! A test-only extension of mask generators (`masks/rawmakase-extension.toml`):
//! each answers `mask.generate` from the photo's pixels and the viewer input
//! alone, so the app's tests can check what was asked and what was kept, end
//! to end through shared memory.
//!
//! - `blob`: automatic, the left half of the photo where it isn't black (no
//!   subject when it is all black); with a prompt, the box, then a disc of a
//!   fifth of the photo around each click, added or taken away;
//! - `flat`: the whole photo; `nothing`: all-zero coverage, unannounced;
//!   `absent`: `no_subject`; `unnamed`: a model with an empty name;
//! - `setup`: `needs_setup` until `mask.setup` has run, which reports half
//!   way and waits for `finish-setup` in its working folder (or a cancel);
//!   then as `blob`;
//! - `unavailable`: `unavailable`;
//! - `gate`: writes `started` in its working folder, then answers as `blob`
//!   once `release` appears there, or `cancelled` when cancelled first; `late`
//!   the same, but it ignores the cancel;
//! - `crash`: exits.
use rawmakase_extension::protocol::message::NeedsSetup;
use rawmakase_extension::protocol::method::{EmptyMask, MaskGenerated};
use rawmakase_extension::protocol::viewer::ViewerInput;
use rawmakase_extension::{Context, ErrorCode, Extension, MaskAsk, MaskGenerator, RemoteError};
use std::path::Path;
use std::time::{Duration, Instant};

/// One of the generators, by its id.
struct Generator(&'static str);

/// The setup's marker in the program's data folder.
const READY: &str = "setup-ready";

impl MaskGenerator for Generator {
    fn generate(
        &mut self,
        cx: &Context,
        mut ask: MaskAsk<'_>,
    ) -> Result<MaskGenerated, RemoteError> {
        let model = |empty| MaskGenerated {
            model: format!("example-{}@1", self.0),
            model_key: format!("example-{}/1", self.0),
            empty,
        };
        match self.0 {
            "flat" => {
                ask.coverage.fill(255);
                Ok(model(None))
            }
            "nothing" => Ok(model(None)),
            "absent" => Ok(model(Some(EmptyMask::NoSubject))),
            "unnamed" => Ok(MaskGenerated {
                model: String::new(),
                ..model(None)
            }),
            "unavailable" => Err(RemoteError::new(ErrorCode::Unavailable, "no runtime here")),
            "crash" => std::process::exit(3),
            "setup" if !cx.data_dir().join(READY).exists() => Err(NeedsSetup {
                what: "the Example model".into(),
                bytes: 12_500_000,
            }
            .error()),
            "gate" | "late" => {
                let obeys = self.0 == "gate";
                // For a test to know the request is running here.
                let _ = std::fs::write("started", b"");
                if !wait_for("release", obeys.then_some(cx)) {
                    return Err(RemoteError::new(ErrorCode::Cancelled, "cancelled"));
                }
                blob(&mut ask);
                Ok(model(None))
            }
            _ => {
                let empty = blob(&mut ask);
                Ok(model(empty))
            }
        }
    }

    fn setup(&mut self, cx: &Context, _generator: &str) -> Result<(), RemoteError> {
        cx.progress(Some(0.5), Some("Halfway"));
        if !wait_for("finish-setup", Some(cx)) {
            return Err(RemoteError::new(ErrorCode::Cancelled, "cancelled"));
        }
        std::fs::write(cx.data_dir().join(READY), b"")
            .map_err(|e| RemoteError::new(ErrorCode::Other("failed".into()), e.to_string()))
    }
}

/// Waits up to 20 s for `file` in the working folder; false when `cx` is
/// cancelled first, or it never appears.
fn wait_for(file: &str, cx: Option<&Context>) -> bool {
    let until = Instant::now() + Duration::from_secs(20);
    while Instant::now() < until {
        if Path::new(file).exists() {
            return true;
        }
        if cx.is_some_and(Context::is_cancelled) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    false
}

/// Writes `blob`'s coverage; says there is no subject in an all-black photo.
fn blob(ask: &mut MaskAsk<'_>) -> Option<EmptyMask> {
    let (image, coverage) = (&ask.request.image, &ask.request.coverage);
    let (w, h) = (image.width as usize, image.height as usize);
    let (stride, out) = (image.stride as usize, coverage.stride as usize);
    let pixel = |x: usize, y: usize| &ask.image[y * stride + x * 3..][..3];
    let mut values = vec![0u8; w * h];
    match &ask.request.input {
        None => {
            if (0..h).all(|y| (0..w).all(|x| pixel(x, y) == [0, 0, 0])) {
                return Some(EmptyMask::NoSubject);
            }
            for y in 0..h {
                for x in 0..w / 2 + w % 2 {
                    values[y * w + x] = if pixel(x, y) == [0, 0, 0] { 0 } else { 255 };
                }
            }
        }
        Some(ViewerInput::Prompt { clicks, bounds }) => {
            let centre = |i: usize, n: usize| (i as f32 + 0.5) / n as f32;
            if let Some(b) = bounds {
                for y in 0..h {
                    for x in 0..w {
                        let (u, v) = (centre(x, w), centre(y, h));
                        if (b.x0..=b.x1).contains(&u) && (b.y0..=b.y1).contains(&v) {
                            values[y * w + x] = 255;
                        }
                    }
                }
            }
            for c in clicks {
                for y in 0..h {
                    for x in 0..w {
                        let (du, dv) = (centre(x, w) - c.x, centre(y, h) - c.y);
                        if du * du + dv * dv <= 0.2 * 0.2 {
                            values[y * w + x] = if c.positive { 255 } else { 0 };
                        }
                    }
                }
            }
        }
        Some(_) => {}
    }
    for (y, row) in values.chunks(w).enumerate() {
        ask.coverage[y * out..][..w].copy_from_slice(row);
    }
    None
}

/// The extension, ready to run.
pub fn extension() -> Extension {
    [
        "blob",
        "flat",
        "nothing",
        "absent",
        "setup",
        "unavailable",
        "gate",
        "late",
        "unnamed",
        "crash",
    ]
    .into_iter()
    .fold(Extension::new(), |e, id| {
        e.mask_generator(id, Generator(id))
    })
}
