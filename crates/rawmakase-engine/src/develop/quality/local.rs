//! The stage before the per-pixel one: the measured Texture's image, and the photo's
//! copies for the Shadows/Highlights map and for measuring it.
use super::*;

/// The measured Texture's detail of `im` at `scale`, through the stage cache when there
/// is one.
pub(crate) fn texture_detail(
    im: &Arc<CameraImage>,
    scale: f32,
    cancel: &AtomicBool,
    cache: Option<&mut StageCache>,
) -> Result<Arc<develop::texture::TextureDetail>> {
    match cache {
        Some(cache) => cache.texture_detail.get_or_try(
            TextureKey::new(im, scale, 0.),
            develop::texture::TextureDetail::bytes,
            || develop::texture::TextureDetail::of(im, scale, cancel),
        ),
        None => Ok(Arc::new(develop::texture::TextureDetail::of(
            im, scale, cancel,
        )?)),
    }
}
/// `im` with the measured Texture `amount`: its detail made once per image and kept,
/// with the result for the amount, in the stage cache when there is one.
pub(super) fn textured(
    im: &Arc<CameraImage>,
    amount: f32,
    scale: f32,
    cancel: &AtomicBool,
    cache: Option<&mut StageCache>,
) -> Result<Arc<CameraImage>> {
    let Some(cache) = cache else {
        return Ok(Arc::new(
            develop::texture::TextureDetail::of(im, scale, cancel)?.apply(im, amount),
        ));
    };
    let detail = cache.texture_detail.get_or_try(
        TextureKey::new(im, scale, 0.),
        develop::texture::TextureDetail::bytes,
        || develop::texture::TextureDetail::of(im, scale, cancel),
    )?;
    cache.textured.get_or_try(
        TextureKey::new(im, scale, amount),
        |im: &CameraImage| im.pixels.len() * 12,
        || Ok(detail.apply(im, amount)),
    )
}
/// `full` (the recovered and retouched photo at full resolution) reduced for measuring
/// it (`Toned::measured`), through the stage cache when there is one.
pub(super) fn measurement_copy(
    full: &Arc<CameraImage>,
    cache: Option<&mut StageCache>,
    cancel: &AtomicBool,
) -> Result<Arc<CameraImage>> {
    let make = || {
        check_cancel(cancel)?;
        Ok(develop::pipeline::preview_source(
            full.as_ref().into(),
            develop::local_tone::MAP_EDGE,
        ))
    };
    match cache {
        Some(cache) => cache.measured.get_or_try(
            crate::develop::stage_cache::Same(full.clone()),
            |im: &CameraImage| im.pixels.len() * 12,
            make,
        ),
        None => make().map(Arc::new),
    }
}
/// `im` (the full-resolution photo `full`, or a pyramid level of it at `scale`) with
/// the measured Texture, its copy for measuring the photo, and the recipe the pixel
/// stage renders it with.
pub(super) fn local_stage(
    im: &Arc<CameraImage>,
    full: &Arc<CameraImage>,
    r: &Recipe,
    scale: f32,
    cancel: &AtomicBool,
    cache: Option<&mut StageCache>,
) -> Result<(Toned, Recipe)> {
    // Shadows, Highlights and Clarity render in the pixel pipeline, from the photo's
    // map (local_tone.rs, clarity.rs).
    let tonal = r.clone();
    let mut cache = cache;
    // The measured Texture makes a new camera image, channel by channel (texture.rs).
    let texture = develop::texture::measured(r);
    let untextured = (texture != 0.).then(|| im.clone());
    let im = &if texture != 0. {
        textured(im, texture, scale, cancel, cache.as_deref_mut())?
    } else {
        im.clone()
    };
    let toned = Toned {
        image: im.clone(),
        scale,
        untextured,
        measured: Some(measurement_copy(full, cache, cancel)?),
    };
    Ok((toned, tonal))
}
