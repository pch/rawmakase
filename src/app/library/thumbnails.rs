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
/// that size, else, should that one not decode, from the largest. That preview
/// is read alone, without the rest of the file, where its format allows
/// (`platform::preview_reads`); the file is read whole only when it doesn't.
pub(super) fn thumbnail(path: &std::path::Path) -> Result<image::RgbImage> {
    thumbnail_of(path, 640)
}
/// A thumbnail of the photo file at `path` within `edge` pixels, as
/// [`thumbnail`] makes one but from the smallest embedded preview reaching
/// `edge`: Import Photos' of a card's photos, which no catalog has yet.
pub(in crate::app) fn thumbnail_of(path: &std::path::Path, edge: u32) -> Result<image::RgbImage> {
    let image = if crate::photo::source(path).is_raw() {
        if crate::platform::preview_reads::in_pieces()
            && let Ok(image) = crate::raw::thumbnail_reaching_from_file(path, 640)
        {
            return Ok(downscale(&image, 640));
        }
        let mut raw = crate::raw::Raw::open_file(path)?;
        match crate::raw::thumbnail_reaching(&mut raw, edge) {
            Ok(image) => image,
            Err(_) => crate::raw::thumbnail(&mut raw)?,
        }
    } else {
        crate::raster::decode(path)?
    };
    Ok(downscale(&image, edge))
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

#[cfg(test)]
mod tests {
    use crate::platform::preview_reads::fixture;

    /// A RAW's thumbnail is its embedded preview, read without the rest of the
    /// file on a local volume too. Read whole, each RAW in a folder took its
    /// size in memory for its thumbnail: 60 RAWs put 2 GB into the allocator's
    /// hands as the Library opened.
    #[test]
    fn a_raw_s_thumbnail_is_read_without_its_sensor_data() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = fixture::dng(dir.path());
        let before = crate::raw::whole_file_reads();
        let thumbnail = super::thumbnail(&path)?;
        assert_eq!(crate::raw::whole_file_reads(), before, "read whole");
        // Within 640 pixels, the preview's shape kept.
        assert_eq!(thumbnail.dimensions(), (640, 427));
        Ok(())
    }

    /// A RAW without an embedded preview that can be read in pieces is still
    /// read whole, and fails as before when it has none at all.
    #[test]
    fn a_raw_without_a_preview_in_reach_is_read_whole() {
        let chart = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/corpus/charts/synthetic-d65.dng");
        let before = crate::raw::whole_file_reads();
        assert!(super::thumbnail(&chart).is_err());
        assert_eq!(crate::raw::whole_file_reads(), before + 1);
    }

    /// An 800×400 JPEG whose EXIF says Orientation 6 (turned a quarter
    /// clockwise), red on its left half.
    fn turned_jpeg() -> Vec<u8> {
        use image::ImageEncoder;
        let image = image::RgbImage::from_fn(800, 400, |x, _| {
            if x < 400 {
                image::Rgb([220, 0, 0])
            } else {
                image::Rgb([0, 0, 220])
            }
        });
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95)
            .write_image(&image, 800, 400, image::ExtendedColorType::Rgb8)
            .unwrap();
        // Big-endian TIFF: one IFD entry, Orientation (0x0112), SHORT, 1, 6.
        let mut exif = b"Exif\0\0MM\0\x2a\0\0\0\x08\0\x01".to_vec();
        exif.extend([0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0, 0, 0, 0, 0]);
        let mut app1 = vec![0xff, 0xe1];
        app1.extend(((exif.len() + 2) as u16).to_be_bytes());
        app1.extend(exif);
        jpeg.splice(2..2, app1);
        jpeg
    }

    /// A JPEG saved under a RAW's name, as some photo tools save an edit, has
    /// the thumbnail it has when named .jpg, upright by its EXIF. A file under
    /// a RAW's name that is neither still has none, and says why.
    #[test]
    fn a_jpeg_named_as_a_raw_has_its_jpeg_thumbnail() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let jpeg = dir.path().join("IMG_1.jpg");
        std::fs::write(&jpeg, turned_jpeg())?;
        let want = super::thumbnail(&jpeg)?;
        assert_eq!(want.dimensions(), (320, 640));
        // Turned upright: the left half's red is now on top.
        assert!(want.get_pixel(160, 100)[0] > 150);
        for name in ["IMG_1.CR2", "DSC_1.NEF", "IMG_1.DNG"] {
            let path = dir.path().join(name);
            std::fs::write(&path, turned_jpeg())?;
            assert_eq!(super::thumbnail(&path)?, want, "{name}");
            assert_eq!(super::thumbnail_of(&path, 100)?.dimensions(), (50, 100));
        }
        let garbage = dir.path().join("garbage.CR2");
        std::fs::write(&garbage, b"not a photo at all, only some text")?;
        let error = super::thumbnail(&garbage).expect_err("no thumbnail");
        assert!(format!("{error:#}").contains("not RAW"), "{error:#}");
        Ok(())
    }
}
