//! Leaf's private data (IFD0 tag 0x8606, ExifTool's LeafData): packets of
//! "PKTS", a version, a 40-byte name and a big-endian size, then the value;
//! a value that is itself packets nests. Values are text, numbers one to a
//! line. RAWmakase reads the back type (ShootObj_back_type, naming the
//! back), the neutral levels (NeutObj_neutrals), the rotation
//! (ImgProf_rotation_angle), the colour filter (CaptProf_mosaic_pattern),
//! the number of planes, the row order (Rows_data: 1 for the even rows of
//! the image first, then the odd ones bottom up) and the embedded JPEG
//! (JPEG_preview_data).
//!
//! Sources: ExifTool's Leaf tag documentation (the packet names), and our
//! own measurements on the test dataset's files (the packet layout, the
//! filter's colour codes, the levels as the white balance).
use crate::io::{Bytes, Order};

/// The most packets read, and how deep they nest.
const MAX_PACKETS: usize = 4096;
const MAX_DEPTH: u32 = 8;
/// The most bytes of a text value read.
const MAX_TEXT: u64 = 256;

/// What Leaf's data says.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Notes {
    pub back_type: Option<u32>,
    pub neutrals: Option<[f64; 4]>,
    pub rotation: Option<u32>,
    /// The colour of each of the 2 × 2 filter's positions, row by row (0
    /// red, 1 green, 2 blue).
    pub cfa: Option<[u8; 4]>,
    pub planes: Option<u32>,
    /// The embedded JPEG: offset in the file and length.
    pub preview: Option<(u64, u64)>,
    /// Rows_data, a big-endian 32-bit value.
    pub rows: Option<u32>,
}

/// The packets of `len` bytes at `at` in `file`, when they start there.
pub(crate) fn read(file: &[u8], at: u64, len: u64) -> Option<Notes> {
    let b = Bytes::new(file, Order::Big);
    if b.slice(at, 4).ok()? != b"PKTS" {
        return None;
    }
    let end = at.checked_add(len)?.min(file.len() as u64);
    let mut notes = Notes::default();
    let mut count = 0;
    walk(&b, at, end, 0, &mut count, &mut notes);
    Some(notes)
}

fn walk(b: &Bytes, mut at: u64, end: u64, depth: u32, count: &mut usize, n: &mut Notes) {
    while at.saturating_add(52) <= end && b.slice(at, 4).ok() == Some(b"PKTS") {
        *count += 1;
        if *count > MAX_PACKETS {
            return;
        }
        let Ok(name) = b.slice(at + 8, 40) else {
            return;
        };
        let name = &name[..name.iter().position(|&c| c == 0).unwrap_or(40)];
        let Ok(size) = b.u32(at + 48) else { return };
        let value = at + 52;
        let next = value.saturating_add(u64::from(size));
        if next > end {
            return;
        }
        if b.slice(value, 4).ok() == Some(b"PKTS") {
            if depth < MAX_DEPTH {
                walk(b, value, next, depth + 1, count, n);
            }
        } else {
            let text = || {
                b.slice(value, u64::from(size).min(MAX_TEXT))
                    .ok()
                    .map(|t| String::from_utf8_lossy(t).into_owned())
            };
            let numbers = || -> Vec<f64> {
                text()
                    .unwrap_or_default()
                    .split(|c: char| c.is_whitespace() || c == '\0')
                    .filter(|s| !s.is_empty())
                    .map_while(|s| s.parse().ok())
                    .collect()
            };
            let int = || {
                numbers()
                    .first()
                    .filter(|v| v.fract() == 0. && (0. ..=f64::from(u32::MAX)).contains(*v))
                    .map(|&v| v as u32)
            };
            match name {
                b"ShootObj_back_type" => n.back_type = int(),
                b"NeutObj_neutrals" => {
                    if let [a, b, c, d, ..] = numbers()[..] {
                        n.neutrals = Some([a, b, c, d]);
                    }
                }
                b"ImgProf_rotation_angle" => n.rotation = int(),
                b"CaptProf_number_of_planes" => n.planes = int(),
                b"CaptProf_mosaic_pattern" => n.cfa = filter(&numbers()),
                b"JPEG_preview_data" => n.preview = Some((value, u64::from(size))),
                b"Rows_data" if size == 4 => n.rows = b.u32(value).ok(),
                _ => {}
            }
        }
        at = next;
    }
}

/// The filter of a mosaic pattern: the colour code of each position, row
/// by row (1 red, 0 blue, 2 and 3 the greens); a Bayer pattern only.
fn filter(codes: &[f64]) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    for (o, &c) in out.iter_mut().zip(codes.get(..4)?) {
        *o = match c as i64 {
            1 => 0,
            2 | 3 => 1,
            0 => 2,
            _ => return None,
        };
    }
    let mut sorted = out;
    sorted.sort();
    let diagonal = (out[0] == 1 && out[3] == 1) || (out[1] == 1 && out[2] == 1);
    (sorted == [0, 1, 1, 2] && diagonal).then_some(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A packet named `name` holding `value`.
    pub(crate) fn packet(name: &str, value: &[u8]) -> Vec<u8> {
        let mut out = b"PKTS\0\0\0\x01".to_vec();
        let mut n = name.as_bytes().to_vec();
        n.resize(40, 0);
        out.extend(n);
        out.extend((value.len() as u32).to_be_bytes());
        out.extend(value);
        out
    }

    #[test]
    fn reads_nested_packets() {
        let inner = [
            packet("ShootObj_back_type", b"12\0"),
            packet("NeutObj_neutrals", b"3270\n2703\n3270\n2232\0"),
            packet("CaptProf_mosaic_pattern", b"2\n0\n1\n3\0"),
        ]
        .concat();
        let data = [
            packet("camera_profile", &inner),
            packet("ImgProf_rotation_angle", b"90\0"),
            packet("JPEG_preview_data", &[0xff, 0xd8, 0xff, 0xd9]),
        ]
        .concat();
        let n = read(&data, 0, data.len() as u64).unwrap();
        assert_eq!(n.back_type, Some(12));
        assert_eq!(n.neutrals, Some([3270., 2703., 3270., 2232.]));
        assert_eq!(n.cfa, Some([1, 2, 0, 1]));
        assert_eq!(n.rotation, Some(90));
        assert_eq!(n.preview, Some((data.len() as u64 - 4, 4)));
        // Cut short: what precedes the cut is read.
        let n = read(&data, 0, 100).unwrap();
        assert_eq!(n.back_type, None);
        assert!(read(b"PKTX", 0, 4).is_none());
        assert_eq!(filter(&[1., 1., 2., 0.]), None);
        assert_eq!(filter(&[1., 3., 2., 0.]), Some([0, 1, 1, 2]));
    }
}
