//! `rawmakase merge-hdr`: Merge to HDR without the desktop app. The bracket is
//! merged by `rawmakase_hdr_merge`, as the Library's Merge to HDR does it, and the
//! DNG written beside the first RAW or where `--output` says, never over a file.
//! Nothing here reaches `app` (`scripts/deps-closures.txt`).
use anyhow::{Result, bail};
use rawmakase_cancel::Cancel;
use rawmakase_hdr_merge::{Destination, FrameInput, MergeOptions, Stage};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(clap::Args)]
pub struct MergeHdr {
    /// The bracket's RAWs (2 to 9). The first names the merge, as the active
    /// photo does in Lightroom: IMG_1.CR3 merges to IMG_1-HDR.dng.
    #[arg(required = true, num_args = 1..)]
    pub raws: Vec<PathBuf>,
    /// Where to write the DNG, which must not exist. By default it goes beside
    /// the first RAW, named after it with -HDR.dng, or -HDR-2.dng and on when
    /// that is taken.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Merge the frames as they were taken, without Auto Align: for a bracket
    /// taken on a tripod, or one Auto Align can't align.
    #[arg(long)]
    pub no_align: bool,
}

/// Merges `args.raws` and writes the DNG; returns its path. Progress and what
/// the merge measured go to `err`. Stops without writing anything soon after
/// `cancel` is set.
pub fn merge_hdr(
    args: &MergeHdr,
    cancel: Cancel<'_>,
    err: &mut (dyn Write + Send),
) -> Result<PathBuf> {
    if let Some(output) = &args.output
        && output.exists()
    {
        bail!("{} already exists", output.display());
    }
    let inputs = args
        .raws
        .iter()
        .map(|path| FrameInput::read(path))
        .collect::<Result<Vec<_>, _>>()?;
    let options = MergeOptions {
        demosaic: Default::default(),
        align: !args.no_align,
        software: format!("RAWmakase {}", env!("CARGO_PKG_VERSION")),
    };
    let err = Mutex::new(err);
    let report = |stage: Stage, done: f32| {
        if done > 0. {
            return;
        }
        let line = match stage {
            Stage::Check => "Checking the bracket".to_string(),
            Stage::Decode(n) => format!("Decoding frame {} of {}", n + 1, inputs.len()),
            Stage::Align(n) => format!("Aligning frame {} of {}", n + 1, inputs.len()),
            Stage::Measure(_) | Stage::Merge(_) => return,
            Stage::Encode => "Writing the DNG".to_string(),
        };
        if let Ok(mut err) = err.lock() {
            let _ = writeln!(err, "{line}");
        }
    };
    let merged = rawmakase_hdr_merge::merge(&inputs, &options, &report, cancel)?;
    let err = err.into_inner().unwrap_or_else(|e| e.into_inner());
    for (k, frame) in merged.frames.iter().enumerate() {
        let name = inputs[frame.input].path.display();
        let how = if frame.measured {
            format!("measured {:+.2}", frame.ev)
        } else {
            "not measurable; EXIF used".to_string()
        };
        let reference = if k == merged.reference {
            ", reference"
        } else {
            ""
        };
        let aligned = match &frame.alignment {
            Some(a) if a.warped => format!(
                ", aligned: moved {:+.2}, {:+.2} px, turned {:+.3}°, residual {:.3} stop",
                a.shift[0], a.shift[1], a.rotation, a.residual
            ),
            Some(a) => format!(", aligned: in place, residual {:.3} stop", a.residual),
            None => String::new(),
        };
        writeln!(
            err,
            "  {name}: {:+.2} EV by EXIF, {how}{reference}{aligned}",
            frame.nominal_ev
        )?;
    }
    if merged.reference_moved {
        writeln!(
            err,
            "The reference moved off the middle exposure to keep BaselineExposure within ±{}",
            rawmakase_hdr_merge::MAX_BASELINE
        )?;
    }
    writeln!(err, "BaselineExposure {:+.2}", merged.baseline_exposure)?;
    let destination = match &args.output {
        Some(path) => Destination::Exactly(path.clone()),
        None => Destination::beside(&args.raws[0]),
    };
    Ok(rawmakase_hdr_merge::write_dng(
        &merged,
        &destination,
        cancel,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An output that exists is refused before anything is read.
    #[test]
    fn an_existing_output_is_refused_first() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("taken.dng");
        std::fs::write(&output, b"mine").unwrap();
        let args = MergeHdr {
            raws: vec![dir.path().join("missing.CR3")],
            output: Some(output.clone()),
            no_align: false,
        };
        let mut err = Vec::new();
        let got = merge_hdr(&args, Cancel::NEVER, &mut err).unwrap_err();
        assert_eq!(
            got.to_string(),
            format!("{} already exists", output.display())
        );
        assert_eq!(std::fs::read(&output).unwrap(), b"mine");
        // Without one, the missing RAW is named.
        let args = MergeHdr {
            output: None,
            ..args
        };
        let got = merge_hdr(&args, Cancel::NEVER, &mut err).unwrap_err();
        assert!(
            got.to_string().starts_with("missing.CR3 can't be read"),
            "{got}"
        );
    }
}
