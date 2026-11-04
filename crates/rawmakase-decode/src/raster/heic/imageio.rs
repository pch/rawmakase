//! A HEIC's pixels decoded by macOS's ImageIO, whose HEVC decoder is the
//! system's, so RAWmakase ships none there.
//!
//! ImageIO decodes the file's primary image, grid and all. It is drawn into a
//! bitmap of the colour space the header says (its ICC profile, one built
//! from its CICP, or sRGB) at the depth the header says, so the pixels are
//! what [`HeicFile`]'s colour type and profile promise whatever colour ImageIO
//! itself makes of the file: CoreGraphics converts them when the two differ.
//! An HDR file's CICP (PQ or HLG, BT.2020) is drawn into CoreGraphics' own
//! space of that name, the one it knows the transfer of. A profile is used
//! only when moxcms, which reads it for the rest of RAWmakase, finds it an RGB
//! one, and CoreGraphics reads it as RGB too; otherwise the pixels are drawn
//! as sRGB. That is how RAWmakase takes a profile moxcms can't read; one only
//! CoreGraphics can't read (none is known) would leave the colours off, as
//! RAWmakase still converts from it. An image with alpha comes out
//! composited over black, as CoreGraphics premultiplies it; photos rarely
//! have any.
//!
//! The image ImageIO makes is not turned (RAWmakase turns it by the header's
//! `irot` and `imir`); whether it is cropped by the `clap` or not, the caller
//! crops what it was not. It all runs in an autorelease pool, so nothing the
//! frameworks autorelease outlives the call on a long-lived worker thread.
//!
//! ImageIO decodes lazily, when the image is drawn, and a decode that fails
//! then (its decoding service unreachable, say, as in a sandbox) draws
//! nothing: the photo would come out black, not as an error. So the image is
//! decoded when it is made, and drawn small onto black and onto white first:
//! an image drawn shows the same on both (where it is opaque), one not drawn
//! shows the background, and is an error. An image transparent everywhere
//! shows the background too, so one with alpha that shows nothing is taken
//! as drawn when its decoded pixels are there to read.
use super::super::heif;
use super::{HeicFile, Pixels};
use anyhow::{Result, anyhow, ensure};
use moxcms::{ColorProfile, DataColorSpace};
use objc2::rc::autoreleasepool;
use objc2_core_foundation::{
    CFBoolean, CFData, CFDictionary, CFRetained, CFString, CGPoint, CGRect, CGSize, kCFBooleanTrue,
};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGColorSpaceModel, CGContext, CGDataProvider, CGImage,
    CGImageAlphaInfo, CGImageByteOrderInfo, CGInterpolationQuality, kCGColorSpaceITUR_2100_HLG,
    kCGColorSpaceITUR_2100_PQ, kCGColorSpaceSRGB,
};
use objc2_image_io::{CGImageSource, CGImageSourceStatus, kCGImageSourceShouldCacheImmediately};

pub(super) fn pixels(mut file: HeicFile) -> Result<Pixels> {
    // Read through the file the header was read from, not opened again by
    // its path, so ImageIO decodes what the header's checks passed.
    let bytes = file.bytes()?;
    autoreleasepool(|_| decode(&file, &bytes))
}

/// The colour space the header says: CoreGraphics' own for an HDR CICP in
/// BT.2020, else the header's profile, and sRGB for none, or one that moxcms
/// or CoreGraphics can't read as RGB.
fn space(file: &HeicFile) -> Option<CFRetained<CGColorSpace>> {
    let named = |name: &CFString| CGColorSpace::with_name(Some(name));
    // SAFETY: constants CoreGraphics defines.
    let (pq, hlg, srgb) = unsafe {
        (
            kCGColorSpaceITUR_2100_PQ,
            kCGColorSpaceITUR_2100_HLG,
            kCGColorSpaceSRGB,
        )
    };
    let space = match (&file.icc, file.nclx.map(|n| (n.primaries, n.transfer))) {
        (None, Some((9, 16))) => named(pq),
        (None, Some((9, 18))) => named(hlg),
        _ => heif::profile(file.icc.as_ref(), file.nclx)
            .filter(|icc| {
                ColorProfile::new_from_slice(icc)
                    .is_ok_and(|profile| profile.color_space == DataColorSpace::Rgb)
            })
            .and_then(|icc| {
                // SAFETY: the data is a CFData, as the function takes.
                unsafe { CGColorSpace::with_icc_data(Some(&CFData::from_bytes(&icc))) }
            }),
    };
    space
        .filter(|space| CGColorSpace::model(Some(space)) == CGColorSpaceModel::RGB)
        .or_else(|| named(srgb))
}

