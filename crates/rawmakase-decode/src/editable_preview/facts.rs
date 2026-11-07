//! The original's facts in an editable preview, field by field (`fields`'s encoding).
//! Every field has a fixed id, and a field missing reads as the default given
//! here, which is [`Metadata::default`]'s unless said otherwise. Ids are never
//! reused: a fact a later release adds takes a new id, and a release that doesn't
//! know it skips it. A change to what a field means needs a new format version.
//!
//! The writers destructure each struct whole, so a field added to [`Metadata`] or
//! a type it holds doesn't compile until it is given an id here or listed as not
//! stored.
use super::fields::{Record, Writer};
use anyhow::{Context, Result, bail, ensure};
use rawmakase_interop::exif::{CameraExif, Field};
use rawmakase_model::camera_data::{
    EmbeddedProfileStatus, HdrSource, HighlightTonePriority, MatrixSource, Metadata,
    NeutralCalibration, Source, WhiteBalancePresets,
};
use rawmakase_model::color::hdr_output::HdrTransfer;
use rawmakase_model::optics::{GainGrid, LensCorrection, Radial};
use rawmakase_model::storage::Identity;

/// What a camera image holds beside its pixels and metadata.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Scale {
    pub factor: f32,
    pub clipped: u32,
}

// FACT record ids. Width and height default to the original's size in the HEAD
// section.
const MAKE: u16 = 1;
const MODEL: u16 = 2;
const EXIF_MAKE: u16 = 3;
const EXIF_MODEL: u16 = 4;
const SENSOR_CROP: u16 = 5;
const WIDTH: u16 = 6;
const HEIGHT: u16 = 7;
const RAW_WIDTH: u16 = 8;
const RAW_HEIGHT: u16 = 9;
/// `[left, top, width, height]` in decoded pixels.
const CROP: u16 = 10;
const CAMERA_CROP: u16 = 11;
const FLIP: u16 = 12;
const XTRANS: u16 = 13;
const FUJI_DYNAMIC_RANGE: u16 = 14;
const HIGHLIGHT_TONE_PRIORITY: u16 = 15;
const SRAW: u16 = 16;
const MONOCHROME: u16 = 17;
const FUJI_EXPOSURE_SHIFT: u16 = 18;
const ISO: u16 = 19;
const SHUTTER: u16 = 20;
const APERTURE: u16 = 21;
const FOCAL: u16 = 22;
const FOCAL_35MM: u16 = 23;
const WB: u16 = 24;
const DAYLIGHT_WB: u16 = 25;
const WB_PRESETS: u16 = 26;
/// Row by row.
const MATRIX: u16 = 27;
/// Row by row.
const CAM_XYZ: u16 = 28;
const MATRIX_SOURCE: u16 = 29;
const CAMERA_LOOK: u16 = 30;
const EMBEDDED_PROFILE_STATUS: u16 = 31;
const LENS: u16 = 32;
const LENS_STAND_IN: u16 = 33;
const LENS_MODEL: u16 = 34;
const LENS_ID: u16 = 35;
const LENS_INFO: u16 = 36;
const ADOBE_NAMES: u16 = 37;
const BASELINE_EXPOSURE: u16 = 38;
const DNG_NEUTRAL_CALIBRATION: u16 = 39;
const DNG_ANALOG_BALANCE: u16 = 40;
const DNG_MATRIX_PROFILE_SIGNATURE: u16 = 41;
const SOURCE: u16 = 42;
const HDR_SOURCE: u16 = 43;
const LEVELS: u16 = 44;
/// Auto CA's measurement of the full-size decode. Missing: not measured. An empty
/// record: measured, and nothing found. Otherwise red (1) and blue (2).
const LATERAL_CA: u16 = 45;
const EMBEDDED_DCP: u16 = 46;
const SCALE_FACTOR: u16 = 47;
const SCALE_CLIPPED: u16 = 48;

