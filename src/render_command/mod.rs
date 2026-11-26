//! `rawmakase render` and `rawmakase render-batch` without the desktop app: a photo
//! developed on the CPU by default (or on the headless GPU compute backend with
//! `--gpu`, falling back to the CPU reference on unsupported edits or errors) and
//! written as an export, with no window, editor or session. The binary parses the
//! command line and passes in what the desktop keeps (the raw defaults chosen in
//! Preferences, AI Denoise with the user's cache, model and runtime) and where to
//! print; nothing here reads the session or reaches `app`
//! (`scripts/deps-closures.txt`).
//!
//! The two commands start from different edits and stay distinct. [`render`] layers
//! its edit sources: `--recipe`, else a legacy sidecar (not with `--catalog`), else the
//! catalog's edit or the raw defaults ([`crate::edits::resolve`], whose warnings it
//! prints); then `--profile`, the decoded photo's `--xmp` preset (with `--exposure`
//! again over it), Auto white balance and Auto. [`render_batch`] develops one photo
//! once and renders each job with its own XMP over
//! [`Recipe::with_profiles`](crate::model::recipe::Recipe::with_profiles), skipping
//! jobs whose output exists.
use crate::ai_denoise::CommandLine;
use crate::color::OutputSpace;
use crate::color::hdr_output::HdrOutput;
use crate::export_settings::{ExportOptions, Format};
use crate::model::recipe::Recipe;
use crate::raw_defaults::DevelopDefaults;
use crate::{camera_data, develop};
use anyhow::{Result, anyhow, ensure};
use rawmakase_cancel::Cancel;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// Where a command prints: its results to `out`, warnings, Auto's choices and the
/// failures of single jobs to `err` (the process's standard output and error).
pub struct Console<'a> {
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
}

/// A stage of the pipeline `render --tap` writes instead of the export.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Tap {
    SceneInput,
    SceneOutput,
}

/// `render --hdr-export`'s transfer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum HdrTransferArg {
    Pq,
    Hlg,
    Linear,
    /// A gain-map JPEG over the SDR rendition (with --hdr-gamut rec709).
    GainMap,
}

/// `render --hdr-gamut`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum HdrGamutArg {
    #[default]
    Rec2020,
    DisplayP3,
    Rec709,
}

// `rawmakase render`'s options, parsed by clap: a field's doc comment is its
// `--help` text, so the ones the help leaves out are plain comments.
#[derive(Clone, Debug, Default, clap::Args)]
pub struct Render {
    pub input: PathBuf,
    /// Save the resolved rendering recipe for reproducible comparisons.
    #[arg(long)]
    pub save_recipe: Option<PathBuf>,
    // A camera profile to render with, at 100%.
    #[arg(long)]
    pub profile: Option<PathBuf>,
    // An XMP preset applied over the edit, once the photo is decoded.
    #[arg(long)]
    pub xmp: Option<PathBuf>,
    pub output: PathBuf,
    #[arg(long)]
    pub exposure: Option<f32>,
    #[arg(long, default_value_t = 0)]
    pub max_edge: u32,
    /// The output's color space: srgb, adobe-rgb or prophoto (16-bit TIFF only).
    /// An output whose format can't hold it is refused before anything renders.
    #[arg(long, value_parser = parse_color_space, default_value = "srgb", conflicts_with = "tap")]
    pub color_space: OutputSpace,
    // Decode at half size.
    #[arg(long)]
    pub fast: bool,
    #[arg(long)]
    pub overwrite: bool,
    // A native preset to start from, before every other source.
    #[arg(long)]
    pub recipe: Option<PathBuf>,
    /// Start from the photo's edit in this RAWmakase catalog (a virtual copy's is
    /// the master's): its saved edit, else its Lightroom edit, else the raw
    /// defaults. Without it, a legacy sidecar, else the raw defaults chosen in
    /// Preferences. `--recipe` comes before both.
    #[arg(long)]
    pub catalog: Option<PathBuf>,
    /// Apply Auto tone, as the Basic panel's Auto button does; white balance is kept.
    /// Auto sets Exposure itself, so it cannot be combined with --exposure.
    #[arg(long, conflicts_with = "exposure")]
    pub auto: bool,
    /// Apply Auto white balance, as the WB menu's Auto does (before --auto).
    #[arg(long)]
    pub auto_wb: bool,
    /// Write the photo at a stage of the pipeline instead, as a 32-bit float TIFF
    /// of linear ProPhoto RGB at full size (docs/scene-tone-stage.md).
    #[arg(long, value_enum)]
    pub tap: Option<Tap>,
    /// Write the edit's HDR master (the edit needs HDR on) as a 32-bit float TIFF
    /// of linear ProPhoto RGB, 1 = graphics white, for comparisons with Camera Raw.
    #[arg(long, conflicts_with_all = ["tap", "color_space"])]
    pub hdr: bool,
    /// Export the edit's HDR master in this encoding (pq, hlg or linear: TIFF or
    /// JPEG XL; gain-map: JPEG), as the Export dialog's HDR export writes it (the
    /// edit needs HDR on).
    #[arg(long, value_enum, conflicts_with_all = ["tap", "color_space", "hdr"])]
    pub hdr_export: Option<HdrTransferArg>,
    /// The HDR export's gamut: rec2020, display-p3 or rec709.
    #[arg(long, value_enum, default_value = "rec2020", requires = "hdr_export")]
    pub hdr_gamut: HdrGamutArg,
    /// Write a .jxl output lossy at this quality (1–100, as the Export dialog's;
    /// 90 is libjxl's distance 1) instead of lossless.
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=100), conflicts_with_all = ["tap", "hdr"])]
    pub jxl_quality: Option<u8>,
    /// A .jpg output's quality (1–100, as the Export dialog's), a gain-map
    /// --hdr-export's included; 92 by default. A .jxl output takes --jxl-quality.
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=100), conflicts_with_all = ["tap", "hdr", "jxl_quality"])]
    pub quality: Option<u8>,
    /// Use the headless GPU compute backend when available, falling back to the CPU
    /// reference on unsupported edits or GPU errors. Off by default for deterministic
    /// CPU reference output.
    #[arg(long, conflicts_with = "tap")]
    pub gpu: bool,
}

