//! RAWmakase's own decoders (`rawmakase-raw`) behind [`super::Raw`]: the file
//! is read into memory and closed, its header facts mapped to the [`Header`]
//! both decoders report, and its sensor data to the [`Sensor`] and [`Cfa`]
//! development reads, with the same arithmetic as LibRaw's bridge.
use super::{Cfa, Header, Sensor};
use anyhow::{Result, ensure};
use rawmakase_raw::RawInfo;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

pub(super) struct Own {
    bytes: Arc<[u8]>,
    info: RawInfo,
}

impl Own {
    pub(super) fn open(path: &Path) -> Result<(Self, Header)> {
        let bytes: Arc<[u8]> = std::fs::read(path)?.into();
        let info = rawmakase_raw::open(&bytes)?;
        let header = header(&info);
        Ok((Self { bytes, info }, header))
    }
    pub(super) fn thumbnail(&self) -> Result<Vec<u8>> {
        Ok(rawmakase_raw::preview(&self.info, &self.bytes)?.to_vec())
    }
    fn decode(&self, cancel: &AtomicBool) -> Result<rawmakase_raw::Sensor> {
        let s = rawmakase_raw::decode(&self.info, &self.bytes, cancel)?;
        let [left, top, w, h] = s.area;
        ensure!(
            s.channels > 0
                && s.data.len() as u64
                    == u64::from(s.width) * u64::from(s.height) * u64::from(s.channels)
                && w > 0
                && h > 0
                && u64::from(left) + u64::from(w) <= u64::from(s.width)
                && u64::from(top) + u64::from(h) <= u64::from(s.height),
            "Invalid RAW dimensions"
        );
        Ok(s)
    }
    /// The sensor data as fingerprints describe it.
    pub(super) fn sensor(&self) -> Result<Sensor> {
        use sha2::Digest;
        let s = self.decode(&AtomicBool::new(false))?;
        let channels = s.channels as usize;
        let [left, top, w, h] = s.area.map(|v| v as usize);
        let pitch = s.width as usize * channels;
        let mut hash = sha2::Sha256::new();
        let mut bytes = Vec::with_capacity(w * channels * 2);
        for y in top..top + h {
            let row = &s.data[y * pitch + left * channels..y * pitch + (left + w) * channels];
            bytes.clear();
            bytes.extend(row.iter().flat_map(|v| v.to_le_bytes()));
            hash.update(&bytes);
        }
        let pattern = match &self.info.cfa {
            Some(cfa) => (0..cfa.rows)
                .map(|r| (0..cfa.cols).map(|c| cfa.color(r, c)).collect())
                .collect(),
            None => Vec::new(),
        };
        Ok(Sensor {
            channels: s.channels,
            origin: [s.area[0], s.area[1]],
            area: [s.area[2], s.area[3]],
            data_sha256: hash.finalize().into(),
            black: s.black,
            black_pattern: s.black_pattern,
            white: s.white,
            linear_white: s.linear_white,
            pattern,
        })
    }
    /// The visible mosaic as development reads it: (raw − black) / (white −
    /// black), not white balanced; `None` when the file is not a mosaic.
    pub(super) fn cfa(&self, cancel: &AtomicBool) -> Result<Option<Cfa>> {
        let Some(cfa) = self.info.cfa.clone() else {
            return Ok(None);
        };
        let s = self.decode(cancel)?;
        if s.channels != 1 || s.white <= s.black.iter().copied().min().unwrap_or(0) {
            return Ok(None);
        }
        let [left, top, w, h] = s.area.map(|v| v as usize);
        let (rows, cols, ref values) = s.black_pattern;
        let pattern_black = |y: usize, x: usize| {
            if rows > 0 && cols > 0 {
                values
                    .get((y % rows as usize) * cols as usize + x % cols as usize)
                    .copied()
                    .unwrap_or(0)
            } else {
                0
            }
        };
        let white = s.white as f32;
        let mut data = vec![0f32; w * h];
        for y in 0..h {
            let src = &s.data[(y + top) * s.width as usize + left..][..w];
            for (x, (dst, &v)) in data[y * w..(y + 1) * w].iter_mut().zip(src).enumerate() {
                let k = usize::from(cfa.color(y, x)).min(3);
                let mut black = s.black[k] as f32;
                black += pattern_black(y, x) as f32;
                *dst = ((f32::from(v) - black) / (white - black).max(1.)).max(0.);
            }
        }
        let n = crate::demosaic::PATTERN;
        let mut tile = [0u8; crate::demosaic::PATTERN * crate::demosaic::PATTERN];
        for r in 0..n {
            for c in 0..n {
                let v = cfa.color(r, c);
                tile[r * n + c] = if v == 3 { 1 } else { v };
            }
        }
        Ok(Some(Cfa {
            width: w as u32,
            height: h as u32,
            pattern: tile,
            data,
        }))
    }
}

/// The header facts both decoders report, from `rawmakase-raw`'s.
fn header(i: &RawInfo) -> Header {
    // As shot, else daylight, relative to green; daylight at least 0.001.
    let mut wb: [f32; 3] = std::array::from_fn(|c| {
        if i.cam_mul[c] > 0. {
            i.cam_mul[c]
        } else {
            i.pre_mul[c]
        }
    });
    let green = if wb[1] > 0. { wb[1] } else { 1. };
    wb.iter_mut().for_each(|v| *v /= green);
    let mut daylight: [f32; 3] = std::array::from_fn(|c| i.pre_mul[c]);
    let green = if daylight[1] > 0. { daylight[1] } else { 1. };
    daylight
        .iter_mut()
        .for_each(|v| *v = (*v / green).max(0.001));
    let presets = if i.dng_version == 0 {
        [
            i.presets.daylight,
            i.presets.shade,
            i.presets.d65,
            i.presets.fine_weather,
        ]
    } else {
        [[0.; 3]; 4]
    };
    Header {
        make: i.make.clone(),
        model: i.model.clone(),
        normalized: [i.normalized_make.clone(), i.normalized_model.clone()],
        width: i.width,
        height: i.height,
        raw_width: i.raw_width,
        raw_height: i.raw_height,
        margins: i.margins,
        frame: i.frame,
        aspect: i.aspect,
        sensor_borders: i.sensor_borders,
        recommended_crop: i.recommended_crop,
        flip: i.flip,
        xtrans: i.cfa.as_ref().is_some_and(|c| c.is_xtrans()),
        fuji_dynamic_range: i.fuji_dynamic_range,
        highlight_tone_priority: i.highlight_tone_priority,
        sraw: i.sraw,
        fuji_exposure_shift: i.fuji_exposure_shift,
        iso: i.iso,
        shutter: i.shutter,
        aperture: i.aperture,
        focal: i.focal,
        focal_35mm: i.focal_35mm,
        wb,
        daylight_wb: daylight,
        presets,
        matrix: i.rgb_cam,
        cam_xyz: i.cam_xyz,
        lens: i.lens.clone(),
        lens_id: i.lens_id,
        dng_version: i.dng_version,
    }
}
