//! Canon's maker notes: the sensor's borders (SensorInfo), the camera's
//! aspect-ratio crop (AspectInfo), white balance (ColorData), the lens, the
//! camera's orientation (CameraInfo) and Highlight Tone Priority (LightingOpt,
//! CustomFunctions2).
//!
//! Sources: the ExifTool Canon tag-name documentation (tag numbers, record
//! layouts, the ColorData versions' word offsets and CameraInfo's orientation
//! offsets), Laurent Clévy's CR2 write-up, and our own measurements on the
//! test dataset's files (the CameraInfo orientation offsets of the bodies the
//! documentation does not list, and the rules below that say so).
use crate::container::tiff::{Ifd, TiffFile};
use crate::info::{RawInfo, UNSET};

/// What RAWmakase reads from a Canon maker note.
#[derive(Clone, Debug, Default)]
pub(crate) struct Notes {
    /// CanonModelID (0x0010).
    pub model_id: u32,
    /// SensorInfo (0x00e0): the sensor's width and height (words 1, 2) and
    /// the image's left, top, right and bottom borders, inclusive (5 to 8).
    pub sensor: Option<[u32; 6]>,
    /// AspectInfo (0x009a): the ratio code, then the cropped width, height,
    /// left and top.
    pub aspect: Option<[u32; 5]>,
    /// ColorData (0x4001) as 16-bit words.
    pub color_data: Vec<u16>,
    /// CameraInfo (0x000d), a block whose layout depends on the body.
    pub camera_info: Vec<u8>,
    /// LensModel (0x0095).
    pub lens: Option<String>,
    /// Highlight Tone Priority as LightingOpt (0x4018) records it.
    pub htp: Option<u32>,
    /// Whether MultiExp (0x4021) says the image is a multiple exposure,
    /// which the camera has white balanced already.
    pub multiple_exposure: bool,
    /// WBInfo (0x0029): the PowerShot G9's preset levels (green, red, blue,
    /// green per preset), for bodies without ColorData.
    pub wb_info: Vec<i32>,
    /// The white balance setting ShotInfo (0x0004) records.
    pub wb_setting: Option<u32>,
    /// Highlight Tone Priority as CustomFunctions2 (0x0099) records it.
    pub custom_htp: Option<u32>,
}

/// The most of each maker-note record read: the fields RAWmakase reads lie
/// within these (a record is as long as the file says, so it is never read
/// whole). ColorData in 16-bit words (the longest known, version 15, has
/// 1602); CameraInfo, CustomFunctions2 and LensModel in bytes; WBInfo in
/// 32-bit words.
const MAX_COLOR_DATA: usize = 2048;
const MAX_CAMERA_INFO: usize = 4096;
const MAX_CUSTOM_FUNCTIONS: usize = 16384;
const MAX_LENS: usize = 256;
const MAX_WB_INFO: usize = 96;

