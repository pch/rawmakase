//! Writes RAWmakase's record for an editable preview's DNGPrivateData: the
//! original's facts as its decode had them, in the format
//! [`rawmakase_decode::editable_preview::record`] defines and reads. Every
//! conversion here destructures its source whole, so a fact added to
//! [`Metadata`] or the lens types doesn't compile until the record keeps it or
//! says why not.
use anyhow::{Result, ensure};
use rawmakase_decode::editable_preview::record::{
    AutoCa, Calibration, Gains, Htp, Lens, MAGIC, MAX_RECORD, MatrixFrom, Presets, ProfileStatus,
    Record, Table, VERSION,
};
use rawmakase_model::camera_data::{
    EmbeddedProfileStatus, HighlightTonePriority, MatrixSource, Metadata, NeutralCalibration,
    Source, WhiteBalancePresets,
};
use rawmakase_model::camera_profiles::{CameraProfile, fitted};
use rawmakase_model::optics::{GainGrid, LensCorrection, Radial};

/// The record of `m`, the metadata of the original's full-size decode, for a
/// copy of `copy` pixels, with the photo's `default_look`. Refused for what
/// version 1 can't keep: a rendered file, an HDR file's light, or a
/// transparency mask.
pub(super) fn of(
    m: &Metadata,
    copy: [u32; 2],
    default_look: Option<&CameraProfile>,
) -> Result<Record> {
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
        focus_distance,
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
        // The user's imported lens profiles, matched again when the copy
        // opens, as `photo::open` matches them.
        lens_profiles: _,
        lateral_ca,
        embedded_dcp,
        coverage,
    } = m;
    ensure!(
        *source == Source::Raw,
        "Editable previews are built for RAWs only"
    );
    ensure!(
        hdr_source.is_none(),
        "An editable preview can't keep an HDR file's light"
    );
    ensure!(
        coverage.is_none(),
        "An editable preview can't keep a DNG's transparency yet"
    );
    let crop = [*crop_left, *crop_top, *crop_width, *crop_height];
    let WhiteBalancePresets {
        daylight,
        shade,
        d65,
        fine_weather,
    } = wb_presets;
    Ok(Record {
        width: *width,
        height: *height,
        copy_width: copy[0],
        copy_height: copy[1],
        raw_width: *raw_width,
        raw_height: *raw_height,
        crop,
        camera_crop: *camera_crop,
        make: make.clone(),
        model: model.clone(),
        exif_make: exif_make.clone(),
        exif_model: exif_model.clone(),
        sensor_crop: *sensor_crop,
        flip: *flip,
        xtrans: *xtrans,
        sraw: *sraw,
        monochrome: *monochrome,
        fuji_dynamic_range: *fuji_dynamic_range,
        fuji_exposure_shift: *fuji_exposure_shift,
        highlight_tone_priority: match highlight_tone_priority {
            HighlightTonePriority::Off => Htp::Off,
            HighlightTonePriority::On => Htp::On,
            HighlightTonePriority::Enhanced => Htp::Enhanced,
        },
        iso: *iso,
        shutter: *shutter,
        aperture: *aperture,
        focal: *focal,
        focal_35mm: *focal_35mm,
        focus_distance: if focus_distance.is_finite() {
            *focus_distance
        } else {
            0.
        },
        focus_at_infinity: *focus_distance == f32::INFINITY,
        wb: *wb,
        daylight_wb: *daylight_wb,
        wb_presets: Presets {
            daylight: *daylight,
            shade: *shade,
            d65: *d65,
            fine_weather: *fine_weather,
        },
        matrix: *matrix,
        cam_xyz: *cam_xyz,
        matrix_source: match matrix_source {
            MatrixSource::Absent => MatrixFrom::Absent,
            MatrixSource::AdobeDng => MatrixFrom::AdobeDng,
            MatrixSource::CompatObserved => MatrixFrom::CompatObserved,
            MatrixSource::File => MatrixFrom::File,
            MatrixSource::FileTungstenOnly => MatrixFrom::FileTungstenOnly,
            MatrixSource::MakerNote => MatrixFrom::MakerNote,
            MatrixSource::Neutral => MatrixFrom::Neutral,
        },
        camera_look: camera_look.clone(),
        embedded_profile_status: match embedded_profile_status {
            EmbeddedProfileStatus::None => ProfileStatus::None,
            EmbeddedProfileStatus::Unreadable => ProfileStatus::Unreadable,
            EmbeddedProfileStatus::MatrixOnly => ProfileStatus::MatrixOnly,
            EmbeddedProfileStatus::OtherCamera => ProfileStatus::OtherCamera,
            EmbeddedProfileStatus::Usable => ProfileStatus::Usable,
        },
        embedded_profile: embedded_dcp.as_deref().map(hex),
        lens: lens.as_ref().map(lens_of),
        lens_stand_in: lens_stand_in.as_ref().map(lens_of),
        lens_model: lens_model.clone(),
        lens_id: *lens_id,
        lens_info: *lens_info,
        adobe_names: adobe_names.clone(),
        baseline_exposure: *baseline_exposure,
        dng_neutral_calibration: dng_neutral_calibration.as_ref().map(
            |NeutralCalibration { gains, signature }| Calibration {
                gains: *gains,
                signature: signature.clone(),
            },
        ),
        dng_analog_balance: *dng_analog_balance,
        dng_matrix_profile_signature: dng_matrix_profile_signature.clone(),
        levels: *levels,
        auto_ca: lateral_ca.get().map(|measured| match measured {
            Some([red, blue]) => AutoCa {
                red: Some(table(red)),
                blue: Some(table(blue)),
            },
            None => AutoCa::default(),
        }),
        default_look: default_look.map(fitted::carried),
    })
}

