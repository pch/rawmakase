//! The editor without a window, for the example's measures:
//!
//! ```text
//! headless export --home <dir> <photo> <out> [--recipe <file>]
//!                 [--set <control>=<value>]... [--curve <x>,<y>]...
//! headless bench --home <dir> <photo> [--edge <pixels>] [--runs <count>]
//!                [--pause <ms>]
//! ```
//!
//! `export` drives the same editor the Node addon wraps: it opens the photo,
//! makes the edits, saves the recipe as a preset when asked and exports.
//!
//! `bench` is the plan's fifth measure in plain Rust, beside the app's own
//! (`npm run probe`). It times the render the app's render thread makes,
//! called directly (the frame, and the draft the library would show before
//! it), back to back or with a pause between renders, as an editor's come;
//! and the same editor the addon wraps, from a slider's change to the frame's
//! RGBA reaching the editor's sink, with no Node.
#![forbid(unsafe_code)]
use rawmakase_core::{Cancel, Context, Decode, Parameter, RawDefaults, View};
use rawmakase_electron::editor::{self, Config, Editor, Update, Work};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

type Failure = Box<dyn std::error::Error>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let done = match args.first().map(String::as_str) {
        Some("export") => export(&args[1..]),
        Some("bench") => bench(&args[1..]),
        _ => Err("usage: headless export|bench … (see src/bin/headless.rs)".into()),
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Flags with their values, in order.
type Flags<'a> = Vec<(&'a str, &'a str)>;

/// The flags' values, by flag, and the other arguments in order.
fn parse(args: &[String]) -> Result<(Flags<'_>, Vec<&str>), Failure> {
    let (mut flags, mut plain) = (Vec::new(), Vec::new());
    let mut args = args.iter().map(String::as_str);
    while let Some(arg) = args.next() {
        match arg.strip_prefix("--") {
            Some(flag) => flags.push((flag, args.next().ok_or(format!("--{flag} needs a value"))?)),
            None => plain.push(arg),
        }
    }
    Ok((flags, plain))
}

fn export(args: &[String]) -> Result<(), Failure> {
    let (flags, plain) = parse(args)?;
    let [photo, out] = plain[..] else {
        return Err("export takes a photo and an output file".into());
    };
    let flag = |name: &str| flags.iter().find(|(flag, _)| *flag == name).map(|f| f.1);
    let home = PathBuf::from(flag("home").ok_or("export needs --home <dir>")?);
    let editor = Editor::new(
        Config {
            data_dir: home.join("data"),
            cache_dir: home.join("cache"),
            build_id: "headless".into(),
        },
        |_| {},
    )?;
    let mut edit = 0;
    let mut next = || {
        edit += 1;
        edit
    };
    editor.open(Path::new(photo), next())?;
    for (name, value) in &flags {
        match *name {
            "set" => {
                let (control, value) = value.split_once('=').ok_or("--set takes name=value")?;
                editor.set_parameter(control, value.parse()?, next())?;
            }
            "curve" => {
                let (x, y) = value.split_once(',').ok_or("--curve takes x,y")?;
                editor.curve_insert(x.parse()?, y.parse()?, next())?;
            }
            _ => {}
        }
    }
    if let Some(recipe) = flag("recipe") {
        editor.save_recipe(Path::new(recipe))?;
    }
    if let Some(note) = editor.export(Path::new(out))? {
        eprintln!("{note}");
    }
    Ok(())
}

fn bench(args: &[String]) -> Result<(), Failure> {
    let (flags, plain) = parse(args)?;
    let [photo] = plain[..] else {
        return Err("bench takes a photo".into());
    };
    let number = |name: &str, default: u32| -> Result<u32, Failure> {
        match flags.iter().find(|(flag, _)| *flag == name) {
            Some((_, value)) => Ok(value.parse()?),
            None => Ok(default),
        }
    };
    let (edge, runs) = (number("edge", 0)?, number("runs", 30)?);
    let pause = Duration::from_millis(number("pause", 0)?.into());
    let home = flags
        .iter()
        .find(|(flag, _)| *flag == "home")
        .map(|(_, home)| PathBuf::from(home))
        .ok_or("bench needs --home <dir>")?;
    let ctx = Context::builder().data_dirs([home.join("data")]).build()?;
    let opened = ctx.open(photo, Cancel::NEVER)?;
    let image = opened.decode(Decode::Full, Cancel::NEVER)?;
    let mut recipe = opened.starting_recipe(&ctx.raw_defaults(&RawDefaults::default()))?;
    let mut renderer = ctx.renderer();
    // The first render of a photo also reduces it for the view; an editor pays
    // that once, when the photo opens.
    // Whether the library's rule would draft the next frame is asked after the
    // full frames, as the render thread asks it.
    let mut times = |draft: bool| -> Result<(Vec<f64>, [u32; 2], bool), Failure> {
        let mut times = Vec::new();
        let mut size = [0; 2];
        for run in 0..=runs {
            // The app's measure moves Exposure the same way.
            let exposure = -1. + 2. * (run % 21) as f32 / 20.;
            opened.set(&mut recipe, Parameter::Exposure, exposure)?;
            let work = Work {
                image: image.clone(),
                recipe: recipe.clone(),
                view: View::Fit(edge),
            };
            std::thread::sleep(pause);
            let started = Instant::now();
            let pixels = editor::render(&mut renderer, &work, draft, Cancel::NEVER)?;
            if run > 0 {
                times.push(started.elapsed().as_secs_f64() * 1000.);
            }
            size = [pixels.width, pixels.height];
        }
        let due = renderer.draft_due(&image, &recipe, View::Fit(edge));
        Ok((times, size, due.is_some()))
    };
    let (frame, frame_size, draft_due) = times(false)?;
    let (draft, draft_size, _) = times(true)?;
    let (to_pixels, editor_size) = through_editor(&home, Path::new(photo), edge, runs, pause)?;
    println!(
        "{}",
        serde_json::json!({
            "photo": photo,
            "runs": runs,
            "pauseMs": pause.as_millis(),
            "frame": { "size": frame_size, "ms": frame },
            "draft": { "size": draft_size, "ms": draft, "due": draft_due },
            "sliderToPixels": { "size": editor_size, "ms": to_pixels },
        })
    );
    Ok(())
}

/// The editor the addon wraps, timed from `set_parameter` to the RGBA of
/// that edit's frame (not a draft) in its sink.
fn through_editor(
    home: &Path,
    photo: &Path,
    edge: u32,
    runs: u32,
    pause: Duration,
) -> Result<(Vec<f64>, [u32; 2]), Failure> {
    let (frames_tx, frames) = channel();
    let editor = Editor::new(
        Config {
            data_dir: home.join("data"),
            cache_dir: home.join("cache"),
            build_id: "headless".into(),
        },
        move |update| {
            if let Update::Frame {
                edit,
                draft: false,
                pixels,
                ..
            } = update
            {
                let _ = frames_tx.send((edit, Instant::now(), [pixels.width, pixels.height]));
            }
        },
    )?;
    let wait = |edit: u32| loop {
        let frame = frames.recv_timeout(Duration::from_secs(60))?;
        if frame.0 == edit {
            break Ok::<_, Failure>(frame);
        }
    };
    editor.open(photo, 1)?;
    editor.set_view(edge, 1);
    let mut size = wait(1)?.2;
    let mut times = Vec::new();
    for run in 0..runs {
        let edit = run + 2;
        let exposure = -1. + 2. * (run % 21) as f32 / 20.;
        std::thread::sleep(pause);
        let started = Instant::now();
        editor.set_parameter("exposure", exposure, edit)?;
        let (_, shown, at) = wait(edit)?;
        times.push((shown - started).as_secs_f64() * 1000.);
        size = at;
    }
    Ok((times, size))
}
