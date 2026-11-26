//! `rawmakase-embed PHOTO OUT [EXPOSURE]`: opens PHOTO with RAWmakase's
//! library, starts from its raw defaults, sets Exposure, and exports OUT in
//! the format its extension names, printing what each step took.
use rawmakase_core::{CacheLimits, Cancel, Context, Decode, ExportSettings, Parameter, View};
use std::time::Instant;

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rawmakase-embed: {e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let (Some(photo), Some(out)) = (args.next(), args.next()) else {
        return Err("usage: rawmakase-embed PHOTO OUT [EXPOSURE]".into());
    };
    let exposure: Option<f32> = args
        .next()
        .map(|e| e.to_string_lossy().parse())
        .transpose()?;
    // The app's own folders, given as values: here, beside the output.
    let base = std::path::Path::new(&out)
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join(".rawmakase-embed");
    let ctx = Context::builder()
        .data_dirs([base.join("data")])
        .cache(
            base.join("cache"),
            CacheLimits::default(),
            concat!("rawmakase-embed ", env!("CARGO_PKG_VERSION")),
        )
        .build()?;

    let started = Instant::now();
    let photo = ctx.open(&photo, Cancel::NEVER)?;
    let mut recipe = photo.starting_recipe()?;
    if let Some(exposure) = exposure {
        photo.set(&mut recipe, Parameter::Exposure, exposure)?;
    }
    println!(
        "opened {} in {:?}",
        photo.metadata().model,
        started.elapsed()
    );

    let started = Instant::now();
    let image = photo.decode(Decode::Full, Cancel::NEVER)?;
    let stats = image.stats();
    println!(
        "decoded {}×{} in {:?}",
        stats.width,
        stats.height,
        started.elapsed()
    );

    let started = Instant::now();
    let frame = ctx.render(&image, &recipe, View::Fit(2048), Cancel::NEVER)?;
    println!(
        "rendered {}×{} in {:?}",
        frame.width(),
        frame.height(),
        started.elapsed()
    );

    let started = Instant::now();
    let note = ctx.export(
        &photo,
        &recipe,
        &ExportSettings::default(),
        &out,
        false,
        Cancel::NEVER,
        |_| {},
    )?;
    println!(
        "exported in {:?}{}",
        started.elapsed(),
        note.map(|n| format!(": {n}")).unwrap_or_default()
    );
    Ok(())
}
