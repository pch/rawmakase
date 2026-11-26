//! RAWmakase's own decoders (`rawmakase-raw`) behind [`super::Raw`]: the file
//! is read into memory and closed, its header facts mapped to the [`Header`]
//! a [`super::Raw`] is made of, and its sensor data to the [`Sensor`] and [`Cfa`]
//! development reads.
use super::{Cfa, Header, Sensor};
use anyhow::{Result, ensure};
use rawmakase_cancel::Cancel;
use rawmakase_raw::{RawInfo, SensorData};
use rayon::prelude::*;
use std::path::Path;
use std::sync::Arc;

/// A developed image: width, height and pixels.
pub(super) type Developed = (u32, u32, Vec<[f32; 3]>);

/// [`Own::bayer_red`]'s answer for an X-Trans mosaic, which AI Denoise takes another way
/// (`Raw::packed_bayer`).
pub(super) const XTRANS: &str = "X-Trans";
pub(super) struct Own {
    bytes: Arc<Vec<u8>>,
    info: RawInfo,
}

std::thread_local! {
    /// How many files this thread has read whole ([`whole_file_reads`]).
    static WHOLE_FILE_READS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}
/// How many RAW files the calling thread has read whole so far: for tests of
/// the callers that want a file's header or embedded preview alone, which
/// read it in pieces and must leave this as it was.
#[doc(hidden)]
pub fn whole_file_reads() -> u64 {
    WHOLE_FILE_READS.get()
}

/// The file at `path`, read whole, as [`std::fs::read`] reads it (its errors
/// too: a file too large for memory is an error, not an abort), and kept in
/// the buffer it was read into. Copied into an `Arc<[u8]>` instead, a 130 MB
/// RAW took 260 MB while it opened.
fn read_whole(path: &Path) -> std::io::Result<Arc<Vec<u8>>> {
    let bytes = std::fs::read(path)?;
    WHOLE_FILE_READS.set(WHOLE_FILE_READS.get() + 1);
    Ok(Arc::new(bytes))
}

