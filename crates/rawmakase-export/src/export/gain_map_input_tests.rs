//! Gain-map JPEG and JPEG XL files opened for Develop (docs/hdr.md#gain-map-files).
//! RAWmakase's own gain-map exports open as HDR edits. Their light is the master they
//! were made from, within the codecs' quantisation, and their SDR rendition is their
//! base. Damaged maps are refused with the reason, and the Library still shows the base.
use super::gain_map::{Metadata, assemble_iso};
use super::jxl;
use super::srgb_fixtures::{photo, synthetic};
use crate::{
    camera_data::CameraImage,
    camera_profiles::CameraProfile,
    color::hdr_output::{HdrGamut, HdrOutput},
    export::{Replace, job},
    export_settings::{ExportSettings, Format, Include},
    model::recipe::Recipe,
};
use anyhow::Result;
use rawmakase_cancel::Cancel;
use std::path::{Path, PathBuf};

/// Rec. 709 luminance, which RAWmakase's single-channel maps encode.
const LUMINANCE: [f32; 3] = [0.2126, 0.7152, 0.0722];

fn hdr_recipe(image: &CameraImage) -> Recipe {
    let mut r = Recipe {
        profile: CameraProfile::camera_matrix_default(&image.metadata).map(std::sync::Arc::new),
        sharpening: 0.,
        ..Default::default()
    };
    r.hdr.enabled = true;
    r.hdr.limit = 4.;
    r.exposure = 3.;
    r
}

/// The photo exported as a full-size `format` file: with a gain map over the SDR
/// rendition when `gain_map`, else the SDR rendition alone.
fn export(photo: &job::Photo, format: Format, gain_map: bool, dir: &Path) -> Result<PathBuf> {
    let mut s = ExportSettings {
        include: Include::AllExceptCameraRaw,
        ..Default::default()
    };
    s.choose_format(format);
    s.quality = 100;
    s.hdr = gain_map.then_some(HdrOutput {
        gamut: HdrGamut::Rec709,
        gain_map: true,
        ..Default::default()
    });
    let extension = if format == Format::Jxl { "jxl" } else { "jpg" };
    let name = if gain_map { "gain-map" } else { "sdr" };
    let target = dir.join(format!("{name}.{extension}"));
    job::run(
        photo.clone(),
        &s,
        &target,
        Replace::NoClobber,
        Cancel::NEVER,
        |_| {},
    )?;
    Ok(target)
}

/// The file at `path` as Develop opens it.
fn opened(path: &Path) -> Result<CameraImage> {
    match crate::photo::open_source(path, Cancel::NEVER)? {
        crate::photo::Opened::Rendered(image) => Ok(image),
        crate::photo::Opened::Raw(_) => anyhow::bail!("a gain-map file opens as rendered"),
    }
}

/// The Rec. 709 luminance of linear ProPhoto `p`, negative channels at 0 as the
/// gain-map export takes them.
fn luminance(p: [f32; 3]) -> f32 {
    let rgb = crate::color::mul(HdrGamut::Rec709.prophoto_to_rgb(), p);
    (0..3).map(|c| LUMINANCE[c] * rgb[c].max(0.)).sum()
}

/// A gain-map export of the synthetic ramps opens as an HDR edit:
/// - its light's luminance is the master's, within `median` and `worst` stops;
/// - it starts with HDR on, and records that its light came from a gain map;
/// - its SDR rendition is its base, the plain SDR export's pixels as Develop opens
///   them.
///
/// RAWmakase's maps are single-channel, so they bring back the master's luminance.
/// The colour is the base's.
fn round_trip(format: Format, median: f32, worst: f32) -> Result<()> {
    let dir = tempfile::tempdir()?;
    let image = synthetic(96, 48);
    let recipe = hdr_recipe(&image);
    let master = crate::develop::render_hdr(&image, &recipe.checked()?, None, 0, Cancel::NEVER)?;
    let photo = photo(image, &dir.path().join("in.dng"), dir.path(), recipe);
    let reopened = opened(&export(&photo, format, true, dir.path())?)?;
    let light = reopened
        .hdr()
        .expect("a gain-map file opens with its light");
    assert_eq!(
        (light.width(), light.height()),
        (master.width, master.height)
    );
    let source = reopened.metadata.hdr_source.expect("an HDR file's facts");
    assert!(source.gain_map, "the light is recorded as a gain map's");
    assert!(
        Recipe::for_new_rendered(&reopened.metadata).hdr.enabled,
        "a gain-map file starts as an HDR edit"
    );
    let sdr = opened(&export(&photo, format, false, dir.path())?)?;
    assert_eq!(reopened.pixels, sdr.pixels, "the SDR rendition is the base");
    // Stops between the luminance of the light and of the master, with the offset
    // the map was computed with.
    let k = 1. / 64.;
    let mut errors: Vec<f32> = light
        .pixels()
        .iter()
        .zip(&master.pixels)
        .map(|(l, m)| ((luminance(*l) + k) / (luminance(*m) + k)).log2().abs())
        .collect();
    errors.sort_by(f32::total_cmp);
    let (p50, max) = (errors[errors.len() / 2], errors[errors.len() - 1]);
    assert!(p50 <= median && max <= worst, "median {p50}, worst {max}");
    // The light reaches above white, as the master does.
    let peak = light
        .pixels()
        .iter()
        .map(|p| luminance(*p))
        .fold(0., f32::max);
    assert!(peak > 4., "{peak}");
    Ok(())
}

