//! Local tone: the blurs Clarity, Texture and Dehaze are built on, and the gain they give each pixel.
use super::*;

pub(super) fn box_blur(
    src: &[f32],
    w: usize,
    h: usize,
    radius: usize,
    cancel: &AtomicBool,
) -> Result<Vec<f32>> {
    check_cancel(cancel)?;
    let mut tmp = vec![0.; src.len()];
    tmp.par_chunks_mut(w)
        .enumerate()
        .try_for_each(|(y, row)| -> Result<()> {
            check_cancel(cancel)?;
            let input = &src[y * w..(y + 1) * w];
            let mut prefix = vec![0.; w + 1];
            for x in 0..w {
                prefix[x + 1] = prefix[x] + input[x];
            }
            for (x, p) in row.iter_mut().enumerate() {
                let a = x.saturating_sub(radius);
                let b = (x + radius + 1).min(w);
                *p = (prefix[b] - prefix[a]) / (b - a) as f32;
            }
            Ok(())
        })?;
    // Independent columns preserve the reference accumulation order, while using
    // all CPU cores. Transposed output gives each task a disjoint contiguous slice.
    let mut columns = vec![0.; src.len()];
    columns
        .par_chunks_mut(h)
        .enumerate()
        .try_for_each(|(x, column)| -> Result<()> {
            check_cancel(cancel)?;
            let mut prefix = vec![0.; h + 1];
            for y in 0..h {
                prefix[y + 1] = prefix[y] + tmp[y * w + x];
            }
            for (y, p) in column.iter_mut().enumerate() {
                let a = y.saturating_sub(radius);
                let b = (y + radius + 1).min(h);
                *p = (prefix[b] - prefix[a]) / (b - a) as f32;
            }
            Ok(())
        })?;
    tmp.par_chunks_mut(w)
        .enumerate()
        .try_for_each(|(y, row)| -> Result<()> {
            check_cancel(cancel)?;
            for (x, p) in row.iter_mut().enumerate() {
                *p = columns[x * h + y];
            }
            Ok(())
        })?;
    Ok(tmp)
}
/// Log2 luminance before exposure and its box blurs. They depend only on white
/// balance, profile and lens vignetting, so Clarity, Texture and exposure edits reuse
/// them; exposure shifts every value by the same amount.
pub(crate) struct LocalBlurs {
    logs: Vec<f32>,
    fine: Vec<f32>,
    texture: Option<Vec<f32>>,
}
impl LocalBlurs {
    fn bytes(&self) -> usize {
        (self.logs.len() * 2 + self.texture.as_ref().map_or(0, Vec::len)) * 4
    }
    /// The gain local Clarity and Texture give a sample at (`x`, `y`) of the `w` × `h`
    /// image the blurs were made from: the global sliders' formula (`apply_local`) on
    /// the blurs interpolated there.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn detail_gain(
        &self,
        x: f32,
        y: f32,
        w: usize,
        h: usize,
        exposure: f32,
        clarity: f32,
        texture: f32,
    ) -> f32 {
        let fx = x.clamp(0., (w - 1) as f32);
        let fy = y.clamp(0., (h - 1) as f32);
        let (ix, iy) = (fx as usize, fy as usize);
        let (jx, jy) = ((ix + 1).min(w - 1), (iy + 1).min(h - 1));
        let (tx, ty) = (fx - ix as f32, fy - iy as f32);
        let at = |v: &[f32]| {
            (v[iy * w + ix] * (1. - tx) + v[iy * w + jx] * tx) * (1. - ty)
                + (v[jy * w + ix] * (1. - tx) + v[jy * w + jx] * tx) * ty
        };
        let raw = at(&self.logs);
        let logs = raw + exposure;
        let d = at(&self.fine) + exposure - logs;
        let fine = logs + d / (1. + d * d);
        let clarity = (logs - fine).clamp(-1., 1.) * clarity * 0.6;
        let texture = self
            .texture
            .as_ref()
            .map_or(0., |t| (raw - at(t)).clamp(-0.5, 0.5) * texture * 0.7);
        (clarity + texture).exp2()
    }
}
/// The local-tone blurs of `im`, through the stage cache when there is one.
pub(crate) fn blurs(
    im: &Arc<CameraImage>,
    r: &Recipe,
    scale: f32,
    texture: bool,
    cache: Option<&mut StageCache>,
    cancel: &AtomicBool,
) -> Result<Arc<LocalBlurs>> {
    match cache {
        Some(cache) => cache.blurs.get_or_try(
            BlurKey::new(im, r, scale, texture),
            LocalBlurs::bytes,
            || local_blurs(im, r, scale, texture, cancel),
        ),
        None => Ok(Arc::new(local_blurs(im, r, scale, texture, cancel)?)),
    }
}
/// `scale` is the image's size relative to the full-resolution photo; radii given in
/// full-resolution pixels shrink with it.
pub(super) fn local_blurs(
    im: &CameraImage,
    r: &Recipe,
    scale: f32,
    texture: bool,
    cancel: &AtomicBool,
) -> Result<LocalBlurs> {
    check_cancel(cancel)?;
    let matrix = develop::profile_matrix(&im.metadata, r);
    let vignetting = develop::pipeline::VignetteField::new(im, r);
    let logs: Vec<f32> = im
        .pixels
        .par_iter()
        .enumerate()
        .map(|(i, p)| {
            if cancel.load(Ordering::Relaxed) {
                return 0.;
            }
            let gain = vignetting.as_ref().map_or(1., |v| {
                v.gain(
                    (i % im.width as usize) as f32,
                    (i / im.width as usize) as f32,
                )
            });
            let p = p.map(|v| v * gain);
            let p = std::array::from_fn(|c| p[c] * r.wb[c]);
            let rgb = if let Some(profile) = &r.profile {
                profile.camera_color(p, matrix, r.temperature)
            } else {
                develop::mul(matrix, p)
            };
            luminance(rgb).max(1e-6).log2()
        })
        .collect();
    // Radii scale with the image (16 px on a 6000 px long edge), so previews rendered
    // from reduced images keep the same local contrast as full renders.
    let long = im.width.max(im.height) as f32;
    let radius = |px: f32| ((px / 6000. * long).round() as usize).max(1);
    let size = (im.width as usize, im.height as usize);
    let fine = box_blur(&logs, size.0, size.1, radius(16.), cancel)?;
    let texture = if texture {
        let radius = ((3. * scale).round() as usize).max(1);
        Some(box_blur(&logs, size.0, size.1, radius, cancel)?)
    } else {
        None
    };
    check_cancel(cancel)?;
    Ok(LocalBlurs {
        logs,
        fine,
        texture,
    })
}
/// Negative Clarity and Texture as a per-pixel gain of the camera image.
fn apply_local(b: &LocalBlurs, r: &Recipe, cancel: &AtomicBool) -> Result<Vec<f32>> {
    let exposure = r.exposure + r.camera_exposure;
    let mut gains = vec![0.; b.logs.len()];
    gains.par_iter_mut().enumerate().for_each(|(i, gain)| {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let logs = b.logs[i] + exposure;
        // Range guidance limits halos at strong boundaries; details stay in the residual.
        let guide = |base: f32| {
            let d = base + exposure - logs;
            logs + d / (1. + d * d)
        };
        let fine = guide(b.fine[i]);
        let clarity = (logs - fine).clamp(-1., 1.) * r.effects.clarity * 0.6;
        let texture = b.texture.as_ref().map_or(0., |t| {
            (b.logs[i] - t[i]).clamp(-0.5, 0.5) * r.effects.texture * 0.7
        });
        *gain = 2f32.powf(clarity + texture);
    });
    check_cancel(cancel)?;
    Ok(gains)
}
#[cfg(test)]
pub(super) fn local_tones(
    im: &CameraImage,
    r: &Recipe,
    scale: f32,
    cancel: &AtomicBool,
) -> Result<CameraImage> {
    let blurs = local_blurs(im, r, scale, r.effects.texture != 0., cancel)?;
    let gains = apply_local(&blurs, r, cancel)?;
    let mut out = im.clone();
    for (p, g) in out.pixels.iter_mut().zip(gains) {
        *p = p.map(|v| v * g);
    }
    Ok(out)
}
/// Negative Clarity and Texture as a gain of the camera image, plus the recipe for the
/// per-pixel stage that follows.
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
pub(super) fn local_stage(
    im: &Arc<CameraImage>,
    r: &Recipe,
    scale: f32,
    cancel: &AtomicBool,
    cache: Option<&mut StageCache>,
) -> Result<(Toned, Recipe)> {
    // Shadows and Highlights render in the pixel pipeline (local_tone.rs); this
    // pre-pass only carries negative Clarity and what Texture leaves.
    let mut spatial = r.clone();
    // The measured positive Clarity is part of the map (clarity.rs).
    if develop::clarity::measured(r) != 0. {
        spatial.effects.clarity = 0.;
    }
    let tonal = r.clone();
    let mut cache = cache;
    // The measured Texture makes a new camera image, channel by channel (texture.rs);
    // the gain below then carries the rest.
    let texture = develop::texture::measured(r);
    let untextured = (texture != 0.).then(|| im.clone());
    let im = &if texture != 0. {
        spatial.effects.texture = 0.;
        textured(im, texture, scale, cancel, cache.as_deref_mut())?
    } else {
        im.clone()
    };
    let mut toned = Toned {
        image: im.clone(),
        scale,
        gain: None,
        gain_key: None,
        reduced: None,
        untextured,
    };
    if spatial.effects.clarity != 0. || spatial.effects.texture != 0. {
        let (gain, key) = local_gain(im, &spatial, scale, cancel, cache.as_deref_mut())?;
        (toned.gain, toned.gain_key) = (Some(gain), key);
    }
    // The Shadows/Highlights map starts from a reduced copy of the toned image,
    // for the global sliders or a mask's.
    if let Some(cache) = cache
        && develop::pipeline::pixel_params::needs_reduced(r)
    {
        let key = ReducedKey::new(&toned);
        let bytes = |im: &CameraImage| im.pixels.len() * 12;
        let reduced = cache.reduced.get_or_try(key, bytes, || {
            check_cancel(cancel)?;
            Ok(develop::pipeline::preview_source(
                toned.source(),
                develop::local_tone::MAP_EDGE,
            ))
        })?;
        toned.reduced = Some(reduced);
    }
    Ok((toned, tonal))
}
/// The local-tone gain of `im`, through the stage cache when there is one, and what
/// a cached gain was computed from.
fn local_gain(
    im: &Arc<CameraImage>,
    spatial: &Recipe,
    scale: f32,
    cancel: &AtomicBool,
    cache: Option<&mut StageCache>,
) -> Result<(Arc<Vec<f32>>, Option<LocalKey>)> {
    let texture = spatial.effects.texture != 0.;
    let Some(cache) = cache else {
        let blurs = local_blurs(im, spatial, scale, texture, cancel)?;
        return Ok((Arc::new(apply_local(&blurs, spatial, cancel)?), None));
    };
    let blur_key = BlurKey::new(im, spatial, scale, texture);
    let key = LocalKey::new(blur_key.clone(), spatial);
    let StageCache { blurs, local, .. } = cache;
    let bytes = |gains: &Vec<f32>| gains.len() * 4;
    let gain = local.get_or_try(key.clone(), bytes, || {
        let blurs = blurs.get_or_try(blur_key, LocalBlurs::bytes, || {
            local_blurs(im, spatial, scale, texture, cancel)
        })?;
        apply_local(&blurs, spatial, cancel)
    })?;
    Ok((gain, Some(key)))
}
