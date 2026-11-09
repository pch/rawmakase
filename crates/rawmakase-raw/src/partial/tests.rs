use super::{HEAD, Sparse, TAIL, checked, header_from_file, in_pieces, preview_from_file};
use crate::container::nef::tests::nef;
use crate::container::pef::tests::{note, pef};
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
    let jpegs = sizes
        .iter()
        .map(|&([w, h], body)| jpeg(w, h, body))
        .collect();
    nef_with_jpegs(gap, jpegs, after)
}

/// [`nef_with_previews`] with these JPEGs as its previews.
fn nef_with_jpegs(gap: usize, jpegs: Vec<Vec<u8>>, after: usize) -> Vec<u8> {
    let mut data = Vec::new();
    for _ in 0..8 {
        let x = 1000u64 | 1000 << 12 | 2048 << 24 | 2048 << 36;
        data.extend(&x.to_le_bytes()[..6]);
    }
    let mut f = nef("NIKON D810", 4, 4, 12, 34713, &[], &data);
    f.resize(f.len() + gap + f.len() % 2, 0x55);
    let mut previews = Vec::new();
    for j in jpegs {
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

/// [`jpeg`] with an APP1 segment of `app` bytes before its frame header, as
/// a camera's EXIF with its own thumbnail can be.
fn jpeg_after_app(w: u16, h: u16, app: u16, body: usize) -> Vec<u8> {
    let plain = jpeg(w, h, body);
    let mut j = vec![0xff, 0xd8, 0xff, 0xe1];
    j.extend(app.to_be_bytes());
    j.extend((2..app).map(|i| (i % 7) as u8));
    j.extend(&plain[2..]);
    j
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
    for edge in [640, 4000] {
        let whole = crate::preview_reaching(&info, &file, edge).unwrap();
        let partial = preview_from_file(&mut Cursor::new(&file), edge).unwrap();
        assert_eq!(partial.preview, whole.as_ref(), "edge {edge}");
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
    let small = preview_from_file(&mut Cursor::new(&file), 640).unwrap();
    assert!(small.preview.len() < 100_000);
    assert!(small.read < file.len() as u64 / 4);
}

#[test]
fn directories_beyond_the_parts_read_are_none_not_another_preview() {
    // The previews' directories are neither near the start nor at the end: the
    // parts read don't list them, and the caller reads the whole file.
    let file = nef_with_previews(3 << 20, &sizes(), 1 << 20);
    assert!(crate::preview(&crate::open(&file).unwrap(), &file).is_ok());
    assert!(preview_from_file(&mut Cursor::new(&file), 640).is_none());
}

#[test]
fn files_that_are_not_raws_are_none() {
    assert!(preview_from_file(&mut Cursor::new(Vec::<u8>::new()), 640).is_none());
    let garbage: Vec<u8> = (0..1_000_000u64).map(|i| (i * 7919 % 256) as u8).collect();
    assert!(preview_from_file(&mut Cursor::new(&garbage), 640).is_none());
    // A NEF without a JPEG preview has none to read.
    let file = nef_with_previews(0, &[], 0);
    assert!(preview_from_file(&mut Cursor::new(&file), 640).is_none());
}

#[test]
fn a_preview_smaller_than_asked_for_is_none() {
    // Read in pieces, a reader that misses a larger preview gives a smaller
    // one instead of an error. One smaller than the thumbnail is read whole,
    // where a larger one may be found.
    let file = nef_with_previews(3 << 20, &[([160, 120], 3_000)], 0);
    let info = crate::open(&file).unwrap();
    assert!(crate::preview_reaching(&info, &file, 640).is_ok());
    assert!(preview_from_file(&mut Cursor::new(&file), 640).is_none());
}

#[test]
fn formats_not_checked_against_whole_reads_are_none() {
    // A packed PEF, whose readers haven't been compared with the whole-file
    // read, with a preview well within the parts read.
    let preview = jpeg(640, 480, 10_000);
    let build = |at: u32| {
        pef(
            ["PENTAX", "PENTAX K-TEST"],
            [2, 2, 12, 32773],
            &[0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc],
            vec![long(LE, 513, &[at]), long(LE, 514, &[preview.len() as u32])],
            note(vec![]),
        )
    };
    let mut file = build(build(0).len() as u32);
    file.extend(&preview);
    let info = crate::open(&file).unwrap();
    assert_eq!(info.variant, crate::Variant::PefPacked);
    assert_eq!(
        crate::preview_reaching(&info, &file, 640).unwrap().as_ref(),
        preview
    );
    assert!(preview_from_file(&mut Cursor::new(&file), 640).is_none());
}

#[test]
fn a_frame_header_past_a_candidate_s_start_is_read() {
    // The full-size JPEG's EXIF fills the part of it read first, so its size
    // is only known once more of it is read. The 640-pixel one is chosen, as
    // from the whole file, not the full-size one.
    let jpegs = vec![
        jpeg(640, 424, 40_000),
        jpeg_after_app(4928, 3264, 65_533, 900_000),
    ];
    let file = nef_with_jpegs(3 << 20, jpegs, 0);
    let info = crate::open(&file).unwrap();
    let whole = crate::preview_reaching(&info, &file, 640).unwrap();
    let partial = preview_from_file(&mut Cursor::new(&file), 640).unwrap();
    assert_eq!(partial.preview, whole.as_ref());
    assert!(partial.read < file.len() as u64 / 4);
}

#[test]
fn more_candidates_than_are_read_are_none() {
    // Sixteen full-size JPEGs, then the 640-pixel one the whole file's read
    // chooses: not read before choosing, so the file is read whole.
    let mut sizes = vec![([4928, 3264], 1_000); 16];
    sizes.push(([640, 424], 1_000));
    let file = nef_with_previews(3 << 20, &sizes, 0);
    let info = crate::open(&file).unwrap();
    assert!(crate::preview_reaching(&info, &file, 640).unwrap().len() < 2_000);
    assert!(preview_from_file(&mut Cursor::new(&file), 640).is_none());
}

#[test]
fn a_preview_of_another_shape_is_not_taken_for_the_largest() {
    // The full-size JPEG starts past the head, so its size isn't known when
    // the file is first opened; a 4:3 preview within the head must not be
    // taken for the largest and chosen, where the whole file's read rejects
    // it for its shape.
    let file = nef_with_previews(0, &[([640, 480], 600_000), ([4928, 3264], 900_000)], 0);
    let info = crate::open(&file).unwrap();
    let whole = crate::preview_reaching(&info, &file, 640).unwrap();
    let partial = preview_from_file(&mut Cursor::new(&file), 640).unwrap();
    assert_eq!(partial.preview, whole.as_ref());
}

#[test]
fn a_candidate_read_whole_without_a_frame_header_is_left_out() {
    // A JPEG's start and an EXIF segment, without a frame: the whole file's
    // read leaves it out, and so does the read in pieces.
    let mut stub = vec![0xff, 0xd8, 0xff, 0xe1, 0, 8];
    stub.extend([0; 6]);
    stub.extend([0xff, 0xd9]);
    let jpegs = vec![stub, jpeg(640, 424, 40_000), jpeg(4928, 3264, 900_000)];
    let file = nef_with_jpegs(3 << 20, jpegs, 0);
    let info = crate::open(&file).unwrap();
    let whole = crate::preview_reaching(&info, &file, 640).unwrap();
    let partial = preview_from_file(&mut Cursor::new(&file), 640).unwrap();
    assert_eq!(partial.preview, whole.as_ref());
}

/// The header facts read alone are those read from the whole file: camera,
/// variant, size, crop and orientation, the frame taken from the largest
/// preview's size included; a little of the file is read. This NEF has no
/// exposure or lens tags, so those compare empty: real files are compared by
/// `rawmakase-decode`'s `photo_info_from_the_header_is_the_whole_file_s`.
#[test]
fn header_facts_read_in_pieces_are_the_whole_file_s() {
    let file = nef_with_previews(3 << 20, &sizes(), 0);
    let whole = crate::open(&file).unwrap();
    let partial = header_from_file(&mut Cursor::new(&file)).unwrap();
    let p = &partial.info;
    assert_eq!(
        (
            p.variant,
            &p.make,
            &p.model,
            &p.file_make,
            &p.file_model,
            &p.lens
        ),
        (
            whole.variant,
            &whole.make,
            &whole.model,
            &whole.file_make,
            &whole.file_model,
            &whole.lens
        )
    );
    assert_eq!(
        (p.width, p.height, p.margins, p.frame, p.aspect, p.flip),
        (
            whole.width,
            whole.height,
            whole.margins,
            whole.frame,
            whole.aspect,
            whole.flip
        )
    );
    assert_eq!(
        [p.iso, p.shutter, p.aperture, p.focal, p.focal_35mm],
        [
            whole.iso,
            whole.shutter,
            whole.aperture,
            whole.focal,
            whole.focal_35mm
        ]
    );
    assert_eq!((p.lens_id, p.lens_info), (whole.lens_id, whole.lens_info));
    assert!(partial.read < HEAD + TAIL + 3 * (64 << 10) + 1);
    assert!(header_from_file(&mut Cursor::new(Vec::<u8>::new())).is_none());
}

#[test]
fn headers_of_formats_not_checked_are_none() {
    // The packed PEF of `formats_not_checked_against_whole_reads_are_none`,
    // whose header is well within the parts read.
    let file = pef(
        ["PENTAX", "PENTAX K-TEST"],
        [2, 2, 12, 32773],
        &[0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc],
        vec![],
        note(vec![]),
    );
    assert_eq!(
        crate::open(&file).unwrap().variant,
        crate::Variant::PefPacked
    );
    assert!(header_from_file(&mut Cursor::new(&file)).is_none());
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

/// Every RAW in `RAWMAKASE_PARTIAL_FIXTURES` (searched recursively), of any
/// format, read in pieces and whole for a 640-pixel thumbnail: prints for each
/// variant how many gave the same preview and orientation, how many another
/// and how many only read whole. Fails if a format [`checked`] accepts gives
/// another; a format may be added there once none of its files does.
#[test]
#[ignore = "Set RAWMAKASE_PARTIAL_FIXTURES to a folder of RAWs"]
fn real_files_give_the_same_preview_read_in_pieces() {
    use std::collections::BTreeMap;
    let dir = std::env::var_os("RAWMAKASE_PARTIAL_FIXTURES").expect("RAWMAKASE_PARTIAL_FIXTURES");
    let mut todo = vec![std::path::PathBuf::from(dir)];
    // Per variant: the same, another, read whole.
    let mut tally: BTreeMap<String, [u32; 3]> = BTreeMap::new();
    let mut differ = Vec::new();
    let (mut total, mut read) = (0u64, 0u64);
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
        let Ok(whole) = crate::preview_reaching(&info, &file, 640) else {
            continue;
        };
        let counts = tally.entry(format!("{:?}", info.variant)).or_default();
        match in_pieces(&mut Cursor::new(&file), 640, |_| true) {
            Some(p) if p.preview == whole.as_ref() && p.info.flip == info.flip => {
                counts[0] += 1;
                total += file.len() as u64;
                read += p.read;
            }
            Some(p) => {
                counts[1] += 1;
                eprintln!("another: {} ({:?})", path.display(), info.variant);
                if checked(info.variant) || checked(p.info.variant) {
                    differ.push(path.display().to_string());
                }
            }
            None => counts[2] += 1,
        }
    }
    for (variant, [same, other, whole]) in &tally {
        eprintln!("{variant}: same {same}, another {other}, read whole {whole}");
    }
    eprintln!(
        "read {read} of {total} bytes ({:.1} %) for the same previews",
        read as f64 * 100. / total.max(1) as f64
    );
    assert!(
        differ.is_empty(),
        "another preview in a checked format: {differ:?}"
    );
}
