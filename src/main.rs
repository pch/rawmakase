// A console subsystem puts the generic console icon on the Windows taskbar.
// Command output still attaches to the parent console when there is one.
#![cfg_attr(windows, windows_subsystem = "windows")]
use anyhow::Result;
mod mcp;
use clap::{Parser, Subcommand};
use rawmakase::model::recipe::Recipe;
use rawmakase::{camera_data, develop, export_settings::ExportOptions, raw};
use std::{path::PathBuf, sync::atomic::AtomicBool, time::Instant};
#[derive(Parser)]
#[command(version, about = "A personal RAW photo editor")]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    /// A RAWmakase catalog to open, or a photo to add to the last catalog and edit
    path: Option<PathBuf>,
}
#[derive(Subcommand)]
enum Command {
    /// Control the running desktop app (enable external control in Preferences first).
    // The client crate has its own version; this command is part of the app.
    #[command(version = env!("CARGO_PKG_VERSION"))]
    Control(rawmakase_ctl::Cli),
    /// Serve editing tools over MCP stdio, connected to the running desktop app.
    Mcp(mcp::Cli),
    /// Import user-selected Lightroom DCP/XMP files into RAWmakase's profile library.
    ImportProfiles {
        #[arg(required = true, num_args = 1..)]
        files: Vec<PathBuf>,
    },
    /// Import Adobe lens profiles (.lcp) for Enable Profile Corrections.
    ImportLensProfiles {
        #[arg(required = true, num_args = 1..)]
        files: Vec<PathBuf>,
    },
    ImportCatalog {
        source: PathBuf,
        output: PathBuf,
    },
    CatalogInfo {
        catalog: PathBuf,
    },
    RelinkCatalog {
        catalog: PathBuf,
        root: i64,
        folder: PathBuf,
    },
    /// Find photos added, deleted, moved or renamed outside RAWmakase in a
    /// catalog folder (by its path on this computer), or in every folder.
    /// Prints what Synchronize Folder would do; --apply does it, accepting
    /// what the review ticks by default. Nothing on disk is changed.
    SyncFolder {
        catalog: PathBuf,
        folder: Option<PathBuf>,
        #[arg(long)]
        apply: bool,
    },
    Compare {
        input: PathBuf,
        reference: PathBuf,
        output: PathBuf,
        #[arg(long)]
        recipe: Option<PathBuf>,
        #[arg(long, num_args = 2)]
        origin: Option<Vec<u32>>,
    },
    Inspect {
        input: PathBuf,
    },
    /// Render one RAW with several XMP sidecars, developing it once: for comparisons
    /// with Camera Raw. `jobs` is a JSON list of {"xmp", "output", "max_edge"}; existing
    /// outputs are skipped.
    RenderBatch {
        input: PathBuf,
        jobs: PathBuf,
    },
    Thumbnail {
        input: PathBuf,
        output: PathBuf,
    },
    Render {
        input: PathBuf,
        /// Save the resolved rendering recipe for reproducible comparisons.
        #[arg(long)]
        save_recipe: Option<PathBuf>,
        #[arg(long)]
        profile: Option<PathBuf>,
        #[arg(long)]
        xmp: Option<PathBuf>,
        output: PathBuf,
        #[arg(long)]
        exposure: Option<f32>,
        #[arg(long, default_value_t = 0)]
        max_edge: u32,
        #[arg(long)]
        fast: bool,
        #[arg(long)]
        overwrite: bool,
        #[arg(long)]
        recipe: Option<PathBuf>,
        /// Apply Auto tone, as the Basic panel's Auto button does; white balance is kept.
        /// Auto sets Exposure itself, so it cannot be combined with --exposure.
        #[arg(long, conflicts_with = "exposure")]
        auto: bool,
        /// Apply Auto white balance, as the WB menu's Auto does (before --auto).
        #[arg(long)]
        auto_wb: bool,
        /// Write the photo at a stage of the pipeline instead, as a 32-bit float TIFF
        /// of linear ProPhoto RGB at full size (docs/scene-tone-stage.md).
        #[arg(long, value_enum)]
        tap: Option<Tap>,
    },
    /// Print each file's decode fingerprint as a line of JSON: what opening and
    /// unpacking it yields, for decode regression checks.
    Fingerprint {
        #[arg(required = true, num_args = 1..)]
        files: Vec<PathBuf>,
        /// The decoder to read them with.
        #[arg(long, value_enum, default_value_t = DecoderArg::Libraw)]
        decoder: DecoderArg,
    },
    /// Print each file's develop fingerprint as a line of JSON: the image each
    /// decode mode (full, half) makes, hashed, for decoder regression checks.
    DecodeDump {
        #[arg(required = true, num_args = 1..)]
        files: Vec<PathBuf>,
        /// Also write each full decode that is not a mosaic here, as raw floats.
        #[arg(long)]
        floats: Option<PathBuf>,
        /// The decoder to read them with.
        #[arg(long, value_enum, default_value_t = DecoderArg::Libraw)]
        decoder: DecoderArg,
    },
    Benchmark {
        input: PathBuf,
        #[arg(long, default_value_t = 20)]
        iterations: usize,
    },
}
/// The decoders the comparison commands can read a RAW with.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum DecoderArg {
    Libraw,
    Own,
}
impl From<DecoderArg> for raw::Decoder {
    fn from(d: DecoderArg) -> Self {
        match d {
            DecoderArg::Libraw => raw::Decoder::Libraw,
            DecoderArg::Own => raw::Decoder::Own,
        }
    }
}
/// A stage `render --tap` writes.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum Tap {
    SceneInput,
    SceneOutput,
}
/// A windows-subsystem program starts without a console, so commands run from
/// a terminal would print nothing. Writes to that terminal instead, unless the
/// output is already redirected to a file or pipe.
#[cfg(windows)]
fn attach_console() {
    use windows_sys::Win32::{
        Foundation::INVALID_HANDLE_VALUE,
        System::Console::{ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE},
    };
    // SAFETY: plain Win32 calls with no pointers; failure (no parent console,
    // as when started from Explorer) leaves the process as it was.
    unsafe {
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        if out.is_null() || out == INVALID_HANDLE_VALUE {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}
/// Started from the Start menu there is no console to print a failure to, so
/// the app would vanish without a word.
#[cfg(windows)]
fn show_error(error: &anyhow::Error) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let text = wide(&format!("RAWmakase could not start.\n\n{error:#}"));
    let title = wide("RAWmakase");
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        )
    };
}
fn main() -> Result<()> {
    #[cfg(windows)]
    attach_console();
    // Before anything else: this process may be the update helper.
    let launch = rawmakase::updates::intercept();
    rayon::ThreadPoolBuilder::new()
        .num_threads(std::thread::available_parallelism().map_or(4, |n| n.get().min(8)))
        .build_global()
        .ok();
    let a = Args::parse_from(&launch.arguments);
    match a.command {
        Some(Command::Mcp(cli)) => mcp::run(cli)?,
        Some(Command::Control(cli)) => rawmakase_ctl::run(cli).map_err(anyhow::Error::msg)?,
        Some(Command::ImportLensProfiles { files }) => {
            // One unusable file (Adobe ships a few) leaves the rest importing.
            let done = rawmakase::lens::lcp::import_each(&files);
            for p in &done.imported {
                println!("Imported {}", p.display());
            }
            for (p, why) in &done.refused {
                eprintln!("Skipped {}: {why}", p.display());
            }
            anyhow::ensure!(!done.imported.is_empty(), "No lens profiles imported");
        }
        Some(Command::ImportProfiles { files }) => {
            for p in rawmakase::camera_profiles::import_files(&files)? {
                println!("Imported {}", p.display());
            }
        }
        Some(Command::ImportCatalog { source, output }) => {
            let path = rawmakase::catalog::lightroom::import_lightroom(&source, &output)?;
            let c = rawmakase::catalog::Catalog::open(&path)?;
            println!(
                "Imported {} photographs into {}",
                c.photos()?.len(),
                path.display()
            );
        }
        Some(Command::CatalogInfo { catalog }) => {
            let c = rawmakase::catalog::Catalog::open(&catalog)?;
            let photos = c.photos()?;
            println!(
                "{} photographs · {} folders · {} collections · {} offline",
                photos.len(),
                c.folders()?.len(),
                c.collections()?.len(),
                photos.iter().filter(|p| !p.path.is_file()).count()
            );
            for (id, source, mapped) in c.roots()? {
                println!(
                    "Root {id}: {source} → {}",
                    mapped.unwrap_or_else(|| "where it was added".into())
                );
            }
        }
        Some(Command::RelinkCatalog {
            catalog,
            root,
            folder,
        }) => {
            rawmakase::catalog::Catalog::open(&catalog)?
                .relink_root(rawmakase::catalog::RootId(root), &folder)?;
            println!("Root folder relinked");
        }

        Some(Command::SyncFolder {
            catalog,
            folder,
            apply,
        }) => sync_folder(&catalog, folder.as_deref(), apply)?,
        Some(Command::Compare {
            input,
            reference,
            output,
            recipe,
            origin,
        }) => {
            rawmakase::comparison::compare(
                &input,
                &reference,
                &output,
                recipe.as_deref(),
                origin.map(|p| [p[0], p[1]]),
            )?;
        }
        Some(Command::Inspect { input }) => {
            let r = rawmakase::photo::open(&input)?;
            let (profiles, errors) = rawmakase::camera_profiles::installed(&r.metadata);
            eprintln!(
                "Profile folders: {:?}\nAvailable profiles: {:?}\nProfile errors: {:?}",
                rawmakase::camera_profiles::library_dirs(),
                profiles.iter().map(|p| &p.name).collect::<Vec<_>>(),
                errors
            );
            println!(
                "LibRaw {}\n{}",
                raw::version(),
                serde_json::to_string_pretty(&r.metadata)?
            );
        }
        Some(Command::RenderBatch { input, jobs }) => {
            #[derive(serde::Deserialize)]
            struct Job {
                xmp: PathBuf,
                output: PathBuf,
                #[serde(default)]
                max_edge: u32,
            }
            let jobs: Vec<Job> = serde_json::from_str(&std::fs::read_to_string(&jobs)?)?;
            let photo = rawmakase::photo::open(&input)?;
            let im = photo.develop(
                camera_data::Decode::full(Default::default()),
                &AtomicBool::new(false),
            )?;
            let (profiles, _) = rawmakase::camera_profiles::installed(&im.metadata);
            let base = Recipe::with_profiles(&im.metadata, &profiles);
            let camera = rawmakase::exif::read(&input);
            for job in jobs.iter().filter(|j| !j.output.exists()) {
                let rendered = (|| -> Result<()> {
                    let preset =
                        rawmakase::xmp::parse(&job.xmp, &std::fs::read_to_string(&job.xmp)?)?;
                    let edit = preset.apply(
                        &base,
                        &im.metadata,
                        &profiles,
                        Some(&rawmakase::develop::Measures(&im)),
                    )?;
                    let out = develop::render(&im, &edit.checked()?, job.max_edge)?;
                    rawmakase::export::export_with(
                        &job.output,
                        &input,
                        &out,
                        &im.metadata,
                        &ExportOptions {
                            max_edge: job.max_edge,
                            ..Default::default()
                        },
                        &rawmakase::export::Embed {
                            camera: camera.clone(),
                            ..Default::default()
                        },
                        rawmakase::export::Replace::NoClobber,
                    )?;
                    Ok(())
                })();
                if let Err(e) = rendered {
                    eprintln!("{}: {e:#}", job.output.display());
                }
            }
        }
        Some(Command::Thumbnail { input, output }) => {
            use std::io::Write;
            let data = rawmakase::photo::open(&input)?.thumbnail()?;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(output)?;
            f.write_all(&data)?;
        }
        Some(Command::Render {
            input,
            save_recipe,
            profile,
            xmp,
            output,
            exposure,
            max_edge,
            fast,
            overwrite,
            recipe,
            auto,
            auto_wb,
            tap,
        }) => {
            let t = Instant::now();
            let r = rawmakase::photo::open(&input)?;
            let mut edit = if let Some(p) = recipe {
                rawmakase::presets::load_preset(&p)?.recipe
            } else {
                rawmakase::catalog::legacy_sidecar::load(&input)?
                    .map(|s| s.recipe)
                    .unwrap_or_else(|| {
                        Recipe::with_profiles(
                            &r.metadata,
                            &rawmakase::camera_profiles::installed(&r.metadata).0,
                        )
                    })
            };
            if let Some(e) = exposure {
                edit.exposure = e;
            }
            if let Some(p) = profile {
                edit.profile = Some(rawmakase::camera_profiles::load(&p, &r.metadata)?);
                // A chosen profile starts at 100%, as in the app.
                edit.profile_amount = 1.;
                edit.use_camera_baseline(&r.metadata);
                edit.sync_white_balance_controls(&r.metadata);
            }
            edit.validate()?;
            let decode = if fast {
                camera_data::Decode::Half
            } else {
                camera_data::Decode::full(Default::default())
            };
            let im = r.develop(decode, &AtomicBool::new(false))?;
            if let Some(path) = xmp {
                let preset = rawmakase::xmp::parse(&path, &std::fs::read_to_string(&path)?)?;
                let (profiles, _) = rawmakase::camera_profiles::installed(&im.metadata);
                edit = preset.apply(
                    &edit,
                    &im.metadata,
                    &profiles,
                    Some(&rawmakase::develop::Measures(&im)),
                )?;
                if let Some(e) = exposure {
                    edit.exposure = e;
                }
            }
            if auto_wb {
                let t = Instant::now();
                edit = develop::auto_white_balance(&im, &edit)?;
                eprintln!(
                    "Auto white balance ({:?}): temperature {:.0} tint {:+.0}",
                    t.elapsed(),
                    edit.temperature,
                    edit.tint
                );
            }
            if auto {
                let t = Instant::now();
                edit = develop::auto_tone(&im, &edit)?;
                eprintln!(
                    "Auto ({:?}): exposure {:+.2} contrast {:+.0} highlights {:+.0} shadows {:+.0} whites {:+.0} blacks {:+.0} vibrance {:+.0} saturation {:+.0}",
                    t.elapsed(),
                    edit.exposure,
                    edit.contrast * 100.,
                    edit.highlights * 100.,
                    edit.shadows * 100.,
                    edit.whites * 100.,
                    edit.blacks * 100.,
                    edit.vibrance * 100.,
                    edit.saturation * 100.
                );
            }
            if let Some(path) = save_recipe {
                anyhow::ensure!(!path.exists(), "Recipe output already exists");
                rawmakase::presets::save_preset(&path, &edit, &im.metadata)?;
            }
            if let Some(tap) = tap {
                let stage = match tap {
                    Tap::SceneInput => develop::quality::Stage::SceneInput,
                    Tap::SceneOutput => develop::quality::Stage::SceneOutput,
                };
                let out =
                    develop::quality::render_stage(&im, &edit, stage, &AtomicBool::new(false))?;
                save_stage(&output, &input, out, overwrite)?;
                return Ok(());
            }
            let developed = t.elapsed();
            let out = develop::render(&im, &edit.checked()?, max_edge)?;
            let rendered = t.elapsed() - developed;
            rawmakase::export::export_with(
                &output,
                &input,
                &out,
                &im.metadata,
                &ExportOptions {
                    max_edge,
                    ..Default::default()
                },
                &rawmakase::export::Embed {
                    camera: rawmakase::exif::read(&input),
                    ..Default::default()
                },
                if overwrite {
                    rawmakase::export::Replace::Overwrite
                } else {
                    rawmakase::export::Replace::NoClobber
                },
            )?;
            let max = im.pixels.iter().flatten().copied().fold(0f32, f32::max);
            println!(
                "{}x{} | develop {:?} | render {:?} | total {:?} | camera max {:.4} | scale {:.6} | integer clipped {}",
                out.width,
                out.height,
                developed,
                rendered,
                t.elapsed(),
                max,
                im.scale_factor,
                im.scale_clipped
            );
        }
        Some(Command::Fingerprint { files, decoder }) => {
            // One file at a time: LibRaw opening files side by side can give one
            // of them different white balance multipliers (issue #420).
            for path in &files {
                let line = serde_json::to_string(&rawmakase::fingerprint::fingerprint(
                    path,
                    decoder.into(),
                ))?;
                println!("{line}");
            }
        }
        Some(Command::DecodeDump {
            files,
            floats,
            decoder,
        }) => {
            for path in &files {
                let dump = rawmakase::decode_dump::dump(path, floats.as_deref(), decoder.into());
                println!("{}", serde_json::to_string(&dump)?);
            }
        }
        Some(Command::Benchmark { input, iterations }) => {
            anyhow::ensure!(
                (1..=1000).contains(&iterations),
                "Iterations must be 1–1000"
            );
            let t = Instant::now();
            let im = rawmakase::photo::open(&input)?.develop(
                camera_data::Decode::full(Default::default()),
                &AtomicBool::new(false),
            )?;
            let decode = t.elapsed();
            let small = develop::preview(&im, 1600);
            let mut r = Recipe::for_metadata(&im.metadata);
            let mut times = Vec::new();
            for i in 0..iterations {
                r.exposure = (i % 8) as f32 / 8.;
                let t = Instant::now();
                std::hint::black_box(develop::render(&small, &r.checked()?, 1600)?);
                times.push(t.elapsed().as_secs_f64() * 1000.);
            }
            times.sort_by(f64::total_cmp);
            println!(
                "development: {decode:?}; preview median {:.1}ms p95 {:.1}ms",
                times[times.len() / 2],
                times[((times.len() - 1) as f64 * 0.95).ceil() as usize]
            );
        }
        None => {
            let run = rawmakase::app::run(a.path, launch);
            #[cfg(windows)]
            if let Err(e) = &run {
                show_error(e);
            }
            run?;
        }
    }
    Ok(())
}