/// The FACT record of `m` and `scale`.
pub(super) fn write_metadata(m: &Metadata, scale: Scale) -> Writer {
    let Metadata {
        make,
        model,
        exif_make,
        exif_model,
        sensor_crop,
        width,
        height,
        raw_width,
        raw_height,
        crop_width,
        crop_height,
        crop_left,
        crop_top,
        camera_crop,
        flip,
        xtrans,
        fuji_dynamic_range,
        highlight_tone_priority,
        sraw,
        monochrome,
        fuji_exposure_shift,
        iso,
        shutter,
        aperture,
        focal,
        focal_35mm,
        wb,
        daylight_wb,
        wb_presets,
        matrix,
        cam_xyz,
        matrix_source,
        camera_look,
        embedded_profile_status,
        lens,
        lens_stand_in,
        lens_model,
        lens_id,
        lens_info,
        adobe_names,
        baseline_exposure,
        dng_neutral_calibration,
        dng_analog_balance,
        dng_matrix_profile_signature,
        source,
        hdr_source,
        levels,
        // The imported lens profiles that fit: the user's library's, matched again
        // when the photo is opened.
        lens_profiles: _,
        lateral_ca,
        embedded_dcp,
    } = m;
    let mut w = Writer::default();
    w.str(MAKE, make);
    w.str(MODEL, model);
    w.str(EXIF_MAKE, exif_make);
    w.str(EXIF_MODEL, exif_model);
    if let Some(v) = sensor_crop {
        w.f32(SENSOR_CROP, *v);
    }
    w.u32(WIDTH, *width);
    w.u32(HEIGHT, *height);
    w.u32(RAW_WIDTH, *raw_width);
    w.u32(RAW_HEIGHT, *raw_height);
    w.u32s(CROP, &[*crop_left, *crop_top, *crop_width, *crop_height]);
    if let Some(v) = camera_crop {
        w.f32s(CAMERA_CROP, v);
    }
    w.i32(FLIP, *flip);
    w.bool(XTRANS, *xtrans);
    w.u32(FUJI_DYNAMIC_RANGE, *fuji_dynamic_range);
    w.u8(HIGHLIGHT_TONE_PRIORITY, htp_code(*highlight_tone_priority));
    w.bool(SRAW, *sraw);
    w.bool(MONOCHROME, *monochrome);
    if let Some(v) = fuji_exposure_shift {
        w.f32(FUJI_EXPOSURE_SHIFT, *v);
    }
    w.f32(ISO, *iso);
    w.f32(SHUTTER, *shutter);
    w.f32(APERTURE, *aperture);
    w.f32(FOCAL, *focal);
    w.f32(FOCAL_35MM, *focal_35mm);
    w.f32s(WB, wb);
    w.f32s(DAYLIGHT_WB, daylight_wb);
    w.record(WB_PRESETS, write_presets(wb_presets));
    w.f32s(MATRIX, matrix.as_flattened());
    w.f32s(CAM_XYZ, cam_xyz.as_flattened());
    w.u8(MATRIX_SOURCE, matrix_source_code(*matrix_source));
    if let Some(v) = camera_look {
        w.str(CAMERA_LOOK, v);
    }
    w.u8(
        EMBEDDED_PROFILE_STATUS,
        profile_status_code(*embedded_profile_status),
    );
    if let Some(v) = lens {
        w.record(LENS, write_lens(v));
    }
    if let Some(v) = lens_stand_in {
        w.record(LENS_STAND_IN, write_lens(v));
    }
    w.str(LENS_MODEL, lens_model);
    w.u32(LENS_ID, *lens_id);
    if let Some(v) = lens_info {
        w.f32s(LENS_INFO, v);
    }
    w.strs(ADOBE_NAMES, adobe_names);
    if let Some(v) = baseline_exposure {
        w.f32(BASELINE_EXPOSURE, *v);
    }
    if let Some(NeutralCalibration { gains, signature }) = dng_neutral_calibration {
        let mut c = Writer::default();
        c.f32s(1, gains);
        c.str(2, signature);
        w.record(DNG_NEUTRAL_CALIBRATION, c);
    }
    if let Some(v) = dng_analog_balance {
        w.f32s(DNG_ANALOG_BALANCE, v);
    }
    if let Some(v) = dng_matrix_profile_signature {
        w.str(DNG_MATRIX_PROFILE_SIGNATURE, v);
    }
    w.u8(SOURCE, source_code(*source));
    if let Some(HdrSource { transfer, peak }) = hdr_source {
        let mut h = Writer::default();
        h.u8(1, transfer_code(*transfer));
        h.f32(2, *peak);
        w.record(HDR_SOURCE, h);
    }
    if let Some(v) = levels {
        w.u32s(LEVELS, v);
    }
    if let Some(measured) = lateral_ca.get() {
        let mut ca = Writer::default();
        if let Some([red, blue]) = measured {
            ca.record(1, write_radial(red));
            ca.record(2, write_radial(blue));
        }
        w.record(LATERAL_CA, ca);
    }
    if let Some(v) = embedded_dcp {
        w.bytes(EMBEDDED_DCP, v);
    }
    w.f32(SCALE_FACTOR, scale.factor);
    w.u32(SCALE_CLIPPED, scale.clipped);
    w
}

