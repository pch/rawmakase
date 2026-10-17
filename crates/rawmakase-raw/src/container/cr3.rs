//! Canon CR3: an ISO base media file (ISO/IEC 14496-12) of brand `crx `.
//! The `moov` box holds Canon's `uuid` box with the TIFF-structured
//! metadata (`CMT1` IFD0, `CMT2` the EXIF directory, `CMT3` the maker notes,
//! `CMT4` GPS), then one track per image: the full-size JPEG (or HEVC)
//! preview, a reduced raw image, the full raw image (and, in Dual Pixel
//! files, a second one) and timed metadata (`CTMD`), whose records hold more
//! maker notes (the white balance among them). A raw track's sample entry
//! (`CRAW`) holds the image header (`CMP1`) and the image areas (`CDI1` →
//! `IAD1`: the crop, the masked borders and the active area, inclusive
//! corners); its one sample, which the sample tables place in `mdat`, is the
//! coded image (`codec::crx`).
//!
//! Sources: ISO/IEC 14496-12, Laurent Clévy's public description of the CR3
//! format, TIFF 6, Exif 2.32, and our own measurements.
use super::tiff::{NAME_MAX, TiffFile};
use crate::codec::crx;
use crate::error::{Error, Result, corrupt};
use crate::info::{Cfa, RawInfo};
use crate::io::Bytes;
use crate::sensor::Sensor;
use crate::variant::{Plan, Variant};
use crate::vendor::canon_cr3 as notes;
use std::sync::atomic::AtomicBool;

/// Canon's metadata box in `moov`.
const CANON_UUID: [u8; 16] = [
    0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b, 0x6a, 0x48,
];
/// The most boxes read and the deepest nesting followed.
const MAX_BOXES: usize = 4096;
/// The children of a `CRAW` sample entry start after its visual sample entry
/// fields (ISO/IEC 14496-12's 78 bytes) and 4 more bytes.
const CRAW_CHILDREN: usize = 82;

/// Where a CR3's raw image is and how it is coded.
#[derive(Clone, Debug)]
pub(crate) struct Cr3Plan {
    pub offset: u64,
    pub len: u64,
    pub header: crx::Header,
    pub black: Black,
    pub white: u32,
}

/// Where the black level comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Black {
    /// The maker notes' per-channel levels (red, green, blue, second green).
    Notes([u32; 4]),
    /// The mean of each channel over this area of the sensor data, `[left,
    /// top, width, height]`, rounded down.
    Measured([u32; 4]),
}

/// Whether `bytes` is a CR3 file.
pub(crate) fn is_cr3(bytes: &[u8]) -> bool {
    bytes.get(4..12) == Some(b"ftypcrx ".as_slice())
}

/// A box: its type, and where its payload (after the header and a `uuid`'s
/// 16 bytes) and the box end.
struct Bx {
    kind: [u8; 4],
    uuid: Option<[u8; 16]>,
    body: usize,
    end: usize,
}

/// The boxes in `bytes[from..to]`, up to the first malformed one.
fn boxes(bytes: &[u8], from: usize, to: usize, budget: &mut usize) -> Vec<Bx> {
    let to = to.min(bytes.len());
    let mut out = Vec::new();
    let mut at = from;
    while at.saturating_add(8) <= to && *budget > 0 {
        *budget -= 1;
        let u32_at = |a: usize| {
            bytes
                .get(a..a + 4)
                .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        };
        let Some(size) = u32_at(at) else { break };
        let mut kind = [0; 4];
        kind.copy_from_slice(&bytes[at + 4..at + 8]);
        let (len, mut head) = match size {
            0 => (to - at, 8),
            1 => match bytes
                .get(at + 8..at + 16)
                .map(|b| u64::from_be_bytes(b.try_into().unwrap_or([0; 8])))
                .and_then(|v| usize::try_from(v).ok())
            {
                Some(len) => (len, 16),
                None => break,
            },
            n => (n as usize, 8),
        };
        let Some(end) = at.checked_add(len).filter(|&e| e <= to && len >= head) else {
            break;
        };
        let mut uuid = None;
        if &kind == b"uuid" {
            let Some(u) = bytes
                .get(at + head..at + head + 16)
                .filter(|_| at + head + 16 <= end)
            else {
                break;
            };
            let mut id = [0; 16];
            id.copy_from_slice(u);
            uuid = Some(id);
            head += 16;
        }
        out.push(Bx {
            kind,
            uuid,
            body: at + head,
            end,
        });
        at = end;
    }
    out
}

