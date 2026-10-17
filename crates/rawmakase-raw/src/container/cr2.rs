//! Canon CR2: a TIFF whose fourth directory holds the sensor data as one
//! lossless JPEG strip, cut into vertical slices (tag 0xC640), and whose maker
//! notes say the sensor's borders, crops, white balance and levels. The small
//! RAWs (sRAW, mRAW), coded as luma and chroma, are not decoded here.
//!
//! Sources: TIFF 6, ITU T.81, Laurent Clévy's CR2 write-up (the directories,
//! the slice layout), the
//! ExifTool Canon and EXIF tag-name documentation, and our own measurements
//! on the test dataset's files.
use super::tiff::{Ifd, TiffFile};
use crate::codec::ljpeg;
use crate::error::{Error, Result, corrupt};
use crate::info::{Cfa, RawInfo};
use crate::sensor::Sensor;
use crate::variant::{Plan, Variant};
use std::sync::atomic::{AtomicBool, Ordering};

/// Where a CR2's sensor data is and how it is laid out.
#[derive(Clone, Debug)]
pub(crate) struct Cr2Plan {
    /// The lossless JPEG strip: offset and length in the file.
    pub strip: (u64, u64),
    /// The width of each vertical slice, left to right, in samples.
    pub slices: Vec<u32>,
    /// The samples the slices lay out: `grid[0]` per row, `grid[1]` rows.
    pub grid: [u32; 2],
    /// The image in pixels.
    pub width: u32,
    pub height: u32,
    /// Samples at the start of the image that are not part of it.
    pub skip: usize,
}

/// The most slices a CR2 strip is cut into (files have up to four).
const MAX_SLICES: u32 = 64;

/// Whether `bytes` is a CR2: a TIFF with "CR" 2 after its header.
pub(crate) fn is_cr2(bytes: &[u8]) -> bool {
    bytes.get(8..11) == Some(b"CR\x02")
}

/// The directory of the sensor data: the one that lists Canon's slices
/// (0xC640) or its tag 0xC5D8, else the fourth.
fn raw_ifd<'t>(t: &'t TiffFile) -> Option<&'t Ifd> {
    t.ifds
        .iter()
        .find(|i| i.has(0xc640) || i.has(0xc5d8))
        .or_else(|| t.ifds.get(3))
}

