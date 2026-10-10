//! Reproducible, unscaled comparisons against an externally rendered sRGB reference.
use crate::develop;
use crate::model::recipe::Recipe;
use anyhow::{Result, ensure};
use std::{path::Path, sync::atomic::AtomicBool};
pub fn compare(
    input: &Path,
    reference: &Path,
    output: &Path,
    recipe: Option<&Path>,
    origin: Option<[u32; 2]>,
) -> Result<()> {
    ensure!(
        !output.exists(),
        "Comparison output directory already exists"
    );
    let raw = crate::photo::open(input)?;
    let edit = if let Some(p) = recipe {
        crate::presets::load_preset(p)?
    } else {
        Recipe::with_profiles(
            &raw.metadata,
            &crate::camera_profiles::installed(&raw.metadata).0,
        )
    };
    let image = raw.develop(
        crate::camera_data::Decode::full(Default::default()),
        &AtomicBool::new(false),
    )?;
    let render = develop::render(&image, &edit.checked()?, 0)?;
    let reference = image::open(reference)?.to_rgb32f();
    ensure!(
        (reference.width(), reference.height()) == (render.width, render.height),
        "Reference must be an uncropped, full-resolution sRGB export with matching geometry: expected {}x{}, got {}x{}",
        render.width,
        render.height,
        reference.width(),
        reference.height()
    );
    let size = 512.min(render.width).min(render.height);
    let [x, y] = origin.unwrap_or([(render.width - size) / 2, (render.height - size) / 2]);
    ensure!(
        x.checked_add(size).is_some_and(|v| v <= render.width)
            && y.checked_add(size).is_some_and(|v| v <= render.height),
        "Crop origin is outside image"
    );
    std::fs::create_dir_all(output)?;
    let own = image::RgbImage::from_raw(render.width, render.height, render.rgb8()).unwrap();
    let other = image::DynamicImage::ImageRgb32F(reference.clone()).to_rgb8();
    image::imageops::crop_imm(&own, x, y, size, size)
        .to_image()
        .save(output.join("rawmakase-100.png"))?;
    image::imageops::crop_imm(&other, x, y, size, size)
        .to_image()
        .save(output.join("reference-100.png"))?;
    let mut difference = image::RgbImage::new(size, size);
    for yy in 0..size {
        for xx in 0..size {
            let a = own.get_pixel(x + xx, y + yy);
            let b = other.get_pixel(x + xx, y + yy);
            difference.put_pixel(
                xx,
                yy,
                image::Rgb(std::array::from_fn(|c| {
                    a[c].abs_diff(b[c]).saturating_mul(4)
                })),
            );
        }
    }
    difference.save(output.join("difference-4x.png"))?;
    let n = (render.pixels.len() * 3) as f64;
    let mut absolute = 0.;
    let mut squared = 0.;
    for (a, b) in render.pixels.iter().zip(reference.pixels()) {
        for c in 0..3 {
            let d = (a[c] - b[c]) as f64;
            absolute += d.abs();
            squared += d * d;
        }
    }
    let mse = squared / n;
    let report = serde_json::json!({"camera":format!("{} {}",image.metadata.make,image.metadata.model),"profile":edit.profile.as_ref().map(|p|&p.name),"width":render.width,"height":render.height,"crop":[x,y,size,size],"mean_absolute_error_srgb":absolute/n,"rmse_srgb":mse.sqrt(),"psnr_db":if mse>0.{Some(-10.*mse.log10())}else{None},"notes":"Reference must use sRGB and matching white balance, exposure, geometry and lens corrections. Metrics measure differences, not subjective quality. No alignment or exposure matching is applied."});
    std::fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    crate::presets::save_preset(&output.join("recipe.json"), &edit)?;
    println!("Comparison written to {}", output.display());
    Ok(())
}