/// The metadata and scale of a FACT record; `size` is the original's, which width
/// and height default to.
pub(super) fn read_metadata(r: &Record, size: [u32; 2]) -> Result<(Metadata, Scale)> {
    let d = Metadata::default();
    let [crop_left, crop_top, crop_width, crop_height] =
        r.u32_array::<4>(CROP)?
            .unwrap_or([d.crop_left, d.crop_top, d.crop_width, d.crop_height]);
    let matrix3 = |id| -> Result<Option<[[f32; 3]; 3]>> {
        Ok(r.f32_array::<9>(id)?
            .map(|v| std::array::from_fn(|i| [v[i * 3], v[i * 3 + 1], v[i * 3 + 2]])))
    };
    let lateral_ca = std::sync::Arc::new(std::sync::OnceLock::new());
    if let Some(ca) = r.record(LATERAL_CA)? {
        let measured = match (ca.record(1)?, ca.record(2)?) {
            (None, None) => None,
            (Some(red), Some(blue)) => Some([read_radial(&red)?, read_radial(&blue)?]),
            _ => bail!("An editable preview's Auto CA has one channel"),
        };
        let _ = lateral_ca.set(measured);
    }
    let metadata = Metadata {
        make: r.str(MAKE)?.unwrap_or(d.make),
        model: r.str(MODEL)?.unwrap_or(d.model),
        exif_make: r.str(EXIF_MAKE)?.unwrap_or(d.exif_make),
        exif_model: r.str(EXIF_MODEL)?.unwrap_or(d.exif_model),
        sensor_crop: r.f32(SENSOR_CROP)?,
        width: r.u32(WIDTH)?.unwrap_or(size[0]),
        height: r.u32(HEIGHT)?.unwrap_or(size[1]),
        raw_width: r.u32(RAW_WIDTH)?.unwrap_or(d.raw_width),
        raw_height: r.u32(RAW_HEIGHT)?.unwrap_or(d.raw_height),
        crop_width,
        crop_height,
        crop_left,
        crop_top,
        camera_crop: r.f32_array::<4>(CAMERA_CROP)?,
        flip: r.i32(FLIP)?.unwrap_or(d.flip),
        xtrans: r.bool(XTRANS)?.unwrap_or(d.xtrans),
        fuji_dynamic_range: r.u32(FUJI_DYNAMIC_RANGE)?.unwrap_or(d.fuji_dynamic_range),
        highlight_tone_priority: r
            .u8(HIGHLIGHT_TONE_PRIORITY)?
            .map(htp_of)
            .transpose()?
            .unwrap_or(d.highlight_tone_priority),
        sraw: r.bool(SRAW)?.unwrap_or(d.sraw),
        monochrome: r.bool(MONOCHROME)?.unwrap_or(d.monochrome),
        fuji_exposure_shift: r.f32(FUJI_EXPOSURE_SHIFT)?,
        iso: r.f32(ISO)?.unwrap_or(d.iso),
        shutter: r.f32(SHUTTER)?.unwrap_or(d.shutter),
        aperture: r.f32(APERTURE)?.unwrap_or(d.aperture),
        focal: r.f32(FOCAL)?.unwrap_or(d.focal),
        focal_35mm: r.f32(FOCAL_35MM)?.unwrap_or(d.focal_35mm),
        wb: r.f32_array(WB)?.unwrap_or(d.wb),
        daylight_wb: r.f32_array(DAYLIGHT_WB)?.unwrap_or(d.daylight_wb),
        wb_presets: r
            .record(WB_PRESETS)?
            .map(|p| read_presets(&p))
            .transpose()?
            .unwrap_or(d.wb_presets),
        matrix: matrix3(MATRIX)?.unwrap_or(d.matrix),
        cam_xyz: matrix3(CAM_XYZ)?.unwrap_or(d.cam_xyz),
        matrix_source: r
            .u8(MATRIX_SOURCE)?
            .map(matrix_source_of)
            .transpose()?
            .unwrap_or(d.matrix_source),
        camera_look: r.str(CAMERA_LOOK)?,
        embedded_profile_status: r
            .u8(EMBEDDED_PROFILE_STATUS)?
            .map(profile_status_of)
            .transpose()?
            .unwrap_or(d.embedded_profile_status),
        lens: r.record(LENS)?.map(|l| read_lens(&l)).transpose()?,
        lens_stand_in: r
            .record(LENS_STAND_IN)?
            .map(|l| read_lens(&l))
            .transpose()?,
        lens_model: r.str(LENS_MODEL)?.unwrap_or(d.lens_model),
        lens_id: r.u32(LENS_ID)?.unwrap_or(d.lens_id),
        lens_info: r.f32_array::<4>(LENS_INFO)?,
        adobe_names: r.strs(ADOBE_NAMES)?.unwrap_or(d.adobe_names),
        baseline_exposure: r.f32(BASELINE_EXPOSURE)?,
        dng_neutral_calibration: r
            .record(DNG_NEUTRAL_CALIBRATION)?
            .map(|c| -> Result<_> {
                Ok(NeutralCalibration {
                    gains: c.f32_array(1)?.unwrap_or([1.; 3]),
                    signature: c.str(2)?.unwrap_or_default(),
                })
            })
            .transpose()?,
        dng_analog_balance: r.f32_array::<3>(DNG_ANALOG_BALANCE)?,
        dng_matrix_profile_signature: r.str(DNG_MATRIX_PROFILE_SIGNATURE)?,
        source: r
            .u8(SOURCE)?
            .map(source_of)
            .transpose()?
            .unwrap_or(d.source),
        hdr_source: r
            .record(HDR_SOURCE)?
            .map(|h| -> Result<_> {
                Ok(HdrSource {
                    transfer: h.u8(1)?.map(transfer_of).transpose()?.unwrap_or_default(),
                    peak: h.f32(2)?.unwrap_or(1.),
                })
            })
            .transpose()?,
        levels: r.u32_array::<3>(LEVELS)?,
        lens_profiles: Default::default(),
        lateral_ca,
        embedded_dcp: r.bytes(EMBEDDED_DCP).map(std::sync::Arc::from),
    };
    let scale = Scale {
        factor: r.f32(SCALE_FACTOR)?.unwrap_or(1.),
        clipped: r.u32(SCALE_CLIPPED)?.unwrap_or(0),
    };
    Ok((metadata, scale))
}