/// A track's image, as its sample entry and sample tables describe it.
#[derive(Default)]
struct Track {
    entry: [u8; 4],
    width: u32,
    height: u32,
    /// Child boxes of the sample entry: `CMP1`'s and `IAD1`'s payloads.
    cmp1: Option<(usize, usize)>,
    iad1: Option<(usize, usize)>,
    jpeg: bool,
    hevc: bool,
    /// The first sample: offset and size.
    sample: Option<(u64, u64)>,
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at.checked_add(4)?)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn track(bytes: &[u8], trak: &Bx, budget: &mut usize) -> Track {
    let mut t = Track::default();
    let mut stack = vec![(trak.body, trak.end, 0)];
    let (mut size, mut offset) = (None, None);
    while let Some((from, to, depth)) = stack.pop() {
        for b in boxes(bytes, from, to, budget) {
            match &b.kind {
                b"mdia" | b"minf" | b"stbl" if depth < 4 => stack.push((b.body, b.end, depth + 1)),
                // Version and flags, the entry count, the entries.
                b"stsd" => {
                    if let Some(e) = boxes(bytes, b.body + 8, b.end, budget).into_iter().next() {
                        entry(bytes, &e, &mut t, budget);
                    }
                }
                // Version and flags, a size for every sample (else 0), the
                // count, the sizes.
                b"stsz" => {
                    size = match (be32(bytes, b.body + 4), be32(bytes, b.body + 8)) {
                        (Some(0), Some(n)) if n > 0 => be32(bytes, b.body + 12),
                        (Some(s), Some(n)) if n > 0 => Some(s),
                        _ => None,
                    };
                }
                b"co64" if be32(bytes, b.body + 4).is_some_and(|n| n > 0) => {
                    offset = bytes
                        .get(b.body + 8..b.body + 16)
                        .map(|s| u64::from_be_bytes(s.try_into().unwrap_or([0; 8])));
                }
                b"stco" if be32(bytes, b.body + 4).is_some_and(|n| n > 0) => {
                    offset = be32(bytes, b.body + 8).map(u64::from);
                }
                _ => {}
            }
        }
    }
    t.sample = offset.zip(size.map(u64::from));
    t
}

fn entry(bytes: &[u8], e: &Bx, t: &mut Track, budget: &mut usize) {
    t.entry = e.kind;
    if &e.kind != b"CRAW" {
        return;
    }
    let u16_at = |a: usize| {
        bytes
            .get(a..a + 2)
            .map_or(0, |b| u32::from(u16::from_be_bytes([b[0], b[1]])))
    };
    t.width = u16_at(e.body + 24);
    t.height = u16_at(e.body + 26);
    for c in boxes(bytes, e.body + CRAW_CHILDREN, e.end, budget) {
        match &c.kind {
            b"JPEG" => t.jpeg = true,
            b"HEVC" => t.hevc = true,
            b"CMP1" => t.cmp1 = Some((c.body, c.end)),
            b"CDI1" => {
                // Version and flags, then the areas.
                for a in boxes(bytes, c.body + 4, c.end, budget) {
                    if &a.kind == b"IAD1" {
                        t.iad1 = Some((a.body, a.end));
                    }
                }
            }
            _ => {}
        }
    }
}

/// The parts of a CR3 read here.
struct Cr3<'a> {
    /// `CMT1` to `CMT4`.
    cmt: [Option<&'a [u8]>; 4],
    tracks: Vec<Track>,
}