impl Render {
    /// The HDR export `--hdr-export` and `--hdr-gamut` ask for.
    fn hdr_output(&self) -> Option<HdrOutput> {
        use crate::color::hdr_output::{HdrGamut, HdrTransfer};
        self.hdr_export.map(|transfer| HdrOutput {
            transfer: match transfer {
                HdrTransferArg::Pq => HdrTransfer::Pq,
                HdrTransferArg::Hlg => HdrTransfer::Hlg,
                HdrTransferArg::Linear => HdrTransfer::Linear,
                HdrTransferArg::GainMap => HdrTransfer::default(),
            },
            gamut: match self.hdr_gamut {
                HdrGamutArg::Rec2020 => HdrGamut::Rec2020,
                HdrGamutArg::DisplayP3 => HdrGamut::DisplayP3,
                HdrGamutArg::Rec709 => HdrGamut::Rec709,
            },
            gain_map: matches!(transfer, HdrTransferArg::GainMap),
        })
    }
}

// `rawmakase render-batch`'s options, parsed by clap.
#[derive(Clone, Debug, clap::Args)]
pub struct RenderBatch {
    pub input: PathBuf,
    // A JSON list of [`Job`]s.
    pub jobs: PathBuf,
    /// The color space of every job without its own: srgb, adobe-rgb or prophoto
    /// (16-bit TIFF only).
    #[arg(long, value_parser = parse_color_space, default_value = "srgb")]
    pub color_space: OutputSpace,
    /// The quality (1–100, as the Export dialog's) of the jobs written as JPEG;
    /// 92 by default. TIFF, PNG and JPEG XL outputs are lossless.
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=100))]
    pub quality: Option<u8>,
    /// Use the headless GPU compute backend when available, falling back to the CPU
    /// reference on unsupported edits or GPU errors. Off by default for deterministic
    /// CPU reference output.
    #[arg(long)]
    pub gpu: bool,
}

/// One job of a `render-batch` list.
#[derive(serde::Deserialize)]
struct Job {
    xmp: PathBuf,
    output: PathBuf,
    #[serde(default)]
    max_edge: u32,
    #[serde(default)]
    color_space: Option<String>,
}

/// A color space by the names `--color-space` and the automation socket use.
pub fn parse_color_space(name: &str) -> Result<OutputSpace, String> {
    OutputSpace::from_name(name)
        .ok_or_else(|| format!("unknown color space {name:?}; use srgb, adobe-rgb or prophoto"))
}

/// Refuses an `output` whose format can't hold `space`, before anything renders.
pub fn check_output(output: &Path, space: OutputSpace) -> Result<()> {
    Format::from_path(output)
        .ok_or_else(|| anyhow!("Export extension must be {}", Format::extensions_listed()))?
        .supports(space)
}

/// The camera's EXIF for an export of `input`, or a stand-in made from `metadata`.
fn camera(input: &Path, metadata: &camera_data::Metadata) -> crate::exif::CameraExif {
    crate::photo::camera_exif(input)
        .unwrap_or_else(|| crate::export::job::stand_in(input, metadata))
}