/// `rawmakase sync-folder`: scans, prints the plan, and applies it with
/// `apply`.
fn sync_folder(
    catalog: &std::path::Path,
    folder: Option<&std::path::Path>,
    apply: bool,
) -> Result<()> {
    use rawmakase::catalog::folder_sync::{SyncScope, scan};
    let mut c = rawmakase::catalog::Catalog::open(catalog)?;
    let scope = match folder {
        None => SyncScope::All,
        Some(path) => {
            let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
            let found = c
                .folders()?
                .into_iter()
                .find(|f| f.path == path)
                .ok_or_else(|| {
                    anyhow::anyhow!("{} is not a catalog folder here", path.display())
                })?;
            SyncScope::Folder(found.id)
        }
    };
    let inputs = c.sync_inputs(scope)?;
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let online = |path: &std::path::Path| rawmakase::volume_mounted(path);
    let plan = scan(&inputs, &online, &cancel, &|_| {})?;
    let shown = |p: &std::path::Path| p.display().to_string();
    for m in &plan.moves {
        let how = if m.verified {
            "moved"
        } else {
            "probably moved"
        };
        println!("{how}: {} → {}", shown(&m.from), shown(&m.to));
    }
    for choice in &plan.choices {
        println!("needs a choice:");
        for m in &choice.options {
            println!("  {} → {}", shown(&m.from), shown(&m.to));
        }
    }
    for a in &plan.additions {
        if a.copy_of.is_empty() {
            println!("new: {}", shown(&a.file));
        } else {
            println!("copy of a photo in the catalog: {}", shown(&a.file));
        }
    }
    for m in &plan.missing {
        println!("missing: {}", shown(&m.path));
    }
    for n in &plan.not_checked {
        println!("not checked: {} ({})", shown(&n.path), n.reason);
    }
    for c in &plan.conflicts {
        println!("not added: {}", c.message());
    }
    for d in &plan.ambiguous {
        println!(
            "not added, more than one catalog folder could be it: {}",
            shown(d)
        );
    }
    if !apply {
        println!("Dry run: nothing changed. Run again with --apply to apply.");
        return Ok(());
    }
    let applied = c.apply_folder_sync(
        &plan,
        &plan.default_accepted(),
        &online,
        None,
        &rawmakase::catalog::MetadataDefaults::load(),
    )?;
    for s in &applied.skipped {
        println!("skipped: {} ({})", shown(&s.path), s.reason);
    }
    println!(
        "Synchronized · {} updated · {} added · {} still missing",
        applied.moved.len(),
        applied.added,
        plan.missing.len()
    );
    if let Some(e) = applied.add_error {
        anyhow::bail!("Adding the new files failed: {e:#}");
    }
    Ok(())
}

