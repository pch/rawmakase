//! RAWmakase's own decoders (`rawmakase-raw`) behind [`super::Raw`]: the file
//! is read into memory and closed, its header facts mapped to the [`Header`]
//! a [`super::Raw`] is made of, and its sensor data to the [`Sensor`] and [`Cfa`]
//! development reads.
use super::{Cfa, Header, Sensor};
use anyhow::{Result, ensure};
use rawmakase_raw::RawInfo;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A developed image: width, height and pixels.
pub(super) type Developed = (u32, u32, Vec<[f32; 3]>);

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
    /// The colour space the maker notes give the camera's JPEGs.
    pub(super) fn preview_space(&self) -> Option<super::MakerPreviewSpace> {
        self.info.preview_space.map(|s| match s {
            rawmakase_raw::PreviewSpace::Srgb => super::MakerPreviewSpace::Srgb,
            rawmakase_raw::PreviewSpace::AdobeRgb => super::MakerPreviewSpace::AdobeRgb,
        })
    }
    pub(super) fn thumbnail(&self) -> Result<Vec<u8>> {
        Ok(rawmakase_raw::preview(&self.info, &self.bytes)?.to_vec())
    }
    pub(super) fn thumbnail_reaching(&self, edge: u32) -> Result<Vec<u8>> {
        Ok(rawmakase_raw::preview_reaching(&self.info, &self.bytes, edge)?.to_vec())
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
    /// The visible area of sensor data that is not a mosaic (small RAWs'
    /// RGB, linear raw DNGs), developed: each channel (raw − black) / (white − black), white
    /// balanced by `wb` unless the camera stored it balanced. A monochrome
    /// DNG's one sample is every channel's, not white balanced. `None` for a
    /// mosaic.
    pub(super) fn rgb(&self, wb: [f32; 3], cancel: &AtomicBool) -> Result<Option<Developed>> {
        if self.info.cfa.is_some() {
            return Ok(None);
        }
        let s = self.decode(cancel)?;
        if self.info.monochrome {
            ensure!(
                s.channels == 1 && s.white > s.black.iter().copied().min().unwrap_or(0),
                "Monochrome sensor data of {} samples a pixel",
                s.channels
            );
            let [_, _, w, h] = s.area;
            let grey = visible(&s, |_, _| 0);
            // Nothing interpolates a monochrome sample, so it is clipped here.
            return Ok(Some((
                w,
                h,
                grey.into_iter().map(|v| [v.max(0.); 3]).collect(),
            )));
        }
        ensure!(
            s.channels >= 3,
            "Sensor data that is neither a mosaic nor RGB"
        );
        let [left, top, w, h] = s.area.map(|v| v as usize);
        let rows: Vec<&[u16]> = (top..top + h)
            .map(|y| {
                let start = (y * s.width as usize + left) * s.channels as usize;
                &s.data[start..start + w * s.channels as usize]
            })
            .collect();
        let wb = if self.info.balanced { [1.; 3] } else { wb };
        let pixels = super::develop_rgb(&rows, s.channels as usize, s.black, s.white, wb);
        Ok(Some((w as u32, h as u32, pixels)))
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
        let [_, _, w, h] = s.area.map(|v| v as usize);
        let data = visible(&s, |y, x| usize::from(cfa.color(y, x)).min(3));
        if let Some(turn) = self.info.super_ccd {
            return turned(turn, &cfa, &data, [w, h], cancel).map(Some);
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

/// The visible area of one-sample sensor data `s` as development reads it:
/// (raw − black) / (white − black), negative below black (noise is clipped only
/// after demosaicing, where interpolated samples carry less of it), each sample's black that of
/// channel `channel(y, x)` plus its position's in the black pattern.
fn visible(s: &rawmakase_raw::Sensor, channel: impl Fn(usize, usize) -> usize) -> Vec<f32> {
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
            let mut black = s.black[channel(y, x)] as f32;
            black += pattern_black(y, x) as f32;
            *dst = (f32::from(v) - black) / (white - black).max(1.);
        }
    }
    data
}

/// A Super CCD's stored mosaic `data` (`size` = stored width and height,
/// filter `cfa`) turned by 45° into the mosaic it develops as: each stored
/// pixel where `turn` places it, 0 outside the turned area. The turned
/// pixels form a Bayer mosaic (checked: one colour per position of its 2 × 2
/// period).
fn turned(
    turn: rawmakase_raw::SuperCcd,
    cfa: &rawmakase_raw::Cfa,
    data: &[f32],
    [w, h]: [usize; 2],
    cancel: &AtomicBool,
) -> Result<Cfa> {
    ensure!(!cancel.load(Ordering::Relaxed), "Development cancelled");
    // Super CCD sensors are near square: the longer stored side at most
    // four times the shorter (the dataset's widest is three), so the turned
    // image is at most 2.25 times the stored area. Anything else is not a
    // Super CCD, and would make the turned and demosaiced buffers huge.
    let [sw, sh] = turn.stored.map(|v| v as usize);
    ensure!(
        [sw, sh] == [w, h] && sw.min(sh) > 0 && sw.max(sh) <= 4 * sw.min(sh),
        "Super CCD data of an impossible shape ({sw} × {sh})"
    );
    let [tw, th] = turn.size().map(|v| v as usize);
    ensure!(
        (tw as u64) * (th as u64) <= 9 * (w as u64) * (h as u64) / 4,
        "Super CCD image larger than its data"
    );
    let mut out = Vec::new();
    out.try_reserve_exact(tw * th)
        .map_err(|_| anyhow::anyhow!("Super CCD image too large to develop"))?;
    out.resize(tw * th, 0f32);
    // The colour at each of the turned mosaic's 2 × 2 period's positions.
    let mut period = [None; 4];
    for y in 0..h {
        if y % 256 == 0 {
            ensure!(!cancel.load(Ordering::Relaxed), "Development cancelled");
        }
        for x in 0..w {
            if let Some([tx, ty]) = turn.position(y as u32, x as u32) {
                let i = ty as usize * tw + tx as usize;
                out[i] = data[y * w + x];
                let c = match cfa.color(y, x) {
                    3 => 1,
                    c => c,
                };
                let p = &mut period[(ty as usize % 2) * 2 + tx as usize % 2];
                ensure!(
                    p.is_none_or(|p| p == c),
                    "Super CCD filter that does not turn into a Bayer mosaic"
                );
                *p = Some(c);
            }
        }
    }
    ensure!(
        period.iter().all(|c| c.is_some_and(|c| c < 3)),
        "Super CCD mosaic without a whole Bayer period"
    );
    let n = crate::demosaic::PATTERN;
    let mut tile = [0u8; crate::demosaic::PATTERN * crate::demosaic::PATTERN];
    for r in 0..n {
        for c in 0..n {
            tile[r * n + c] = period[(r % 2) * 2 + c % 2].unwrap_or(1);
        }
    }
    Ok(Cfa {
        width: tw as u32,
        height: th as u32,
        pattern: tile,
        data: out,
    })
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
        file_names: [i.file_make.clone(), i.file_model.clone()],
        sensor_crop: i.sensor_crop,
        // A Super CCD's image is its stored rows turned by 45°.
        width: i.super_ccd.map_or(i.width, |s| s.size()[0]),
        height: i.super_ccd.map_or(i.height, |s| s.size()[1]),
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
        matrix_source: matrix_source(i.matrix_source),
        camera_look: i.camera_look.clone(),
        lens: i.lens.clone(),
        lens_id: i.lens_id,
        dng_version: i.dng_version,
        levels: i.levels,
        analog_balance: i.analog_balance,
        monochrome: i.monochrome,
    }
}