fn replace(overwrite: bool) -> crate::export::Replace {
    if overwrite {
        crate::export::Replace::Overwrite
    } else {
        crate::export::Replace::NoClobber
    }
}

/// `rawmakase render`: develops `o.input` with the edit its options choose and writes
/// it to `o.output`, or the stage or HDR master they ask for. An existing output is
/// refused without `o.overwrite`.
///
/// What the desktop keeps is asked for only when it is needed, as the command always
/// has: `raw_defaults` once a photo without an edit resolves its starting settings
/// (never with `--recipe` or a sidecar, nor for an output refused before that; reading
/// them can read presets, and a preset on a stalled share must not hold up a render
/// that doesn't use it), and `ai_denoise` once the photo is developed, dropped as soon
/// as its AI Denoise source is made, so its worker and model are gone before the render.
pub fn render(
    o: &Render,
    raw_defaults: &dyn Fn() -> DevelopDefaults,
    ai_denoise: &dyn Fn() -> CommandLine,
    cancel: Cancel<'_>,
    console: &mut Console<'_>,
) -> Result<()> {
    let input = &o.input;
    let output = &o.output;
    ensure!(
        o.jxl_quality.is_none() || Format::from_path(output) == Some(Format::Jxl),
        "--jxl-quality is for a .jxl output"
    );
    ensure!(
        o.quality.is_none() || Format::from_path(output) == Some(Format::Jpeg),
        "--quality is for a .jpg or .jpeg output; a .jxl output takes --jxl-quality"
    );
    let export_options = ExportOptions {
        max_edge: o.max_edge,
        quality: o
            .jxl_quality
            .or(o.quality)
            .unwrap_or(ExportOptions::default().quality),
        jxl_lossy: o.jxl_quality.is_some(),
        ..Default::default()
    };
    if let Some(encoding) = o.hdr_output() {
        let format = Format::from_path(output).ok_or_else(|| {
            anyhow!(
                "An HDR export is a TIFF, JPEG XL or gain-map JPEG: name the output .tif, .tiff, .jxl or .jpg"
            )
        })?;
        Format::check_hdr(format, encoding)?;
    }
    if o.tap.is_none() && !o.hdr && o.hdr_export.is_none() {
        check_output(output, o.color_space)?;
    }
    let t = Instant::now();
    // Read before decoding: AI Denoise's result must be of the same file.
    let file = crate::storage::Identity::read(input)?;
    let r = crate::photo::open(input)?;
    let mut edit = if let Some(p) = &o.recipe {
        crate::presets::load_preset(p)?.recipe
    } else if let Some(sidecar) = o
        .catalog
        .is_none()
        .then(|| crate::catalog::legacy_sidecar::load(input))
        .transpose()?
        .flatten()
    {
        sidecar.recipe
    } else {
        let record = match &o.catalog {
            Some(catalog) => catalog_edit(catalog, input, console.err)?,
            None => crate::edits::EditRecord::default(),
        };
        // As the app opens it: a photo without color data starts from its fitted
        // camera look.
        let profiles = crate::camera_look::with_default(
            input,
            &r.metadata,
            crate::camera_profiles::installed(&r.metadata).0,
            cancel,
        );
        let resolved =
            crate::edits::resolve(&record, input, &r.metadata, &profiles, &raw_defaults())?;
        for warning in &resolved.warnings {
            writeln!(console.err, "{warning}")?;
        }
        resolved.recipe
    };
    if let Some(e) = o.exposure {
        edit.exposure = e;
    }
    if let Some(p) = &o.profile {
        edit.profile = Some(crate::camera_profiles::load(p, &r.metadata)?);
        // A chosen profile starts at 100%, as in the app.
        edit.profile_amount = 1.;
        edit.use_camera_baseline(&r.metadata);
        edit.sync_white_balance_controls(&r.metadata);
    }
    edit.validate()?;
    let decode = if o.fast {
        camera_data::Decode::Half
    } else {
        camera_data::Decode::full(Default::default())
    };
    let im = r.develop(decode, cancel)?;
    if let Some(path) = &o.xmp {
        let preset = crate::xmp::parse(path, &std::fs::read_to_string(path)?)?;
        let (profiles, _) = crate::camera_profiles::installed(&im.metadata);
        edit = preset.apply(
            &edit,
            &im.metadata,
            &profiles,
            Some(&develop::Measures(&im)),
        )?;
        if let Some(e) = o.exposure {
            edit.exposure = e;
        }
    }
    if o.auto_wb {
        let t = Instant::now();
        edit = develop::auto_white_balance(&im, &edit, cancel)?;
        writeln!(
            console.err,
            "Auto white balance ({:?}): temperature {:.0} tint {:+.0}",
            t.elapsed(),
            edit.temperature,
            edit.tint
        )?;
    }
    if o.auto {
        let t = Instant::now();
        edit = develop::auto_tone(&im, &edit, cancel)?;
        writeln!(
            console.err,
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
        )?;
    }
    if let Some(path) = &o.save_recipe {
        ensure!(!path.exists(), "Recipe output already exists");
        crate::presets::save_preset(path, &edit, &im.metadata)?;
    }
    if let Some(tap) = o.tap {
        // A stage tap renders without AI Denoise: refused rather than written as if
        // the recipe had none.
        ensure!(
            !edit.as_rendered().ai_denoise.renders(),
            "--tap does not render AI Denoise; switch it off in the recipe"
        );
        let stage = match tap {
            Tap::SceneInput => develop::quality::Stage::SceneInput,
            Tap::SceneOutput => develop::quality::Stage::SceneOutput,
        };
        let out = develop::quality::render_stage(&im, &edit, stage, cancel)?;
        return save_stage(output, input, out, o.overwrite);
    }
    let developed = t.elapsed();
    let im = Arc::new(im);
    let ai = ai_denoise().source(input, &file, &im, &edit, cancel)?;
    let backend = if o.gpu {
        develop::ExportBackend::auto()
    } else {
        develop::ExportBackend::cpu_only()
    };
    if let Some(encoding) = o.hdr_output() {
        let master = develop::render_hdr(&im, &edit.checked()?, ai.as_ref(), o.max_edge, cancel)?;
        let pixels = if encoding.gain_map {
            let base = backend.render_export(
                &im,
                &edit.checked()?,
                ai.as_ref(),
                o.max_edge,
                OutputSpace::Srgb,
                develop::ExportFormat::F32,
                cancel,
            )?;
            crate::export::Pixels::GainMap(crate::export::GainMapPixels::new(base, &master)?)
        } else {
            crate::export::Pixels::Hdr(crate::export::HdrPixels::encode(&master, encoding))
        };
        crate::export::export_pixels(
            output,
            input,
            &pixels,
            &im.metadata,
            &export_options,
            &crate::export::Embed::default(),
            replace(o.overwrite),
        )?;
        writeln!(
            console.out,
            "{}x{} HDR export, peak {} | total {:?}",
            master.width,
            master.height,
            master.peak,
            t.elapsed()
        )?;
        return Ok(());
    }
    if o.hdr {
        let out = develop::render_hdr(&im, &edit.checked()?, ai.as_ref(), o.max_edge, cancel)?;
        let rendered = crate::rendered::Rendered {
            width: out.width,
            height: out.height,
            pixels: out.pixels,
        };
        save_stage(output, input, rendered, o.overwrite)?;
        writeln!(
            console.out,
            "{}x{} HDR master, peak {} | total {:?}",
            out.width,
            out.height,
            out.peak,
            t.elapsed()
        )?;
        return Ok(());
    }
    let target_format = Format::from_path(output).unwrap_or(Format::Jpeg);
    let pixel_format = crate::export::job::pixel_format_for(target_format, false);
    let out = backend.render_export(
        &im,
        &edit.checked()?,
        ai.as_ref(),
        o.max_edge,
        o.color_space,
        pixel_format,
        cancel,
    )?;
    let rendered = t.elapsed() - developed;
    crate::export::export_image(
        output,
        input,
        &out,
        &im.metadata,
        &export_options,
        &crate::export::Embed {
            camera: Some(camera(input, &im.metadata)),
            ..Default::default()
        },
        replace(o.overwrite),
    )?;
    let max = im.pixels.iter().flatten().copied().fold(0f32, f32::max);
    writeln!(
        console.out,
        "{}x{} | develop {:?} | render {:?} | total {:?} | camera max {:.4} | scale {:.6} | integer clipped {}",
        out.width(),
        out.height(),
        developed,
        rendered,
        t.elapsed(),
        max,
        im.scale_factor,
        im.scale_clipped
    )?;
    Ok(())
}