/// Reads a CR2's header facts.
pub(crate) fn open(bytes: &[u8], t: TiffFile) -> Result<RawInfo> {
    let ifd = raw_ifd(&t).ok_or_else(|| corrupt("CR2 without a raw image"))?;
    let at = t.int(ifd, 273).ok_or_else(|| corrupt("CR2 strip offset"))?;
    let len = t.int(ifd, 279).ok_or_else(|| corrupt("CR2 strip length"))?;
    let strip = (t.base + u64::from(at), u64::from(len));
    let file = crate::io::Bytes::new(bytes, crate::io::Order::Little);
    let jpeg = ljpeg::header(file.slice(strip.0, strip.1)?)?;
    let notes = t
        .exif
        .as_ref()
        .and_then(|e| e.entries.get(&37500))
        .and_then(|e| t.ifd_at(t.offset_of(e)?))
        .map(|note| crate::vendor::canon::read(&t, &note))
        .unwrap_or_default();
    let per_mcu = jpeg.samples_per_mcu();
    // A mosaic codes one sample per component.
    if per_mcu != jpeg.components.len() {
        return Err(Error::Unsupported("CR2 small RAW".into()));
    }
    let (across, rows) = jpeg.mcus();
    let total = (across * rows * per_mcu) as u64;
    // Canon's slices (0xC640): three SHORTs or LONGs, how many slices of what
    // width, then the last one's width; none, or a count of 0, for an
    // unsliced strip. A tag that is there but of another shape, or whose
    // value does not read, is no layout to guess.
    let slice_tag = match t.shape(ifd, 0xc640) {
        None => None,
        Some((rawmakase_tiff::kind::SHORT | rawmakase_tiff::kind::LONG, 3)) => {
            match t.ints_upto(ifd, 0xc640, 3).as_deref() {
                Some([0, _, _]) => None,
                Some(&[n, w, last]) if n <= MAX_SLICES && w > 0 && last > 0 => {
                    Some(vec![n, w, last])
                }
                _ => return Err(corrupt("CR2 slices")),
            }
        }
        Some(_) => return Err(corrupt("CR2 slices")),
    };
    let slice_sum = slice_tag
        .as_ref()
        .map(|s| u64::from(s[0]) * u64::from(s[1]) + u64::from(s[2]));
    // The sensor's width: SensorInfo's, else the slices', else the stream's.
    let width = match (notes.sensor, slice_sum) {
        (Some(s), _) => s[0],
        (None, Some(sum)) => u32::try_from(sum).map_err(|_| corrupt("CR2 slices"))?,
        _ => (across * per_mcu) as u32,
    };
    let grid_width = u64::from(width);
    if grid_width == 0 || !total.is_multiple_of(grid_width) {
        return Err(corrupt("CR2 slices do not divide the image"));
    }
    let grid = [
        u32::try_from(grid_width).map_err(|_| corrupt("CR2 width"))?,
        u32::try_from(total / grid_width).map_err(|_| corrupt("CR2 height"))?,
    ];
    let height = grid[1];
    crate::io::samples(width, height, 1)?;
    // The slices lay out the strip's rows exactly, or the file is wrong.
    let slices = match &slice_tag {
        Some(_) if slice_sum != Some(grid_width) => {
            return Err(corrupt("CR2 slices do not make the image's width"));
        }
        Some(s) => {
            let mut v = vec![s[1]; s[0] as usize];
            v.push(s[2]);
            v
        }
        None => vec![grid[0]],
    };
    let mut info = crate::info::base(Variant::Cr2LosslessJpeg);
    info.raw_width = width;
    info.raw_height = height;
    // The visible area: SensorInfo's borders, unless the camera table says
    // otherwise.
    let [left, top, right, bottom] = notes
        .sensor
        .map(|s| [s[2], s[3], s[4], s[5]])
        .filter(|b| b[2] > b[0] && b[3] > b[1] && b[2] < width && b[3] < height)
        .unwrap_or([0, 0, width - 1, height - 1]);
    info.margins = [left, top];
    info.width = right - left + 1;
    info.height = bottom - top + 1;
    // CR2CFAPattern (0xC5E0) names the filter at the sensor's corner.
    let colors = match t.int(ifd, 0xc5e0) {
        Some(2) => [2, 1, 1, 0],
        Some(3) => [1, 2, 0, 1],
        Some(4) => [1, 0, 2, 1],
        _ => [0, 1, 1, 2],
    };
    let cfa = crate::info::second_green(Cfa {
        rows: 2,
        cols: 2,
        colors: colors.to_vec(),
    });
    info.cfa = Some(cfa.shifted(top as usize, left as usize));
    let ifd0 = &t.ifds[0];
    // Make and model: their first 64 bytes (a name is as long as the file
    // says).
    info.make = t.text_upto(ifd0, 271, 64).unwrap_or_default();
    info.model = t.text_upto(ifd0, 272, 64).unwrap_or_default();
    super::tiff_exif::read(&t, &mut info);
    notes.apply(&mut info, t.int(ifd0, 274).unwrap_or(1));
    info.previews = t.previews();
    // IFD0's strip: the camera's full-size JPEG.
    if let (Some(at), Some(len)) = (t.int(ifd0, 273), t.int(ifd0, 279)) {
        info.previews.push(crate::preview::Candidate {
            offset: t.base + u64::from(at),
            len: u64::from(len),
            pixels: u64::from(t.int(ifd0, 256).unwrap_or(0))
                * u64::from(t.int(ifd0, 257).unwrap_or(0)),
        });
    }
    info.uncompressed_preview = t.has_uncompressed_thumbnail();
    info.plan = Plan::Cr2(Cr2Plan {
        strip,
        slices,
        grid,
        width,
        height,
        skip: notes.skipped_samples(),
    });
    Ok(info)
}