impl Own {
    pub(super) fn open(path: &Path) -> Result<(Self, Header)> {
        let bytes = read_whole(path)?;
        let info = rawmakase_raw::open(&bytes)?;
        let header = header(&info);
        Ok((Self { bytes, info }, header))
    }
    /// The file at `path` read in pieces, its header alone
    /// ([`rawmakase_raw::header_from_file`]): for its metadata, with no bytes
    /// of the file kept, so decoding it fails rather than decoding zeros.
    pub(super) fn open_header(path: &Path) -> Result<(Self, Header)> {
        // Unbuffered: each piece is read whole, and a buffer would read past it.
        let mut file = std::fs::File::open(path)?;
        let partial = rawmakase_raw::header_from_file(&mut file)
            .ok_or_else(|| anyhow::anyhow!("No header found without reading the whole file"))?;
        let header = header(&partial.info);
        Ok((
            Self {
                bytes: Arc::default(),
                info: partial.info,
            },
            header,
        ))
    }
    /// Lets go of the file's bytes once nothing more is read from them: what is
    /// asked of the file afterwards fails, as for one opened by its header.
    pub(super) fn release_file(&mut self) {
        self.bytes = Arc::default();
    }
    /// The color space the maker notes, or the file's writer, give the
    /// camera's JPEGs.
    pub(super) fn preview_space(&self) -> Option<super::MakerPreviewSpace> {
        self.info.preview_space.map(|s| match s {
            rawmakase_raw::PreviewSpace::Srgb => super::MakerPreviewSpace::Srgb,
            rawmakase_raw::PreviewSpace::AdobeRgb => super::MakerPreviewSpace::AdobeRgb,
        })
    }
    /// Where the image is empty, from a DNG's transparency mask, over the
    /// visible area development reads (see [`rawmakase_raw::transparency`]);
    /// `None` for a file without one, or whose mask can't be read, which then
    /// opens as before.
    pub(super) fn coverage(&self) -> Option<rawmakase_model::camera_data::Coverage> {
        let stored = rawmakase_raw::transparency(&self.info, &self.bytes, Cancel::NEVER)
            .ok()
            .flatten()?;
        let [left, top] = self.info.margins.map(|v| v as usize);
        let (w, h) = (self.info.width as usize, self.info.height as usize);
        let stride = self.info.raw_width as usize;
        let rows = stored.chunks(stride.max(1));
        let values: Vec<u8> = rows
            .skip(top)
            .take(h)
            .flat_map(|row| row.get(left..left + w).unwrap_or_default().iter().copied())
            .collect();
        rawmakase_model::camera_data::Coverage::new(w as u32, h as u32, values)
    }
    pub(super) fn thumbnail(&self) -> Result<Vec<u8>> {
        Ok(rawmakase_raw::preview(&self.info, &self.bytes)?.to_vec())
    }
    pub(super) fn thumbnail_reaching(&self, edge: u32) -> Result<Vec<u8>> {
        Ok(rawmakase_raw::preview_reaching(&self.info, &self.bytes, edge)?.to_vec())
    }
    fn decode(&self, cancel: Cancel<'_>) -> Result<rawmakase_raw::Sensor> {
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
    /// The sensor data as fingerprints describe it: the visible area's
    /// samples hashed as little-endian bytes (two a sample, four for
    /// floating-point ones).
    pub(super) fn sensor(&self) -> Result<Sensor> {
        use sha2::Digest;
        let s = self.decode(Cancel::NEVER)?;
        let channels = s.channels as usize;
        let [left, top, w, h] = s.area.map(|v| v as usize);
        let pitch = s.width as usize * channels;
        let mut hash = sha2::Sha256::new();
        let mut bytes = Vec::with_capacity(w * channels * 4);
        let row = |y: usize| y * pitch + left * channels..y * pitch + (left + w) * channels;
        for y in top..top + h {
            bytes.clear();
            match &s.data {
                SensorData::U16(d) => bytes.extend(d[row(y)].iter().flat_map(|v| v.to_le_bytes())),
                SensorData::F32(d) => bytes.extend(d[row(y)].iter().flat_map(|v| v.to_le_bytes())),
            }
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
    pub(super) fn rgb(&self, wb: [f32; 3], cancel: Cancel<'_>) -> Result<Option<Developed>> {
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
        let wb = if self.info.balanced { [1.; 3] } else { wb };
        let channels = s.channels as usize;
        let pixels = match &s.data {
            SensorData::U16(d) => {
                let rows = visible_rows(d, &s, [left, top, w, h]);
                super::develop_rgb(&rows, channels, s.black, s.white, wb)
            }
            SensorData::F32(d) => {
                let rows = visible_rows(d, &s, [left, top, w, h]);
                super::develop_rgb(&rows, channels, s.black, s.white, wb)
            }
        };
        Ok(Some((w as u32, h as u32, pixels)))
    }
    /// Where red sits in the visible mosaic's 2 × 2 period (`[x, y]`), when AI Denoise's
    /// model can take it: a Bayer mosaic of red, two greens and blue, not turned. The
    /// reason otherwise, for the user.
    pub(super) fn bayer_red(&self) -> Result<[usize; 2], &'static str> {
        if self.info.monochrome {
            return Err("AI Denoise needs a color sensor; this one is monochrome.");
        }
        let Some(cfa) = &self.info.cfa else {
            return Err(
                "AI Denoise works on the camera's mosaic; this file stores demosaiced RGB.",
            );
        };
        if cfa.is_xtrans() {
            return Err(XTRANS);
        }
        if self.info.super_ccd.is_some() {
            return Err("AI Denoise does not support Super CCD sensors.");
        }
        let colour = |y: usize, x: usize| match cfa.color(y, x) {
            3 => 1,
            c => c,
        };
        let bayer = (0..cfa.rows.max(2))
            .all(|y| (0..cfa.cols.max(2)).all(|x| colour(y, x) == colour(y % 2, x % 2)));
        let count = |c: u8| {
            (0..2)
                .flat_map(|y| (0..2).map(move |x| (y, x)))
                .filter(|&(y, x)| colour(y, x) == c)
                .count()
        };
        if !bayer || count(0) != 1 || count(1) != 2 || count(2) != 1 {
            return Err("AI Denoise supports Bayer sensors only; this mosaic is not one.");
        }
        let (y, x) = (0..2)
            .flat_map(|y| (0..2).map(move |x| (y, x)))
            .find(|&(y, x)| colour(y, x) == 0)
            .unwrap_or_default();
        if colour(1 - y, 1 - x) != 2 {
            return Err("AI Denoise supports Bayer sensors only; this mosaic is not one.");
        }
        Ok([x, y])
    }
    /// The visible mosaic as development reads it: (raw − black) / (white −
    /// black), not white balanced; `None` when the file is not a mosaic.
    pub(super) fn cfa(&self, cancel: Cancel<'_>) -> Result<Option<Cfa>> {
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

/// The rows of the visible area `[left, top, width, height]` of `data`, the
/// samples of `s`.
fn visible_rows<'a, T>(data: &'a [T], s: &rawmakase_raw::Sensor, area: [usize; 4]) -> Vec<&'a [T]> {
    let [left, top, w, h] = area;
    let channels = s.channels as usize;
    (top..top + h)
        .map(|y| {
            let start = (y * s.width as usize + left) * channels;
            &data[start..start + w * channels]
        })
        .collect()
}

/// The visible area of one-sample sensor data `s` as development reads it:
/// (raw − black) / (white − black), negative below black (noise is clipped only
/// after demosaicing, where interpolated samples carry less of it), each sample's black that of
/// channel `channel(y, x)` plus its position's in the black pattern.
fn visible(s: &rawmakase_raw::Sensor, channel: impl Fn(usize, usize) -> usize + Sync) -> Vec<f32> {
    let [left, top, w, h] = s.area.map(|v| v as usize);
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let (rows, cols, ref values) = s.black_pattern;
    let white = s.white as f32;
    let black_f = s.black.map(|b| b as f32);
    let range = black_f.map(|b| (white - b).max(1.));
    let mut data = vec![0f32; w * h];
    let stride = s.width as usize;
    if rows > 0 && cols > 0 {
        let (rows, cols) = (rows as usize, cols as usize);
        data.par_chunks_mut(w).enumerate().for_each(|(y, dst_row)| {
            let at = (y + top) * stride + left;
            let pat_y = (y % rows) * cols;
            let normalize = |x: usize, v: f32| {
                let c = channel(y, x);
                let pat = values.get(pat_y + x % cols).copied().unwrap_or(0);
                let black = black_f[c] + pat as f32;
                (v - black) / (white - black).max(1.)
            };
            match &s.data {
                SensorData::U16(d) => {
                    for (x, (dst, &v)) in dst_row.iter_mut().zip(&d[at..at + w]).enumerate() {
                        *dst = normalize(x, f32::from(v));
                    }
                }
                SensorData::F32(d) => {
                    for (x, (dst, &v)) in dst_row.iter_mut().zip(&d[at..at + w]).enumerate() {
                        *dst = normalize(x, v);
                    }
                }
            }
        });
    } else {
        data.par_chunks_mut(w).enumerate().for_each(|(y, dst_row)| {
            let at = (y + top) * stride + left;
            match &s.data {
                SensorData::U16(d) => {
                    for (x, (dst, &v)) in dst_row.iter_mut().zip(&d[at..at + w]).enumerate() {
                        let c = channel(y, x);
                        *dst = (f32::from(v) - black_f[c]) / range[c];
                    }
                }
                SensorData::F32(d) => {
                    for (x, (dst, &v)) in dst_row.iter_mut().zip(&d[at..at + w]).enumerate() {
                        let c = channel(y, x);
                        *dst = (v - black_f[c]) / range[c];
                    }
                }
            }
        });
    }
    data
}

/// A Super CCD's stored mosaic `data` (`size` = stored width and height,
/// filter `cfa`) turned by 45° into the mosaic it develops as: each stored
/// pixel where `turn` places it, 0 outside the turned area. The turned
/// pixels form a Bayer mosaic (checked: one color per position of its 2 × 2
/// period).
fn turned(
    turn: rawmakase_raw::SuperCcd,
    cfa: &rawmakase_raw::Cfa,
    data: &[f32],
    [w, h]: [usize; 2],
    cancel: Cancel<'_>,
) -> Result<Cfa> {
    ensure!(!cancel.is_cancelled(), "Development cancelled");
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
    // The color at each of the turned mosaic's 2 × 2 period's positions.
    let mut period = [None; 4];
    for y in 0..h {
        if y % 256 == 0 {
            ensure!(!cancel.is_cancelled(), "Development cancelled");
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

/// The decoder's header facts, from `rawmakase-raw`'s [`RawInfo`].
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
        focus_distance: i.focus_distance,
        wb,
        daylight_wb: daylight,
        presets,
        matrix: i.rgb_cam,
        cam_xyz: i.cam_xyz,
        matrix_source: matrix_source(i.matrix_source),
        camera_look: i.camera_look.clone(),
        lens: i.lens.clone(),
        lens_id: i.lens_id,
        lens_info: i.lens_info,
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
    use rawmakase_cancel::Cancel;
    use std::sync::atomic::AtomicBool;

    /// A file read whole is the file's bytes, whatever its size (an empty one
    /// too), and counts as one whole read on the thread that read it; a file
    /// that can't be read is an error and counts as none.
    #[test]
    fn a_file_read_whole_is_its_bytes_and_is_counted() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        for len in [0usize, 1, 4096, (1 << 20) + 3] {
            let path = dir.path().join(format!("{len}.bin"));
            let bytes: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            std::fs::write(&path, &bytes)?;
            let before = super::whole_file_reads();
            let read = super::read_whole(&path)?;
            assert_eq!(&read[..], &bytes[..], "{len} bytes");
            assert_eq!(super::whole_file_reads(), before + 1);
        }
        let before = super::whole_file_reads();
        assert!(super::read_whole(&dir.path().join("missing.bin")).is_err());
        assert_eq!(super::whole_file_reads(), before);
        Ok(())
    }

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
        let out = super::turned(turn, &cfa, &data, [2, 4], Cancel::NEVER).unwrap();
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
        assert!(super::turned(turn, &cfa, &data, [2, 4], Cancel::new(&cancelled)).is_err());
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
        // XTransLayout record lists it last color first.
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
        let mut cfa = own.cfa(Cancel::NEVER).unwrap().unwrap();
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
            .develop(Decode::Full(Demosaic::Rawmakase), Cancel::new(&cancel))
            .unwrap();
        assert_eq!([full.width, full.height], [w, h]);
        assert!(full.pixels == expected, "the full size is not X-Trans's");
        let half = open().develop(Decode::Half, Cancel::new(&cancel)).unwrap();
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
            Cancel::NEVER,
        );
        let error = format!("{:#}", developed.err().unwrap());
        assert!(error.contains("impossible shape"), "{error}");
    }

    #[test]
    fn visible_normalizes_u16_and_f32_bit_exactly_with_and_without_black_pattern() {
        let reference =
            |s: &rawmakase_raw::Sensor, ch: &dyn Fn(usize, usize) -> usize| -> Vec<f32> {
                let [left, top, w, h] = s.area.map(|v| v as usize);
                let (rows, cols, ref values) = s.black_pattern;
                let white = s.white as f32;
                let mut out = Vec::with_capacity(w * h);
                for y in 0..h {
                    let at = (y + top) * s.width as usize + left;
                    for x in 0..w {
                        let pat = if rows > 0 && cols > 0 {
                            values
                                .get((y % rows as usize) * cols as usize + x % cols as usize)
                                .copied()
                                .unwrap_or(0)
                        } else {
                            0
                        };
                        let mut black = s.black[ch(y, x)] as f32;
                        black += pat as f32;
                        let v = match &s.data {
                            rawmakase_raw::SensorData::U16(d) => f32::from(d[at + x]),
                            rawmakase_raw::SensorData::F32(d) => d[at + x],
                        };
                        out.push((v - black) / (white - black).max(1.));
                    }
                }
                out
            };
        let ch = |y: usize, x: usize| (y % 2) * 2 + (x % 2);
        let u16_vals: Vec<u16> = (0..10 * 8).map(|i| ((i * 257) % 16384) as u16).collect();
        let f32_vals: Vec<f32> = (0..10 * 8).map(|i| (i as f32) * 213.5 - 50.25).collect();
        for data in [
            rawmakase_raw::SensorData::U16(u16_vals),
            rawmakase_raw::SensorData::F32(f32_vals),
        ] {
            for black_pattern in [
                (0, 0, Vec::new()),
                (2, 3, vec![0, 5, 11, 7, 13, 19]),
                // Short pattern slice exercises the unwrap_or(0) fallback.
                (3, 3, vec![4, 9]),
            ] {
                for white in [16383u32, 100] {
                    let sensor = rawmakase_raw::Sensor {
                        width: 10,
                        height: 8,
                        channels: 1,
                        data: match &data {
                            rawmakase_raw::SensorData::U16(v) => {
                                rawmakase_raw::SensorData::U16(v.clone())
                            }
                            rawmakase_raw::SensorData::F32(v) => {
                                rawmakase_raw::SensorData::F32(v.clone())
                            }
                        },
                        area: [2, 1, 7, 6],
                        black: [128, 130, 129, 131],
                        black_pattern: black_pattern.clone(),
                        white,
                        linear_white: [0; 4],
                    };
                    let got = super::visible(&sensor, ch);
                    let want = reference(&sensor, &ch);
                    assert_eq!(got.len(), want.len());
                    for (g, w) in got.iter().zip(&want) {
                        assert_eq!(g.to_bits(), w.to_bits());
                    }
                }
            }
        }
        // Empty visible area returns empty without panicking.
        let empty = rawmakase_raw::Sensor {
            width: 4,
            height: 4,
            channels: 1,
            data: rawmakase_raw::SensorData::U16(vec![0; 16]),
            area: [0, 0, 0, 4],
            black: [0; 4],
            black_pattern: (0, 0, Vec::new()),
            white: 4095,
            linear_white: [0; 4],
        };
        assert!(super::visible(&empty, ch).is_empty());
    }
}
