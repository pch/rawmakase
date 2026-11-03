use crate::catalog::Photo;
use anyhow::Result;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};
pub(super) fn available_paths(photos: &[Photo]) -> HashSet<PathBuf> {
    use rayon::prelude::*;
    let mut directories: HashMap<PathBuf, HashSet<PathBuf>> = HashMap::new();
    for photo in photos {
        if let Some(parent) = photo.path.parent() {
            directories
                .entry(parent.into())
                .or_default()
                .insert(photo.path.clone());
        }
    }
    directories
        .into_par_iter()
        .flat_map_iter(|(directory, wanted)| {
            // GVFS directory listings carry file types; avoid one remote stat per photo.
            std::fs::read_dir(directory)
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .filter_map(move |entry| {
                    let path = entry.path();
                    if wanted.contains(&path)
                        && entry
                            .file_type()
                            .is_ok_and(|t| t.is_file() || (t.is_symlink() && path.is_file()))
                    {
                        Some(path)
                    } else {
                        None
                    }
                })
        })
        .collect()
}

/// The Library's thumbnail of `path`, within 640 pixels. A RAW's comes from
/// its file's facts alone (no lens tables or profiles, which `photo::open`
/// adds for developing) and from the smallest embedded preview that reaches
/// that size, else, should that one not decode, from the largest.
pub(super) fn thumbnail(path: &std::path::Path) -> Result<image::RgbImage> {
    let image = if crate::storage::is_raw(path) {
        let mut raw = crate::raw::Raw::open_file(path)?;
        match crate::raw::thumbnail_reaching(&mut raw, 640) {
            Ok(image) => image,
            Err(_) => crate::raw::thumbnail(&mut raw)?,
        }
    } else {
        crate::raster::decode(path)?
    };
    Ok(downscale(&image, 640))
}
/// `image` within `edge`×`edge`; never enlarged.
pub(super) fn downscale(image: &image::RgbImage, edge: u32) -> image::RgbImage {
    let (width, height) = fit(image.width(), image.height(), edge);
    if (width, height) == image.dimensions() {
        return image.clone();
    }
    image::imageops::thumbnail(image, width, height)
}
/// Largest size within `edge`×`edge` that keeps the source aspect ratio.
pub(super) fn fit(width: u32, height: u32, edge: u32) -> (u32, u32) {
    let scale = (edge as f32 / width.max(height).max(1) as f32).min(1.);
    (
        ((width as f32 * scale).round() as u32).max(1),
        ((height as f32 * scale).round() as u32).max(1),
    )
}
