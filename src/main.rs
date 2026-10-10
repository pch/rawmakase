// Release builds on Windows open no console window beside the app.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
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
    Benchmark {
        input: PathBuf,
        #[arg(long, default_value_t = 20)]
        iterations: usize,
    },
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
                rawmakase::presets::load_preset(&p)?
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
                rawmakase::presets::save_preset(&path, &edit)?;
            }
            if let Some(tap) = tap {
                let stage = match tap {
                    Tap::SceneInput => develop::quality::Stage::SceneInput,
                    Tap::SceneOutput => develop::quality::Stage::SceneOutput,
                };
                let out =
                    develop::quality::render_stage(&im, &edit, stage, &AtomicBool::new(false))?;
                anyhow::ensure!(overwrite || !output.exists(), "Output already exists");
                image::Rgb32FImage::from_raw(
                    out.width,
                    out.height,
                    out.pixels.into_iter().flatten().collect(),
                )
                .ok_or_else(|| anyhow::anyhow!("Stage image has the wrong size"))?
                .save_with_format(&output, image::ImageFormat::Tiff)?;
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