fn write_presets(p: &WhiteBalancePresets) -> Writer {
    let WhiteBalancePresets {
        daylight,
        shade,
        d65,
        fine_weather,
    } = p;
    let mut w = Writer::default();
    for (id, v) in [(1, daylight), (2, shade), (3, d65), (4, fine_weather)] {
        if let Some(v) = v {
            w.f32s(id, v);
        }
    }
    w
}

fn read_presets(r: &Record) -> Result<WhiteBalancePresets> {
    Ok(WhiteBalancePresets {
        daylight: r.f32_array(1)?,
        shade: r.f32_array(2)?,
        d65: r.f32_array(3)?,
        fine_weather: r.f32_array(4)?,
    })
}

fn write_radial(r: &Radial) -> Writer {
    let Radial { knots, values } = r;
    let mut w = Writer::default();
    w.f32s(1, knots);
    w.f32s(2, values);
    w
}

/// A radial table, checked as every reader of one checks it.
fn read_radial(r: &Record) -> Result<Radial> {
    Radial::new(
        r.f32s(1)?.unwrap_or_default(),
        r.f32s(2)?.unwrap_or_default(),
    )
    .context("An editable preview's lens table is not a valid one")
}

fn write_lens(l: &LensCorrection) -> Writer {
    let LensCorrection {
        source,
        default_on,
        vignetting,
        distortion,
        chromatic,
        chromatic_at_source,
        gain_map,
        centre,
    } = l;
    let mut w = Writer::default();
    w.str(1, source);
    w.bool(2, *default_on);
    if let Some(v) = vignetting {
        w.record(3, write_radial(v));
    }
    if let Some(v) = distortion {
        w.record(4, write_radial(v));
    }
    if let Some([red, blue]) = chromatic {
        w.record(5, write_radial(red));
        w.record(6, write_radial(blue));
    }
    w.bool(7, *chromatic_at_source);
    if let Some(g) = gain_map {
        w.record(8, write_gain_grid(g));
    }
    w.f32s(9, centre);
    w
}