/// Whether `image` draws: drawn 16 × 16 onto black and onto white, an opaque
/// image comes out the same on both, and one with alpha isn't the background
/// everywhere, or, transparent, has its decoded pixels. The edge pixels,
/// which smoothing may blend with the background, are not compared.
fn drawn(image: &CGImage) -> Result<bool> {
    const SIDE: usize = 16;
    // SAFETY: a constant CoreGraphics defines.
    let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))
        .ok_or_else(|| anyhow!("macOS can't make an sRGB colour space"))?;
    let draw = |background: u8| -> Result<Vec<u8>> {
        let mut rgbx = vec![background; SIDE * SIDE * 4];
        // SAFETY: `rgbx` holds SIDE rows of SIDE * 4 bytes, outlives the
        // context, and nothing else uses it while the context draws.
        let context = unsafe {
            CGBitmapContextCreate(
                rgbx.as_mut_ptr().cast(),
                SIDE,
                SIDE,
                8,
                SIDE * 4,
                Some(&space),
                CGImageAlphaInfo::NoneSkipLast.0,
            )
        }
        .ok_or_else(|| anyhow!("macOS could not decode the HEIF image"))?;
        let side = SIDE as f64;
        CGContext::draw_image(
            Some(&context),
            CGRect::new(CGPoint::new(0., 0.), CGSize::new(side, side)),
            Some(image),
        );
        drop(context);
        Ok(rgbx)
    };
    let (black, white) = (draw(0)?, draw(255)?);
    let rgb = |pixels: &[u8]| -> Vec<[u8; 3]> {
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(i, _)| {
                (1..SIDE - 1).contains(&(i % SIDE)) && (1..SIDE - 1).contains(&(i / SIDE))
            })
            .map(|(_, p)| [p[0], p[1], p[2]])
            .collect()
    };
    let (black, white) = (rgb(&black), rgb(&white));
    let alpha = CGImage::alpha_info(Some(image));
    let opaque = matches!(
        alpha,
        CGImageAlphaInfo::None | CGImageAlphaInfo::NoneSkipLast | CGImageAlphaInfo::NoneSkipFirst
    );
    Ok(if opaque {
        black == white
    } else {
        black
            .iter()
            .zip(&white)
            .any(|(b, w)| *b != [0; 3] || *w != [255; 3])
            || decoded(image)
    })
}

/// Whether `image`'s decoded pixels are there to read: all its rows, from its
/// data provider.
fn decoded(image: &CGImage) -> bool {
    let size = CGImage::bytes_per_row(Some(image)).checked_mul(CGImage::height(Some(image)));
    CGImage::data_provider(Some(image))
        .and_then(|provider| CGDataProvider::data(Some(&provider)))
        .is_some_and(|data| size.is_some_and(|size| size > 0 && data.length() as usize >= size))
}

fn decode(file: &HeicFile, bytes: &[u8]) -> Result<Pixels> {
    let failed = || anyhow!("macOS could not decode the HEIF image");
    // SAFETY: no options are passed.
    let source =
        unsafe { CGImageSource::with_data(&CFData::from_bytes(bytes), None) }.ok_or_else(failed)?;
    // SAFETY: the source is a live CGImageSource.
    let index = unsafe { source.primary_image_index() };
    // SAFETY: the index is the source's own.
    let status = unsafe { source.status_at_index(index) };
    ensure!(
        status == CGImageSourceStatus::StatusComplete,
        "macOS could not read the HEIF image"
    );
    // Decoded now, so a decode that fails can fail here.
    // SAFETY: a constant ImageIO defines, and CoreFoundation's true.
    let (key, yes) = unsafe { (kCGImageSourceShouldCacheImmediately, kCFBooleanTrue) };
    let yes: &CFBoolean = yes.ok_or_else(failed)?;
    let options = CFDictionary::<CFString, CFBoolean>::from_slices(&[key], &[yes]);
    // SAFETY: the index is the source's own; the options are a dictionary of
    // an ImageIO key to a CFBoolean, as it takes.
    let image =
        unsafe { source.image_at_index(index, Some(options.as_ref())) }.ok_or_else(failed)?;
    ensure!(drawn(&image)?, "macOS could not decode the HEIF image");
    let (width, height) = (CGImage::width(Some(&image)), CGImage::height(Some(&image)));
    ensure!(
        file.expects(u32::try_from(width)?, u32::try_from(height)?),
        "The HEIF image is not the size it says"
    );
    let space = space(file).ok_or_else(|| anyhow!("macOS can't make an sRGB colour space"))?;
    // RGB with an unused fourth sample, as CoreGraphics draws no packed RGB:
    // 8-bit, or 16-bit in the Mac's own (little-endian) order.
    let (bits, order) = if file.deep {
        (16, CGImageByteOrderInfo::Order16Little)
    } else {
        (8, CGImageByteOrderInfo::OrderDefault)
    };
    let sample = bits / 8;
    let row = width * 4 * sample;
    let mut rgbx = vec![0u8; row * height];
    // SAFETY: `rgbx` holds `height` rows of `row` bytes, outlives the
    // context, and nothing else uses it while the context draws.
    let context = unsafe {
        CGBitmapContextCreate(
            rgbx.as_mut_ptr().cast(),
            width,
            height,
            bits,
            row,
            Some(&space),
            CGImageAlphaInfo::NoneSkipLast.0 | order.0,
        )
    }
    .ok_or_else(failed)?;
    CGContext::set_interpolation_quality(Some(&context), CGInterpolationQuality::None);
    let size = CGSize::new(width as f64, height as f64);
    CGContext::draw_image(
        Some(&context),
        CGRect::new(CGPoint::new(0., 0.), size),
        Some(&image),
    );
    drop(context);
    let rgb = rgbx
        .chunks_exact(4 * sample)
        .flat_map(|pixel| &pixel[..3 * sample])
        .copied()
        .collect();
    Ok(Pixels {
        width: u32::try_from(width)?,
        height: u32::try_from(height)?,
        rgb,
    })
}
