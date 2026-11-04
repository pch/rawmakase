//! Upright analysis harness: cargo run --release --example upright_lines -- PHOTO
//!
//! Prints the analysis image size and the straight segments Upright finds, one per
//! line, in centred long-edge units of the displayed photo, after a `#` line with the
//! corrections for each Upright mode.
use anyhow::{Context, Result};
use rawmakase::model::recipe::Recipe;
use rawmakase::{camera_data, develop::upright};
use rawmakase_cancel::Cancel;
use std::sync::atomic::AtomicBool;

fn main() -> Result<()> {
    let path = std::env::args().nth(1).context("Supply a RAW path")?;
    let cancel = AtomicBool::new(false);
    let image = rawmakase::photo::open(std::path::Path::new(&path))?
        .develop(camera_data::Decode::Half, Cancel::new(&cancel))?;
    let r = Recipe::with_profiles(&image.metadata, &[]);
    let (lum, w, h) = upright::analysis_image(&image, &r);
    let segments = upright::segments(&lum, w, h);
    println!("{w} {h} {}", image.metadata.focal_35mm);
    let corrections = upright::analyse(&image, &r);
    println!("# {}", serde_json::to_string(&corrections)?);
    for s in segments {
        println!("{} {} {} {}", s.a[0], s.a[1], s.b[0], s.b[1]);
    }
    Ok(())
}