fn read_lens(r: &Record) -> Result<LensCorrection> {
    let radial = |id| r.record(id)?.map(|t| read_radial(&t)).transpose();
    let chromatic = match (radial(5)?, radial(6)?) {
        (None, None) => None,
        (Some(red), Some(blue)) => Some([red, blue]),
        _ => bail!("An editable preview's lens table has one chromatic channel"),
    };
    Ok(LensCorrection {
        source: r.str(1)?.unwrap_or_default(),
        default_on: r.bool(2)?.unwrap_or(false),
        vignetting: radial(3)?,
        distortion: radial(4)?,
        chromatic,
        chromatic_at_source: r.bool(7)?.unwrap_or(false),
        gain_map: r.record(8)?.map(|g| read_gain_grid(&g)).transpose()?,
        centre: r.f32_array(9)?.unwrap_or_else(LensCorrection::image_centre),
    })
}

fn write_gain_grid(g: &GainGrid) -> Writer {
    let GainGrid {
        area,
        after_warp,
        points,
        origin,
        spacing,
        planes,
        gains,
    } = g;
    let mut w = Writer::default();
    w.f32s(1, area);
    w.bool(2, *after_warp);
    w.u32s(3, &points.map(|v| v as u32));
    w.f32s(4, origin);
    w.f32s(5, spacing);
    w.u32(6, *planes as u32);
    w.f32s(7, gains);
    w
}

/// A gain map, checked as the DNG reader checks one, since rendering indexes its
/// gains by its size.
fn read_gain_grid(r: &Record) -> Result<GainGrid> {
    let d = GainGrid::default();
    let g = GainGrid {
        area: r.f32_array(1)?.unwrap_or(d.area),
        after_warp: r.bool(2)?.unwrap_or(d.after_warp),
        points: r
            .u32_array::<2>(3)?
            .map_or(d.points, |p| p.map(|v| v as usize)),
        origin: r.f32_array(4)?.unwrap_or(d.origin),
        spacing: r.f32_array(5)?.unwrap_or(d.spacing),
        planes: r.u32(6)?.map_or(d.planes, |v| v as usize),
        gains: r.f32s(7)?.unwrap_or(d.gains),
    };
    ensure!(
        g.valid(),
        "An editable preview's gain map is not a valid one"
    );
    Ok(g)
}

// The codes of each kind of fact. A code is never reused; one this release
// doesn't know was written by a later release.

fn later(what: &str, code: u8) -> anyhow::Error {
    anyhow::anyhow!(
        "An editable preview names a {what} ({code}) only a later release of RAWmakase knows"
    )
}

fn htp_code(v: HighlightTonePriority) -> u8 {
    match v {
        HighlightTonePriority::Off => 0,
        HighlightTonePriority::On => 1,
        HighlightTonePriority::Enhanced => 2,
    }
}
fn htp_of(code: u8) -> Result<HighlightTonePriority> {
    Ok(match code {
        0 => HighlightTonePriority::Off,
        1 => HighlightTonePriority::On,
        2 => HighlightTonePriority::Enhanced,
        _ => return Err(later("Highlight Tone Priority", code)),
    })
}