fn parse(bytes: &[u8]) -> Cr3<'_> {
    let mut budget = MAX_BOXES;
    let mut out = Cr3 {
        cmt: [None; 4],
        tracks: Vec::new(),
    };
    for b in boxes(bytes, 0, bytes.len(), &mut budget) {
        if &b.kind != b"moov" {
            continue;
        }
        for m in boxes(bytes, b.body, b.end, &mut budget) {
            match &m.kind {
                b"uuid" if m.uuid == Some(CANON_UUID) => {
                    for c in boxes(bytes, m.body, m.end, &mut budget) {
                        let i = match &c.kind {
                            b"CMT1" => 0,
                            b"CMT2" => 1,
                            b"CMT3" => 2,
                            b"CMT4" => 3,
                            _ => continue,
                        };
                        out.cmt[i] = bytes.get(c.body..c.end);
                    }
                }
                b"trak" => out.tracks.push(track(bytes, &m, &mut budget)),
                _ => {}
            }
        }
    }
    out
}

/// The records and items read from the timed metadata, and the maker notes
/// parsed from it, at most: hostile files can repeat a huge record.
const MAX_TIMED_ITEMS: usize = 1 << 16;
const MAX_TIMED_NOTES: usize = 16;

/// The first of the maker notes in the timed metadata's records (types 7 to
/// 9: little-endian lengths, then items of a length, a tag and a TIFF
/// stream) that `f` reads something from. Each sample is read once, however
/// many tracks name it, and the work is bounded.
fn first_timed_note<T>(
    bytes: &[u8],
    tracks: &[Track],
    mut f: impl FnMut(&[u8]) -> Option<T>,
) -> Option<T> {
    let le = |b: &[u8], at: usize| -> Option<u32> {
        b.get(at..at.checked_add(4)?)
            .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    };
    let (mut items, mut notes) = (0, 0);
    let mut seen: Vec<(u64, u64)> = Vec::new();
    for t in tracks.iter().filter(|t| &t.entry == b"CTMD") {
        let Some(key) = t.sample.filter(|k| !seen.contains(k)) else {
            continue;
        };
        // At most one entry per track, and the tracks are bounded by the box
        // budget.
        seen.push(key);
        let Some(sample) = usize::try_from(key.0)
            .ok()
            .and_then(|at| bytes.get(at..at.checked_add(usize::try_from(key.1).ok()?)?))
        else {
            continue;
        };
        let mut at = 0;
        while let Some(len) = le(sample, at).map(|l| l as usize) {
            items += 1;
            if items > MAX_TIMED_ITEMS {
                return None;
            }
            let Some(record) = sample.get(at..at.saturating_add(len)).filter(|_| len >= 12) else {
                break;
            };
            let kind = u16::from_le_bytes([record[4], record[5]]);
            if (7..=9).contains(&kind) {
                let mut i = 12;
                while let Some(n) = le(record, i).map(|l| l as usize) {
                    items += 1;
                    if items > MAX_TIMED_ITEMS {
                        return None;
                    }
                    let Some(item) = record.get(i..i.saturating_add(n)).filter(|_| n >= 8) else {
                        break;
                    };
                    if le(item, 4) == Some(0x927c) {
                        notes += 1;
                        if notes > MAX_TIMED_NOTES {
                            return None;
                        }
                        if let Some(found) = f(&item[8..]) {
                            return Some(found);
                        }
                    }
                    i += n;
                }
            }
            at += len;
        }
    }
    None
}

