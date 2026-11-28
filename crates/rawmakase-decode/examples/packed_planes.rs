//! A RAW's packed mosaic as AI Denoise's model sees it, for calibrating the Neural Engine's
//! conversion of the model (`scripts/models/convert_denoise_ane.py`):
//! `cargo run --release -p rawmakase-decode --example packed_planes -- <raw> <out.f32>`
//! writes the R, G1, G2 and B planes as little-endian `f32` and prints their width and
//! height.
use rawmakase_cancel::Cancel;
use std::path::Path;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    anyhow::ensure!(args.len() == 3, "usage: packed_planes <raw> <out.f32>");
    let packed = rawmakase_decode::photo::open(
        Path::new(&args[1]),
        &rawmakase_decode::photo::LensProfiles::none(),
    )?
    .packed_bayer(Cancel::NEVER)?;
    let bytes: Vec<u8> = packed.planes.iter().flat_map(|v| v.to_le_bytes()).collect();
    std::fs::write(&args[2], bytes)?;
    println!(
        "{} {}",
        packed.layout.packed_width, packed.layout.packed_height
    );
    Ok(())
}