// The map's 8-bit steps set the bounds: its gains span about 4.2 stops here, so a
// step is 0.017 stop, and rounding to one leaves a median near a quarter of it. The
// lossless JPEG XL stays within a step; JPEG's compression of the base and the map
// adds about as much again (measured: median 0.0054 in both, worst 0.015 and 0.031).
#[test]
fn a_gain_map_jpeg_opens_as_an_hdr_edit_of_its_master() -> Result<()> {
    round_trip(Format::Jpeg, 0.008, 0.05)
}

#[test]
fn a_gain_map_jpeg_xl_opens_as_an_hdr_edit_of_its_master() -> Result<()> {
    round_trip(Format::Jxl, 0.008, 0.02)
}

/// A base JPEG of `width` × `height`: horizontal and vertical ramps, so a turned or
/// mirrored image doesn't match itself.
fn base(width: u32, height: u32) -> image::RgbImage {
    image::RgbImage::from_fn(width, height, |x, y| {
        let r = (40 + x * 200 / width) as u8;
        let g = (30 + y * 200 / height) as u8;
        image::Rgb([r, g, ((x + y) * 6 % 250) as u8])
    })
}

/// A grey map of `width` × `height`, brightest at the top right.
fn map(width: u32, height: u32) -> image::GrayImage {
    image::GrayImage::from_fn(width, height, |x, y| {
        image::Luma([(x * 127 / width + (height - 1 - y) * 127 / height) as u8])
    })
}

fn jpeg(image: impl Into<image::DynamicImage>) -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 100)
        .encode_image(&image.into())
        .unwrap();
    out
}

/// The metadata the malformed files start from: gains from 0 to 2 stops, an
/// alternate headroom of 2.
fn metadata() -> Vec<u8> {
    Metadata {
        min: 0.,
        max: 2.,
        base_headroom: 0.,
        alternate_headroom: 2.,
    }
    .iso_21496()
}

/// `jpeg` with an EXIF APP1 segment, first after its SOI, saying `orientation`.
fn with_orientation(jpeg: &[u8], orientation: u16) -> Vec<u8> {
    let mut exif = b"Exif\0\0II*\0".to_vec();
    exif.extend(8u32.to_le_bytes());
    exif.extend(1u16.to_le_bytes());
    exif.extend(0x0112u16.to_le_bytes());
    exif.extend(3u16.to_le_bytes());
    exif.extend(1u32.to_le_bytes());
    exif.extend(u32::from(orientation).to_le_bytes());
    exif.extend(0u32.to_le_bytes());
    let mut out = jpeg[..2].to_vec();
    out.extend([0xff, 0xe1]);
    out.extend(((exif.len() + 2) as u16).to_be_bytes());
    out.extend(exif);
    out.extend_from_slice(&jpeg[2..]);
    out
}