/// Reads a CR3's header facts.
pub(crate) fn open(bytes: &[u8]) -> Result<RawInfo> {
    let c = parse(bytes);
    // The full raw image: the first of the largest raw tracks (a Dual Pixel
    // file's second image is as large).
    let raw = c
        .tracks
        .iter()
        .filter(|t| t.cmp1.is_some())
        .fold(None::<&Track>, |best, t| match best {
            Some(b)
                if u64::from(b.width) * u64::from(b.height)
                    >= u64::from(t.width) * u64::from(t.height) =>
            {
                Some(b)
            }
            _ => Some(t),
        })
        .ok_or_else(|| Error::Unsupported("CR3 without a raw image".into()))?;
    let (start, end) = raw.cmp1.ok_or_else(|| corrupt("CR3 image header"))?;
    let header = crx::Header::parse(bytes.get(start..end).unwrap_or_default())?;
    let variant = match header.levels {
        0 => Variant::Cr3Lossless,
        _ => Variant::Cr3Craw,
    };
    let (offset, len) = raw.sample.ok_or_else(|| corrupt("CR3 raw image sample"))?;
    let mut info = crate::info::base(variant);
    let tiff = |i: usize| c.cmt[i].and_then(|b| TiffFile::parse(b, 0));
    let ifd0 = tiff(0).ok_or_else(|| corrupt("CR3 IFD0"))?;
    info.make = ifd0
        .text_upto(&ifd0.ifds[0], 271, NAME_MAX)
        .unwrap_or_default();
    info.model = ifd0
        .text_upto(&ifd0.ifds[0], 272, NAME_MAX)
        .unwrap_or_default();
    info.flip = crate::geometry::flip(ifd0.int(&ifd0.ifds[0], 274).unwrap_or(1));
    super::tiff_exif::read(&ifd0, &mut info);
    if let Some(exif) = tiff(1) {
        super::tiff_exif::read(&exif, &mut info);
    }
    info.raw_width = header.width;
    info.raw_height = header.height;
    let (width, height) = (header.width, header.height);
    // The image areas: the crop, then (in a full-size image's areas) the
    // left and top masked borders and the active area; inclusive corners.
    let areas = raw
        .iad1
        .and_then(|(s, e)| bytes.get(s..e))
        .unwrap_or_default();
    // Four 16-bit corners from word `at` on.
    let rect = |at: usize| -> Option<[u32; 4]> {
        let w = areas.get(at * 2..at * 2 + 8)?;
        let v: [u32; 4] =
            std::array::from_fn(|i| u32::from(u16::from_be_bytes([w[2 * i], w[2 * i + 1]])));
        (v[2] >= v[0] && v[3] >= v[1] && v[2] < width && v[3] < height)
            .then(|| [v[0], v[1], v[2] - v[0] + 1, v[3] - v[1] + 1])
    };
    // The camera table is keyed by RAWmakase's names; it gives the visible
    // area RAWmakase has always read where it is not the active area.
    crate::names::apply(&mut info);
    let fits = |[l, t, w, h]: [u32; 4]| {
        w > 0
            && h > 0
            && l.checked_add(w).is_some_and(|r| r <= width)
            && t.checked_add(h).is_some_and(|b| b <= height)
    };
    let [left, top, w, h] = crate::facts::camera(&info.make, &info.model, false, [width, height])
        .find_map(|c| c.visible)
        .filter(|&v| fits(v))
        .or_else(|| rect(20))
        .unwrap_or([0, 0, width, height]);
    info.margins = [left, top];
    info.width = w;
    info.height = h;
    let cfa = Cfa {
        rows: 2,
        cols: 2,
        colors: match header.cfa {
            0 => vec![0, 1, 1, 2],
            1 => vec![1, 0, 2, 1],
            2 => vec![1, 2, 0, 1],
            _ => vec![2, 1, 1, 0],
        },
    };
    info.cfa = Some(crate::info::second_green(cfa).shifted(top as usize, left as usize));
    if let Some(crop) = rect(8) {
        info.frame = crop;
    }
    if let Some(t) = tiff(2) {
        let notes = &t.ifds[0];
        if let Some([x, y, w, h]) = notes::aspect(&t, notes)
            && info.frame[2] > 0
            && let (Some(x), Some(y)) = (info.frame[0].checked_add(x), info.frame[1].checked_add(y))
        {
            info.aspect = [x, y, w, h];
        }
        info.highlight_tone_priority = notes::highlight_tone_priority(&t, notes);
    }
    let (mut noted_black, mut noted_white) = (None, None);
    if let Some((cam_mul, presets, black, white)) = first_timed_note(bytes, &c.tracks, |note| {
        let t = TiffFile::parse(note, 0)?;
        let (cam_mul, presets) = notes::white_balance(&t, &t.ifds[0])?;
        Some((
            cam_mul,
            presets,
            notes::black(&t, &t.ifds[0]),
            notes::white(&t, &t.ifds[0]),
        ))
    }) {
        info.cam_mul = cam_mul;
        info.presets = presets;
        noted_black = black;
        noted_white = white;
    }
    // Black: ColorData's per-channel levels, which Adobe DNG Converter's
    // BlackLevel follows on our files; without them, each channel's mean in
    // the masked columns left of the image, from the third to 26 short of
    // the visible (or active, if earlier) area, over the visible rows.
    let black = match noted_black {
        Some(levels) => Black::Notes(levels),
        None => {
            let edge = rect(20).map_or(left, |a| a[0].min(left));
            Black::Measured([2, top, edge.saturating_sub(28), h])
        }
    };
    // White: Adobe DNG Converter's level for the cameras whose files it
    // reads unclipped at their specular level (the camera table), else
    // ColorData's SpecularWhiteLevel, else the bit depth's range.
    let range = 1u32
        .checked_shl(u32::from(header.bits))
        .map_or(u32::MAX, |r| r - 1);
    let white = crate::facts::camera(&info.make, &info.model, false, [width, height])
        .find_map(|c| c.white)
        .or(noted_white.filter(|&w| w > 0 && w <= range))
        .unwrap_or(range);
    for t in &c.tracks {
        if t.jpeg
            && let Some((offset, len)) = t.sample
        {
            info.previews.push(crate::preview::Candidate {
                offset,
                len,
                pixels: u64::from(t.width) * u64::from(t.height),
            });
        }
        // HDR shots' previews are HEVC, not JPEG.
        info.uncompressed_preview |= t.hevc;
    }
    info.plan = Plan::Cr3(Cr3Plan {
        offset,
        len,
        header,
        black,
        white,
    });
    Ok(info)
}

