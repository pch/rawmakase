//! `rawmakase merge-hdr`: Merge to HDR without the desktop app. The bracket is
//! merged by `rawmakase_hdr_merge`, as the Library's Merge to HDR does it, and the
//! DNG published by [`crate::merge_hdr`] with its starting edit beside the first
//! RAW or where `--output` says, never over a file; with `--catalog`, added to
//! that catalog and, with `--stack`, stacked with its sources. Nothing here
//! reaches `app` (`scripts/deps-closures.txt`).
use crate::catalog::{Catalog, PhotoId, StackChange};
use crate::raw_defaults::DevelopDefaults;
use anyhow::{Context, Result, bail};
use rawmakase_cancel::Cancel;
use rawmakase_hdr_merge::{Deghost, Destination, FrameInput, MergeOptions, Stage};
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
    /// Leave what moved between the frames out of the merge, taking it from
    /// the reference frame: low leaves out only what clearly moved, high also
    /// what moved little.
    #[arg(long, value_enum, default_value_t = DeghostLevel::None)]
    pub deghost: DeghostLevel,
    /// Add the DNG to this catalog, which must have the first RAW: the merge
    /// starts from its edit there (white balance, profile and lens profile
    /// corrections), as in the Library.
    #[arg(long)]
    pub catalog: Option<PathBuf>,
    /// Stack the DNG with its RAWs in the catalog, on top.
    #[arg(long, requires = "catalog")]
    pub stack: bool,
}

/// The command line's names of [`Deghost`]'s levels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum DeghostLevel {
    None,
    Low,
    Medium,
    High,
}

impl From<DeghostLevel> for Deghost {
    fn from(level: DeghostLevel) -> Self {
        match level {
            DeghostLevel::None => Self::None,
            DeghostLevel::Low => Self::Low,
            DeghostLevel::Medium => Self::Medium,
            DeghostLevel::High => Self::High,
        }
    }
}