/// Writes `bytes` to `name` in `dir`.
fn file(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

/// Big-endian u32 `v` at `at` in `bytes`.
fn put(bytes: &mut [u8], at: usize, v: u32) {
    bytes[at..at + 4].copy_from_slice(&v.to_be_bytes());
}

/// A malformed gain map is refused for Develop with the reason, before anything is
/// decoded (`raster::probe`) and when the file is opened; the Library still shows
/// the base. The same file with its metadata intact opens as an HDR edit, so each
/// refusal is the damage's.
#[test]
fn a_malformed_gain_map_jpeg_is_refused_and_shows_its_base() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let primary = jpeg(base(32, 16));
    let good_map = jpeg(map(16, 8));
    let good = assemble_iso(&primary, &good_map, &metadata())?;
    let intact = file(dir.path(), "intact.jpg", &good);
    rawmakase_decode::raster::probe(&intact).expect("an intact gain map opens");
    assert!(opened(&intact)?.hdr().is_some());
    let shown = rawmakase_decode::raster::decode(&file(dir.path(), "base.jpg", &primary))?;
    // Without the version marker on its primary image, the map isn't an ISO 21496-1
    // one RAWmakase reads (Adobe's and Android's earlier XMP form): the file opens as
    // its SDR base, as it did before gain maps were read.
    let mut unmarked = good.clone();
    let urn = b"urn:iso:std:iso:ts:21496:-1\0";
    let first = unmarked.windows(urn.len()).position(|w| w == urn).unwrap();
    unmarked[first] = b'x';
    let unmarked = file(dir.path(), "unmarked.jpg", &unmarked);
    rawmakase_decode::raster::probe(&unmarked).expect("an SDR JPEG opens");
    let sdr = opened(&unmarked)?;
    assert!(sdr.hdr().is_none() && sdr.metadata.hdr_source.is_none());

    let iso = |change: fn(&mut Vec<u8>)| -> Vec<u8> {
        let mut m = metadata();
        change(&mut m);
        assemble_iso(&primary, &good_map, &m).unwrap()
    };
    let mpf = good.windows(4).position(|w| w == b"MPF\0").unwrap() + 4;
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        ("a newer version", iso(|m| m[1] = 1), "version 1 gain-map"),
        (
            "truncated metadata",
            iso(|m| m.truncate(40)),
            "damaged gain-map",
        ),
        (
            "a zero denominator",
            iso(|m| put(m, 25, 0)),
            "damaged gain-map",
        ),
        (
            "a maximum below the minimum",
            iso(|m| put(m, 21, 3 << 16)),
            "damaged gain-map",
        ),
        ("a gamma of 0", iso(|m| put(m, 37, 0)), "damaged gain-map"),
        (
            "a headroom of 40 stops",
            iso(|m| put(m, 13, 40 << 16)),
            "damaged gain-map",
        ),
        (
            "a negative offset",
            iso(|m| put(m, 45, (-1i32) as u32)),
            "damaged gain-map",
        ),
        (
            "an HDR base by its flag",
            iso(|m| m[4] |= 0x04),
            "HDR-base gain-map",
        ),
        (
            "an HDR base by its headroom",
            iso(|m| put(m, 5, 2 << 16)),
            "HDR-base gain-map",
        ),
        (
            "a map of another shape",
            assemble_iso(&primary, &jpeg(map(16, 16)), &metadata())?,
            "damaged gain-map",
        ),
        (
            "a map larger than its base",
            assemble_iso(&primary, &jpeg(map(64, 32)), &metadata())?,
            "damaged gain-map",
        ),
        (
            "a file cut off in its map",
            good[..good.len() - 40].to_vec(),
            "damaged gain-map",
        ),
        (
            "an MPF entry past the file's end",
            {
                let mut b = good.clone();
                put(&mut b, mpf + 50 + 24, 1 << 30);
                b
            },
            "damaged gain-map",
        ),
        (
            "a map without its metadata",
            {
                let mut b = good.clone();
                let urn = b"urn:iso:std:iso:ts:21496:-1\0";
                let first = b.windows(urn.len()).position(|w| w == urn).unwrap();
                let second = first
                    + 1
                    + b[first + 1..]
                        .windows(urn.len())
                        .position(|w| w == urn)
                        .unwrap();
                b[second] = b'x';
                b
            },
            "damaged gain-map",
        ),
    ];
    for (what, bytes, reason) in cases {
        let path = file(dir.path(), "damaged.jpg", &bytes);
        let refused = rawmakase_decode::raster::probe(&path)
            .err()
            .unwrap_or_else(|| panic!("{what}: probed as editable"));
        assert_eq!(refused.0, reason, "{what}");
        let error = opened(&path)
            .err()
            .unwrap_or_else(|| panic!("{what}: opened"));
        assert!(error.to_string().contains(reason), "{what}: {error}");
        assert_eq!(
            rawmakase_decode::raster::decode(&path)?.as_raw(),
            shown.as_raw(),
            "{what}: the Library shows the base"
        );
    }
    Ok(())
}

