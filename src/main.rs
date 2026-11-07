// A console subsystem puts the generic console icon on the Windows taskbar.
// Command output still attaches to the parent console when there is one.
#![cfg_attr(windows, windows_subsystem = "windows")]
use anyhow::Result;
use rawmakase_cancel::Cancel;
mod mcp;
use clap::{Parser, Subcommand};
use rawmakase::model::recipe::Recipe;
use rawmakase::{camera_data, develop, export_settings::ExportOptions, raw};
use std::{path::PathBuf, time::Instant};
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
    /// Import Lightroom DCP and look XMP profiles and .cube LUTs (as looks), or the
    /// folders holding them, into RAWmakase's profile library.
    ImportProfiles {
        /// The colour space the .cube LUTs expect, as Camera Raw's Create Profile
        /// asks; keep sRGB unless the LUT's maker says otherwise.
        #[arg(long, value_enum, default_value_t = LutSpace::Srgb)]
        lut_space: LutSpace,
        #[arg(required = true, num_args = 1..)]
        files: Vec<PathBuf>,
    },
    /// Import Adobe lens profiles (.lcp) for Enable Profile Corrections.
    ImportLensProfiles {
        #[arg(required = true, num_args = 1..)]
        files: Vec<PathBuf>,
    },
    /// Lensfun's community lens profiles (CC BY-SA 3.0, from lensfun.github.io):
    /// what is installed, installing, updating and removing them.
    #[command(subcommand)]
    Lensfun(LensfunCommand),
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
    /// Find on Disk for an offline photo: checks that FILE is photo
    /// PHOTO_ID (a virtual copy stands for its master) and looks beside it
    /// for the other offline photos of its folder. Prints the plan; --apply
    /// relinks what the review ticks by default. Nothing on disk is changed.
    FindPhoto {
        catalog: PathBuf,
        photo_id: i64,
        file: PathBuf,
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
    /// Print the system report the Report an Issue dialog shows, for a bug
    /// report. Opens no window, catalog, photo or GPU, so the window's lines
    /// say so.
    SystemInfo,
    /// Render one RAW with several XMP sidecars, developing it once: for comparisons
    /// with Camera Raw. `jobs` is a JSON list of {"xmp", "output", "max_edge",
    /// "color_space"}; existing outputs are skipped. A job's `color_space` (srgb,
    /// adobe-rgb or prophoto) replaces --color-space.
    RenderBatch {
        input: PathBuf,
        jobs: PathBuf,
        /// The colour space of every job without its own: srgb, adobe-rgb or prophoto
        /// (16-bit TIFF only).
        #[arg(long, value_parser = parse_color_space, default_value = "srgb")]
        color_space: rawmakase::color::OutputSpace,
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
        /// The output's colour space: srgb, adobe-rgb or prophoto (16-bit TIFF only).
        /// An output whose format can't hold it is refused before anything renders.
        #[arg(long, value_parser = parse_color_space, default_value = "srgb", conflicts_with = "tap")]
        color_space: rawmakase::color::OutputSpace,
        #[arg(long)]
        fast: bool,
        #[arg(long)]
        overwrite: bool,
        #[arg(long)]
        recipe: Option<PathBuf>,
        /// Start from the photo's edit in this RAWmakase catalog (a virtual copy's is
        /// the master's): its saved edit, else its Lightroom edit, else the raw
        /// defaults. Without it, a legacy sidecar, else the raw defaults chosen in
        /// Preferences. `--recipe` comes before both.
        #[arg(long)]
        catalog: Option<PathBuf>,
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
        /// Write the edit's HDR master (the edit needs HDR on) as a 32-bit float TIFF
        /// of linear ProPhoto RGB, 1 = graphics white, for comparisons with Camera Raw.
        #[arg(long, conflicts_with_all = ["tap", "color_space"])]
        hdr: bool,
        /// Export the edit's HDR master in this encoding (pq, hlg or linear: TIFF or
        /// JPEG XL; gain-map: JPEG), as the Export dialog's HDR export writes it (the
        /// edit needs HDR on).
        #[arg(long, value_enum, conflicts_with_all = ["tap", "color_space", "hdr"])]
        hdr_export: Option<HdrTransferArg>,
        /// The HDR export's gamut: rec2020, display-p3 or rec709.
        #[arg(long, value_enum, default_value = "rec2020", requires = "hdr_export")]
        hdr_gamut: HdrGamutArg,
        /// Write a .jxl output lossy at this quality (1–100, as the Export dialog's;
        /// 90 is libjxl's distance 1) instead of lossless.
        #[arg(long, value_parser = clap::value_parser!(u8).range(1..=100), conflicts_with_all = ["tap", "hdr"])]
        jxl_quality: Option<u8>,
    },
    /// Print each file's decode fingerprint as a line of JSON: what opening and
    /// unpacking it yields, for decode regression checks.
    Fingerprint {
        #[arg(required = true, num_args = 1..)]
        files: Vec<PathBuf>,
    },
    /// Development command: run AI Denoise (RawNIND UtNet2) on one RAW and print the
    /// times and the fitted gain as JSON; optionally render it and the plain image.
    Denoise {
        input: PathBuf,
        /// The model's folder (the installed model by default).
        #[arg(long)]
        model: Option<PathBuf>,
        /// cpu or coreml.
        #[arg(long, default_value = "cpu")]
        provider: String,
        #[arg(long, default_value_t = 1.0)]
        amount: f32,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long)]
        plain_output: Option<PathBuf>,
        #[arg(long, default_value_t = 0)]
        max_edge: u32,
        #[arg(long)]
        dump: Option<PathBuf>,
        /// Read and write the user's AI Denoise cache.
        #[arg(long)]
        cache: bool,
        /// Time Develop's renders while Amount moves (Fit view of this long edge, and a
        /// 100% region).
        #[arg(long)]
        bench_fit: Option<u32>,
        /// With --bench-fit: the baseline without AI Denoise.
        #[arg(long)]
        bench_plain: bool,
        /// Write centre crops of the camera image (plain, AI, manual noise reduction)
        /// to this folder, for phase 0's quality measures.
        #[arg(long)]
        camera_dump: Option<PathBuf>,
        #[arg(long, default_value_t = 2048)]
        camera_crop: u32,
        /// Luminance noise reduction amounts (0–1) for --camera-dump.
        #[arg(long, value_delimiter = ',')]
        nr_luma: Vec<f32>,
    },
    /// Print each file's develop fingerprint as a line of JSON: the image each
    /// decode mode (full, half) makes, hashed, for decoder regression checks.
    DecodeDump {
        #[arg(required = true, num_args = 1..)]
        files: Vec<PathBuf>,
        /// Also write each full decode that is not a mosaic here, as raw floats.
        #[arg(long)]
        floats: Option<PathBuf>,
    },
    /// Print each file's half-size check as a line of JSON: RAWmakase's
    /// half-size decode against its full decode reduced (colour, detail) and
    /// its time over the full develop's.
    HalfCheck {
        #[arg(required = true, num_args = 1..)]
        files: Vec<PathBuf>,
        /// The files' reference as JSON (`{"ratio":…,"de_mean":…,"de_p95":…,
        /// "detail":…}`): LibRaw's half-size decode, recorded, which
        /// scripts/corpus/dataset-half.py passes. Without one, absolute limits.
        #[arg(long, value_parser = parse_reference)]
        reference: Option<rawmakase::half_check::Reference>,
    },
    /// Fit Camera Look (fitted) to each RAW's embedded JPEG, the camera's own
    /// rendering, and say whether the look was used or why not, with its held-out
    /// error against the JPEG (docs/lightroom-profiles.md#camera-look-fitted).
    /// Nothing is saved.
    FitLook {
        #[arg(required = true, num_args = 1..)]
        files: Vec<PathBuf>,
        /// Print each file's full measurements as a line of JSON.
        #[arg(long)]
        report: bool,
        /// Fit a matrix even for a camera that has one, as for a camera without, and
        /// compare it with the camera's own (the no-matrix trial).
        #[arg(long)]
        drop_matrix: bool,
    },
    Benchmark {
        input: PathBuf,
        #[arg(long, default_value_t = 20)]
        iterations: usize,
    },
}
/// A `--color-space`, by the names the automation socket uses too.
fn parse_color_space(name: &str) -> Result<rawmakase::color::OutputSpace, String> {
    rawmakase::color::OutputSpace::from_name(name)
        .ok_or_else(|| format!("unknown color space {name:?}; use srgb, adobe-rgb or prophoto"))
}
/// Refuses an `output` whose format can't hold `space`, before anything renders.
fn check_output(output: &std::path::Path, space: rawmakase::color::OutputSpace) -> Result<()> {
    use rawmakase::export_settings::Format;
    Format::from_path(output)
        .ok_or_else(|| anyhow::anyhow!("Export extension must be {}", Format::extensions_listed()))?
        .supports(space)
}
/// A half-size check's `--reference`.
fn parse_reference(json: &str) -> Result<rawmakase::half_check::Reference, String> {
    serde_json::from_str(json).map_err(|e| e.to_string())
}
/// An `import-profiles --lut-space`.
#[derive(Subcommand)]
enum LensfunCommand {
    /// The version installed, and where it came from.
    Status,
    /// Download and install Lensfun's database, or install an archive already
    /// downloaded (Lensfun's version_2.tar.bz2).
    Install {
        #[arg(long)]
        from: Option<PathBuf>,
    },
    /// Install Lensfun's newest database if it is newer than the one installed.
    Update,
    /// Remove the installed database; edits keep the corrections they use.
    Remove,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum LutSpace {
    Srgb,
    AdobeRgb,
    DisplayP3,
    #[value(name = "prophoto-rgb")]
    ProPhotoRgb,
    Rec2020,
}
impl From<LutSpace> for rawmakase::camera_profiles::LutSpace {
    fn from(space: LutSpace) -> Self {
        match space {
            LutSpace::Srgb => Self::Srgb,
            LutSpace::AdobeRgb => Self::AdobeRgb,
            LutSpace::DisplayP3 => Self::DisplayP3,
            LutSpace::ProPhotoRgb => Self::ProPhotoRgb,
            LutSpace::Rec2020 => Self::Rec2020,
        }
    }
}
/// `render --hdr-export`'s transfer.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum HdrTransferArg {
    Pq,
    Hlg,
    Linear,
    /// A gain-map JPEG over the SDR rendition (with --hdr-gamut rec709).
    GainMap,
}
/// `render --hdr-gamut`.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum HdrGamutArg {
    Rec2020,
    DisplayP3,
    Rec709,
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
/// the app would vanish without a word. The failure may come at startup or, as a
/// panic, later on.
#[cfg(windows)]
fn show_error(error: &anyhow::Error) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let text = wide(&format!(
        "RAWmakase stopped because of an error.\n\n{error:#}"
    ));
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
        Some(Command::Lensfun(command)) => lensfun(command)?,
        Some(Command::ImportProfiles { lut_space, files }) => {
            // Files and folders, imported as the app's Import does: a file that
            // can't be imported is reported and leaves the rest importing.
            let summary = rawmakase::app::import_profiles(&files, lut_space.into());
            for line in summary.lines() {
                println!("{line}");
            }
            println!("{}", summary.message());
            anyhow::ensure!(
                summary.failures() == 0 && summary.done() > 0,
                "{}",
                summary.message()
            );
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
        Some(Command::FindPhoto {
            catalog,
            photo_id,
            file,
            apply,
        }) => find_photo(
            &catalog,
            rawmakase::catalog::PhotoId(photo_id),
            &file,
            apply,
        )?,
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
        Some(Command::SystemInfo) => {
            use rawmakase::diagnostics;
            print!(
                "{}",
                diagnostics::report(
                    &diagnostics::Build::current(),
                    &diagnostics::System::probe(),
                    None
                )
            );
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
                "{}\n{}",
                raw::version(),
                serde_json::to_string_pretty(&r.metadata)?
            );
        }
        Some(Command::RenderBatch {
            input,
            jobs,
            color_space,
        }) => {
            #[derive(serde::Deserialize)]
            struct Job {
                xmp: PathBuf,
                output: PathBuf,
                #[serde(default)]
                max_edge: u32,
                #[serde(default)]
                color_space: Option<String>,
            }
            let jobs: Vec<Job> = serde_json::from_str(&std::fs::read_to_string(&jobs)?)?;
            // Read before decoding: AI Denoise's result must be of the same file.
            let file = rawmakase::storage::Identity::read(&input)?;
            let photo = rawmakase::photo::open(&input)?;
            let im = std::sync::Arc::new(
                photo.develop(camera_data::Decode::full(Default::default()), Cancel::NEVER)?,
            );
            let (profiles, _) = rawmakase::camera_profiles::installed(&im.metadata);
            let base = Recipe::with_profiles(&im.metadata, &profiles);
            let camera = Some(
                rawmakase::photo::camera_exif(&input)
                    .unwrap_or_else(|| rawmakase::export::job::stand_in(&input, &im.metadata)),
            );
            // One AI Denoise service for every job: the model loads once.
            let ai_denoise =
                rawmakase::ai_denoise::CommandLine::new(rawmakase::app::user_ai_denoise_cache());
            for job in jobs.iter().filter(|j| !j.output.exists()) {
                let rendered = (|| -> Result<()> {
                    let space = match &job.color_space {
                        Some(name) => parse_color_space(name).map_err(anyhow::Error::msg)?,
                        None => color_space,
                    };
                    check_output(&job.output, space)?;
                    let preset =
                        rawmakase::xmp::parse(&job.xmp, &std::fs::read_to_string(&job.xmp)?)?;
                    let edit = preset.apply(
                        &base,
                        &im.metadata,
                        &profiles,
                        Some(&rawmakase::develop::Measures(&im)),
                    )?;
                    let ai = ai_denoise.source(&input, &file, &im, &edit)?;
                    let out = develop::render_export(
                        &im,
                        &edit.checked()?,
                        ai.as_ref(),
                        job.max_edge,
                        space,
                        Cancel::NEVER,
                    )?;
                    rawmakase::export::export_image(
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
            color_space,
            fast,
            overwrite,
            recipe,
            catalog,
            auto,
            auto_wb,
            tap,
            hdr,
            hdr_export,
            hdr_gamut,
            jxl_quality,
        }) => {
            anyhow::ensure!(
                jxl_quality.is_none()
                    || rawmakase::export_settings::Format::from_path(&output)
                        == Some(rawmakase::export_settings::Format::Jxl),
                "--jxl-quality is for a .jxl output"
            );
            let export_options = ExportOptions {
                max_edge,
                quality: jxl_quality.unwrap_or(ExportOptions::default().quality),
                jxl_lossy: jxl_quality.is_some(),
            };
            use rawmakase::color::hdr_output::{HdrGamut, HdrOutput, HdrTransfer};
            let hdr_output = hdr_export.map(|transfer| HdrOutput {
                transfer: match transfer {
                    HdrTransferArg::Pq => HdrTransfer::Pq,
                    HdrTransferArg::Hlg => HdrTransfer::Hlg,
                    HdrTransferArg::Linear => HdrTransfer::Linear,
                    HdrTransferArg::GainMap => HdrTransfer::default(),
                },
                gamut: match hdr_gamut {
                    HdrGamutArg::Rec2020 => HdrGamut::Rec2020,
                    HdrGamutArg::DisplayP3 => HdrGamut::DisplayP3,
                    HdrGamutArg::Rec709 => HdrGamut::Rec709,
                },
                gain_map: matches!(transfer, HdrTransferArg::GainMap),
            });
            if let Some(encoding) = hdr_output {
                let format = rawmakase::export_settings::Format::from_path(&output).ok_or_else(
                    || anyhow::anyhow!("An HDR export is a TIFF, JPEG XL or gain-map JPEG: name the output .tif, .tiff, .jxl or .jpg"),
                )?;
                rawmakase::export_settings::Format::check_hdr(format, encoding)?;
            }
            if tap.is_none() && !hdr && hdr_output.is_none() {
                check_output(&output, color_space)?;
            }
            let t = Instant::now();
            // Read before decoding: AI Denoise's result must be of the same file.
            let file = rawmakase::storage::Identity::read(&input)?;
            let r = rawmakase::photo::open(&input)?;
            let mut edit = if let Some(p) = recipe {
                rawmakase::presets::load_preset(&p)?.recipe
            } else if let Some(sidecar) = catalog
                .is_none()
                .then(|| rawmakase::catalog::legacy_sidecar::load(&input))
                .transpose()?
                .flatten()
            {
                sidecar.recipe
            } else {
                let record = match &catalog {
                    Some(catalog) => catalog_edit(catalog, &input)?,
                    None => rawmakase::edits::EditRecord::default(),
                };
                // As the app opens it: a photo without colour data starts from its
                // fitted camera look.
                let profiles = rawmakase::camera_look::with_default(
                    &input,
                    &r.metadata,
                    rawmakase::camera_profiles::installed(&r.metadata).0,
                    Cancel::NEVER,
                );
                let resolved = rawmakase::edits::resolve(
                    &record,
                    &input,
                    &r.metadata,
                    &profiles,
                    &rawmakase::app::user_raw_defaults(),
                )?;
                for warning in &resolved.warnings {
                    eprintln!("{warning}");
                }
                resolved.recipe
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
            let im = r.develop(decode, Cancel::NEVER)?;
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
                edit = develop::auto_white_balance(&im, &edit, Cancel::NEVER)?;
                eprintln!(
                    "Auto white balance ({:?}): temperature {:.0} tint {:+.0}",
                    t.elapsed(),
                    edit.temperature,
                    edit.tint
                );
            }
            if auto {
                let t = Instant::now();
                edit = develop::auto_tone(&im, &edit, Cancel::NEVER)?;
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
                // A stage tap renders without AI Denoise: refused rather than written
                // as if the recipe had none.
                anyhow::ensure!(
                    !edit.as_rendered().ai_denoise.renders(),
                    "--tap does not render AI Denoise; switch it off in the recipe"
                );
                let stage = match tap {
                    Tap::SceneInput => develop::quality::Stage::SceneInput,
                    Tap::SceneOutput => develop::quality::Stage::SceneOutput,
                };
                let out = develop::quality::render_stage(&im, &edit, stage, Cancel::NEVER)?;
                save_stage(&output, &input, out, overwrite)?;
                return Ok(());
            }
            let developed = t.elapsed();
            let im = std::sync::Arc::new(im);
            let ai =
                rawmakase::ai_denoise::CommandLine::new(rawmakase::app::user_ai_denoise_cache())
                    .source(&input, &file, &im, &edit)?;
            if let Some(encoding) = hdr_output {
                let master = develop::render_hdr(
                    &im,
                    &edit.checked()?,
                    ai.as_ref(),
                    max_edge,
                    Cancel::NEVER,
                )?;
                let pixels = if encoding.gain_map {
                    let base = develop::render_export(
                        &im,
                        &edit.checked()?,
                        ai.as_ref(),
                        max_edge,
                        rawmakase::color::OutputSpace::Srgb,
                        Cancel::NEVER,
                    )?;
                    rawmakase::export::Pixels::GainMap(rawmakase::export::GainMapPixels::new(
                        base, &master,
                    )?)
                } else {
                    rawmakase::export::Pixels::Hdr(rawmakase::export::HdrPixels::encode(
                        &master, encoding,
                    ))
                };
                rawmakase::export::export_pixels(
                    &output,
                    &input,
                    &pixels,
                    &im.metadata,
                    &export_options,
                    &rawmakase::export::Embed::default(),
                    if overwrite {
                        rawmakase::export::Replace::Overwrite
                    } else {
                        rawmakase::export::Replace::NoClobber
                    },
                )?;
                println!(
                    "{}x{} HDR export, peak {} | total {:?}",
                    master.width,
                    master.height,
                    master.peak,
                    t.elapsed()
                );
                return Ok(());
            }
            if hdr {
                let out = develop::render_hdr(
                    &im,
                    &edit.checked()?,
                    ai.as_ref(),
                    max_edge,
                    Cancel::NEVER,
                )?;
                let rendered = rawmakase::rendered::Rendered {
                    width: out.width,
                    height: out.height,
                    pixels: out.pixels,
                };
                save_stage(&output, &input, rendered, overwrite)?;
                println!(
                    "{}x{} HDR master, peak {} | total {:?}",
                    out.width,
                    out.height,
                    out.peak,
                    t.elapsed()
                );
                return Ok(());
            }
            let out = develop::render_export(
                &im,
                &edit.checked()?,
                ai.as_ref(),
                max_edge,
                color_space,
                Cancel::NEVER,
            )?;
            let rendered = t.elapsed() - developed;
            rawmakase::export::export_image(
                &output,
                &input,
                &out,
                &im.metadata,
                &export_options,
                &rawmakase::export::Embed {
                    camera: Some(
                        rawmakase::photo::camera_exif(&input).unwrap_or_else(|| {
                            rawmakase::export::job::stand_in(&input, &im.metadata)
                        }),
                    ),
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
                out.width(),
                out.height(),
                developed,
                rendered,
                t.elapsed(),
                max,
                im.scale_factor,
                im.scale_clipped
            );
        }
        Some(Command::Fingerprint { files }) => {
            for path in &files {
                let line = serde_json::to_string(&rawmakase::fingerprint::fingerprint(path))?;
                println!("{line}");
            }
        }
        Some(Command::DecodeDump { files, floats }) => {
            for path in &files {
                let dump = rawmakase::decode_dump::dump(path, floats.as_deref());
                println!("{}", serde_json::to_string(&dump)?);
            }
        }
        Some(Command::HalfCheck { files, reference }) => {
            for path in &files {
                let check = rawmakase::half_check::check(path, reference);
                println!("{}", serde_json::to_string(&check)?);
            }
        }
        Some(Command::FitLook {
            files,
            report,
            drop_matrix,
        }) => {
            use rawmakase::camera_look::{FitInputs, fit};
            let inputs = FitInputs { drop_matrix };
            for path in &files {
                let result = fit(path, &inputs, Cancel::NEVER);
                let (measured, outcome) = match &result {
                    Ok(f) => (Some(&f.report), format!("{:?} look", f.report.kind)),
                    Err(e) => (e.report.as_deref(), format!("refused: {}", e.reason)),
                };
                if report {
                    // A file that could not even be opened still gets its line.
                    let mut line = match measured {
                        Some(r) => serde_json::to_value(r)?,
                        None => serde_json::json!({
                            "refusal": result.as_ref().err().map(|e| e.reason.clone()),
                        }),
                    };
                    if let Some(o) = line.as_object_mut() {
                        o.insert("file".into(), path.display().to_string().into());
                    }
                    println!("{line}");
                } else {
                    let scores = measured.map_or(String::new(), |r| {
                        // The look used, else the first tried.
                        let shown = r
                            .candidates
                            .iter()
                            .find(|c| Some(c.kind) == r.kind)
                            .or(r.candidates.first());
                        let fitted = shown.map_or(f32::NAN, |c| c.scores.de00);
                        format!(" (held-out ΔE00 {:.2} → {fitted:.2})", r.baseline.de00)
                    });
                    println!("{}: {outcome}{scores}", path.display());
                }
            }
        }
        Some(Command::Denoise {
            input,
            model,
            provider,
            amount,
            output,
            plain_output,
            max_edge,
            dump,
            cache,
            bench_fit,
            bench_plain,
            camera_dump,
            camera_crop,
            nr_luma,
        }) => {
            let provider = match provider.as_str() {
                "cpu" => rawmakase_inference::Provider::Cpu,
                "coreml" => rawmakase_inference::Provider::CoreML,
                other => anyhow::bail!("Unknown provider {other}"),
            };
            rawmakase::ai_denoise::command(rawmakase::ai_denoise::Command {
                input,
                model,
                provider,
                amount,
                output,
                plain_output,
                max_edge,
                dump,
                cache: cache.then(rawmakase::app::user_ai_denoise_cache),
                bench_fit,
                bench_plain,
                camera_dump,
                camera_crop,
                nr_luma,
            })?;
        }
        Some(Command::Benchmark { input, iterations }) => {
            anyhow::ensure!(
                (1..=1000).contains(&iterations),
                "Iterations must be 1–1000"
            );
            let t = Instant::now();
            let im = rawmakase::photo::open(&input)?
                .develop(camera_data::Decode::full(Default::default()), Cancel::NEVER)?;
            let decode = t.elapsed();
            let small = develop::preview(&im, 1600);
            let mut r = Recipe::for_metadata(&im.metadata);
            let mut times = Vec::new();
            for i in 0..iterations {
                r.exposure = (i % 8) as f32 / 8.;
                let t = Instant::now();
                std::hint::black_box(develop::render(&small, &r.checked()?, 1600, Cancel::NEVER)?);
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
/// The stored edit of the photo at `input` in `catalog`: of the master, as a path
/// names it, not of a virtual copy. A photo the catalog doesn't have, or has more
/// than once (two folders found at the same place), is an error.
fn catalog_edit(
    catalog: &std::path::Path,
    input: &std::path::Path,
) -> Result<rawmakase::edits::EditRecord> {
    let c = rawmakase::catalog::Catalog::open(catalog)?;
    let input = input.canonicalize()?;
    let found: Vec<_> = c
        .photos()?
        .into_iter()
        .filter(|p| p.master.is_none() && p.path.canonicalize().is_ok_and(|p| p == input))
        .map(|p| p.id)
        .collect();
    let photo = match found[..] {
        [photo] => photo,
        [] => anyhow::bail!("{} is not in the catalog", input.display()),
        _ => anyhow::bail!(
            "{} is {} photos in the catalog: two of its folders are found at the same place",
            input.display(),
            found.len()
        ),
    };
    // Masks made from a selection are read back from the catalog, as in the app.
    rawmakase::storage::mask_assets::add_loader(c.mask_asset_loader());
    // One consistent read: the recipe with the spots and masks saved with it.
    c.photo_records(&[photo])?
        .pop()
        .map(|record| record.edit)
        .ok_or_else(|| anyhow::anyhow!("{} is not in the catalog", input.display()))
}

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
    let plan = scan(&inputs, &online, Cancel::new(&cancel), &|_| {})?;
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
        &rawmakase::catalog::NewPhotoDefaults::new(
            rawmakase::catalog::MetadataDefaults::load(),
            rawmakase::photo::camera_exif,
            rawmakase::photo::raw_xmp,
        )
        .with_options(rawmakase::catalog::ImportOptions::load()),
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

/// `rawmakase find-photo`: finds `photo` at `file` and the other offline
/// photos of its folder beside it, prints the plan, and relinks with `apply`.
fn find_photo(
    catalog: &std::path::Path,
    photo: rawmakase::catalog::PhotoId,
    file: &std::path::Path,
    apply: bool,
) -> Result<()> {
    use rawmakase::catalog::folder_sync::find_on_disk;
    let mut c = rawmakase::catalog::Catalog::open(catalog)?;
    let inputs = c.find_inputs(photo, file)?;
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let online = |path: &std::path::Path| rawmakase::volume_mounted(path);
    let plan = find_on_disk(&inputs, &online, Cancel::new(&cancel), &|_| {})?;
    let shown = |p: &std::path::Path| p.display().to_string();
    let ticked = plan.default_accepted();
    for m in &plan.moves {
        let how = match (m.verified, m.by_name) {
            (true, _) => "verified",
            (false, true) => "same name, not verified",
            (false, false) => "probably, not verified",
        };
        let tick = if ticked.moves.contains(&(m.photo, m.to.clone())) {
            "[x]"
        } else {
            "[ ]"
        };
        let offline = if m.offline {
            " (its old drive isn't connected)"
        } else {
            ""
        };
        println!(
            "{tick} {how}: {} → {}{offline}",
            shown(&m.from),
            shown(&m.to)
        );
    }
    for choice in &plan.choices {
        println!("needs a choice:");
        for m in &choice.options {
            println!("  {} → {}", shown(&m.from), shown(&m.to));
        }
    }
    if let Some(root) = plan.picked().and_then(|m| m.new_root()) {
        println!("{} becomes a new root folder", shown(root));
    }
    if !apply {
        println!("Dry run: nothing changed. Run again with --apply to relink.");
        return Ok(());
    }
    let applied = c.apply_folder_sync(&plan, &ticked, &online, None, &Default::default())?;
    for s in &applied.skipped {
        println!("skipped: {} ({})", shown(&s.path), s.reason);
    }
    println!("Relinked {} photos", applied.moved.len());
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

/// `rawmakase lensfun`.
fn lensfun(command: LensfunCommand) -> Result<()> {
    use rawmakase::lens::lensfun::{choice, installed};
    use rawmakase::lensfun_install::{self as install, Installed, Progress, Source};
    let dir = installed::dir();
    let terms = || {
        println!(
            "Lensfun lens database by {}, {} ({}), from {}",
            choice::ATTRIBUTION,
            choice::LICENSE,
            choice::LICENSE_URI,
            install::ARCHIVE_URL
        )
    };
    let progress = Progress::default();
    match command {
        LensfunCommand::Status => {
            match install::current(&dir) {
                Some(m) => println!(
                    "Installed: version {} ({}), {} lenses, {} cameras, downloaded {} from {}, SHA-256 {}",
                    m.version,
                    m.date(),
                    m.lenses,
                    m.cameras,
                    m.downloaded,
                    m.url,
                    m.sha256
                ),
                None => println!("Not installed"),
            }
            println!("Folder: {}", dir.display());
            terms();
        }
        LensfunCommand::Install { from } => {
            if from.is_none()
                && let Some(m) = install::current(&dir)
            {
                println!(
                    "Version {} ({}) is installed; `rawmakase lensfun update` installs a newer one",
                    m.version,
                    m.date()
                );
                return Ok(());
            }
            terms();
            let source = from.map_or(Source::Lensfun, Source::File);
            match install::install(&dir, &source, &progress)? {
                Installed::New(m) => println!(
                    "Installed version {} ({}): {} lenses, {} cameras",
                    m.version,
                    m.date(),
                    m.lenses,
                    m.cameras
                ),
                Installed::Current(m) => {
                    println!("Version {} ({}) is already installed", m.version, m.date())
                }
            }
        }
        LensfunCommand::Update => match install::install(&dir, &Source::Lensfun, &progress)? {
            Installed::New(m) => println!("Updated to version {} ({})", m.version, m.date()),
            Installed::Current(m) => println!("Up to date: version {} ({})", m.version, m.date()),
        },
        LensfunCommand::Remove => {
            if install::remove(&dir, &progress)? {
                println!("Removed; edits keep the Lensfun corrections they use");
            } else {
                println!("Not installed");
            }
        }
    }
    Ok(())
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

    /// `--color-space` takes the socket's names, and an output whose format can't hold
    /// the space is refused before anything is read.
    #[test]
    fn render_takes_a_color_space_and_refuses_prophoto_outside_tiff() {
        use rawmakase::color::OutputSpace;
        let parse = |args: &[&str]| {
            Args::try_parse_from(
                ["rawmakase", "render", "in.dng", "out.tif"]
                    .iter()
                    .chain(args),
            )
        };
        for (name, space) in [
            ("srgb", OutputSpace::Srgb),
            ("adobe-rgb", OutputSpace::AdobeRgb),
            ("prophoto", OutputSpace::ProPhoto),
        ] {
            let cli = parse(&["--color-space", name]).unwrap();
            assert!(
                matches!(cli.command, Some(Command::Render { color_space, .. }) if color_space == space),
                "{name}"
            );
        }
        assert!(matches!(
            parse(&[]).unwrap().command,
            Some(Command::Render {
                color_space: OutputSpace::Srgb,
                ..
            })
        ));
        assert!(parse(&["--color-space", "display-p3"]).is_err());
        assert!(parse(&["--color-space", "prophoto", "--tap", "scene-input"]).is_err());
        for name in ["a.jpg", "a.png"] {
            let refused = check_output(std::path::Path::new(name), OutputSpace::ProPhoto);
            assert!(refused.unwrap_err().to_string().contains("TIFF"), "{name}");
            check_output(std::path::Path::new(name), OutputSpace::AdobeRgb).unwrap();
        }
        check_output(std::path::Path::new("a.tiff"), OutputSpace::ProPhoto).unwrap();
        assert!(check_output(std::path::Path::new("a.webp"), OutputSpace::Srgb).is_err());
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
