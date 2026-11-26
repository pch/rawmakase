//! The stage before the per-pixel one: the measured Texture's image, and the photo's
//! copies for the Shadows/Highlights map and for measuring it.
use super::*;
use rawmakase_cancel::Cancel;

/// Computes [`develop::texture::TextureDetail`] using `backend`'s shared GPU pyramid
/// runner when available and within device limits, falling back to CPU otherwise.
fn compute_texture_detail(
    im: &Arc<CameraImage>,
    scale: f32,
    cancel: Cancel<'_>,
    backend: Option<&mut develop::preview_renderer::Backend>,
) -> Result<develop::texture::TextureDetail> {
    if let Some(b) = backend
        && b.gpu.as_ref().is_some_and(|gpu| gpu.fits_pyramid(im))
        && let Some(detail) = b.run_resident(cancel, |gpu| gpu.texture_detail(im, scale, cancel))
    {
        return Ok(detail);
    }
    check_cancel(cancel)?;
    develop::texture::TextureDetail::of(im, scale, cancel)
}

/// The measured Texture's detail of `im` at `scale`, through the stage cache when there
/// is one.
pub(crate) fn texture_detail(
    im: &Arc<CameraImage>,
    scale: f32,
    cancel: Cancel<'_>,
    cache: Option<&mut StageCache>,
    backend: Option<&mut develop::preview_renderer::Backend>,
) -> Result<Arc<develop::texture::TextureDetail>> {
    match cache {
        Some(cache) => cache.texture_detail.get_or_try(
            TextureKey::new(im, scale, 0.),
            develop::texture::TextureDetail::bytes,
            || compute_texture_detail(im, scale, cancel, backend),
        ),
        None => Ok(Arc::new(compute_texture_detail(
            im, scale, cancel, backend,
        )?)),
    }
}
/// `im` with the measured Texture `amount`: its detail made once per image and kept,
/// with the result for the amount, in the stage cache when there is one.
pub(crate) fn textured(
    im: &Arc<CameraImage>,
    amount: f32,
    scale: f32,
    cancel: Cancel<'_>,
    cache: Option<&mut StageCache>,
    backend: Option<&mut develop::preview_renderer::Backend>,
) -> Result<Arc<CameraImage>> {
    let Some(cache) = cache else {
        return Ok(Arc::new(
            compute_texture_detail(im, scale, cancel, backend)?.apply(im, amount),
        ));
    };
    let detail = cache.texture_detail.get_or_try(
        TextureKey::new(im, scale, 0.),
        develop::texture::TextureDetail::bytes,
        || compute_texture_detail(im, scale, cancel, backend),
    )?;
    cache.textured.get_or_try(
        TextureKey::new(im, scale, amount),
        |im: &CameraImage| im.pixels.len() * 12,
        || Ok(detail.apply(im, amount)),
    )
}
/// `full` (the photo at full resolution) reduced for measuring it, with the file's
/// own lens shading when `file_correction` says so, through the stage cache when
/// there is one (`pipeline::measurement_source`).
fn measurement_copy(
    full: &Arc<CameraImage>,
    file_correction: bool,
    cache: Option<&mut StageCache>,
    cancel: Cancel<'_>,
) -> Result<Arc<CameraImage>> {
    let make = || {
        check_cancel(cancel)?;
        Ok(develop::pipeline::measurement_source(
            full.as_ref().into(),
            develop::local_tone::MAP_EDGE,
            file_correction,
        ))
    };
    match cache {
        Some(cache) => cache.measured.get_or_try(
            crate::develop::stage_cache::MeasuredKey {
                image: crate::develop::stage_cache::Same::of(full),
                file_correction,
            },
            |im: &CameraImage| im.pixels.len() * 12,
            make,
        ),
        None => make().map(Arc::new),
    }
}
/// The photo's measurement copies: the retouched photo's (`Toned::measured`) and the
/// unretouched one's (`Toned::unretouched`), one copy when they are the same image.
/// `r` is the recipe as rendered, which says whether the file's own lens correction
/// is left out ([`Recipe::file_lens_correction_left_out`]).
pub(crate) fn measurement_copies(
    full: &Retouched,
    r: &Recipe,
    mut cache: Option<&mut StageCache>,
    cancel: Cancel<'_>,
) -> Result<(Arc<CameraImage>, Arc<CameraImage>)> {
    let file = !r.file_lens_correction_left_out(&full.image.metadata);
    let measured = measurement_copy(&full.image, file, cache.as_deref_mut(), cancel)?;
    let unretouched = if Arc::ptr_eq(&full.image, &full.unretouched) {
        measured.clone()
    } else {
        measurement_copy(&full.unretouched, file, cache, cancel)?
    };
    Ok((measured, unretouched))
}
/// `im` (the full-resolution photo `full`, or a pyramid level of it at `scale`) with
/// the measured Texture, its copy for measuring the photo, and the recipe the pixel
/// stage renders it with.
pub(super) fn local_stage(
    im: &Arc<CameraImage>,
    full: &Retouched,
    r: &Recipe,
    scale: f32,
    cancel: Cancel<'_>,
    cache: Option<&mut StageCache>,
    backend: Option<&mut develop::preview_renderer::Backend>,
) -> Result<(Toned, Recipe)> {
    // Shadows, Highlights and Clarity render in the pixel pipeline, from the photo's
    // map (local_tone.rs, clarity.rs).
    let tonal = r.clone();
    let mut cache = cache;
    let measures = cache.as_ref().map(|c| c.measures.clone());
    // The measured Texture makes a new camera image, channel by channel (texture.rs).
    let texture = develop::texture::measured(r);
    let untextured = (texture != 0.).then(|| im.clone());
    let im = &if texture != 0. {
        textured(im, texture, scale, cancel, cache.as_deref_mut(), backend)?
    } else {
        im.clone()
    };
    let (measured, unretouched) = measurement_copies(full, r, cache, cancel)?;
    let toned = Toned {
        image: im.clone(),
        scale,
        untextured,
        measured: Some(measured),
        unretouched: Some(unretouched),
        measures,
    };
    Ok((toned, tonal))
}