/// A JPEG XL whose `jhgm` box is malformed is refused with the reason and shows its
/// base; with the box intact, the same file opens as an HDR edit.
#[test]
fn a_malformed_gain_map_jpeg_xl_is_refused_and_shows_its_base() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let rgb = base(32, 16);
    let codestream = jxl::codestream(
        32,
        16,
        jxl::Samples::Eight(rgb.as_raw()),
        jxl::Colour::Srgb,
        None,
    )?;
    let grey = |w, h| jxl::gray_codestream(w, h, map(w, h).as_raw(), None).unwrap();
    let container = |bundle: &[u8]| jxl::container(&codestream, None, None, Some(bundle)).unwrap();
    let good = jxl::gain_map_bundle(&metadata(), &grey(16, 8))?;
    let intact = file(dir.path(), "intact.jxl", &container(&good));
    rawmakase_decode::raster::probe(&intact).expect("an intact gain map opens");
    assert!(opened(&intact)?.hdr().is_some());
    let shown = rawmakase_decode::raster::decode(&file(
        dir.path(),
        "base.jxl",
        &jxl::container(&codestream, None, None, None)?,
    ))?;

    let change = |f: fn(&mut Vec<u8>)| {
        let mut b = good.clone();
        f(&mut b);
        b
    };
    let alternate = {
        // A colour encoding for the alternate rendition, in a map that says it
        // applies in the alternate's colour space.
        let mut m = metadata();
        m[4] &= !0x40;
        let mut b = vec![0];
        b.extend((m.len() as u16).to_be_bytes());
        b.extend(&m);
        b.extend([1, 0x80]);
        b.extend(0u32.to_be_bytes());
        b.extend(grey(16, 8));
        b
    };
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        ("a newer bundle", change(|b| b[0] = 1), "version 1 gain-map"),
        (
            "metadata longer than its box",
            change(|b| b[1..3].copy_from_slice(&0xfff0u16.to_be_bytes())),
            "damaged gain-map",
        ),
        (
            "a map of another shape",
            jxl::gain_map_bundle(&metadata(), &grey(16, 16))?,
            "damaged gain-map",
        ),
        (
            "a map that is not a codestream",
            jxl::gain_map_bundle(&metadata(), &[0x13; 40])?,
            "damaged gain-map",
        ),
        (
            "a map in the alternate's colour space",
            alternate,
            "alternate-colour gain-map",
        ),
    ];
    for (what, bundle, reason) in cases {
        let path = file(dir.path(), "damaged.jxl", &container(&bundle));
        let refused = rawmakase_decode::raster::probe(&path)
            .err()
            .unwrap_or_else(|| panic!("{what}: probed as editable"));
        assert_eq!(refused.0, reason, "{what}");
        assert!(opened(&path).is_err(), "{what}: opened");
        assert_eq!(
            rawmakase_decode::raster::decode(&path)?.as_raw(),
            shown.as_raw(),
            "{what}: the Library shows the base"
        );
    }
    Ok(())
}

/// A `jhgm` box after the codestream that runs past the file's end is damaged, not
/// absent: the file would otherwise open as SDR. The same box whole opens as HDR.
#[test]
fn a_gain_map_box_cut_off_at_the_files_end_is_refused() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let rgb = base(32, 16);
    let codestream = jxl::codestream(
        32,
        16,
        jxl::Samples::Eight(rgb.as_raw()),
        jxl::Colour::Srgb,
        None,
    )?;
    let map = jxl::gray_codestream(16, 8, map(16, 8).as_raw(), None)?;
    let bundle = jxl::gain_map_bundle(&metadata(), &map)?;
    let mut whole = jxl::container(&codestream, None, None, None)?;
    whole.extend(((bundle.len() + 8) as u32).to_be_bytes());
    whole.extend(b"jhgm");
    whole.extend(&bundle);
    let intact = file(dir.path(), "intact.jxl", &whole);
    rawmakase_decode::raster::probe(&intact).expect("a trailing gain map opens");
    assert!(opened(&intact)?.hdr().is_some());
    let cut = file(dir.path(), "cut.jxl", &whole[..whole.len() - 20]);
    let refused = rawmakase_decode::raster::probe(&cut).expect_err("a cut-off gain map");
    assert_eq!(refused.0, "damaged gain-map");
    assert!(opened(&cut).is_err());
    Ok(())
}

