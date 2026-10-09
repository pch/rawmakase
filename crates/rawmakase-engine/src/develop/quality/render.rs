//! The render entry points: the resident GPU render, a level of the preview, and full-resolution output.
use super::*;

/// The GPU path for everything before the per-pixel stage, with the photo (or pyramid
/// level) `source` kept on the device: the Shadows/Highlights map's reduced input
/// and the region's samples are made there, then developed and presented. `scale` is
/// `source`'s size relative to the full-resolution photo. `None` without a display or
/// GPU, or when the port does not cover the recipe; the CPU stages then run as before.
#[allow(clippy::too_many_arguments)]
fn render_resident(
    source: &Arc<CameraImage>,
    full: &Arc<CameraImage>,
    r: &Recipe,
    scale: f32,
    g: &Geometry,
    region: [u32; 4],
    spread: f32,
    finish: &develop::gpu::Finish,
    cancel: &AtomicBool,
    stages: &mut Stages,
) -> Result<Option<develop::gpu::Frame>> {
    let Some(display) = stages.display else {
        return Ok(None);
    };
    if !stages
        .backend
        .gpu
        .as_ref()
        .is_some_and(|gpu| gpu.fits_resident(source))
        || stages.backend.resident_fallback.is_some()
    {
        return Ok(None);
    }
    let mut base = r.clone();
    base.sharpening = 0.;
    base.validate()?;
    if !develop::pipeline::pixel_params::supported(&base) {
        return Ok(None);
    }
    // Masks whose ranges need developed colours, whose detail changes the samples or
    // whose finish runs on the CPU take the CPU sampling path.
    if base.masks.iter().filter(|m| m.is_active()).any(|m| {
        let a = &m.adjust;
        m.components.iter().any(|c| c.shape.is_range())
            || [a.texture, a.clarity, a.sharpness, a.noise]
                .iter()
                .any(|v| *v != 0.)
    }) {
        return Ok(None);
    }
    // The measured Texture's image is made on the CPU, its detail once per image.
    let texture = develop::texture::measured(&base);
    let untextured = (texture != 0.).then(|| source.clone());
    let source = &if texture != 0. {
        base.effects.texture = 0.;
        textured(source, texture, scale, cancel, Some(&mut *stages.cache))?
    } else {
        source.clone()
    };
    let mut toned = Toned {
        image: source.clone(),
        scale,
        reduced: None,
        untextured,
        measured: Some(measurement_copy(full, Some(&mut *stages.cache), cancel)?),
    };
    if develop::pipeline::pixel_params::needs_reduced(&base) {
        let edge = develop::local_tone::MAP_EDGE;
        let size = if source.width.max(source.height) <= edge {
            (source.width, source.height)
        } else {
            let k = edge as f32 / source.width.max(source.height) as f32;
            (
                (source.width as f32 * k).round() as u32,
                (source.height as f32 * k).round() as u32,
            )
        };
        let key = ReducedKey::new(&toned);
        let bytes = |im: &CameraImage| im.pixels.len() * 12;
        let Stages { cache, backend, .. } = stages;
        let reduced = cache.reduced.get_or_try(key, bytes, || {
            backend
                .run_resident(cancel, |gpu| {
                    gpu.scoped(|gpu| gpu.reduce_toned(source, size, cancel))
                })
                .context("GPU reduction failed")
        });
        let Ok(reduced) = reduced else {
            return Ok(None);
        };
        toned.reduced = Some(reduced);
    }
    let Some(mut params) = develop::pipeline::pixel_params::pixel_params(toned.source(), &base)
    else {
        return Ok(None);
    };
    let weights = develop::pipeline::mask_weights(
        &toned,
        &base,
        g,
        region,
        spread,
        None,
        Some(&mut *stages),
        cancel,
    )?;
    if !params.set_masks(toned.source(), &base, weights.as_deref()) {
        return Ok(None);
    }
    let key = develop::stage_cache::SampleKey::new(&toned, &base, g, region, spread);
    use develop::gpu::sampling as slot;
    let mut sampling = vec![0f32; slot::HEADER];
    let [x0, y0, w, h] = region;
    sampling[slot::WIDTH.start] = source.width as f32;
    sampling[slot::HEIGHT.start] = source.height as f32;
    sampling[slot::OUT].copy_from_slice(&[g.width as f32, g.height as f32]);
    sampling[slot::REGION].copy_from_slice(&[x0 as f32, y0 as f32, w as f32, h as f32]);
    sampling[slot::SPREAD.start] = spread;
    sampling[slot::CROP.start..slot::HOMOGRAPHY.end].copy_from_slice(&g.gpu_params());
    sampling[slot::MANUAL].copy_from_slice(&g.gpu_manual());
    let e = &base.effects;
    sampling[slot::NOISE].copy_from_slice(&[
        base.noise_luma,
        // The sampling stage's colour term: Color noise reduction runs on the camera
        // image (`color_noise`).
        0.,
        e.luma_detail,
        e.chroma_detail,
        e.luma_contrast,
        e.chroma_smoothness,
    ]);
    let mut tables = Vec::new();
    let lens = develop::pipeline::lens_gpu_params(source, &base, slot::HEADER, &mut tables);
    sampling[slot::LENS.start..slot::VIGNETTING_AMOUNT.end].copy_from_slice(&lens);
    sampling.extend(tables);
    let backend = &mut *stages.backend;
    let Some(samples) = backend.run_resident(cancel, |gpu| {
        gpu.scoped(|gpu| gpu.sample(source, sampling, (w, h), key, cancel))
    }) else {
        return Ok(None);
    };
    Ok(backend.run(cancel, |gpu| {
        gpu.present(
            develop::gpu::Input::Device(&samples),
            &params,
            r,
            finish,
            display,
            cancel,
        )
    }))
}
/// Fit and zoomed-out previews from a pyramid level (see `pyramid.rs`). Each output
/// pixel is developed once, from the level sampled over the pixel's footprint, and
/// radius-based effects are scaled to the output, so the result approximates the
/// full-resolution render resized to `size` at a fraction of the cost. `full` is the
/// full-resolution image the level was reduced from; `region` is a rectangle of the
/// `size` output.
pub(crate) fn render_level(
    level: &Arc<CameraImage>,
    full: &Arc<CameraImage>,
    r: &ValidRecipe,
    size: (u32, u32),
    region: [u32; 4],
    cancel: &AtomicBool,
    stages: &mut Stages,
) -> Result<Output> {
    check_cancel(cancel)?;
    let effective = r.resolved(&level.metadata);
    let r = effective.as_ref();
    if let Some(p) = &r.profile {
        p.ensure_camera(&level.metadata)?;
    }
    let level_scale = level.width.max(level.height) as f32 / full.width.max(full.height) as f32;
    let mut g = Geometry::new(level, r, 0);
    let footprint = g.width.max(g.height) as f32 / size.0.max(size.1) as f32;
    (g.width, g.height) = size;
    // Output pixels per full-resolution pixel.
    let scale = level_scale / footprint;
    let sigma = Sharpener::new(r).sigma * scale;
    let [x, y, w, h] = region;
    ensure!(
        w > 0
            && h > 0
            && x.checked_add(w).is_some_and(|v| v <= size.0)
            && y.checked_add(h).is_some_and(|v| v <= size.1),
        "Invalid viewport region"
    );
    let halo = if r.sharpening > 0. {
        gaussian(sigma).0 as u32
    } else {
        0
    };
    let (left, top) = (x.saturating_sub(halo), y.saturating_sub(halo));
    let right = (x + w + halo).min(size.0);
    let bottom = (y + h + halo).min(size.1);
    check_cancel(cancel)?;
    let base = [left, top, right - left, bottom - top];
    let spread = develop::pipeline::footprint_spread(footprint);
    let finish = develop::gpu::Finish {
        sigma,
        origin: [left, top],
        full: [size.0, size.1],
        scale,
        crop: [x - left, y - top, w, h],
    };
    let frame = render_resident(
        level,
        full,
        r,
        level_scale,
        &g,
        base,
        spread,
        &finish,
        cancel,
        stages,
    )?;
    if let Some(frame) = frame {
        return Ok(Output::Frame(Box::new(frame)));
    }
    let (toned, tonal_recipe) =
        local_stage(level, full, r, level_scale, cancel, Some(stages.cache))?;
    let frame = develop::pipeline::render_display(
        &toned,
        &tonal_recipe,
        r,
        &g,
        base,
        spread,
        &finish,
        cancel,
        stages,
    )?;
    if let Some(frame) = frame {
        return Ok(Output::Frame(Box::new(frame)));
    }
    let (mut out, weights) = develop::render_base(
        &toned,
        &tonal_recipe,
        &g,
        base,
        spread,
        cancel,
        Some(stages),
    )?;
    sharpen_with_radius(&mut out, r, sigma, weights.as_deref(), cancel)?;
    local_noise(&mut out, weights.as_deref());
    crate::develop::effects::spatial_finish_scaled(
        &mut out,
        r,
        [left, top],
        [size.0, size.1],
        scale,
    );
    check_cancel(cancel)?;
    Ok(Output::Pixels(crop(out, [x - left, y - top, w, h])))
}
fn crop(im: Rendered, [x, y, w, h]: [u32; 4]) -> Rendered {
    if (x, y, w, h) == (0, 0, im.width, im.height) {
        return im;
    }
    let pixels = (0..h)
        .flat_map(|row| {
            let a = ((y + row) * im.width + x) as usize;
            im.pixels[a..a + w as usize].iter().copied()
        })
        .collect();
    Rendered {
        width: w,
        height: h,
        pixels,
    }
}
pub fn render(
    im: &CameraImage,
    r: &ValidRecipe,
    max_edge: u32,
    region: Option<[u32; 4]>,
) -> Result<Rendered> {
    render_cancellable(
        im,
        r,
        max_edge,
        region,
        &std::sync::atomic::AtomicBool::new(false),
    )
}
pub fn render_cancellable(
    im: &CameraImage,
    r: &ValidRecipe,
    max_edge: u32,
    region: Option<[u32; 4]>,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Rendered> {
    render_preview(im, r, max_edge, region, cancel, None).map(Output::pixels)
}
pub(crate) fn render_preview(
    im: &CameraImage,
    r: &ValidRecipe,
    max_edge: u32,
    region: Option<[u32; 4]>,
    cancel: &std::sync::atomic::AtomicBool,
    mut stages: Option<&mut Stages>,
) -> Result<Output> {
    ensure!(
        !cancel.load(std::sync::atomic::Ordering::Relaxed),
        "Render superseded"
    );
    let effective = r.resolved(&im.metadata);
    let r = effective.as_ref();
    if let Some(p) = &r.profile {
        p.ensure_camera(&im.metadata)?;
    }
    if r.lens_ca {
        crate::lens::auto_ca::prime(im);
    }
    let source = retouched(im, r, cancel, stages.as_mut().map(|s| &mut *s.retouch))?;
    let g = Geometry::new(&source, r, 0);
    let [x, y, w, h] = region.unwrap_or([0, 0, g.width, g.height]);
    ensure!(
        w > 0
            && h > 0
            && x.checked_add(w).is_some_and(|v| v <= g.width)
            && y.checked_add(h).is_some_and(|v| v <= g.height),
        "Invalid viewport region"
    );
    let halo = if r.sharpening > 0. {
        (3. * Sharpener::new(r).sigma).ceil() as u32
    } else {
        0
    };
    let left = x.saturating_sub(halo);
    let top = y.saturating_sub(halo);
    let right = (x + w + halo).min(g.width);
    let bottom = (y + h + halo).min(g.height);
    // Highlight recovery is prepared once per decoded image by the caller; see CameraImage.
    ensure!(
        !cancel.load(std::sync::atomic::Ordering::Relaxed),
        "Render superseded"
    );
    let base = [left, top, right - left, bottom - top];
    // A display shows the render unresized; a Fit smaller than the photo comes from
    // the pyramid instead (`PreviewRenderer::render_fit`).
    let unresized =
        region.is_some() || output_size(g.width, g.height, max_edge) == (g.width, g.height);
    if unresized && let Some(stages) = stages.as_mut() {
        let finish = develop::gpu::Finish {
            sigma: Sharpener::new(r).sigma,
            origin: [left, top],
            full: [g.width, g.height],
            scale: 1.,
            crop: [x - left, y - top, w, h],
        };
        let frame = render_resident(
            &source, &source, r, 1., &g, base, 0., &finish, cancel, stages,
        )?;
        if let Some(frame) = frame {
            return Ok(Output::Frame(Box::new(frame)));
        }
    }
    let (toned, tonal_recipe) = local_stage(
        &source,
        &source,
        r,
        1.,
        cancel,
        stages.as_mut().map(|s| &mut *s.cache),
    )?;
    if unresized && let Some(stages) = stages.as_mut() {
        let finish = develop::gpu::Finish {
            sigma: Sharpener::new(r).sigma,
            origin: [left, top],
            full: [g.width, g.height],
            scale: 1.,
            crop: [x - left, y - top, w, h],
        };
        let frame = develop::pipeline::render_display(
            &toned,
            &tonal_recipe,
            r,
            &g,
            base,
            0.,
            &finish,
            cancel,
            stages,
        )?;
        if let Some(frame) = frame {
            return Ok(Output::Frame(Box::new(frame)));
        }
    }
    let (mut out, weights) = develop::render_base(
        &toned,
        &tonal_recipe,
        &g,
        base,
        0.,
        cancel,
        stages.as_deref_mut(),
    )?;
    let local_finish = weights
        .as_ref()
        .is_some_and(|w| w.uses(&[slot::SHARPNESS, slot::NOISE]));
    let spatial = r.effects.grain != 0. || r.effects.vignette != 0. || local_finish;
    let mut gpu_sharpened = false;
    let edge = if region.is_some() { 0 } else { max_edge };
    if !spatial
        && let Some(finished) = stages
            .as_mut()
            .and_then(|s| s.backend.finish(&out, r, edge, cancel))
    {
        if region.is_none() {
            return Ok(Output::Pixels(finished));
        }
        out = finished;
        gpu_sharpened = true;
    }
    ensure!(
        !cancel.load(std::sync::atomic::Ordering::Relaxed),
        "Render superseded"
    );
    if !gpu_sharpened {
        sharpen_cancellable(&mut out, r, weights.as_deref(), cancel)?;
        local_noise(&mut out, weights.as_deref());
    }
    crate::develop::effects::spatial_finish(&mut out, r, [left, top], [g.width, g.height]);
    ensure!(
        !cancel.load(std::sync::atomic::Ordering::Relaxed),
        "Render superseded"
    );
    if region.is_some() {
        Ok(Output::Pixels(crop(out, [x - left, y - top, w, h])))
    } else {
        // Spatial effects retain their CPU reference implementation. Resize can
        // still use compute after those effects, without sharpening twice.
        if spatial {
            let mut finished_recipe = r.clone();
            finished_recipe.sharpening = 0.;
            let finished = stages
                .as_mut()
                .and_then(|s| s.backend.finish(&out, &finished_recipe, max_edge, cancel));
            if let Some(finished) = finished {
                return Ok(Output::Pixels(finished));
            }
            ensure!(
                !cancel.load(std::sync::atomic::Ordering::Relaxed),
                "Render superseded"
            );
        }
        Ok(Output::Pixels(resize(out, max_edge)))
    }
}
