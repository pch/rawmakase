use super::{HEAD, Sparse, TAIL, preview_from_file};
use crate::container::nef::tests::nef;
use crate::container::test_tiff::{dir, long};
use crate::io::Order;
use std::io::Cursor;

const LE: Order = Order::Little;

/// A JPEG of `w` × `h` pixels as far as its frame header says, `body` bytes
/// long between that and its end.
fn jpeg(w: u16, h: u16, body: usize) -> Vec<u8> {
    let mut j = vec![0xff, 0xd8, 0xff, 0xc0, 0, 17, 8];
    j.extend(h.to_be_bytes());
    j.extend(w.to_be_bytes());
    j.extend([3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
    j.extend((0..body).map(|i| (i % 251) as u8));
    j.extend([0xff, 0xd9]);
    j
}

/// A small NEF the readers open (a Nikon small RAW), its sensor data followed
/// by `gap` bytes standing in for a camera's, then JPEG previews of `sizes`,
/// then a directory for each, linked from IFD0: at the file's end, or before
/// `after` more bytes of filler.
fn nef_with_previews(gap: usize, sizes: &[([u16; 2], usize)], after: usize) -> Vec<u8> {
    let mut data = Vec::new();
    for _ in 0..8 {
        let x = 1000u64 | 1000 << 12 | 2048 << 24 | 2048 << 36;
        data.extend(&x.to_le_bytes()[..6]);
    }
    let mut f = nef("NIKON D810", 4, 4, 12, 34713, &[], &data);
    f.resize(f.len() + gap + f.len() % 2, 0x55);
    let mut previews = Vec::new();
    for &([w, h], body) in sizes {
        let j = jpeg(w, h, body);
        previews.push((f.len() as u32, j.len() as u32));
        f.extend(j);
        f.resize(f.len() + f.len() % 2, 0);
    }
    let mut links = Vec::new();
    for (at, len) in previews {
        let here = f.len() as u32;
        let d = dir(&[long(LE, 513, &[at]), long(LE, 514, &[len])], here, LE);
        links.push(here);
        f.extend(d);
    }
    // IFD0 links to the first, each to the next.
    let ifd0 = u32::from_le_bytes(f[4..8].try_into().unwrap()) as usize;
    let count = u16::from_le_bytes(f[ifd0..ifd0 + 2].try_into().unwrap()) as usize;
    let mut link_at = ifd0 + 2 + 12 * count;
    for here in links {
        f[link_at..link_at + 4].copy_from_slice(&here.to_le_bytes());
        link_at = here as usize + 2 + 12 * 2;
    }
    f.resize(f.len() + after, 0x55);
    f
}

fn sizes() -> Vec<([u16; 2], usize)> {
    // A Nikon's: a small JPEG, one of 640 pixels and a full-size one.
    vec![
        ([160, 120], 3_000),
        ([640, 424], 40_000),
        ([4928, 3264], 900_000),
    ]
}

#[test]
fn previews_past_the_head_are_read_without_the_rest() {
    let file = nef_with_previews(3 << 20, &sizes(), 0);
    let info = crate::open(&file).unwrap();
    for edge in [Some(640), Some(4000), None] {
        let whole = match edge {
            Some(edge) => crate::preview_reaching(&info, &file, edge),
            None => crate::preview(&info, &file),
        }
        .unwrap();
        let partial = preview_from_file(&mut Cursor::new(&file), edge).unwrap();
        assert_eq!(partial.preview, whole.as_ref(), "edge {edge:?}");
        assert_eq!(partial.info.flip, info.flip);
        assert_eq!(partial.info.variant, info.variant);
        // The head, the tail, each candidate's start and the preview.
        assert!(
            partial.read < HEAD + TAIL + 3 * (64 << 10) + whole.len() as u64,
            "read {} of {}",
            partial.read,
            file.len()
        );
    }
    // The 640-pixel preview is chosen for a 640-pixel thumbnail, not the full-size one.
    let small = preview_from_file(&mut Cursor::new(&file), Some(640)).unwrap();
    assert!(small.preview.len() < 100_000);
    assert!(small.read < file.len() as u64 / 4);
}

#[test]
fn directories_beyond_the_parts_read_are_none_not_another_preview() {
    // The previews' directories are neither near the start nor at the end: the
    // parts read don't list them, and the caller reads the whole file.
    let file = nef_with_previews(3 << 20, &sizes(), 1 << 20);
    assert!(crate::preview(&crate::open(&file).unwrap(), &file).is_ok());
    assert!(preview_from_file(&mut Cursor::new(&file), Some(640)).is_none());
}

#[test]
fn files_that_are_not_raws_are_none() {
    assert!(preview_from_file(&mut Cursor::new(Vec::<u8>::new()), None).is_none());
    let garbage: Vec<u8> = (0..1_000_000u64).map(|i| (i * 7919 % 256) as u8).collect();
    assert!(preview_from_file(&mut Cursor::new(&garbage), None).is_none());
    // A NEF without a JPEG preview has none to read.
    let file = nef_with_previews(0, &[], 0);
    assert!(preview_from_file(&mut Cursor::new(&file), None).is_none());
}

#[test]
fn each_byte_is_read_once() {
    let file: Vec<u8> = (0..10_000u32).map(|i| i as u8).collect();
    let mut cursor = Cursor::new(&file);
    let mut s = Sparse::new(&mut cursor).unwrap();
    s.fill(100, 100).unwrap();
    s.fill(150, 100).unwrap();
    s.fill(50, 300).unwrap();
    s.fill(9_990, 100).unwrap();
    assert_eq!(s.read, 300 + 10);
    assert_eq!(s.filled, [(50, 350), (9_990, 10_000)]);
    assert_eq!(&s.bytes[50..350], &file[50..350]);
    assert!(s.bytes[350..9_990].iter().all(|b| *b == 0));
}

/// Every RAW in `RAWMAKASE_PARTIAL_FIXTURES` (searched recursively) gives the
/// same preview read in pieces as read whole, or `None`; prints what it read.
#[test]
#[ignore = "Set RAWMAKASE_PARTIAL_FIXTURES to a folder of RAWs"]
fn real_files_give_the_same_preview_read_in_pieces() {
    let dir = std::env::var_os("RAWMAKASE_PARTIAL_FIXTURES").expect("RAWMAKASE_PARTIAL_FIXTURES");
    let mut todo = vec![std::path::PathBuf::from(dir)];
    let (mut same, mut whole_only, mut total, mut read) = (0, 0, 0u64, 0u64);
    while let Some(path) = todo.pop() {
        if path.is_dir() {
            todo.extend(
                std::fs::read_dir(&path)
                    .unwrap()
                    .flatten()
                    .map(|e| e.path()),
            );
            continue;
        }
        let Ok(file) = std::fs::read(&path) else {
            continue;
        };
        let Ok(info) = crate::open(&file) else {
            continue;
        };
        for edge in [Some(640), None] {
            let whole = match edge {
                Some(edge) => crate::preview_reaching(&info, &file, edge),
                None => crate::preview(&info, &file),
            };
            let partial = preview_from_file(&mut Cursor::new(&file), edge);
            match (whole, partial) {
                (Ok(w), Some(p)) => {
                    assert_eq!(p.preview, w.as_ref(), "{} edge {edge:?}", path.display());
                    same += 1;
                    if edge.is_some() {
                        total += file.len() as u64;
                        read += p.read;
                    }
                }
                (Ok(_), None) => {
                    eprintln!("read whole: {} ({:?})", path.display(), info.variant);
                    whole_only += 1;
                }
                (Err(_), Some(_)) => panic!("{}: a preview only read in pieces", path.display()),
                (Err(_), None) => {}
            }
        }
    }
    eprintln!(
        "same: {same}, read whole: {whole_only}, thumbnails read {read} of {total} bytes ({:.1} %)",
        read as f64 * 100. / total.max(1) as f64
    );
    assert!(same > 0);
}