/// Reads the maker note directory `note` of the TIFF `t`.
pub(crate) fn read(t: &TiffFile, note: &Ifd) -> Notes {
    let mut n = Notes {
        model_id: t.int(note, 0x0010).unwrap_or(0),
        ..Notes::default()
    };
    if let Some(v) = t.ints_upto(note, 0x00e0, 9).filter(|v| v.len() >= 9) {
        n.sensor = Some([v[1], v[2], v[5], v[6], v[7], v[8]]);
    }
    if let Some(v) = t.ints_upto(note, 0x009a, 5).filter(|v| v.len() >= 5) {
        n.aspect = Some([v[0], v[1], v[2], v[3], v[4]]);
    }
    let word = |c: &[u8; 2]| {
        if t.little() {
            u16::from_le_bytes(*c)
        } else {
            u16::from_be_bytes(*c)
        }
    };
    if let Some(raw) = t.raw_upto(note, 0x4001, 2 * MAX_COLOR_DATA) {
        n.color_data = raw.as_chunks::<2>().0.iter().map(word).collect();
    }
    n.camera_info = t
        .raw_upto(note, 0x000d, MAX_CAMERA_INFO)
        .unwrap_or_default();
    n.lens = t
        .raw_upto(note, 0x0095, MAX_LENS)
        .map(|raw| {
            let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
            String::from_utf8_lossy(&raw[..end]).trim().to_string()
        })
        .filter(|s| !s.is_empty());
    let longs = |tag: u16, n: usize| -> Vec<u32> {
        t.raw_upto(note, tag, 4 * n)
            .map(|raw| {
                raw.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| {
                        if t.little() {
                            u32::from_le_bytes(*c)
                        } else {
                            u32::from_be_bytes(*c)
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    // LightingOpt and MultiExp: 32-bit values after their size.
    n.htp = longs(0x4018, 4).get(3).copied();
    n.multiple_exposure = longs(0x4021, 2).get(1) == Some(&1);
    n.wb_info = longs(0x0029, MAX_WB_INFO)
        .into_iter()
        .map(|v| v as i32)
        .collect();
    n.wb_setting = t.ints_upto(note, 0x0004, 8).and_then(|v| v.get(7).copied());
    if let Some(raw) = t.raw_upto(note, 0x0099, MAX_CUSTOM_FUNCTIONS) {
        n.custom_htp = custom_function(&raw, t.little(), 0x0203);
    }
    n
}

impl Notes {
    /// The facts the maker note gives a CR2 or CRW: crops, orientation, white
    /// balance, lens and Highlight Tone Priority. `orientation` is the EXIF
    /// orientation the file's TIFF directories give, 1 when none.
    pub(crate) fn apply(&self, info: &mut RawInfo, orientation: u32) {
        if let Some([.., left, top, right, bottom]) = self.sensor
            && right > left
            && right < crate::io::MAX_SIDE
            && bottom < crate::io::MAX_SIDE
            && bottom > top
        {
            let frame = [left, top, right - left + 1, bottom - top + 1];
            info.frame = frame;
            // It starts on even rows and columns, keeping the filter.
            info.aspect = even(self.aspect_crop(frame));
        }
        info.flip = crate::geometry::flip(orientation);
        if orientation == 1
            && let Some(flip) = self.camera_orientation()
        {
            info.flip = flip;
        }
        if let Some(wb) = self.white_balance() {
            let [r, g, g2, b] = wb.as_shot.map(f32::from);
            info.cam_mul = [r, g, b, g2];
            let rgb = |q: [u16; 4]| [f32::from(q[0]), f32::from(q[1]), f32::from(q[3])];
            info.presets.daylight = rgb(wb.daylight);
            info.presets.shade = rgb(wb.shade);
        } else if let Some((as_shot, daylight)) = self.wb_info_levels() {
            info.cam_mul = as_shot.map(|v| v as f32);
            info.presets.daylight = [daylight[0], daylight[1], daylight[2]].map(|v| v as f32);
        }
        // A multiple exposure is white balanced in the camera.
        if self.multiple_exposure {
            info.cam_mul = [1.; 4];
        }
        if info.lens.is_empty()
            && let Some(lens) = &self.lens
        {
            info.lens = spaced_lens(lens);
        }
        info.highlight_tone_priority = self.htp.map_or(0, |v| v.min(2) as i32);
        // Camera Raw follows the custom function where the bodies from the 40D
        // to the 600D record it (the 60D has no LightingOpt; the 550D's says
        // Off when the custom function is on).
        if info.highlight_tone_priority == 0 && self.custom_htp == Some(1) {
            info.highlight_tone_priority = 1;
        }
    }

    /// Samples at the start of a CR2's image that belong to no pixel: the
    /// EOS-1D Mark III's image starts two samples into its strip (our
    /// measurements: its first two samples are the end of a row, and the
    /// image RAWmakase has always read starts after them).
    pub(crate) fn skipped_samples(&self) -> usize {
        if self.model_id == 0x8000_0169 { 2 } else { 0 }
    }

    /// The camera's aspect-ratio crop in sensor coordinates: AspectInfo's
    /// crop inside SensorInfo's frame when it is smaller, else the frame.
    fn aspect_crop(&self, frame: [u32; 4]) -> [u32; 4] {
        match self.aspect {
            Some([_, w, h, x, y])
                if w > 0
                    && h > 0
                    && (w < frame[2] || h < frame[3])
                    && x.saturating_add(w) <= frame[2]
                    && y.saturating_add(h) <= frame[3] =>
            {
                [frame[0].saturating_add(x), frame[1].saturating_add(y), w, h]
            }
            _ => frame,
        }
    }

    /// The orientation CameraInfo records, as a flip code, on the bodies
    /// whose CameraInfo layout is known: 0 level, 1 rotated 90° clockwise, 2
    /// rotated 270°.
    fn camera_orientation(&self) -> Option<i32> {
        let at = ORIENTATION_OFFSETS
            .iter()
            .find(|(id, _)| *id == self.model_id)?
            .1;
        match self.camera_info.get(at)? {
            0 => Some(0),
            1 => Some(6),
            2 => Some(5),
            _ => None,
        }
    }

    /// The as-shot white balance and the Daylight and Shade presets from
    /// ColorData, each the red, green, second green and blue levels.
    pub(crate) fn white_balance(&self) -> Option<WhiteBalance> {
        let cd = &self.color_data;
        let [as_shot, daylight, shade] = color_data_layout(cd)?;
        let quad = |at: usize| -> Option<[u16; 4]> { cd.get(at..at + 4)?.try_into().ok() };
        Some(WhiteBalance {
            as_shot: quad(as_shot)?,
            daylight: quad(daylight)?,
            shade: quad(shade)?,
        })
    }
}

impl Notes {
    /// From WBInfo, the as-shot levels (red, green, blue, second green) of
    /// the preset ShotInfo names, and the Daylight levels.
    fn wb_info_levels(&self) -> Option<([i32; 4], [i32; 4])> {
        // The row of each white balance setting: Auto, Daylight, Cloudy,
        // Tungsten, Fluorescent, Flash, then Custom 1 and 2 and Underwater.
        let row = match self.wb_setting? {
            0 => 2,
            1 => 10,
            2 => 18,
            3 => 26,
            4 => 34,
            5 => 50,
            15 => 66,
            16 => 74,
            17 => 58,
            _ => return None,
        };
        let levels = |row: usize| -> Option<[i32; 4]> {
            let [g, r, b, g2]: [i32; 4] = self.wb_info.get(row..row + 4)?.try_into().ok()?;
            Some([r, g, b, g2])
        };
        Some((levels(row)?, levels(10)?))
    }
}

/// White balance levels, red, green, second green and blue.
pub(crate) struct WhiteBalance {
    pub as_shot: [u16; 4],
    pub daylight: [u16; 4],
    pub shade: [u16; 4],
}

/// Where a ColorData record keeps the as-shot, Daylight and Shade levels
/// (word offsets), by its length (the first three layouts) or version.
fn color_data_layout(cd: &[u16]) -> Option<[usize; 3]> {
    let version = *cd.first()? as i16;
    Some(match (cd.len(), version) {
        // ColorData1 (20D, 350D), 2 (1D Mark II, 1Ds Mark II), 3 (version 1).
        (582, _) => [25, 35, 40],
        (653, _) => [34, 39, 44],
        (796, _) => [63, 78, 83],
        // ColorData4: ColorCoefs at 63.
        (_, 2..=7 | 9) => [63, 83, 88],
        // ColorData5: ColorCoefs (version -3) or ColorCoefs2 (-4) at 71.
        (_, -3) => [71, 91, 96],
        (_, -4) => [71, 103, 111],
        // ColorData6 (600D, 1200D) and 7 share version 10; 6 is shorter.
        (n, 10) if n < 1300 => [63, 103, 108],
        (_, 10 | 11) => [63, 128, 133],
        (_, 12..=15) => [63, 133, 138],
        (_, 16..=19) => [71, 136, 141],
        (_, 32 | 33) => [85, 150, 155],
        (_, 34 | 48) => [105, 205, 210],
        (_, 64..=66) => [105, 110, 115],
        _ => return None,
    })
}

/// Where CameraInfo keeps the camera's orientation, by CanonModelID: the
/// ExifTool documentation's offsets, and our measurements for the bodies it
/// does not list (confirmed on the test dataset's rotated files of each).
const ORIENTATION_OFFSETS: &[(u32, usize)] = &[
    (0x8000_0169, 48),  // EOS-1D Mark III
    (0x8000_0176, 48),  // EOS 450D
    (0x8000_0190, 48),  // EOS 40D
    (0x8000_0213, 39),  // EOS 5D
    (0x8000_0218, 49),  // EOS 5D Mark II
    (0x8000_0250, 53),  // EOS 7D
    (0x8000_0252, 49),  // EOS 500D
    (0x8000_0254, 48),  // EOS 1000D
    (0x8000_0269, 125), // EOS-1D X
    (0x8000_0270, 53),  // EOS 550D
    (0x8000_0281, 53),  // EOS-1D Mark IV
    (0x8000_0285, 125), // EOS 5D Mark III
    (0x8000_0286, 56),  // EOS 600D
    (0x8000_0287, 54),  // EOS 60D
    (0x8000_0288, 56),  // EOS 1100D
    (0x8000_0301, 125), // EOS 650D
    (0x8000_0302, 131), // EOS 6D
    (0x8000_0325, 132), // EOS 70D
    (0x8000_0326, 125), // EOS 700D
    (0x8000_0327, 58),  // EOS 1200D
    (0x8000_0349, 150), // EOS 5D Mark IV
    (0x8000_0350, 150), // EOS 80D
    (0x8000_0393, 150), // EOS 750D
    (0x8000_0401, 150), // EOS 5DS R
    (0x8000_0404, 58),  // EOS 1300D
    (0x8000_0405, 154), // EOS 800D
    (0x8000_0406, 154), // EOS 6D Mark II
    (0x8000_0432, 58),  // EOS 1500D
];

/// A crop starting on even rows and columns, keeping its far edges.
fn even([mut x, mut y, mut w, mut h]: [u32; 4]) -> [u32; 4] {
    if x == UNSET || y == UNSET {
        return [x, y, w, h];
    }
    if x % 2 == 1 {
        x = x.saturating_add(1);
        w = w.saturating_sub(1);
    }
    if y % 2 == 1 {
        y = y.saturating_add(1);
        h = h.saturating_sub(1);
    }
    [x, y, w, h]
}

/// A lens name as the maker note writes it ("EF-S18-55mm f/3.5-5.6 IS"),
/// with a space between the mount and the focal length, as RAWmakase has
/// always named the lenses of bodies without an EXIF lens model.
fn spaced_lens(lens: &str) -> String {
    for mount in ["EF-S", "EF-M", "EF"] {
        if let Some(rest) = lens.strip_prefix(mount)
            && rest.starts_with(|c: char| c.is_ascii_digit())
        {
            return format!("{mount} {rest}");
        }
    }
    lens.to_string()
}

/// The first value of record `record` in a CustomFunctions2 maker note: a
/// 2-byte size, 2 bytes, the group count; per group its number, length (from
/// the length field on) and record count; per record its tag, value count and
/// 4-byte values.
pub(crate) fn custom_function(b: &[u8], little: bool, record: u32) -> Option<u32> {
    let u32_at = |at: usize| {
        let c: [u8; 4] = b.get(at..at + 4)?.try_into().ok()?;
        Some(if little {
            u32::from_le_bytes(c)
        } else {
            u32::from_be_bytes(c)
        })
    };
    let size = b.len();
    let mut at = 8usize;
    let groups = u32_at(4)?;
    for _ in 0..groups.min(64) {
        if at + 12 > size {
            return None;
        }
        let end = size.min((at + 4).saturating_add(usize::try_from(u32_at(at + 4)?).ok()?));
        if end <= at + 12 {
            return None;
        }
        let mut r = at + 12;
        for _ in 0..u32_at(at + 8)?.min(1024) {
            if r + 8 > end {
                break;
            }
            let (tag, values) = (u32_at(r)?, u32_at(r + 4)?);
            if tag == record && values >= 1 && r + 12 <= end {
                return u32_at(r + 8);
            }
            if values as usize > (end - r) / 4 {
                return None;
            }
            r += 8 + values as usize * 4;
        }
        at = end;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lens_names_get_a_space_after_the_mount() {
        assert_eq!(
            spaced_lens("EF-S18-55mm f/3.5-5.6 IS"),
            "EF-S 18-55mm f/3.5-5.6 IS"
        );
        assert_eq!(spaced_lens("EF50mm f/1.8 II"), "EF 50mm f/1.8 II");
        assert_eq!(spaced_lens("EF 50mm f/1.8"), "EF 50mm f/1.8");
        assert_eq!(spaced_lens("TS-E24mm"), "TS-E24mm");
    }

    #[test]
    fn aspect_crops_start_on_even_rows() {
        let notes = Notes {
            aspect: Some([2, 5334, 4000, 333, 0]),
            ..Notes::default()
        };
        let frame = [84, 46, 6000, 4000];
        assert_eq!(even(notes.aspect_crop(frame)), [418, 46, 5333, 4000]);
        // AspectInfo the size of the frame is no crop: the frame, evened.
        let full = Notes {
            aspect: Some([0, 3888, 2592, 0, 0]),
            ..Notes::default()
        };
        assert_eq!(
            even(full.aspect_crop([52, 23, 3888, 2592])),
            [52, 24, 3888, 2591]
        );
    }

    #[test]
    fn white_balance_comes_from_the_color_data_version() {
        // ColorData7 (version 10): as shot at 63, Daylight at 128, Shade at 133.
        let mut cd = vec![0u16; 1312];
        cd[0] = 10;
        cd[63..67].copy_from_slice(&[2000, 1024, 1025, 1500]);
        cd[128..132].copy_from_slice(&[2100, 1024, 1024, 1400]);
        cd[133..137].copy_from_slice(&[2400, 1024, 1024, 1200]);
        let notes = Notes {
            color_data: cd.clone(),
            ..Notes::default()
        };
        let wb = notes.white_balance().unwrap();
        assert_eq!(wb.as_shot, [2000, 1024, 1025, 1500]);
        assert_eq!(wb.shade, [2400, 1024, 1024, 1200]);
        // The shorter ColorData6 of the same version keeps Daylight at 103.
        cd.truncate(1273);
        cd[103..107].copy_from_slice(&[1900, 1024, 1024, 1600]);
        let notes = Notes {
            color_data: cd,
            ..Notes::default()
        };
        assert_eq!(
            notes.white_balance().unwrap().daylight,
            [1900, 1024, 1024, 1600]
        );
        let unknown = Notes {
            color_data: vec![99; 10],
            ..Notes::default()
        };
        assert!(unknown.white_balance().is_none());
    }

    #[test]
    fn custom_functions_find_their_record() {
        // One group of two records: 0x0101 = 7, then 0x0203 = 1.
        let mut b = vec![0u8; 8];
        b[4..8].copy_from_slice(&1u32.to_le_bytes());
        let records: [(u32, u32); 2] = [(0x0101, 7), (0x0203, 1)];
        let group_len = 8 + records.len() as u32 * 12;
        for v in [1u32, group_len, records.len() as u32] {
            b.extend(v.to_le_bytes());
        }
        for (tag, value) in records {
            for v in [tag, 1, value] {
                b.extend(v.to_le_bytes());
            }
        }
        assert_eq!(custom_function(&b, true, 0x0203), Some(1));
        assert_eq!(custom_function(&b, true, 0x0101), Some(7));
        assert_eq!(custom_function(&b, true, 0x0999), None);
        for end in 0..b.len() {
            let _ = custom_function(&b[..end], true, 0x0203);
        }
    }
}