/// The bytes of DNGPrivateData holding `record`. Refused unless they
/// read back as this record, so nothing is written that can't be read (a
/// value JSON can't hold, such as a NaN, among them).
pub(super) fn to_bytes(record: &Record) -> Result<Vec<u8>> {
    let mut out = MAGIC.to_vec();
    out.extend(VERSION.to_be_bytes());
    out.extend(serde_json::to_vec(record)?);
    ensure!(
        out.len() as u64 <= MAX_RECORD,
        "An editable preview's record would be larger than one may be"
    );
    ensure!(
        Record::parse(&out).ok().as_ref() == Some(record),
        "An editable preview's record doesn't read back as written"
    );
    Ok(out)
}

/// The record's form of a lens correction.
fn lens_of(l: &LensCorrection) -> Lens {
    let LensCorrection {
        source,
        default_on,
        vignetting,
        vignetting_measured,
        distortion,
        chromatic,
        chromatic_at_source,
        gain_map,
        centre,
    } = l;
    Lens {
        source: source.clone(),
        default_on: *default_on,
        vignetting: vignetting.as_ref().map(table),
        vignetting_measured: *vignetting_measured,
        distortion: distortion.as_ref().map(table),
        chromatic: chromatic.as_ref().map(|[r, b]| [table(r), table(b)]),
        chromatic_at_source: *chromatic_at_source,
        gain_map: gain_map.as_ref().map(gains),
        centre: *centre,
    }
}

fn table(r: &Radial) -> Table {
    let Radial { knots, values } = r;
    Table {
        knots: knots.clone(),
        values: values.clone(),
    }
}

fn gains(g: &GainGrid) -> Gains {
    let GainGrid {
        area,
        after_warp,
        points,
        origin,
        spacing,
        planes,
        gains,
    } = g;
    Gains {
        area: *area,
        after_warp: *after_warp,
        points: points.map(|v| u32::try_from(v).unwrap_or(u32::MAX)),
        origin: *origin,
        spacing: *spacing,
        planes: u32::try_from(*planes).unwrap_or(u32::MAX),
        gains: gains.clone(),
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}
