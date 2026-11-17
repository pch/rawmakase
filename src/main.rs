// A console subsystem puts the generic console icon on the Windows taskbar.
// Command output still attaches to the parent console when there is one.
#![cfg_attr(windows, windows_subsystem = "windows")]
use anyhow::Result;
use clap::{Parser, Subcommand};
use rawmakase::model::recipe::Recipe;
use rawmakase::{camera_data, develop, raw};
use rawmakase_cancel::Cancel;
use std::{path::Path, path::PathBuf, time::Instant};
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
    /// Serve editing tools over MCP stdio, connected to the running desktop app
    /// (the bundled MCP extension, unless it is disabled in Preferences > Extensions).
    Mcp {
        /// Data folder of the running app; defaults to RAWMAKASE_DATA_DIR or the platform default.
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
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
    /// Extensions: what is installed, installing from a package file or a
    /// GitHub repository, removing, enabling and disabling them, and checking
    /// for updates.
    #[command(subcommand)]
    Extensions(ExtensionsCommand),
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
    RenderBatch(rawmakase::render_command::RenderBatch),
    Thumbnail {
        input: PathBuf,
        output: PathBuf,
    },
    Render(rawmakase::render_command::Render),
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

/// `rawmakase extensions`.
#[derive(Subcommand)]
enum ExtensionsCommand {
    /// The extensions installed, with their version, source and state.
    List,
    /// Install an extension from a package file (.rmext) or from a GitHub
    /// repository's releases (https://github.com/owner/repository). Asks first,
    /// unless --yes: an extension runs with your permissions, and nothing
    /// installed this way has been reviewed.
    Install {
        /// A package file or a repository's URL.
        from: String,
        /// Install without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Uninstall an extension; its private data folder stays.
    Remove { id: String },
    /// Turn an extension on; a running RAWmakase notices on its next start.
    Enable { id: String },
    /// Turn an extension off; a running RAWmakase notices on its next start.
    Disable { id: String },
    /// Look for newer versions of the extensions installed from GitHub.
    Check,
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
        Some(Command::Mcp { data_dir }) => mcp(data_dir)?,
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
        Some(Command::Extensions(command)) => extensions(command)?,
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
            let c = open_catalog(&path)?;
            println!(
                "Imported {} photographs into {}",
                c.photos()?.len(),
                path.display()
            );
            let unsupported = c
                .smart_rules()?
                .values()
                .filter(|r| r.rules().map_or(true, |g| g.unsupported().is_some()))
                .count();
            if unsupported > 0 {
                println!(
                    "{unsupported} smart collection(s) have rules RAWmakase can't evaluate; \
                     they list no photos until those rules are removed"
                );
            }
        }
        Some(Command::CatalogInfo { catalog }) => {
            let c = open_catalog(&catalog)?;
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
            open_catalog(&catalog)?.relink_root(rawmakase::catalog::RootId(root), &folder)?;
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
        Some(Command::RenderBatch(options)) => {
            rawmakase::render_command::render_batch(
                &options,
                &user_ai_denoise,
                Cancel::NEVER,
                &mut rawmakase::render_command::Console {
                    out: &mut std::io::stdout(),
                    err: &mut std::io::stderr(),
                },
            )?;
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
        Some(Command::Render(options)) => {
            rawmakase::render_command::render(
                &options,
                &rawmakase::app::user_raw_defaults,
                &user_ai_denoise,
                Cancel::NEVER,
                &mut rawmakase::render_command::Console {
                    out: &mut std::io::stdout(),
                    err: &mut std::io::stderr(),
                },
            )?;
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
                model: model.unwrap_or_else(rawmakase::ai_denoise::model_dir),
                runtime: rawmakase_model_install::runtime_dir(),
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

/// AI Denoise for `render` and `render-batch`, as the app runs it: with the user's
/// cache where the app keeps it, and the model and the ONNX Runtime installed in this
/// computer's own data folder.
fn user_ai_denoise() -> rawmakase::ai_denoise::CommandLine {
    rawmakase::ai_denoise::CommandLine::new(
        rawmakase::app::user_ai_denoise_cache(),
        rawmakase::ai_denoise::model_dir(),
        rawmakase_model_install::runtime_dir(),
    )
}

/// `rawmakase sync-folder`: scans, prints the plan, and applies it with
/// `apply`.
fn sync_folder(
    catalog: &std::path::Path,
    folder: Option<&std::path::Path>,
    apply: bool,
) -> Result<()> {
    use rawmakase::catalog::folder_sync::{SyncScope, scan};
    let mut c = open_catalog(catalog)?;
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
    let mut c = open_catalog(catalog)?;
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

/// The ids of the extensions that ship with the app, which `enable` and
/// `disable` accept without an install.
const BUNDLED_EXTENSIONS: &[&str] = &["rawmakase.mcp", "rawmakase.midi"];

/// `rawmakase mcp`, the command MCP clients are configured with: runs the
/// bundled MCP extension's program beside this executable, with this
/// process's standard streams and the same arguments, or fails when the
/// extension is disabled. The program serves MCP itself; this binary carries
/// no MCP server.
fn mcp(data_dir: Option<PathBuf>) -> Result<()> {
    let dirs = rawmakase_extensions::bundled::program_search_dirs();
    let state_dir = data_dir
        .clone()
        .unwrap_or_else(rawmakase_protocol::paths::data_dir);
    let program = rawmakase_extensions::external::program(
        include_str!("../extensions/mcp/rawmakase-extension.toml"),
        &state_dir,
        &dirs,
    )?;
    let mut args = Vec::new();
    if let Some(dir) = data_dir {
        args.push("--data-dir".into());
        args.push(dir.into_os_string());
    }
    let code = rawmakase_extensions::external::run(&program, &args)
        .map_err(|e| anyhow::anyhow!("{}: {e}", program.display()))?;
    std::process::exit(code)
}

/// `rawmakase extensions`.
fn extensions(command: ExtensionsCommand) -> Result<()> {
    use rawmakase_extensions::state::{Source, State};
    use rawmakase_extensions::{github, store::Store};
    let app = rawmakase_extensions::semver::Version::parse(env!("CARGO_PKG_VERSION"))?;
    let data_dir = rawmakase_protocol::paths::data_dir();
    let store = Store::new(&data_dir);
    let next_start = "a running RAWmakase notices on its next start";
    match command {
        ExtensionsCommand::List => {
            let listed = store.list()?;
            let developer = State::read(&data_dir).map(|s| s.developer)?;
            if listed.is_empty() && developer.is_empty() {
                println!("No extensions installed");
            }
            for item in listed {
                let state = if item.entry.enabled {
                    "enabled"
                } else {
                    "disabled"
                };
                let source = item.entry.source().map_or("?", |s| s.label());
                let version = item.entry.active.as_deref().unwrap_or("bundled");
                let mut line = format!("{}  {version}  {source}  {state}", item.id);
                match item.manifest {
                    Some(Ok(m)) => {
                        line = format!("{} ({line})", m.extension.name);
                        if let Err(e) = rawmakase_extensions::check_compatible(&m, &app) {
                            line += &format!("  incompatible: {e}");
                        }
                    }
                    Some(Err(e)) => line += &format!("  damaged: {e}"),
                    None => {}
                }
                println!("{line}");
            }
            for (id, entry) in developer {
                let state = if entry.enabled { "enabled" } else { "disabled" };
                println!("{id}  developer  {}  {state}", entry.path.display());
            }
        }
        ExtensionsCommand::Install { from, yes } => {
            if from.starts_with("https://") {
                let offer = github::inspect(&store, &github::Https, &from, &app)?;
                describe(
                    &offer.manifest,
                    github::official(&offer.manifest.extension.id, &offer.repository),
                );
                println!(
                    "From {} release {} ({})",
                    offer.repository, offer.release, offer.asset
                );
                if !confirm(yes)? {
                    return Ok(());
                }
                let installed = store.install_offer(offer, &app)?;
                println!(
                    "Installed {} {}",
                    installed.manifest.extension.id, installed.manifest.extension.version
                );
            } else {
                let path =
                    std::fs::canonicalize(&from).map_err(|e| anyhow::anyhow!("{from}: {e}"))?;
                let inspected = store.inspect(&path, &app)?;
                // A file is never official, whatever id it claims.
                describe(&inspected.manifest, false);
                if !confirm(yes)? {
                    return Ok(());
                }
                let installed = store.install(&path, &Source::File { path: path.clone() }, &app)?;
                println!(
                    "Installed {} {}",
                    installed.manifest.extension.id, installed.manifest.extension.version
                );
            }
        }
        ExtensionsCommand::Remove { id } => {
            store.remove(&id)?;
            println!(
                "Removed {id}; its data in {} stays",
                store.extension_data_dir(&id).display()
            );
            // Its keys, recorded with the removal: one the store keeps is
            // deleted at RAWmakase's next start.
            if let Err(e) = rawmakase::delete_forgotten_keys(store.data_dir(), &id) {
                eprintln!(
                    "Its saved keys couldn't be removed from the system's credential store \
                     ({e}); RAWmakase tries again when it starts"
                );
            }
        }
        ExtensionsCommand::Enable { id } => {
            store.set_enabled(&id, true, BUNDLED_EXTENSIONS)?;
            println!("Enabled {id}; {next_start}");
        }
        ExtensionsCommand::Disable { id } => {
            store.set_enabled(&id, false, BUNDLED_EXTENSIONS)?;
            println!("Disabled {id}; {next_start}");
        }
        ExtensionsCommand::Check => {
            let checked = github::check_updates(&store, &github::Https, &app)?;
            for update in &checked.updates {
                let m = &update.offer.manifest;
                let mut line = format!(
                    "{}: {} → {}",
                    update.id, update.installed, m.extension.version
                );
                if !update.more_access.is_empty() {
                    line += &format!(" (asks for more access: {})", update.more_access.join(", "));
                }
                println!(
                    "{line}; install it with `rawmakase extensions install {}`",
                    update.offer.repository
                );
            }
            for (id, error) in &checked.failed {
                eprintln!("{id}: {error}");
            }
            if checked.updates.is_empty() && checked.failed.is_empty() {
                println!("Every extension installed from GitHub is up to date");
            }
        }
    }
    Ok(())
}

/// What an extension is and what it reaches, before it is installed.
fn describe(manifest: &rawmakase_extensions::Manifest, official: bool) {
    let e = &manifest.extension;
    println!("{} ({}) {}", e.name, e.id, e.version);
    if let Some(description) = &e.description {
        println!("  {description}");
    }
    if !e.authors.is_empty() {
        println!("  By {}", e.authors.join(", "));
    }
    if let Some(repository) = &e.repository {
        println!("  Source: {repository}");
    }
    let a = &manifest.access;
    let mut access = vec![match a.files {
        rawmakase_extensions::protocol::manifest::Files::Photos => {
            "the photos RAWmakase hands it".to_string()
        }
        rawmakase_extensions::protocol::manifest::Files::Any => "any file".to_string(),
    }];
    if a.network {
        access.push("the network".into());
    }
    access.extend(a.devices.iter().map(|d| format!("{d} devices")));
    println!("  Declares access to: {}", access.join(", "));
    let contributions = manifest.contributions();
    if !contributions.is_empty() {
        println!("  Adds: {}", contributions.join(", "));
    }
    println!("{}", trust(official));
}

/// What `extensions install` says of the package's trust: an official one
/// ([`rawmakase_extensions::github::official`]) comes from the repository
/// RAWmakase lists for it; anything else is not reviewed.
fn trust(official: bool) -> &'static str {
    if official {
        "Official: RAWmakase lists this extension, and this copy comes from the repository listed \
         for it. It runs with your permissions."
    } else {
        "Not reviewed: an extension runs with your permissions and can do anything you can."
    }
}

/// Whether to go on: `--yes`, or the user's answer when there is a terminal.
fn confirm(yes: bool) -> Result<bool> {
    use std::io::{BufRead, IsTerminal, Write};
    if yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        anyhow::bail!("Nothing installed: pass --yes to install without a terminal to ask in");
    }
    print!("Install it? [y/N] ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    let yes = matches!(answer.trim(), "y" | "Y" | "yes");
    if !yes {
        println!("Nothing installed");
    }
    Ok(yes)
}

/// [`rawmakase::catalog_command::open_catalog`], reporting on standard error.
fn open_catalog(path: impl AsRef<Path>) -> Result<rawmakase::catalog::Catalog> {
    rawmakase::catalog_command::open_catalog(path.as_ref(), &mut std::io::stderr())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rawmakase::render_command::{Render, RenderBatch};

    /// `--color-space` takes the socket's names.
    #[test]
    fn render_takes_a_color_space() {
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
                matches!(cli.command, Some(Command::Render(Render { color_space, .. })) if color_space == space),
                "{name}"
            );
        }
        assert!(matches!(
            parse(&[]).unwrap().command,
            Some(Command::Render(Render {
                color_space: OutputSpace::Srgb,
                ..
            }))
        ));
        assert!(parse(&["--color-space", "display-p3"]).is_err());
        assert!(parse(&["--color-space", "prophoto", "--tap", "scene-input"]).is_err());
    }

    /// `--quality` takes the Export dialog's range, for `render` and `render-batch`,
    /// and leaves the default (92) to the export when it isn't given.
    #[test]
    fn render_and_render_batch_take_a_quality() {
        let render = |args: &[&str]| {
            Args::try_parse_from(
                ["rawmakase", "render", "in.dng", "out.jpg"]
                    .iter()
                    .chain(args),
            )
        };
        let batch = |args: &[&str]| {
            Args::try_parse_from(
                ["rawmakase", "render-batch", "in.dng", "jobs.json"]
                    .iter()
                    .chain(args),
            )
        };
        for q in ["1", "80", "100"] {
            let expected = q.parse::<u8>().ok();
            assert!(
                matches!(render(&["--quality", q]).unwrap().command,
                    Some(Command::Render(Render { quality, .. })) if quality == expected),
                "{q}"
            );
            assert!(
                matches!(batch(&["--quality", q]).unwrap().command,
                    Some(Command::RenderBatch(RenderBatch { quality, .. })) if quality == expected),
                "{q}"
            );
        }
        assert!(matches!(
            render(&[]).unwrap().command,
            Some(Command::Render(Render { quality: None, .. }))
        ));
        assert!(matches!(
            batch(&[]).unwrap().command,
            Some(Command::RenderBatch(RenderBatch { quality: None, .. }))
        ));
        for q in ["0", "101", "high"] {
            assert!(render(&["--quality", q]).is_err(), "{q}");
            assert!(batch(&["--quality", q]).is_err(), "{q}");
        }
        // One quality flag per format: JPEG XL keeps its own, which also makes it lossy.
        assert!(render(&["--quality", "80", "--jxl-quality", "80"]).is_err());
        assert!(render(&["--quality", "80", "--tap", "scene-input"]).is_err());
        assert!(render(&["--quality", "80", "--hdr"]).is_err());
    }
}