/// Decodes the raw image.
pub(crate) fn decode(
    info: &RawInfo,
    plan: &Cr3Plan,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<Sensor> {
    let file = Bytes::new(bytes, crate::io::Order::Big);
    let sample = file.slice(plan.offset, plan.len)?;
    let data = crx::decode(&plan.header, sample, cancel)?;
    let [left, top] = info.margins;
    let black = match plan.black {
        Black::Notes(levels) => levels,
        Black::Measured(area) => mean(&data, plan.header.width, info, area)?,
    };
    Ok(Sensor {
        width: plan.header.width,
        height: plan.header.height,
        channels: 1,
        data,
        area: [left, top, info.width, info.height],
        black,
        black_pattern: (0, 0, Vec::new()),
        white: plan.white,
        linear_white: [0; 4],
    })
}

/// Each channel's mean over `area` (`[left, top, width, height]`), rounded
/// down; a channel the area does not cover is 0.
fn mean(data: &[u16], width: u32, info: &RawInfo, area: [u32; 4]) -> Result<[u32; 4]> {
    let [x0, y0, w, h] = area.map(|v| v as usize);
    let width = width as usize;
    let mut sum = [0u64; 4];
    let mut n = [0u64; 4];
    let Some(cfa) = &info.cfa else {
        return Ok([0; 4]);
    };
    let [left, top] = info.margins.map(|v| v as usize);
    for y in y0..y0 + h {
        let row = data
            .get(y * width + x0..y * width + x0 + w)
            .ok_or_else(|| corrupt("CR3 black area"))?;
        // The filter is described from the visible area's corner.
        let r = (y + 2 - top % 2) % 2;
        for (x, &v) in (x0..).zip(row) {
            let c = usize::from(cfa.color(r, (x + 2 - left % 2) % 2)).min(3);
            sum[c] += u64::from(v);
            n[c] += 1;
        }
    }
    Ok(std::array::from_fn(|c| {
        sum[c].checked_div(n[c]).unwrap_or(0) as u32
    }))
}

#[cfg(test)]
mod tests {
    use super::CANON_UUID;
    use crate::info::UNSET;
    use std::sync::atomic::AtomicBool;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend(kind);
        v.extend(body);
        v
    }

    /// A little-endian TIFF: one directory of (tag, type, value bytes).
    fn tiff(entries: &[(u16, u16, Vec<u8>)]) -> Vec<u8> {
        let mut entries = entries.to_vec();
        entries.sort_by_key(|e| e.0);
        let end = 8 + 2 + 12 * entries.len() + 4;
        let mut out = b"II*\0".to_vec();
        out.extend(8u32.to_le_bytes());
        out.extend((entries.len() as u16).to_le_bytes());
        let mut values: Vec<u8> = Vec::new();
        for (tag, kind, v) in &entries {
            let size = match kind {
                3 => 2,
                4 | 9 => 4,
                _ => 1,
            };
            out.extend(tag.to_le_bytes());
            out.extend(kind.to_le_bytes());
            out.extend(((v.len() / size) as u32).to_le_bytes());
            if v.len() <= 4 {
                let mut inline = v.clone();
                inline.resize(4, 0);
                out.extend(inline);
            } else {
                out.extend(((end + values.len()) as u32).to_le_bytes());
                values.extend(v);
            }
        }
        out.extend(0u32.to_le_bytes());
        out.extend(values);
        out
    }

    fn text(tag: u16, v: &str) -> (u16, u16, Vec<u8>) {
        let mut b = v.as_bytes().to_vec();
        b.push(0);
        (tag, 2, b)
    }

    fn words(tag: u16, kind: u16, v: &[u32]) -> (u16, u16, Vec<u8>) {
        let bytes = match kind {
            3 => v.iter().flat_map(|&x| (x as u16).to_le_bytes()).collect(),
            _ => v.iter().flat_map(|&x| x.to_le_bytes()).collect(),
        };
        (tag, kind, bytes)
    }

    /// A track whose one sample of `size` bytes is at `offset`.
    fn trak(entry: Vec<u8>, offset: u64, size: u32) -> Vec<u8> {
        let mut stsd = vec![0; 4];
        stsd.extend(1u32.to_be_bytes());
        stsd.extend(entry);
        let mut stsz = vec![0; 4];
        stsz.extend(size.to_be_bytes());
        stsz.extend(1u32.to_be_bytes());
        let mut co64 = vec![0; 4];
        co64.extend(1u32.to_be_bytes());
        co64.extend(offset.to_be_bytes());
        let stbl = [bx(b"stsd", &stsd), bx(b"stsz", &stsz), bx(b"co64", &co64)].concat();
        bx(b"trak", &bx(b"mdia", &bx(b"minf", &bx(b"stbl", &stbl))))
    }

    fn craw(w: u16, h: u16, children: &[u8]) -> Vec<u8> {
        let mut b = vec![0; 82];
        b[24..26].copy_from_slice(&w.to_be_bytes());
        b[26..28].copy_from_slice(&h.to_be_bytes());
        b.extend(children);
        bx(b"CRAW", &b)
    }

    /// A 4 × 4 lossless CR3 (`crx::tests::flat`, every sample 8193), its
    /// maker notes in `CMT3` and the timed metadata (ColorData version 33),
    /// and a JPEG or HEVC preview.
    fn cr3(jpeg: bool) -> Vec<u8> {
        cr3_of(jpeg, false)
    }

    /// As `cr3`, or (`craw`) a 16 × 16 C-RAW image (`crx::tests::craw`, every
    /// sample 8198).
    fn cr3_of(jpeg: bool, craw_image: bool) -> Vec<u8> {
        cr3_built(jpeg, craw_image, None, 1)
    }

    /// As `cr3_of`, the timed metadata's sample `timed` when given, named by
    /// `tracks` tracks.
    fn cr3_built(jpeg: bool, craw_image: bool, timed: Option<Vec<u8>>, tracks: usize) -> Vec<u8> {
        let n: u16 = if craw_image { 16 } else { 4 };
        let cmt1 = tiff(&[
            text(271, "Canon"),
            text(272, "Canon EOS R6m2"),
            words(274, 3, &[6]),
        ]);
        let cmt3 = tiff(&[
            // AspectInfo: ratio, 4 × 2 at (0, 1) in the crop.
            words(0x9a, 4, &[7, 4, 2, 0, 1]),
            // LightingOpt: Highlight Tone Priority on.
            words(0x4018, 9, &[0, 0, 0, 1]),
        ]);
        let mut cd = vec![0u32; 820];
        cd[0] = 33;
        cd[85..89].copy_from_slice(&[2048, 1024, 1024, 1536]);
        cd[150..154].copy_from_slice(&[1900, 1024, 1024, 1600]);
        cd[155..159].copy_from_slice(&[2200, 1024, 1024, 1300]);
        cd[343..347].copy_from_slice(&[100, 101, 102, 103]);
        cd[811] = 14008;
        let note = tiff(&[words(0x4001, 3, &cd)]);
        let mut item = ((note.len() + 8) as u32).to_le_bytes().to_vec();
        item.extend(0x927cu32.to_le_bytes());
        item.extend(note);
        let mut ctmd = ((item.len() + 12) as u32).to_le_bytes().to_vec();
        ctmd.extend(8u16.to_le_bytes());
        ctmd.extend([0; 6]);
        ctmd.extend(item);
        let ctmd = timed.unwrap_or(ctmd);
        let mut cmp1 = vec![0u8; 60];
        cmp1[0..2].copy_from_slice(&[0xff, 0]);
        cmp1[4..6].copy_from_slice(&0x100u16.to_be_bytes());
        for at in [8, 12, 16, 20] {
            cmp1[at..at + 4].copy_from_slice(&u32::from(n).to_be_bytes());
        }
        cmp1[24] = 14;
        cmp1[25] = 0x40;
        cmp1[26] = if craw_image { 3 } else { 0 };
        let head: u32 = if craw_image { 540 } else { 108 };
        cmp1[28..32].copy_from_slice(&head.to_be_bytes());
        // Image areas: crop, masked left and top, active (inclusive).
        let m = n - 1;
        let iad: Vec<u8> = [
            0u16, 0, n, n, 1, 2, 1, 0, 0, 0, m, m, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, m, m,
        ]
        .iter()
        .flat_map(|v| v.to_be_bytes())
        .collect();
        let cdi1 = bx(b"CDI1", &[vec![0; 4], bx(b"IAD1", &iad)].concat());
        let sample = if craw_image {
            crate::codec::crx::tests::craw(0x02, 10)
        } else {
            crate::codec::crx::tests::flat()
        };
        let preview = [0xff, 0xd8, 0xff, 0xd9];
        let base = if tracks > 1 { 1u64 << 20 } else { 4096 };
        let canon = bx(
            b"uuid",
            &[CANON_UUID.to_vec(), bx(b"CMT1", &cmt1), bx(b"CMT3", &cmt3)].concat(),
        );
        let codec = if jpeg { b"JPEG" } else { b"HEVC" };
        let moov = [
            canon,
            trak(craw(64, 48, &bx(codec, &[0; 4])), base, 4),
            trak(
                craw(n, n, &[bx(b"CMP1", &cmp1), cdi1].concat()),
                base + 4,
                sample.len() as u32,
            ),
        ]
        .concat();
        let timed_track = trak(
            bx(b"CTMD", &[0; 8]),
            base + 4 + sample.len() as u64,
            ctmd.len() as u32,
        );
        let moov = [moov, timed_track.repeat(tracks)].concat();
        let mut file = bx(b"ftyp", b"crx \0\0\0\x01crx isom");
        file.extend(bx(b"moov", &moov));
        assert!(file.len() < base as usize);
        file.resize(base as usize, 0);
        file.extend(preview);
        file.extend(sample);
        file.extend(ctmd);
        file
    }

    #[test]
    fn reads_the_header_facts_and_the_sensor_data() {
        let file = cr3(true);
        let info = crate::open(&file).unwrap();
        assert_eq!(info.variant, crate::Variant::Cr3Lossless);
        // "EOS R6m2" is the camera table's EOS R6 Mark II.
        assert_eq!(
            (info.make.as_str(), info.model.as_str()),
            ("Canon", "EOS R6 Mark II")
        );
        assert_eq!(
            [info.raw_width, info.raw_height, info.width, info.height],
            [4; 4]
        );
        assert_eq!(info.margins, [0, 0]);
        assert_eq!(info.frame, [0, 0, 4, 4]);
        assert_eq!(info.aspect, [0, 1, 4, 2]);
        assert_eq!((info.flip, info.highlight_tone_priority), (6, 1));
        assert_eq!(info.cam_mul, [2048., 1024., 1536., 1024.]);
        assert_eq!(info.presets.daylight, [1900., 1024., 1600.]);
        assert_eq!(info.presets.shade, [2200., 1024., 1300.]);
        // RGGB, the green beside blue the second.
        assert_eq!(info.cfa.as_ref().unwrap().colors, vec![0, 1, 3, 2]);
        assert_eq!(
            crate::preview(&info, &file).unwrap()[..],
            [0xff, 0xd8, 0xff, 0xd9]
        );
        let s = crate::decode(&info, &file, &AtomicBool::new(false)).unwrap();
        assert_eq!(s.data, vec![8193; 16]);
        assert_eq!(s.area, [0, 0, 4, 4]);
        // The notes' levels red, green, green, blue as red, green, blue,
        // second green.
        assert_eq!(s.black, [100, 101, 103, 102]);
        // ColorData's SpecularWhiteLevel.
        assert_eq!(s.white, 14008);
        assert_eq!(
            crate::decode(&info, &file, &AtomicBool::new(true)).unwrap_err(),
            crate::Error::Cancelled
        );
    }

    #[test]
    fn reads_c_raw() {
        let file = cr3_of(true, true);
        let info = crate::open(&file).unwrap();
        assert_eq!(info.variant, crate::Variant::Cr3Craw);
        assert_eq!([info.width, info.height], [16, 16]);
        let s = crate::decode(&info, &file, &AtomicBool::new(false)).unwrap();
        assert_eq!(s.data, vec![8198; 256]);
        for n in (4096..file.len()).step_by(5) {
            if let Ok(info) = crate::open(&file[..n]) {
                let _ = crate::decode(&info, &file[..n], &AtomicBool::new(false));
            }
        }
    }

    #[test]
    fn many_timed_metadata_tracks_of_one_huge_record_open_quickly() {
        // One record of 2^20 eight-byte maker-note items (no TIFF in them),
        // named by 400 tracks.
        let n = 1usize << 20;
        let mut record = ((12 + 8 * n) as u32).to_le_bytes().to_vec();
        record.extend(8u16.to_le_bytes());
        record.extend([0; 6]);
        for _ in 0..n {
            record.extend(8u32.to_le_bytes());
            record.extend(0x927cu32.to_le_bytes());
        }
        let file = cr3_built(true, false, Some(record), 400);
        let start = std::time::Instant::now();
        let info = crate::open(&file).unwrap();
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        // No white balance found in the bounded search.
        assert_eq!(info.cam_mul, [0.; 4]);
    }

    #[test]
    fn an_hevc_preview_is_not_jpeg() {
        let file = cr3(false);
        let info = crate::open(&file).unwrap();
        assert_eq!(
            crate::preview(&info, &file).unwrap_err(),
            crate::NoPreview::NotJpeg
        );
        assert_eq!(info.aspect[0], 0);
        assert_ne!(info.aspect[0], UNSET);
    }

    #[test]
    fn truncated_or_damaged_files_are_errors_not_panics() {
        let file = cr3(true);
        let cancel = AtomicBool::new(false);
        for n in (0..file.len()).step_by(7) {
            if let Ok(info) = crate::open(&file[..n]) {
                let _ = crate::preview(&info, &file[..n]);
                let decoded = crate::decode(&info, &file[..n], &cancel);
                assert!(n >= 4096 + 4 + 112 || decoded.is_err(), "{n}");
            }
        }
        for i in 0..file.len().min(1200) {
            for v in [0, 1, 0x7f, 0xff] {
                let mut d = file.clone();
                d[i] = v;
                if let Ok(info) = crate::open(&d) {
                    let _ = crate::preview(&info, &d);
                    let _ = crate::decode(&info, &d, &cancel);
                }
            }
        }
    }
}
