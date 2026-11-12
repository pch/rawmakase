//! `rawmakase-remote-worker`: runs RAWmakase's selection models on this
//! computer, answering the app over the small TCP protocol in
//! `rawmakase-remote`. Meant for the computer that has the capable GPU; the
//! app reaches it through an SSH tunnel and stays otherwise unchanged.
//!
//! ```text
//! rawmakase-remote-worker --model ~/.local/share/rawmakase/models/sam2.1-hiera-small-v1
//! ```
//!
//! Binds the loopback by default: only this computer, and whatever is
//! tunnelled to it, can reach it. There are no credentials; anyone who can
//! connect can ask for inferences, which is why the default is the loopback
//! and the documentation pairs the worker with `ssh -L`.

use rawmakase_inference::{LoadOptions, Provider};
use rawmakase_remote::server::{Engine, Model, Server};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

const USAGE: &str = "\
rawmakase-remote-worker — run RAWmakase's selection models for another computer

    rawmakase-remote-worker --model DIR [--sky-model DIR] [--listen ADDR]
                            [--provider cpu|cuda|coreml] [--threads N]
                            [--runtime-lib FILE]

    --model DIR        the installed models' folder (required)
    --sky-model DIR    the installed sky model's folder (without it, Sky requests
                       are refused; it holds about 2.5 GB while running)
    --listen ADDR      address to serve on (default 127.0.0.1:8765; the
                       loopback is deliberate — publish it only through an
                       SSH tunnel: `ssh -N -L 8765:127.0.0.1:8765 host`)
    --provider WHICH   cpu (default), cuda for NVIDIA GPUs or coreml on Apple
                       Silicon; a provider the runtime cannot register stops
                       the worker with the runtime's own error
    --threads N        intra-op threads (default: the CPU count, capped at 8)
    --runtime-lib FILE the ONNX Runtime library to load, instead of
                       RAWMAKASE_ORT_LIB or the packaged locations
    -h, --help         this text
";

fn main() -> ExitCode {
    let mut model = None;
    let mut sky_model = None;
    let mut listen = String::from("127.0.0.1:8765");
    let mut provider = Provider::default();
    let mut threads = None;
    let mut runtime_library = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .unwrap_or_else(|| die(format!("{name} needs a value")))
        };
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "--model" => model = Some(PathBuf::from(value("--model"))),
            "--sky-model" => sky_model = Some(PathBuf::from(value("--sky-model"))),
            "--listen" => listen = value("--listen"),
            "--provider" => {
                provider = parse_provider(&value("--provider"))
                    .unwrap_or_else(|| die("provider is cpu, cuda or coreml".into()));
            }
            "--threads" => {
                threads = value("--threads")
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n > 0)
                    .map(Some)
                    .unwrap_or_else(|| die("threads must be a number above zero".into()));
            }
            "--runtime-lib" => runtime_library = Some(PathBuf::from(value("--runtime-lib"))),
            other => die(format!("unknown argument `{other}`; see --help")),
        }
    }
    let Some(model_dir) = model else {
        die("--model is required; see --help".into())
    };
    let address: std::net::SocketAddr = match listen.parse() {
        Ok(address) => address,
        Err(_) => die(format!("`{listen}` is not an address like 127.0.0.1:8765")),
    };
    if !address.ip().is_loopback() {
        // The protocol has no credentials; binding a reachable interface
        // would offer the models to the network.
        die("the worker serves the loopback only; publish it through an SSH tunnel".into());
    }
    let listener = match TcpListener::bind(address) {
        Ok(listener) => listener,
        Err(e) => die(format!("cannot listen on {listen}: {e}")),
    };
    let options = LoadOptions {
        runtime_library,
        threads,
        provider: Some(provider),
        ..Default::default()
    };
    let engine = match Model::load(&model_dir, &options) {
        Ok(engine) => engine,
        Err(e) => die(format!(
            "the models at {} would not load: {e}",
            model_dir.display()
        )),
    };
    let engine = match &sky_model {
        Some(dir) => match engine.with_sky(dir, &options) {
            Ok(engine) => engine,
            Err(e) => die(format!(
                "the sky model at {} would not load: {e}",
                dir.display()
            )),
        },
        None => engine,
    };
    let info = engine.describe();
    eprintln!(
        "rawmakase-remote-worker: {}, {} and {} on {}, serving on {}",
        info.model,
        info.panoptic,
        if info.sky.is_empty() {
            "no sky model"
        } else {
            &info.sky
        },
        // Said by the runtime's own answer at load time: a requested
        // accelerator that could not be registered never got this far.
        info.provider,
        listener
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or(listen)
    );
    Arc::new(Server::new(engine)).serve(listener)
}

/// `cpu`, `cuda` or `coreml`, case aside.
fn parse_provider(text: &str) -> Option<Provider> {
    match text.to_ascii_lowercase().as_str() {
        "cpu" => Some(Provider::Cpu),
        "cuda" => Some(Provider::Cuda),
        "coreml" => Some(Provider::CoreML),
        _ => None,
    }
}

/// Says why not, on stderr, and stops.
fn die(why: String) -> ! {
    eprintln!("rawmakase-remote-worker: {why}");
    std::process::exit(2);
}