/// `rawmakase render-batch`: develops `o.input` once and renders each job of
/// `o.jobs` with its own XMP over the photo's starting settings, at its own size and
/// color space. A job whose output exists is skipped; one that fails is reported on
/// `console.err` and the others still render, so only a list that can't be read, or a
/// photo that can't be developed, fails the command. `ai_denoise` is made once the
/// photo is developed, and serves every job, so its model loads once.
pub fn render_batch(
    o: &RenderBatch,
    ai_denoise: &dyn Fn() -> CommandLine,
    cancel: Cancel<'_>,
    console: &mut Console<'_>,
) -> Result<()> {
    let input = &o.input;
    let jobs: Vec<Job> = serde_json::from_str(&std::fs::read_to_string(&o.jobs)?)?;
    // Read before decoding: AI Denoise's result must be of the same file.
    let file = crate::storage::Identity::read(input)?;
    let photo = crate::photo::open(input)?;
    let im = Arc::new(photo.develop(camera_data::Decode::full(Default::default()), cancel)?);
    let (profiles, _) = crate::camera_profiles::installed(&im.metadata);
    let base = Recipe::with_profiles(&im.metadata, &profiles);
    let camera = Some(camera(input, &im.metadata));
    // One AI Denoise service and one export backend for every job: the model and GPU
    // pipelines initialize once.
    let ai_denoise = ai_denoise();
    let backend = if o.gpu {
        develop::ExportBackend::auto()
    } else {
        develop::ExportBackend::cpu_only()
    };
    for job in jobs.iter().filter(|j| !j.output.exists()) {
        let rendered = (|| -> Result<()> {
            let space = match &job.color_space {
                Some(name) => parse_color_space(name).map_err(anyhow::Error::msg)?,
                None => o.color_space,
            };
            check_output(&job.output, space)?;
            let preset = crate::xmp::parse(&job.xmp, &std::fs::read_to_string(&job.xmp)?)?;
            let edit = preset.apply(
                &base,
                &im.metadata,
                &profiles,
                Some(&develop::Measures(&im)),
            )?;
            let ai = ai_denoise.source(input, &file, &im, &edit, cancel)?;
            let target_format = Format::from_path(&job.output).unwrap_or(Format::Jpeg);
            let pixel_format = crate::export::job::pixel_format_for(target_format, false);
            let out = backend.render_export(
                &im,
                &edit.checked()?,
                ai.as_ref(),
                job.max_edge,
                space,
                pixel_format,
                cancel,
            )?;
            crate::export::export_image(
                &job.output,
                input,
                &out,
                &im.metadata,
                &ExportOptions {
                    max_edge: job.max_edge,
                    quality: o.quality.unwrap_or(ExportOptions::default().quality),
                    ..Default::default()
                },
                &crate::export::Embed {
                    camera: camera.clone(),
                    ..Default::default()
                },
                crate::export::Replace::NoClobber,
            )?;
            Ok(())
        })();
        if let Err(e) = rendered {
            writeln!(console.err, "{}: {e:#}", job.output.display())?;
        }
    }
    Ok(())
}

