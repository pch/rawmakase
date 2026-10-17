//! Small TIFF files for tests: data blobs first (at known offsets), then
//! IFD0 and an EXIF directory; and lone directories for maker notes.
use crate::io::Order;

/// A directory entry: tag, type and its value's bytes (in the file's order).
pub(crate) struct E(pub u16, pub u16, pub Vec<u8>);

pub(crate) fn shorts(order: Order, v: &[u16]) -> Vec<u8> {
    v.iter()
        .flat_map(|x| match order {
            Order::Little => x.to_le_bytes(),
            Order::Big => x.to_be_bytes(),
        })
        .collect()
}

pub(crate) fn longs(order: Order, v: &[u32]) -> Vec<u8> {
    v.iter()
        .flat_map(|x| match order {
            Order::Little => x.to_le_bytes(),
            Order::Big => x.to_be_bytes(),
        })
        .collect()
}

pub(crate) fn short(order: Order, tag: u16, v: &[u16]) -> E {
    E(tag, 3, shorts(order, v))
}

pub(crate) fn long(order: Order, tag: u16, v: &[u32]) -> E {
    E(tag, 4, longs(order, v))
}

pub(crate) fn ascii(tag: u16, s: &str) -> E {
    let mut b = s.as_bytes().to_vec();
    b.push(0);
    E(tag, 2, b)
}

/// A directory to be placed at `at` (offsets counted from the same origin
/// as `at`), its values longer than four bytes after it.
pub(crate) fn dir(entries: &[E], at: u32, order: Order) -> Vec<u8> {
    let size = |kind: u16| match kind {
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 => 8,
        _ => 1,
    };
    let mut sorted: Vec<&E> = entries.iter().collect();
    sorted.sort_by_key(|e| e.0);
    let mut out = shorts(order, &[sorted.len() as u16]);
    let mut values = Vec::new();
    let mut extra = at + 2 + 12 * sorted.len() as u32 + 4;
    for E(tag, kind, value) in sorted {
        out.extend(shorts(order, &[*tag, *kind]));
        out.extend(longs(order, &[(value.len() / size(*kind)) as u32]));
        if value.len() <= 4 {
            let mut v = value.clone();
            v.resize(4, 0);
            out.extend(v);
        } else {
            out.extend(longs(order, &[extra]));
            let mut v = value.clone();
            v.resize(v.len() + v.len() % 2, 0);
            extra += v.len() as u32;
            values.extend(v);
        }
    }
    out.extend([0; 4]);
    out.extend(values);
    out
}

/// Where each blob lands in a [`tiff`] of these blobs.
pub(crate) fn offsets(blobs: &[&[u8]]) -> Vec<u32> {
    let mut at = 8;
    blobs
        .iter()
        .map(|b| {
            let here = at;
            at += b.len() as u32;
            here
        })
        .collect()
}

/// A TIFF with header `magic`: the blobs, then IFD0 (with `ifd0` and, when
/// `exif` has entries, a pointer to an EXIF directory of them).
pub(crate) fn tiff(
    magic: &[u8; 4],
    order: Order,
    ifd0: Vec<E>,
    exif: Vec<E>,
    blobs: &[&[u8]],
) -> Vec<u8> {
    let mut out = magic.to_vec();
    let start: u32 = 8 + blobs.iter().map(|b| b.len() as u32).sum::<u32>();
    let start = start + start % 2;
    out.extend(longs(order, &[start]));
    for b in blobs {
        out.extend(*b);
    }
    out.resize(start as usize, 0);
    let mut ifd0 = ifd0;
    let has_exif = !exif.is_empty();
    if has_exif {
        ifd0.push(long(order, 34665, &[0]));
    }
    let first = dir(&ifd0, start, order);
    let exif_at = start + first.len() as u32;
    if has_exif {
        ifd0.pop();
        ifd0.push(long(order, 34665, &[exif_at]));
    }
    out.extend(dir(&ifd0, start, order));
    if has_exif {
        out.extend(dir(&exif, exif_at, order));
    }
    out
}

/// Points the entry `tag` (UNDEFINED, `len` bytes, its value out of line)
/// of the last directory in `file` holding it at `at`: a maker note blob.
pub(crate) fn point(file: &mut [u8], order: Order, tag: u16, len: usize, at: u32) {
    let mut entry = shorts(order, &[tag, 7]);
    entry.extend(longs(order, &[len as u32]));
    let pos = file.windows(8).rposition(|w| w == entry).unwrap();
    file[pos + 8..pos + 12].copy_from_slice(&longs(order, &[at]));
}