/// Decodes the strip into the sample grid, laying its samples out slice by
/// slice: each slice filled top to bottom, row by row, then the next to its
/// right.
fn grid(plan: &Cr2Plan, bytes: &[u8], cancel: &AtomicBool) -> Result<(Vec<u16>, u8)> {
    let file = crate::io::Bytes::new(bytes, crate::io::Order::Little);
    let strip = file.slice(plan.strip.0, plan.strip.1)?;
    let header = ljpeg::header(strip)?;
    let (width, height) = (plan.grid[0] as usize, plan.grid[1] as usize);
    let count = crate::io::samples(plan.grid[0], plan.grid[1], 1)?;
    // Every coded sample takes a bit at least: a strip far too short for the
    // image it claims is no CR2 (a truncated one still decodes, its missing
    // samples 0).
    if count as u64 > (strip.len() as u64).saturating_mul(8) {
        return Err(corrupt("CR2 strip too short for its image"));
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    let mut stream = crate::io::zeroed(count)?;
    ljpeg::decode_into(&header, strip, cancel, &mut stream)?;
    if stream.len() != count {
        return Err(corrupt("CR2 stream size"));
    }
    // The slices lay out exactly the grid's rows, every sample once.
    if plan.slices.iter().map(|&w| w as usize).sum::<usize>() != width {
        return Err(corrupt("CR2 slices do not fill the image"));
    }
    let mut data = crate::io::zeroed(count)?;
    let mut at = 0usize;
    let mut x0 = 0usize;
    for &w in &plan.slices {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let w = w as usize;
        for y in 0..height {
            data[y * width + x0..y * width + x0 + w].copy_from_slice(&stream[at..at + w]);
            at += w;
        }
        x0 += w;
    }
    if plan.skip > 0 && plan.skip < data.len() {
        data.copy_within(plan.skip.., 0);
        let n = data.len();
        data[n - plan.skip..].fill(0);
    }
    Ok((data, header.precision))
}

/// Decodes a mosaic CR2, with the black level measured on its masked
/// borders.
pub(crate) fn decode(
    info: &RawInfo,
    plan: &Cr2Plan,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<Sensor> {
    let (data, precision) = grid(plan, bytes, cancel)?;
    let width = plan.width as usize;
    let camera = || crate::facts::camera(&info.make, &info.model, false, [plan.width, plan.height]);
    let white = camera()
        .find_map(|c| c.white)
        .unwrap_or((1u32 << precision) - 1);
    let area = [info.margins[0], info.margins[1], info.width, info.height];
    let cfa = info
        .cfa
        .as_ref()
        .ok_or_else(|| corrupt("CR2 without a colour filter"))?;
    let columns = camera().find_map(|c| c.black_columns);
    let mut black =
        crate::levels::masked_black(&data, width, area, cfa, columns, 2).unwrap_or([0; 4]);
    if let Some(b) =
        crate::levels::masked_border_black(&data, width, area, black.iter().sum::<u32>() / 4, white)
    {
        black = [b; 4];
    }
    Ok(Sensor {
        width: plan.width,
        height: plan.height,
        channels: 1,
        data,
        area,
        black,
        black_pattern: (0, 0, Vec::new()),
        white,
        linear_white: [0; 4],
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::atomic::AtomicBool;

    /// An IFD entry: tag, type, count and value bytes.
    pub(crate) type Entry = (u16, u16, u32, Vec<u8>);

    /// Appends a little-endian IFD of `entries` (values over four bytes
    /// first) linking to `next`; returns its offset.
    pub(crate) fn put_ifd(f: &mut Vec<u8>, entries: &[Entry], next: u32) -> u32 {
        let mut entries = entries.to_vec();
        entries.sort_by_key(|e| e.0);
        let mut offsets = Vec::new();
        for (_, _, _, v) in &entries {
            if v.len() > 4 {
                if f.len() % 2 == 1 {
                    f.push(0);
                }
                offsets.push(f.len() as u32);
                f.extend(v);
            } else {
                offsets.push(0);
            }
        }
        if f.len() % 2 == 1 {
            f.push(0);
        }
        let at = f.len() as u32;
        f.extend((entries.len() as u16).to_le_bytes());
        for ((tag, kind, count, v), off) in entries.iter().zip(offsets) {
            f.extend(tag.to_le_bytes());
            f.extend(kind.to_le_bytes());
            f.extend(count.to_le_bytes());
            if v.len() > 4 {
                f.extend(off.to_le_bytes());
            } else {
                f.extend(v.iter().copied().chain([0; 4]).take(4));
            }
        }
        f.extend(next.to_le_bytes());
        at
    }

    pub(crate) fn shorts(v: &[u16]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    /// A CR2: IFD0 (make, model, EXIF with a maker note of `note`), two
    /// small directories, and IFD3 holding `jpeg` with `raw` entries.
    pub(crate) fn cr2(jpeg: &[u8], raw: &[Entry], note: &[Entry]) -> Vec<u8> {
        let mut f = b"II*\0\x10\0\0\0CR\x02\0\0\0\0\0".to_vec();
        let strip = f.len() as u32;
        f.extend(jpeg);
        let mut raw = raw.to_vec();
        raw.push((273, 4, 1, strip.to_le_bytes().to_vec()));
        raw.push((279, 4, 1, (jpeg.len() as u32).to_le_bytes().to_vec()));
        let ifd3 = put_ifd(&mut f, &raw, 0);
        let ifd2 = put_ifd(&mut f, &[(259, 3, 1, shorts(&[1]))], ifd3);
        let ifd1 = put_ifd(&mut f, &[(259, 3, 1, shorts(&[6]))], ifd2);
        let note_at = put_ifd(&mut f, note, 0);
        let note_len = f.len() as u32 - note_at;
        // The EXIF directory: its maker note is the directory just written.
        let exif = f.len() as u32;
        f.extend(1u16.to_le_bytes());
        f.extend(37500u16.to_le_bytes());
        f.extend(7u16.to_le_bytes());
        f.extend(note_len.to_le_bytes());
        f.extend(note_at.to_le_bytes());
        f.extend(0u32.to_le_bytes());
        let ifd0 = put_ifd(
            &mut f,
            &[
                (271, 2, 6, b"Canon\0".to_vec()),
                (272, 2, 13, b"Canon EOS T1\0".to_vec()),
                (34665, 4, 1, exif.to_le_bytes().to_vec()),
            ],
            ifd1,
        );
        f[4..8].copy_from_slice(&ifd0.to_le_bytes());
        f
    }

    #[test]
    fn more_slices_than_any_file_has_are_an_error_not_zeros() {
        // 66 × 2 samples of 1234 in 65 slices of one column and one more:
        // the slices sum to the width, but no CR2 is cut so finely.
        let image = vec![1234u16; 66 * 2];
        let jpeg = crate::codec::ljpeg::tests::encode(&image, 66, 12, 1);
        let bytes = cr2(&jpeg, &[(0xc640, 3, 3, shorts(&[65, 1, 1]))], &[]);
        assert!(crate::open(&bytes).is_err());
        // Up to the limit the slices still lay the image out whole.
        let bytes = cr2(&jpeg, &[(0xc640, 3, 3, shorts(&[64, 1, 2]))], &[]);
        let info = crate::open(&bytes).unwrap();
        let sensor = crate::decode(&info, &bytes, &AtomicBool::new(false)).unwrap();
        assert_eq!(sensor.data, image);
    }

    #[test]
    fn slices_that_do_not_make_the_width_are_an_error() {
        // A 10 × 4 sensor: slices of 6 and 4 columns lay it out; 5 and 4
        // (or no slices but a count) would leave columns unplaced.
        let image: Vec<u16> = (0..40).map(|i| (i * 37 % 4096) as u16).collect();
        let jpeg = crate::codec::ljpeg::tests::encode(&image, 10, 12, 1);
        let note = [(0x00e0, 3, 9, shorts(&[18, 10, 4, 0, 0, 0, 0, 9, 3]))];
        let open =
            |slices: &[u16]| crate::open(&cr2(&jpeg, &[(0xc640, 3, 3, shorts(slices))], &note));
        assert!(open(&[1, 6, 4]).is_ok());
        assert!(open(&[0, 0, 0]).is_ok());
        assert!(matches!(open(&[1, 5, 4]), Err(crate::Error::Corrupt(_))));
        assert!(matches!(open(&[2, 0, 4]), Err(crate::Error::Corrupt(_))));
        // A slice tag of another count or type, or whose value is past the
        // end of the file.
        let open_entry = |entry: super::tests::Entry| crate::open(&cr2(&jpeg, &[entry], &note));
        assert!(matches!(
            open_entry((0xc640, 3, 4, shorts(&[1, 6, 4, 0]))),
            Err(crate::Error::Corrupt(_))
        ));
        assert!(matches!(
            open_entry((0xc640, 2, 6, b"1,6,4\0".to_vec())),
            Err(crate::Error::Corrupt(_))
        ));
        let bytes = cr2(&jpeg, &[(0xc640, 3, 3, shorts(&[1, 6, 4]))], &note);
        let at = bytes
            .windows(8)
            .position(|w| w == [0x40, 0xc6, 3, 0, 3, 0, 0, 0])
            .unwrap();
        let mut past = bytes.clone();
        past[at + 8..at + 12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(crate::open(&past), Err(crate::Error::Corrupt(_))));
    }

    #[test]
    fn maker_note_records_as_long_as_the_file_are_read_by_their_fields() {
        // SensorInfo, ColorData, CameraInfo, CustomFunctions2, WBInfo and
        // LensModel each 4 MB (a file may claim any length): their leading
        // fields are read, and the image opens as its SensorInfo says.
        let image: Vec<u16> = (0..64).map(|i| i * 40).collect();
        let jpeg = crate::codec::ljpeg::tests::encode(&image, 8, 12, 1);
        let mut sensor = shorts(&[18, 8, 8, 0, 0, 0, 0, 7, 7]);
        sensor.resize(4 << 20, 0x41);
        let big = |fill: u8| vec![fill; 4 << 20];
        let note = [
            (0x00e0, 3, (4 << 20) / 2, sensor),
            (0x4001, 3, (4 << 20) / 2, big(0x0a)),
            (0x000d, 7, 4 << 20, big(0x01)),
            (0x0099, 7, 4 << 20, big(0x02)),
            (0x0029, 4, (4 << 20) / 4, big(0x03)),
            (0x0095, 2, 4 << 20, big(b'E')),
        ];
        let bytes = cr2(&jpeg, &[], &note);
        let info = crate::open(&bytes).unwrap();
        assert_eq!((info.raw_width, info.raw_height), (8, 8));
        assert_eq!(info.lens.len(), 256);
        let sensor = crate::decode(&info, &bytes, &AtomicBool::new(false)).unwrap();
        assert_eq!(sensor.data, image);
    }

    #[test]
    fn lays_slices_out_left_to_right() {
        // A 10 × 4 image in two slices of 6 and 4 columns, each slice's rows
        // one after another in the stream.
        let (w, h) = (10usize, 4usize);
        let image: Vec<u16> = (0..w * h).map(|i| (i * 37 % 4096) as u16).collect();
        let mut stream = Vec::new();
        for (x0, sw) in [(0, 6), (6, 4)] {
            for y in 0..h {
                stream.extend(&image[y * w + x0..y * w + x0 + sw]);
            }
        }
        let jpeg = crate::codec::ljpeg::tests::encode(&stream, w, 12, 1);
        // SensorInfo: a 10 × 4 sensor, the image from column 4, row 1 to the
        // end; the filter GBRG at the sensor's corner.
        let note = [(0x00e0, 3, 9, shorts(&[18, 10, 4, 0, 0, 4, 1, 9, 3]))];
        let raw = [
            (0xc640, 3, 3, shorts(&[1, 6, 4])),
            (0xc5e0, 4, 1, 3u32.to_le_bytes().to_vec()),
        ];
        let bytes = cr2(&jpeg, &raw, &note);
        let info = crate::open(&bytes).unwrap();
        assert_eq!(info.variant, crate::Variant::Cr2LosslessJpeg);
        assert_eq!((info.raw_width, info.raw_height), (10, 4));
        assert_eq!(info.margins, [4, 1]);
        assert_eq!((info.width, info.height), (6, 3));
        assert_eq!(info.model, "EOS T1");
        // GBRG seen from row 1, column 4: R G / G B.
        assert_eq!(info.cfa.as_ref().unwrap().colors, [0, 1, 3, 2]);
        let sensor = crate::decode(&info, &bytes, &AtomicBool::new(false)).unwrap();
        assert_eq!(sensor.data, image);
        assert_eq!(sensor.white, 4095);
    }

    #[test]
    fn hostile_maker_notes_and_sizes_are_errors_not_panics() {
        let image: Vec<u16> = (0..64).map(|i| i * 40).collect();
        let jpeg = crate::codec::ljpeg::tests::encode(&image, 8, 12, 1);
        let longs = |v: &[u32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
        let max = u32::MAX;
        // SensorInfo, AspectInfo and CustomFunctions2 at their extremes.
        let note = [
            (
                0x00e0,
                4,
                9,
                longs(&[max, max, max, 0, 0, max - 1, 0, max, max]),
            ),
            (0x009a, 4, 5, longs(&[0, max, max, max, max])),
            (0x0099, 4, 4, longs(&[16, max, 1, max])),
            (0x4001, 3, 3, shorts(&[7, 0xffff, 0xffff])),
        ];
        let bytes = cr2(&jpeg, &[(0xc640, 3, 3, shorts(&[1, 4, 4]))], &note);
        if let Ok(info) = crate::open(&bytes) {
            let _ = crate::decode(&info, &bytes, &AtomicBool::new(false));
        }
        let note = [(0x00e0, 3, 9, shorts(&[0, 8, 8, 0, 0, 7, 1, 0xffff, 0xffff]))];
        let bytes = cr2(&jpeg, &[], &note);
        let info = crate::open(&bytes).unwrap();
        assert_eq!(info.frame, [crate::UNSET, crate::UNSET, 0, 0]);
        // A frame header claiming far more samples than the strip can code.
        let mut huge = jpeg.clone();
        let sof = huge.windows(2).position(|w| w == [0xff, 0xc3]).unwrap();
        huge[sof + 5..sof + 9].copy_from_slice(&[0x40, 0, 0x40, 0]);
        let bytes = cr2(&huge, &[], &[]);
        if let Ok(info) = crate::open(&bytes) {
            assert!(crate::decode(&info, &bytes, &AtomicBool::new(false)).is_err());
        }
    }

    #[test]
    fn decoding_stops_when_cancelled() {
        let image: Vec<u16> = (0..64).map(|i| i * 40).collect();
        let jpeg = crate::codec::ljpeg::tests::encode(&image, 8, 12, 1);
        let bytes = cr2(&jpeg, &[], &[]);
        let info = crate::open(&bytes).unwrap();
        assert_eq!(
            crate::decode(&info, &bytes, &AtomicBool::new(true)).unwrap_err(),
            crate::Error::Cancelled
        );
    }

    #[test]
    fn truncated_and_garbage_files_are_errors_not_panics() {
        let image: Vec<u16> = (0..64).map(|i| i * 40).collect();
        let jpeg = crate::codec::ljpeg::tests::encode(&image, 8, 12, 1);
        let bytes = cr2(&jpeg, &[(0xc640, 3, 3, shorts(&[1, 4, 4]))], &[]);
        assert!(crate::open(&bytes).is_ok());
        for end in 0..bytes.len() {
            if let Ok(info) = crate::open(&bytes[..end]) {
                let _ = crate::decode(&info, &bytes[..end], &AtomicBool::new(false));
            }
        }
        let mut garbage = bytes.clone();
        for (i, b) in garbage.iter_mut().enumerate().skip(16) {
            *b ^= (i * 131 % 251) as u8;
        }
        if let Ok(info) = crate::open(&garbage) {
            let _ = crate::decode(&info, &garbage, &AtomicBool::new(false));
        }
    }
}