/// The decoders' matrix provenance as the model names it.
fn matrix_source(s: rawmakase_raw::MatrixSource) -> rawmakase_model::camera_data::MatrixSource {
    use rawmakase_model::camera_data::MatrixSource as M;
    use rawmakase_raw::MatrixSource as R;
    match s {
        R::Absent => M::Absent,
        R::AdobeDng => M::AdobeDng,
        R::CompatObserved => M::CompatObserved,
        R::File => M::File,
        R::FileTungstenOnly => M::FileTungstenOnly,
        R::MakerNote => M::MakerNote,
        R::Neutral => M::Neutral,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    /// A tall Super CCD's 2 × 4 stored samples develop as a 4 × 3 diamond:
    /// each sample where the turn places it, 0 elsewhere, a Bayer period.
    #[test]
    fn super_ccd_mosaic_develops_turned() {
        let turn = rawmakase_raw::SuperCcd {
            stored: [2, 4],
            wide: false,
        };
        let cfa = rawmakase_raw::Cfa {
            rows: 4,
            cols: 2,
            colors: vec![0, 2, 1, 3, 2, 0, 3, 1],
        };
        let data: Vec<f32> = (1..=8).map(|v| v as f32 / 10.).collect();
        let out = super::turned(turn, &cfa, &data, [2, 4], &AtomicBool::new(false)).unwrap();
        assert_eq!([out.width, out.height], [4, 3]);
        // Row 0 runs up and right from (0, 1); row 1 one column on.
        let at = |x: usize, y: usize| out.data[y * 4 + x];
        assert_eq!(
            [at(0, 1), at(1, 0), at(1, 1), at(2, 0)],
            [0.1, 0.2, 0.3, 0.4]
        );
        assert_eq!(out.data.iter().filter(|&&v| v > 0.).count(), 8);
        let mut period: Vec<u8> = out.pattern[..2].to_vec();
        period.extend(&out.pattern[crate::demosaic::PATTERN..crate::demosaic::PATTERN + 2]);
        period.sort();
        assert_eq!(period, [0, 1, 1, 2]);
        let cancelled = AtomicBool::new(true);
        assert!(super::turned(turn, &cfa, &data, [2, 4], &cancelled).is_err());
    }

    /// A RAF of the older layout: header, the records, then `data`.
    fn older_raf(records: &[(u16, Vec<u8>)], data: &[u8]) -> Vec<u8> {
        let mut dir = (records.len() as u32).to_be_bytes().to_vec();
        for (tag, v) in records {
            dir.extend(tag.to_be_bytes());
            dir.extend((v.len() as u16).to_be_bytes());
            dir.extend(v);
        }
        let mut out = b"FUJIFILMCCD-RAW 0201FF000000FinePix F700".to_vec();
        out.resize(160, 0);
        let jpeg = [0xff, 0xd8, 0xff, 0xd9];
        let (d, r) = (164u32, 164 + dir.len() as u32);
        let pointers = [160, 4, d, dir.len() as u32, r, data.len() as u32];
        for (i, v) in pointers.into_iter().enumerate() {
            out[84 + 4 * i..88 + 4 * i].copy_from_slice(&v.to_be_bytes());
        }
        out.extend(jpeg);
        out.extend(dir);
        out.extend(data);
        out
    }

    /// A RAF of the current layout: header, a JPEG, the records, then a
    /// little-endian TIFF whose raw image directory (0xF000) holds `width` ×
    /// `height` 14-bit `samples` as 16-bit words.
    fn raf(records: &[(u16, Vec<u8>)], width: u32, height: u32, samples: &[u16]) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|v| v.to_le_bytes()).collect();
        // The raw image directory's LONG entries, then the data.
        let mut entries: Vec<(u16, Vec<u32>)> = vec![
            (0xf001, vec![width]),
            (0xf002, vec![height]),
            (0xf003, vec![14]),
            (0xf00a, vec![64, 64, 64, 64]),
            (0xf00e, vec![300, 600, 450]),
        ];
        let n = entries.len() + 2;
        let ifd_at = 26u32;
        let values_at = ifd_at + 2 + 12 * n as u32 + 4;
        let extra: u32 = entries
            .iter()
            .filter(|e| e.1.len() > 1)
            .map(|e| 4 * e.1.len() as u32)
            .sum();
        entries.push((0xf007, vec![values_at + extra]));
        entries.push((0xf008, vec![data.len() as u32]));
        entries.sort_by_key(|e| e.0);
        let mut block = b"II*\0\x08\0\0\0\x01\0\x00\xf0\x0d\0\x01\0\0\0".to_vec();
        block.extend(ifd_at.to_le_bytes());
        block.extend(0u32.to_le_bytes());
        block.extend((n as u16).to_le_bytes());
        let mut values = Vec::new();
        for (tag, v) in &entries {
            block.extend(tag.to_le_bytes());
            block.extend(4u16.to_le_bytes());
            block.extend((v.len() as u32).to_le_bytes());
            if v.len() == 1 {
                block.extend(v[0].to_le_bytes());
            } else {
                block.extend((values_at + values.len() as u32).to_le_bytes());
                values.extend(v.iter().flat_map(|x| x.to_le_bytes()));
            }
        }
        block.extend(0u32.to_le_bytes());
        block.extend(values);
        block.extend(data);
        let mut dir = (records.len() as u32).to_be_bytes().to_vec();
        for (tag, v) in records {
            dir.extend(tag.to_be_bytes());
            dir.extend((v.len() as u16).to_be_bytes());
            dir.extend(v);
        }
        let jpeg = [0xff, 0xd8, 0xff, 0xd9];
        let mut out = b"FUJIFILMCCD-RAW 0201FF000000X-T9".to_vec();
        out.resize(160, 0);
        let (d, r) = (164u32, 164 + dir.len() as u32);
        let pointers = [160, 4, d, dir.len() as u32, r, block.len() as u32];
        for (i, v) in pointers.into_iter().enumerate() {
            out[84 + 4 * i..88 + 4 * i].copy_from_slice(&v.to_be_bytes());
        }
        out.extend(jpeg);
        out.extend(dir);
        out.extend(block);
        out
    }

    /// RAWmakase's own decoders develop X-Trans data with its X-Trans
    /// demosaic (`demosaic::xtrans`, whose directional candidates and blend
    /// keep fine periodic detail free of moiré), in full and at half size,
    /// whose pixels are 2 × 2 means of that demosaic, never a Bayer path.
    #[test]
    fn x_trans_develops_through_the_x_trans_demosaic() {
        use crate::camera_data::{Decode, Demosaic};
        use crate::raw::Raw;
        // An X-Trans period, rows top first (0 red, 1 green, 2 blue); the
        // XTransLayout record lists it last colour first.
        let period: [u8; 36] = [
            1, 1, 0, 1, 1, 2, //
            1, 1, 2, 1, 1, 0, //
            2, 0, 1, 0, 2, 1, //
            1, 1, 2, 1, 1, 0, //
            1, 1, 0, 1, 1, 2, //
            0, 2, 1, 2, 0, 1,
        ];
        let layout: Vec<u8> = period.iter().rev().copied().collect();
        let (w, h) = (24u32, 18u32);
        // Stripes three pixels wide over a ramp: detail along every direction.
        let samples: Vec<u16> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let stripe = if (x + y) / 3 % 2 == 0 { 3000 } else { 0 };
                (500 + 200 * x + 90 * y + stripe) as u16
            })
            .collect();
        let file = raf(&[(0x131, layout)], w, h, &samples);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x-trans.raf");
        std::fs::write(&path, file).unwrap();
        let open = || Raw::open_file(&path).unwrap();
        let raw = open();
        assert!(raw.metadata.xtrans);
        let wb = raw.metadata.wb;
        let own = &raw.own;
        let mut cfa = own.cfa(&AtomicBool::new(false)).unwrap().unwrap();
        let n = crate::demosaic::PATTERN;
        for (i, v) in cfa.data.iter_mut().enumerate() {
            let (x, y) = (i % w as usize, i / w as usize);
            *v *= wb[cfa.pattern[(y % n) * n + x % n] as usize];
        }
        let expected = crate::demosaic::xtrans_demosaic(&crate::demosaic::Cfa {
            data: &cfa.data,
            width: w as usize,
            height: h as usize,
            pattern: &cfa.pattern,
            white: wb,
        });
        let cancel = AtomicBool::new(false);
        let full = open()
            .develop(Decode::Full(Demosaic::Rawmakase), &cancel)
            .unwrap();
        assert_eq!([full.width, full.height], [w, h]);
        assert!(full.pixels == expected, "the full size is not X-Trans's");
        let half = open().develop(Decode::Half, &cancel).unwrap();
        assert_eq!([half.width, half.height], [w / 2, h / 2]);
        for (k, got) in half.pixels.iter().enumerate() {
            let (i, j) = (k % (w as usize / 2), k / (w as usize / 2));
            let mut sum = [0f32; 3];
            for y in 2 * j..2 * j + 2 {
                for x in 2 * i..2 * i + 2 {
                    for c in 0..3 {
                        sum[c] += expected[y * w as usize + x][c];
                    }
                }
            }
            assert_eq!(*got, sum.map(|v| v / 4.), "half-size pixel {i}, {j}");
        }
    }

    /// A tall Super CCD of 4 × 65,532 stored samples (512 KiB) would turn
    /// into 32,770 × 32,769 pixels (4.3 GB of floats): refused before the
    /// turned or demosaiced image is allocated.
    #[test]
    fn super_ccd_of_an_impossible_shape_does_not_develop() {
        let pair = |a: u16, b: u16| [a.to_be_bytes(), b.to_be_bytes()].concat();
        let records = [
            (0x100, pair(65532, 4)),
            (0x120, pair(0, 0)),
            (0x121, pair(65532, 8)),
            (0x130, vec![0x80, 0x81, 0x82, 0x83]),
        ];
        let file = older_raf(&records, &vec![0x10; 4 * 65532 * 2]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tall.raf");
        std::fs::write(&path, file).unwrap();
        let raw = crate::raw::Raw::open_file(&path).unwrap();
        assert_eq!([raw.metadata.width, raw.metadata.height], [32770, 32769]);
        let developed = raw.develop(
            crate::camera_data::Decode::Full(crate::camera_data::Demosaic::Rawmakase),
            &AtomicBool::new(false),
        );
        let error = format!("{:#}", developed.err().unwrap());
        assert!(error.contains("impossible shape"), "{error}");
    }
}