/// The stored edit of the photo at `input` in `catalog`: of the master, as a path
/// names it, not of a virtual copy. A photo the catalog doesn't have, or has more
/// than once (two folders found at the same place), is an error. Opening the catalog
/// prepares it as the app does, and its selection masks' rasters are read back from it
/// from then on, for this process. Opening it reports to `err`
/// ([`open_catalog`](crate::catalog_command::open_catalog)).
fn catalog_edit(
    catalog: &Path,
    input: &Path,
    err: &mut dyn Write,
) -> Result<crate::edits::EditRecord> {
    let c = crate::catalog_command::open_catalog(catalog, err)?;
    let photo = crate::catalog_command::photo_at(&c, input)?;
    crate::catalog_command::photo_edit(&c, photo)
}

/// Writes a stage tap as a 32-bit float TIFF at `output`, with an export's
/// protections: never over a RAW file or the `input` it was rendered from, never over
/// an existing file unless `overwrite`, and encoded beside it before being put in
/// place, so a failed write leaves the destination as it was.
fn save_stage(
    output: &Path,
    input: &Path,
    out: crate::rendered::Rendered,
    overwrite: bool,
) -> Result<()> {
    use crate::storage::{RasterFormat, Replace, is_raw, write_atomic};
    ensure!(!is_raw(output), "A stage tap cannot overwrite a RAW file");
    ensure!(
        RasterFormat::of(output) == Some(RasterFormat::Tiff),
        "A stage tap is a TIFF: name the output .tif or .tiff"
    );
    if output.exists() {
        ensure!(
            std::fs::canonicalize(output)? != std::fs::canonicalize(input)?,
            "Cannot overwrite the source"
        );
        ensure!(overwrite, "Output already exists");
    }
    let image = image::Rgb32FImage::from_raw(
        out.width,
        out.height,
        out.pixels.into_iter().flatten().collect(),
    )
    .ok_or_else(|| anyhow!("Stage image has the wrong size"))?;
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
mod tests;