/// Merges `args.raws` and writes the DNG, with its starting edit worked out
/// with `raw_defaults` (the session's); returns its path. Progress and what
/// the merge measured go to `err`. Stops without writing anything soon after
/// `cancel` is set.
pub fn merge_hdr(
    args: &MergeHdr,
    libraries: &crate::environment::Libraries,
    raw_defaults: &dyn Fn() -> DevelopDefaults,
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
    let raw_defaults = raw_defaults();
    // The catalog, its photos of the RAWs, and the edit of the first, the
    // active photo; checked before anything is merged.
    let catalog = match &args.catalog {
        Some(path) => Some(crate::catalog_command::open_catalog(path, err)?),
        None => None,
    };
    let sources: Vec<PhotoId> = match &catalog {
        Some(c) if args.stack => args
            .raws
            .iter()
            .map(|raw| crate::catalog_command::photo_at(c, raw))
            .collect::<Result<_>>()?,
        Some(c) => vec![crate::catalog_command::photo_at(c, &args.raws[0])?],
        None => Vec::new(),
    };
    let record = match (&catalog, sources.first()) {
        (Some(c), Some(active)) => crate::catalog_command::photo_edit(c, *active)?,
        _ => crate::edits::EditRecord::default(),
    };
    let active = crate::merge_hdr::ActiveEdit::resolve(
        &record,
        &args.raws[0],
        &raw_defaults,
        libraries,
        cancel,
    )?;
    for warning in &active.warnings {
        writeln!(err, "{warning}")?;
    }
    let options = MergeOptions {
        demosaic: Default::default(),
        align: !args.no_align,
        deghost: args.deghost.into(),
        software: format!("RAWmakase {}", env!("CARGO_PKG_VERSION")),
    };
    let err = Mutex::new(err);
    let report = |stage: Stage, done: f32| {
        if done > 0. {
            return;
        }
        let Some(line) = crate::merge_hdr::stage_text(stage, inputs.len()) else {
            return;
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
        let deghosted = if frame.deghosted > 0. {
            format!(", deghosted {:.1}% of it", frame.deghosted * 100.)
        } else {
            String::new()
        };
        writeln!(
            err,
            "  {name}: {:+.2} EV by EXIF, {how}{reference}{aligned}{deghosted}",
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
    let published = crate::merge_hdr::publish(
        &merged,
        active.active(),
        &raw_defaults,
        libraries,
        &destination,
        &options.software,
        cancel,
    )?;
    for note in &published.notes {
        writeln!(err, "{note}")?;
    }
    if let Some(mut catalog) = catalog {
        for note in add(&mut catalog, &published, args.stack.then_some(&sources[..]))? {
            writeln!(err, "{note}")?;
        }
    }
    Ok(published.path)
}

/// Adds `published` to `catalog` with its starting edit and, with `stack`,
/// stacks it on top of those photos; returns what to tell of it. The errors
/// say where the DNG was saved and how far it got.
fn add(
    catalog: &mut Catalog,
    published: &crate::merge_hdr::Published,
    stack: Option<&[PhotoId]>,
) -> Result<Vec<String>> {
    let path = &published.path;
    let defaults = crate::catalog::NewPhotoDefaults::new(
        crate::catalog::MetadataDefaults::load(),
        crate::photo::camera_exif,
        crate::photo::raw_xmp,
    );
    let added = crate::merge_hdr::import(catalog, published, &defaults)?;
    let mut notes = Vec::new();
    if !added {
        notes.push(format!(
            "{} was already in the catalog, which keeps its edit there",
            path.display()
        ));
    }
    if let Some(sources) = stack {
        let unstacked = "The merge was added to the catalog, but couldn't be stacked with its RAWs";
        let photo = crate::catalog_command::photo_at(catalog, path).context(unstacked)?;
        catalog
            .change_stacks(&StackChange::Join {
                photo,
                with: sources.to_vec(),
            })
            .context(unstacked)?;
        if !catalog.stack_places()?.contains_key(&photo) {
            notes.push(format!(
                "{} wasn't stacked: none of its RAWs is in its folder",
                path.display()
            ));
        }
    }
    Ok(notes)
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
            deghost: DeghostLevel::None,
            catalog: None,
            stack: false,
        };
        let mut err = Vec::new();
        let got = merge_hdr(
            &args,
            &crate::environment::Libraries::default(),
            &Default::default,
            Cancel::NEVER,
            &mut err,
        )
        .unwrap_err();
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
        let got = merge_hdr(
            &args,
            &crate::environment::Libraries::default(),
            &Default::default,
            Cancel::NEVER,
            &mut err,
        )
        .unwrap_err();
        assert!(
            got.to_string().starts_with("missing.CR3 can't be read"),
            "{got}"
        );
    }

    /// With `--catalog`, the merge is added with its starting edit as its
    /// catalog edit; with `--stack`, on top of a stack of its RAWs.
    #[test]
    fn a_merge_is_added_and_stacked() {
        let dir = tempfile::tempdir().unwrap();
        let photos = dir.path().join("photos");
        std::fs::create_dir(&photos).unwrap();
        let merged = crate::merge_hdr::tests::merged(&photos, None);
        let mut catalog = Catalog::create(&dir.path().join("Photos.rawmakase")).unwrap();
        catalog.add_folder(&photos).unwrap();
        let sources: Vec<PhotoId> = merged
            .inputs
            .iter()
            .map(|input| crate::catalog_command::photo_at(&catalog, &input.path).unwrap())
            .collect();
        let (recipe, m) = crate::merge_hdr::tests::active();
        let publish = || {
            crate::merge_hdr::publish(
                &merged,
                crate::merge_hdr::Active {
                    recipe: &recipe,
                    metadata: &m,
                },
                &Default::default(),
                &crate::environment::Libraries::default(),
                &Destination::beside(&merged.inputs[0].path),
                "RAWmakase test",
                Cancel::NEVER,
            )
            .unwrap()
        };
        let published = publish();
        assert!(
            add(&mut catalog, &published, Some(&sources))
                .unwrap()
                .is_empty()
        );
        let photo = crate::catalog_command::photo_at(&catalog, &published.path).unwrap();
        let edit = catalog.load_edit(photo, &published.path).unwrap().unwrap();
        assert_eq!(edit.recipe, published.recipe);
        assert_eq!(edit.recipe.temperature, 4000.);
        let mut stack = vec![photo];
        stack.extend(&sources);
        assert_eq!(catalog.stack_of(photo).unwrap(), stack);

        // Without `--stack`, added alone.
        let second = publish();
        add(&mut catalog, &second, None).unwrap();
        let photo = crate::catalog_command::photo_at(&catalog, &second.path).unwrap();
        assert!(catalog.stack_of(photo).unwrap().is_empty());

        // Added already (by a watched folder, say): it keeps its edit, and
        // that is said.
        let notes = add(&mut catalog, &published, None).unwrap();
        assert!(notes[0].contains("already in the catalog"), "{notes:?}");

        // Saved in another folder than its RAWs: added, but not stacked,
        // and that is said.
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        let moved = elsewhere.join("merge.dng");
        std::fs::copy(&second.path, &moved).unwrap();
        let apart = crate::merge_hdr::Published {
            path: moved.clone(),
            recipe: second.recipe.clone(),
            notes: Vec::new(),
        };
        let notes = add(&mut catalog, &apart, Some(&sources)).unwrap();
        assert!(notes[0].contains("wasn't stacked"), "{notes:?}");
        let photo = crate::catalog_command::photo_at(&catalog, &moved).unwrap();
        assert!(catalog.stack_of(photo).unwrap().is_empty());
    }
}