fn matrix_source_code(v: MatrixSource) -> u8 {
    match v {
        MatrixSource::Absent => 0,
        MatrixSource::AdobeDng => 1,
        MatrixSource::CompatObserved => 2,
        MatrixSource::File => 3,
        MatrixSource::FileTungstenOnly => 4,
        MatrixSource::MakerNote => 5,
        MatrixSource::Neutral => 6,
    }
}
fn matrix_source_of(code: u8) -> Result<MatrixSource> {
    Ok(match code {
        0 => MatrixSource::Absent,
        1 => MatrixSource::AdobeDng,
        2 => MatrixSource::CompatObserved,
        3 => MatrixSource::File,
        4 => MatrixSource::FileTungstenOnly,
        5 => MatrixSource::MakerNote,
        6 => MatrixSource::Neutral,
        _ => return Err(later("colour matrix source", code)),
    })
}

fn profile_status_code(v: EmbeddedProfileStatus) -> u8 {
    match v {
        EmbeddedProfileStatus::None => 0,
        EmbeddedProfileStatus::Unreadable => 1,
        EmbeddedProfileStatus::MatrixOnly => 2,
        EmbeddedProfileStatus::OtherCamera => 3,
        EmbeddedProfileStatus::Usable => 4,
    }
}
fn profile_status_of(code: u8) -> Result<EmbeddedProfileStatus> {
    Ok(match code {
        0 => EmbeddedProfileStatus::None,
        1 => EmbeddedProfileStatus::Unreadable,
        2 => EmbeddedProfileStatus::MatrixOnly,
        3 => EmbeddedProfileStatus::OtherCamera,
        4 => EmbeddedProfileStatus::Usable,
        _ => return Err(later("embedded profile status", code)),
    })
}

fn source_code(v: Source) -> u8 {
    match v {
        Source::Raw => 0,
        Source::Rendered => 1,
    }
}
fn source_of(code: u8) -> Result<Source> {
    Ok(match code {
        0 => Source::Raw,
        1 => Source::Rendered,
        _ => return Err(later("kind of source file", code)),
    })
}

fn transfer_code(v: HdrTransfer) -> u8 {
    match v {
        HdrTransfer::Pq => 0,
        HdrTransfer::Hlg => 1,
        HdrTransfer::Linear => 2,
    }
}
fn transfer_of(code: u8) -> Result<HdrTransfer> {
    Ok(match code {
        0 => HdrTransfer::Pq,
        1 => HdrTransfer::Hlg,
        2 => HdrTransfer::Linear,
        _ => return Err(later("HDR transfer", code)),
    })
}

// The EXIF record: main (1), EXIF (2) and GPS (3) directories, each a list of
// fields, every one its tag (u16), type (u16), count (u32), byte length (u32) and
// bytes. An empty record: the original has no EXIF.

pub(super) fn write_exif(exif: Option<&CameraExif>) -> Writer {
    let mut w = Writer::default();
    if let Some(CameraExif { main, exif, gps }) = exif {
        for (id, fields) in [(1, main), (2, exif), (3, gps)] {
            let mut out = Vec::new();
            for Field {
                tag,
                kind,
                count,
                bytes,
            } in fields
            {
                out.extend(tag.to_le_bytes());
                out.extend(kind.to_le_bytes());
                out.extend(count.to_le_bytes());
                out.extend(
                    u32::try_from(bytes.len())
                        .expect("an EXIF field under 4 GiB")
                        .to_le_bytes(),
                );
                out.extend_from_slice(bytes);
            }
            w.bytes(id, &out);
        }
    }
    w
}

