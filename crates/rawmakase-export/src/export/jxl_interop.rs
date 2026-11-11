//! The export half of the JPEG XL interoperability harness (`scripts/jxl-interop.py`,
//! docs/records/hdr-jxl-interop.md). Opt-in: ignored unless run by name, with
//! `RAWMAKASE_JXL_INTEROP` naming a folder of HDR files (PNG, TIFF or JPEG XL files
//! that open as HDR edits, at their new-file settings) and RAWs (with HDR on at a limit
//! of 4 stops and the camera matrix). Each one is exported through the export job, as
//! the Export dialog's queue runs it, in every JPEG XL encoding #894 covers, into
//! `out/` beside them, with its HDR master (linear ProPhoto, 1 = graphics white) as
//! little-endian f32 for the script's comparisons. `RAWMAKASE_JXL_INTEROP_LONG_EDGE`
//! resizes every export to that long edge.
use super::srgb_fixtures::photo;
use crate::{
    color::hdr_output::{HdrGamut, HdrOutput, HdrTransfer},
    export::{Replace, job},
    export_settings::{ExportSettings, Format, Include},
    model::recipe::Recipe,
};
use anyhow::{Context, Result};
use rawmakase_cancel::Cancel;

/// Every JPEG XL encoding of #894, by the names the script reads: PQ and HLG in each
/// gamut, the gain map over the SDR rendition, and the SDR rendition alone (the gain
/// map's base must decode as it), each lossy at the dialog's default quality and
/// lossless.
fn encodings() -> Vec<(String, Option<HdrOutput>, bool)> {
    let mut out = Vec::new();
    for lossy in [true, false] {
        let mode = if lossy { "lossy" } else { "lossless" };
        for (transfer, t) in [(HdrTransfer::Pq, "pq"), (HdrTransfer::Hlg, "hlg")] {
            for (gamut, g) in [
                (HdrGamut::Rec2020, "rec2020"),
                (HdrGamut::DisplayP3, "p3"),
                (HdrGamut::Rec709, "rec709"),
            ] {
                let output = HdrOutput {
                    gamut,
                    transfer,
                    gain_map: false,
                };
                out.push((format!("{t}-{g}-{mode}"), Some(output), lossy));
            }
        }
        let gain_map = HdrOutput {
            gamut: HdrGamut::Rec709,
            gain_map: true,
            ..Default::default()
        };
        out.push((format!("gainmap-{mode}"), Some(gain_map), lossy));
        out.push((format!("sdr-{mode}"), None, lossy));
    }
    out
}

#[test]
#[ignore = "opt-in: run by scripts/jxl-interop.py with RAWMAKASE_JXL_INTEROP set"]
fn jxl_interop_exports() -> Result<()> {
    let dir = std::path::PathBuf::from(
        std::env::var_os("RAWMAKASE_JXL_INTEROP").context("set RAWMAKASE_JXL_INTEROP")?,
    );
    let long_edge: u32 = match std::env::var("RAWMAKASE_JXL_INTEROP_LONG_EDGE") {
        Ok(v) => v.parse()?,
        Err(_) => 0,
    };
    let out = dir.join("out");
    std::fs::create_dir_all(&out)?;
    let cache = tempfile::tempdir()?;
    let mut fixtures: Vec<_> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .collect();
    fixtures.sort();
    anyhow::ensure!(!fixtures.is_empty(), "no fixtures in {}", dir.display());
    for source in fixtures {
        let stem = source
            .file_stem()
            .context("a fixture's name")?
            .to_string_lossy()
            .to_string();
        let (image, recipe) = match crate::photo::open_source(&source, Cancel::NEVER)? {
            crate::photo::Opened::Rendered(image) => {
                let recipe = Recipe::for_new_rendered(&image.metadata);
                (image, recipe)
            }
            crate::photo::Opened::Raw(raw) => {
                let image = raw.develop(
                    crate::camera_data::Decode::full(Default::default()),
                    Cancel::NEVER,
                )?;
                let mut recipe = Recipe {
                    profile: crate::camera_profiles::CameraProfile::camera_matrix_default(
                        &image.metadata,
                    )
                    .map(std::sync::Arc::new),
                    ..Recipe::for_metadata(&image.metadata)
                };
                recipe.hdr.enabled = true;
                recipe.hdr.limit = 4.;
                (image, recipe)
            }
        };
        anyhow::ensure!(
            recipe.hdr.is_active(),
            "{} opens as an SDR file",
            source.display()
        );
        let master =
            crate::develop::render_hdr(&image, &recipe.checked()?, None, long_edge, Cancel::NEVER)?;
        let floats: Vec<u8> = master
            .pixels
            .iter()
            .flatten()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        std::fs::write(out.join(format!("{stem}.master.f32")), floats)?;
        std::fs::write(
            out.join(format!("{stem}.master.json")),
            format!(
                "{{\"width\": {}, \"height\": {}, \"peak\": {}, \"limit\": {}}}",
                master.width, master.height, master.peak, recipe.hdr.limit
            ),
        )?;
        let photo = photo(image, &source, cache.path(), recipe);
        for (name, output, lossy) in encodings() {
            let mut s = ExportSettings {
                include: Include::AllExceptCameraRaw,
                ..Default::default()
            };
            s.choose_format(Format::Jxl);
            s.jxl_lossy = lossy;
            s.hdr = output;
            s.resize = long_edge > 0;
            s.long_edge = long_edge;
            let target = out.join(format!("{stem}.{name}.jxl"));
            job::run(
                photo.clone(),
                &s,
                &target,
                Replace::Overwrite,
                Cancel::NEVER,
                |_| {},
            )
            .with_context(|| format!("{stem} {name}"))?;
        }
    }
    Ok(())
}