/// Writes a stage tap as a 32-bit float TIFF at `output`, with an export's
/// protections: never over a RAW file or the `input` it was rendered from, never over
/// an existing file unless `overwrite`, and encoded beside it before being put in
/// place, so a failed write leaves the destination as it was.
fn save_stage(
    output: &std::path::Path,
    input: &std::path::Path,
    out: rawmakase::rendered::Rendered,
    overwrite: bool,
) -> Result<()> {
    use rawmakase::storage::{RasterFormat, Replace, is_raw, write_atomic};
    anyhow::ensure!(!is_raw(output), "A stage tap cannot overwrite a RAW file");
    anyhow::ensure!(
        RasterFormat::of(output) == Some(RasterFormat::Tiff),
        "A stage tap is a TIFF: name the output .tif or .tiff"
    );
    if output.exists() {
        anyhow::ensure!(
            std::fs::canonicalize(output)? != std::fs::canonicalize(input)?,
            "Cannot overwrite the source"
        );
        anyhow::ensure!(overwrite, "Output already exists");
    }
    let image = image::Rgb32FImage::from_raw(
        out.width,
        out.height,
        out.pixels.into_iter().flatten().collect(),
    )
    .ok_or_else(|| anyhow::anyhow!("Stage image has the wrong size"))?;
    let replace = if overwrite {
        Replace::Overwrite
    } else {
        Replace::NoClobber
    };
    write_atomic(output, replace, |file| {
        // Flushed here, so a failed last write fails the tap instead of being lost when
        // the writer is dropped.
        let mut writer = std::io::BufWriter::new(file);
        image.write_to(&mut writer, image::ImageFormat::Tiff)?;
        std::io::Write::flush(&mut writer)?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tap() -> rawmakase::rendered::Rendered {
        rawmakase::rendered::Rendered {
            width: 2,
            height: 1,
            pixels: vec![[0.1, 0.2, 0.3], [0.4, 0.5, 0.6]],
        }
    }

    #[test]
    fn stage_taps_never_replace_raws_their_source_or_unasked_files() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("photo.dng");
        std::fs::write(&raw, b"raw bytes").unwrap();
        // Over the RAW it was rendered from, with or without --overwrite.
        for overwrite in [false, true] {
            assert!(save_stage(&raw, &raw, tap(), overwrite).is_err());
            assert_eq!(std::fs::read(&raw).unwrap(), b"raw bytes");
        }
        // Over the source under a non-RAW name.
        let source = dir.path().join("source.tif");
        std::fs::write(&source, b"source").unwrap();
        assert!(save_stage(&source, &source, tap(), true).is_err());
        assert_eq!(std::fs::read(&source).unwrap(), b"source");
        // An existing file only with --overwrite.
        let output = dir.path().join("tap.tif");
        std::fs::write(&output, b"old").unwrap();
        assert!(save_stage(&output, &raw, tap(), false).is_err());
        assert_eq!(std::fs::read(&output).unwrap(), b"old");
        save_stage(&output, &raw, tap(), true).unwrap();
        let written = image::open(&output).unwrap().to_rgb32f();
        assert_eq!(written.dimensions(), (2, 1));
        assert_eq!(written.get_pixel(1, 0).0, [0.4, 0.5, 0.6]);
    }

    #[test]
    fn a_stage_tap_is_only_written_under_a_tiff_name() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("photo.dng");
        for name in ["tap.jpg", "tap.png", "tap.exr", "tap"] {
            let output = dir.path().join(name);
            let refused = save_stage(&output, &source, tap(), false).unwrap_err();
            assert!(refused.to_string().contains(".tif"), "{name}: {refused}");
            assert!(!output.exists(), "{name}");
        }
        for name in ["tap.tif", "tap.TIFF"] {
            save_stage(&dir.path().join(name), &source, tap(), false).unwrap();
        }
    }
}