pub(super) fn read_exif(r: &Record) -> Result<Option<CameraExif>> {
    if r.is_empty() {
        return Ok(None);
    }
    let list = |id| -> Result<Vec<Field>> {
        let mut b = r.bytes(id).unwrap_or_default();
        let mut out = Vec::new();
        while !b.is_empty() {
            ensure!(
                b.len() >= 12,
                "An editable preview's EXIF ends inside a field"
            );
            let tag = u16::from_le_bytes([b[0], b[1]]);
            let kind = u16::from_le_bytes([b[2], b[3]]);
            let count = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
            let len = u32::from_le_bytes([b[8], b[9], b[10], b[11]]) as usize;
            let bytes = b
                .get(12..12 + len)
                .context("An editable preview's EXIF ends inside a field")?
                .to_vec();
            out.push(Field {
                tag,
                kind,
                count,
                bytes,
            });
            b = &b[12 + len..];
        }
        Ok(out)
    };
    Ok(Some(CameraExif {
        main: list(1)?,
        exif: list(2)?,
        gps: list(3)?,
    }))
}

// The original's identity, in the HEAD record (`super`'s ids).

pub(super) fn write_identity(w: &mut Writer, [size, modified, hash]: [u16; 3], id: &Identity) {
    let Identity {
        size: bytes,
        modified_ns,
        prefix_hash,
    } = id;
    w.u64(size, *bytes);
    w.u128(modified, *modified_ns);
    w.str(hash, prefix_hash);
}

pub(super) fn read_identity(r: &Record, [size, modified, hash]: [u16; 3]) -> Result<Identity> {
    Ok(Identity {
        size: r
            .u64(size)?
            .context("An editable preview without its original's size")?,
        modified_ns: r
            .u128(modified)?
            .context("An editable preview without its original's modification time")?,
        prefix_hash: r
            .str(hash)?
            .context("An editable preview without its original's identity")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The codes are part of the format: each kind's table, pinned. A code never
    /// changes meaning; a variant added takes a new one.
    #[test]
    fn fact_codes_keep_their_meaning() {
        use EmbeddedProfileStatus as E;
        use HighlightTonePriority as H;
        use MatrixSource as M;
        let htp = [(H::Off, 0), (H::On, 1), (H::Enhanced, 2)];
        for (v, code) in htp {
            assert_eq!(htp_code(v), code);
            assert_eq!(htp_of(code).unwrap(), v);
        }
        let matrix = [
            (M::Absent, 0),
            (M::AdobeDng, 1),
            (M::CompatObserved, 2),
            (M::File, 3),
            (M::FileTungstenOnly, 4),
            (M::MakerNote, 5),
            (M::Neutral, 6),
        ];
        for (v, code) in matrix {
            assert_eq!(matrix_source_code(v), code);
            assert_eq!(matrix_source_of(code).unwrap(), v);
        }
        let status = [
            (E::None, 0),
            (E::Unreadable, 1),
            (E::MatrixOnly, 2),
            (E::OtherCamera, 3),
            (E::Usable, 4),
        ];
        for (v, code) in status {
            assert_eq!(profile_status_code(v), code);
            assert_eq!(profile_status_of(code).unwrap(), v);
        }
        for (v, code) in [(Source::Raw, 0), (Source::Rendered, 1)] {
            assert_eq!(source_code(v), code);
            assert_eq!(source_of(code).unwrap(), v);
        }
        let transfer = [
            (HdrTransfer::Pq, 0),
            (HdrTransfer::Hlg, 1),
            (HdrTransfer::Linear, 2),
        ];
        for (v, code) in transfer {
            assert_eq!(transfer_code(v), code);
            assert_eq!(transfer_of(code).unwrap(), v);
        }
        // A code only a later release knows says so.
        for e in [
            htp_of(3).unwrap_err(),
            matrix_source_of(7).unwrap_err(),
            profile_status_of(5).unwrap_err(),
            source_of(2).unwrap_err(),
            transfer_of(3).unwrap_err(),
        ] {
            assert!(e.to_string().contains("later release"), "{e}");
        }
    }

    /// Auto CA measured with nothing found is kept apart from not measured.
    #[test]
    fn auto_ca_found_nothing_is_not_unmeasured() {
        let m = Metadata::default();
        let _ = m.lateral_ca.set(None);
        let scale = Scale {
            factor: 1.,
            clipped: 0,
        };
        let bytes = write_metadata(&m, scale).into_bytes();
        let (back, _) = read_metadata(&Record::parse(&bytes).unwrap(), [1, 1]).unwrap();
        assert_eq!(back.lateral_ca.get(), Some(&None));
    }
}