/// JPEG allows any number of fill bytes before a marker. Megabytes of them are read
/// through once, not a read a byte: the header scan runs for every JPEG the Library
/// or Develop probes (it took 28 s for 8 MiB of them read a byte at a time).
#[test]
fn fill_bytes_before_a_marker_are_scanned_quickly() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let good = assemble_iso(&jpeg(base(32, 16)), &jpeg(map(16, 8)), &metadata())?;
    let mut filled = good[..2].to_vec();
    filled.extend(std::iter::repeat_n(0xff, 8 << 20));
    filled.extend_from_slice(&good[2..]);
    let path = file(dir.path(), "filled.jpg", &filled);
    let start = std::time::Instant::now();
    // Past 16 MiB of header a JPEG isn't read as a gain map's base; 8 MiB is within.
    let probed = rawmakase_decode::raster::probe(&path);
    let took = start.elapsed();
    assert!(probed.is_ok(), "{probed:?}");
    assert!(took < std::time::Duration::from_secs(5), "{took:?}");
    Ok(())
}

/// A gain-map JPEG turned by its EXIF orientation opens upright: its map is in the
/// base's stored frame and turns with it, so its light is the unturned file's light
/// turned, and lines up with its SDR rendition.
#[test]
fn a_turned_gain_map_jpeg_opens_upright() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let primary = jpeg(base(32, 16));
    let map = jpeg(map(16, 8));
    let upright = opened(&file(
        dir.path(),
        "upright.jpg",
        &assemble_iso(&primary, &map, &metadata())?,
    ))?;
    let turned = opened(&file(
        dir.path(),
        "turned.jpg",
        &assemble_iso(&with_orientation(&primary, 6), &map, &metadata())?,
    ))?;
    let (upright_light, turned_light) = (upright.hdr().unwrap(), turned.hdr().unwrap());
    assert_eq!((turned.width, turned.height), (16, 32));
    assert_eq!((turned_light.width(), turned_light.height()), (16, 32));
    // Orientation 6: the stored image turned 90° clockwise.
    let at = |x: u32, y: u32| (y * 32 + x) as usize;
    for y in 0..16 {
        for x in 0..32 {
            let (tx, ty) = (15 - y, x);
            let t = (ty * 16 + tx) as usize;
            let (got, want) = (turned_light.pixels()[t], upright_light.pixels()[at(x, y)]);
            assert!(
                (0..3).all(|c| (got[c] - want[c]).abs() <= 1e-5 * want[c].abs().max(1.)),
                "({x}, {y}): {got:?} against {want:?}"
            );
            assert_eq!(turned.pixels[t], upright.pixels[at(x, y)]);
        }
    }
    Ok(())
}

/// The light is the alternate rendition at the map's full weight, the display
/// that shows the whole of the alternate's headroom, whatever that headroom is: no
/// display's headroom reaches the decoded pixels. Two files that differ only in
/// their alternate headroom decode to the same light, the map's full gain over the
/// base.
#[test]
fn display_headroom_never_changes_the_decoded_light() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let primary = jpeg(base(32, 16));
    let map_jpeg = jpeg(map(32, 16));
    let light = |headroom: u32| -> Result<Vec<[f32; 3]>> {
        let mut m = metadata();
        put(&mut m, 13, headroom << 16);
        let path = file(
            dir.path(),
            &format!("headroom-{headroom}.jpg"),
            &assemble_iso(&primary, &map_jpeg, &m)?,
        );
        Ok(opened(&path)?.hdr().unwrap().pixels().to_vec())
    };
    let (two, six) = (light(2)?, light(6)?);
    assert_eq!(two, six);
    // At full weight: each pixel's luminance is the base's raised by the map's gain,
    // 0 to 2 stops, in the base's linear sRGB.
    let base_pixels = image::load_from_memory(&primary)?.to_rgb8();
    let map_pixels = image::load_from_memory(&map_jpeg)?.to_luma8();
    let k = 1. / 64.;
    for ((got, b), g) in two
        .iter()
        .zip(base_pixels.pixels())
        .zip(map_pixels.pixels())
    {
        let sdr: f32 = (0..3)
            .map(|c| LUMINANCE[c] * crate::color::srgb_decode(f32::from(b.0[c]) / 255.))
            .sum();
        let want = (sdr + k) * (2. * f32::from(g.0[0]) / 255.).exp2() - k;
        let got = luminance(*got);
        assert!(
            (got - want).abs() <= 2e-3 * want.max(1.),
            "{got} against {want}"
        );
    }
    Ok(())
}
