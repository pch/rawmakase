//! Rendering an edited preview: the recipe resolved for the photo, quickly from a reduced
//! copy, with a finished AI Denoise result when there is one.
use super::source::{EditSource, PreviewAi};
use crate::model::recipe::Recipe;
use rawmakase_cancel::Cancel;
use rawmakase_decode::ai::cache::AiCache;
use std::path::Path;

/// A 640 px preview of `path` developed with `source`, as Develop's Fit draws it
/// (`develop::render_fitted`): from a reduced copy of the photo, several times
/// faster than rendering it whole and reducing the result.
pub(super) fn render_edited(
    path: &Path,
    source: &EditSource,
    cancel: Cancel<'_>,
) -> anyhow::Result<image::RgbImage> {
    let (image, recipe) = open_quickly(path, Some(source), cancel)?;
    rgb(crate::develop::render_fitted(&image, &recipe, 640, cancel)?)
}
/// [`develop_with_ai`] at the Library's preview size.
pub(super) fn render_edited_with_ai(
    path: &Path,
    file: &crate::storage::Identity,
    source: &EditSource,
    entry: &rawmakase_decode::ai::cache::AiEntry,
    cancel: Cancel<'_>,
) -> anyhow::Result<image::RgbImage> {
    develop_with_ai(path, file, source, entry, 640, cancel)
}
/// What a render of `source` without an AI Denoise result shows.
pub(super) fn plain_of(source: &EditSource) -> PreviewAi {
    if source.renders_ai() {
        PreviewAi::Plain
    } else {
        PreviewAi::Off
    }
}
/// The file at `path` as it is now and the finished AI Denoise result for it in
/// `ai`, when `source` renders AI Denoise and there is one.
pub(super) fn finished_entry(
    path: &Path,
    source: &EditSource,
    ai: Option<&AiCache>,
) -> Option<(
    crate::storage::Identity,
    rawmakase_decode::ai::cache::AiEntry,
)> {
    if !source.renders_ai() {
        return None;
    }
    let (file, key) = finished_ai(path, ai)?;
    Some((file, ai?.load(&key)?))
}
/// The tags a preview of `source` at `path` may be kept under, in the order it is
/// looked for: the plain render of an edit with AI Denoise only while no finished
/// result is there to render from instead. An offline photo shows whichever it has.
pub(super) fn preview_tags(path: &Path, source: &EditSource, ai: Option<&AiCache>) -> Vec<String> {
    if !source.renders_ai() {
        return vec![source.preview_tag(PreviewAi::Off)];
    }
    let mut tags = vec![source.preview_tag(PreviewAi::Finished)];
    if finished_ai(path, ai).is_none() {
        tags.push(source.preview_tag(PreviewAi::Plain));
    }
    tags
}
/// The file at `path` as it is now and the key of a finished AI Denoise result
/// for it in `cache`, if there is one.
fn finished_ai(path: &Path, ai: Option<&AiCache>) -> Option<(crate::storage::Identity, String)> {
    let file = crate::storage::Identity::read(path).ok()?;
    let key = crate::ai_denoise::finished(ai?, &file)?;
    Some((file, key))
}
/// `path` developed with `source` within `edge` pixels for a preview, and what it
/// shows of AI Denoise: an edit with it on renders from a finished result of the
/// installed model when `ai` has one for the file, decoding the photo at full size;
/// otherwise from the half-size decode, without it, as [`develop_quickly`] does.
pub(in crate::app::library) fn develop_for_preview(
    path: &Path,
    source: &EditSource,
    edge: u32,
    ai: Option<&AiCache>,
    cancel: Cancel<'_>,
) -> anyhow::Result<(image::RgbImage, PreviewAi)> {
    if let Some((file, entry)) = finished_entry(path, source, ai) {
        return Ok((
            develop_with_ai(path, &file, source, &entry, edge, cancel)?,
            PreviewAi::Finished,
        ));
    }
    Ok((
        develop_quickly(path, Some(source), edge, cancel)?,
        plain_of(source),
    ))
}
/// The RAW at `path`, the version `file` names, decoded at full size and rendered
/// with `source` and AI Denoise's finished result `entry` within `edge` pixels. A
/// file changed meanwhile fails: the result is of the version it was made for.
fn develop_with_ai(
    path: &Path,
    file: &crate::storage::Identity,
    source: &EditSource,
    entry: &rawmakase_decode::ai::cache::AiEntry,
    edge: u32,
    cancel: Cancel<'_>,
) -> anyhow::Result<image::RgbImage> {
    use rawmakase_decode::ai::denoise::{ModelOutput, assemble};
    let raw = crate::photo::open(path)?;
    let recipe = EditSource::recipe(Some(source), &raw.metadata, path, cancel)?;
    let plain = raw.develop(crate::camera_data::Decode::Full(Default::default()), cancel)?;
    let denoised = assemble(
        &ModelOutput {
            layout: entry.layout,
            rgb: &entry.rgb,
            done: None,
            clipped: &entry.clipped,
        },
        &plain,
        cancel,
    )?;
    let ai = crate::develop::AiSource::recover(denoised, cancel)?;
    let mut renderer = crate::develop::PreviewRenderer::default();
    renderer.set_ai(Some(ai));
    let rendered = renderer.render(&plain, &recipe, edge, None, cancel)?;
    anyhow::ensure!(
        crate::storage::Identity::read(path)? == *file,
        "The photo's file changed while its preview was made"
    );
    rgb(rendered)
}
/// `path` developed with `edit` (see [`EditSource::recipe`]) within `edge` pixels:
/// a RAW from the fast half-size decode, a JPEG, PNG or TIFF from the file.
pub(in crate::app::library) fn develop_quickly(
    path: &Path,
    edit: Option<&EditSource>,
    edge: u32,
    cancel: Cancel<'_>,
) -> anyhow::Result<image::RgbImage> {
    let (image, recipe) = open_quickly(path, edit, cancel)?;
    finish_quickly(&image, &recipe, edge, cancel)
}
/// `path` opened for a quick render, with the recipe `edit` gives it: a RAW from
/// the fast half-size decode, a JPEG, PNG or TIFF from the file.
pub(super) fn open_quickly(
    path: &Path,
    edit: Option<&EditSource>,
    cancel: Cancel<'_>,
) -> anyhow::Result<(crate::camera_data::CameraImage, Recipe)> {
    let opened = crate::photo::open_source(path, cancel)?;
    let recipe = EditSource::recipe(edit, opened.metadata(), path, cancel)?;
    let image = match opened {
        crate::photo::Opened::Raw(raw) => raw.develop(crate::camera_data::Decode::Half, cancel)?,
        crate::photo::Opened::Rendered(image) => image,
    };
    anyhow::ensure!(!cancel.is_cancelled(), "Cancelled");
    Ok((image, recipe))
}
pub(super) fn rgb(out: crate::rendered::Rendered) -> anyhow::Result<image::RgbImage> {
    image::RgbImage::from_raw(out.width, out.height, out.rgb8())
        .ok_or_else(|| anyhow::anyhow!("Invalid preview size"))
}
/// `image` rendered with `recipe` within `edge` pixels, given up once `cancel`
/// is set: a render that gives way to Develop or a close stops in its stages,
/// not only between them.
pub(super) fn finish_quickly(
    image: &crate::camera_data::CameraImage,
    recipe: &Recipe,
    edge: u32,
    cancel: Cancel<'_>,
) -> anyhow::Result<image::RgbImage> {
    rgb(crate::develop::render(
        image,
        &recipe.checked()?,
        edge,
        cancel,
    )?)
}
