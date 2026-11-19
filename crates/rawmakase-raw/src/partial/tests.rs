use super::{
    HEAD, MAX_PART, Sparse, TAIL, checked, header_from_file, in_pieces, largest_in_pieces,
    largest_preview_from_file, preview_from_file,
};
use crate::container::cr2::tests::{Entry, put_ifd, shorts};
use crate::container::nef::tests::nef;
use crate::container::raf::tests::{block, entries, raf_with_jpeg, records};
use crate::container::test_tiff::{E, dir, long, short, tiff};
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
fn directories_beyond_the_start_and_the_end_are_followed() {
    // The previews' directories are neither near the start nor at the end,
    // as an older PEF's or a Nikon 1's: each is read where the link before
    // it says, not taken for a file with fewer previews.
    let file = nef_with_previews(3 << 20, &sizes(), 1 << 20);
    let info = crate::open(&file).unwrap();
    for edge in [640, 4000] {
        let whole = crate::preview_reaching(&info, &file, edge).unwrap();
        let partial = preview_from_file(&mut Cursor::new(&file), edge).unwrap();
        assert_eq!(partial.preview, whole.as_ref(), "edge {edge}");
        assert!(partial.read < HEAD + TAIL + 4 * (64 << 10) + whole.len() as u64);
    }
    let header = header_from_file(&mut Cursor::new(&file)).unwrap();
    assert_eq!(facts(&header.info), facts(&info));
}

#[test]
fn a_directory_cut_by_the_end_of_the_start_is_read_whole() {
    // The first preview's directory starts ten bytes before the end of the
    // part read first: its count reads, its entries would read as zeros.
    let first_link = |file: &[u8]| {
        let ifd0 = u32::from_le_bytes(file[4..8].try_into().unwrap()) as usize;
        let count = u16::from_le_bytes(file[ifd0..ifd0 + 2].try_into().unwrap()) as usize;
        let link = ifd0 + 2 + 12 * count;
        u32::from_le_bytes(file[link..link + 4].try_into().unwrap()) as usize
    };
    let small = [([160, 120], 3_000), ([640, 424], 40_000)];
    let unmoved = first_link(&nef_with_previews(0, &small, 1 << 20));
    let file = nef_with_previews(HEAD as usize - 10 - unmoved, &small, 1 << 20);
    assert_eq!(first_link(&file), HEAD as usize - 10);
    let info = crate::open(&file).unwrap();
    assert_eq!(info.previews.len(), 2);
    let whole = crate::preview_reaching(&info, &file, 640).unwrap();
    let partial = preview_from_file(&mut Cursor::new(&file), 640).unwrap();
    assert_eq!(partial.preview, whole.as_ref());
    assert_eq!(partial.info.previews, info.previews);
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

/// An EOS-1D's TIF with a 640-pixel preview after its directories, well
/// within the parts read. Its variant is not on the list: none of the
/// archive's has a preview to compare.
fn canon_tif_with_a_preview() -> (Vec<u8>, Vec<u8>) {
    let preview = jpeg(640, 480, 10_000);
    let long = |v: usize| (v as u32).to_le_bytes().to_vec();
    let build = |at: usize| {
        let more = [(513, 4, 1, long(at)), (514, 4, 1, long(preview.len()))];
        crate::container::cr2::tests::canon_tif_with(&more).0
    };
    let mut file = build(build(0).len());
    file.extend(&preview);
    (file, preview)
}

#[test]
fn formats_not_checked_against_whole_reads_are_none() {
    // A variant the list of those compared with whole-file reads refuses is
    // read whole, though its preview and header are within the parts read.
    let (file, preview) = canon_tif_with_a_preview();
    let info = crate::open(&file).unwrap();
    assert_eq!(info.variant, crate::Variant::CanonTif);
    assert!(!checked(info.variant));
    assert_eq!(
        crate::preview_reaching(&info, &file, 640).unwrap().as_ref(),
        preview
    );
    assert!(preview_from_file(&mut Cursor::new(&file), 640).is_none());
    assert!(largest_preview_from_file(&mut Cursor::new(&file)).is_none());
    assert!(header_from_file(&mut Cursor::new(&file)).is_none());
    // Asked for any variant, each is read in pieces as from the whole file.
    let read = in_pieces(&mut Cursor::new(&file), 640, |_| true).unwrap();
    assert_eq!(read.preview, preview);
    assert_eq!(facts(&read.info), facts(&info));
    let largest = largest_in_pieces(&mut Cursor::new(&file), |_| true).unwrap();
    assert_eq!(largest.preview, preview);
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
    assert_eq!(facts(&partial.info), facts(&whole));
    assert!(partial.read < HEAD + TAIL + 3 * (64 << 10) + 1);
    assert!(header_from_file(&mut Cursor::new(Vec::<u8>::new())).is_none());
}

/// The header facts photo info and thumbnails are made of, and the levels
/// a reader takes from its directories, for comparing a read in pieces with
/// the whole file's.
fn facts(info: &crate::RawInfo) -> impl PartialEq + std::fmt::Debug + '_ {
    (
        (
            info.variant,
            &info.make,
            &info.model,
            &info.file_make,
            &info.file_model,
            &info.lens,
        ),
        (
            info.width,
            info.height,
            info.margins,
            info.frame,
            info.aspect,
            info.flip,
        ),
        [
            info.iso,
            info.shutter,
            info.aperture,
            info.focal,
            info.focal_35mm,
        ],
        (info.lens_id, info.lens_info),
        (info.cam_mul, info.image_size),
    )
}

/// [`jpeg`] with a camera's EXIF before its frame header: turned (an
/// orientation of 6), at ISO 400 and 1/250 s.
fn jpeg_with_exif(w: u16, h: u16, body: usize) -> Vec<u8> {
    let exposure = [1u32.to_le_bytes(), 250u32.to_le_bytes()].concat();
    let exif = tiff(
        b"II*\0",
        LE,
        vec![short(LE, 274, &[6])],
        vec![E(33434, 5, exposure), short(LE, 34855, &[400])],
        &[],
    );
    let plain = jpeg(w, h, body);
    let mut j = vec![0xff, 0xd8, 0xff, 0xe1];
    j.extend((2 + 6 + exif.len() as u16).to_be_bytes());
    j.extend(b"Exif\0\0");
    j.extend(exif);
    j.extend(&plain[2..]);
    j
}

/// A RAF as a camera lays it out: the header, a JPEG of `jpeg` bytes after
/// its frame header with the EXIF, the records, then the raw data, a TIFF
/// whose directory has `raw` more entries, and 3 MB of 16-bit samples.
fn raf_like_a_camera_s(jpeg: usize, raw: &[(u16, Vec<u32>)]) -> Vec<u8> {
    let mut e = entries(8, 6, 14);
    e.extend(raw.iter().cloned());
    raf_with_jpeg(
        "X-T9",
        &jpeg_with_exif(1920, 1280, jpeg),
        &records(false),
        &block(&e, &vec![0x55; 3 << 20]),
        0,
    )
}

/// A RAF's records and raw data directory follow its embedded JPEG, past
/// the file's start: they are read where the header's pointers say, and its
/// EXIF from the JPEG.
#[test]
fn a_raf_s_records_and_raw_directory_after_its_jpeg_are_read() {
    let file = raf_like_a_camera_s(900_000, &[]);
    let whole = crate::open(&file).unwrap();
    assert_eq!(whole.variant, crate::Variant::RafUnpacked);
    // From the records, the raw directory and the JPEG's EXIF.
    assert_eq!(whole.recommended_crop, Some([2, 3, 6, 4]));
    assert_eq!(whole.cam_mul, [2., 1., 1.5, 1.]);
    assert_eq!((whole.flip, whole.iso), (crate::geometry::flip(6), 400.));
    let header = header_from_file(&mut Cursor::new(&file)).unwrap();
    assert_eq!(facts(&header.info), facts(&whole));
    assert_eq!(header.info.recommended_crop, whole.recommended_crop);
    assert!(
        header.read < HEAD + TAIL + 3 * (64 << 10),
        "{}",
        header.read
    );
    let preview = crate::preview_reaching(&whole, &file, 640).unwrap();
    let partial = preview_from_file(&mut Cursor::new(&file), 640).unwrap();
    assert_eq!(partial.preview, preview.as_ref());
    assert_eq!(facts(&partial.info), facts(&whole));
    assert!(partial.read < file.len() as u64 / 2);
    let largest = largest_preview_from_file(&mut Cursor::new(&file)).unwrap();
    assert_eq!(largest.preview, preview.as_ref());
}

#[test]
fn an_older_raf_s_records_after_its_jpeg_are_read() {
    // Samples right after the records, their size in record 0x100.
    let size = [2u16.to_be_bytes(), 4u16.to_be_bytes()].concat();
    let file = raf_with_jpeg(
        "FinePix S1000",
        &jpeg_with_exif(1600, 1200, 900_000),
        &[(0x100, size)],
        &vec![0x55; 3 << 20],
        0,
    );
    let whole = crate::open(&file).unwrap();
    assert_eq!([whole.raw_width, whole.raw_height], [4, 2]);
    let header = header_from_file(&mut Cursor::new(&file)).unwrap();
    assert_eq!(facts(&header.info), facts(&whole));
    assert!(header.read < HEAD + TAIL + 3 * (64 << 10));
}

#[test]
fn a_raf_s_raw_directory_values_past_its_start_are_read() {
    // A value of 80 KB before the white balances in the raw directory: they
    // are past the part of the raw data read first, and read once the
    // directory says where the samples start.
    let file = raf_like_a_camera_s(900_000, &[(0xf00b, vec![7; 20_000])]);
    let whole = crate::open(&file).unwrap();
    assert_eq!(whole.cam_mul, [2., 1., 1.5, 1.]);
    let header = header_from_file(&mut Cursor::new(&file)).unwrap();
    assert_eq!(facts(&header.info), facts(&whole));
    assert_eq!(header.info.presets.d65, whole.presets.d65);
}

#[test]
fn a_raf_whose_records_are_longer_than_a_part_is_none() {
    let mut r = records(false);
    r.extend((0..20).map(|i| (0x9000 + i, vec![0; 60_000])));
    let file = raf_with_jpeg(
        "X-T9",
        &jpeg_with_exif(1920, 1280, 900_000),
        &r,
        &block(&entries(8, 6, 14), &vec![0x55; 3 << 20]),
        0,
    );
    const { assert!(20 * 60_000 > MAX_PART) };
    assert!(crate::open(&file).is_ok());
    assert!(header_from_file(&mut Cursor::new(&file)).is_none());
    assert!(preview_from_file(&mut Cursor::new(&file), 640).is_none());
}

/// A CR2 as the older bodies lay it out, each directory after the image
/// before it: IFD0 (with the EXIF and a maker note) and the camera's JPEG
/// `full`, a directory and its thumbnail, a directory and 1 MB standing in
/// for its small uncompressed image, then the directory of the sensor data,
/// `gap` bytes, the 8 × 8 mosaic's strip with `strip` put after its first two
/// bytes, and 3 MB standing in for the rest of a camera's.
fn cr2_of_an_older_body(full: &[u8], gap: usize, strip: &[u8]) -> Vec<u8> {
    let image: Vec<u16> = (0..64).map(|i| i * 40).collect();
    let coded = crate::codec::ljpeg::tests::encode(&image, 8, 12, 1);
    let coded = [&coded[..2], strip, &coded[2..]].concat();
    let thumbnail = jpeg(160, 120, 3_000);
    let long = |v: usize| (v as u32).to_le_bytes().to_vec();
    // SensorInfo (8 × 8, all of it visible) and LensModel.
    let note: [Entry; 2] = [
        (0x00e0, 3, 9, shorts(&[18, 8, 8, 0, 0, 0, 0, 7, 7])),
        (0x0095, 2, 14, b"EF50mm f/1.8\0\0".to_vec()),
    ];
    // Where the JPEG, the thumbnail, the strip and the three later
    // directories land: placed in two passes.
    let mut at = [0usize; 6];
    let mut f = Vec::new();
    for _ in 0..2 {
        f = b"II*\0\x10\0\0\0CR\x02\0\0\0\0\0".to_vec();
        let note_at = put_ifd(&mut f, &note, 0);
        let note_len = f.len() as u32 - note_at;
        // The EXIF directory: the ISO speed and the maker note just written.
        let exif = f.len();
        f.extend(2u16.to_le_bytes());
        f.extend([34855u16.to_le_bytes(), 3u16.to_le_bytes()].concat());
        f.extend([1u32.to_le_bytes(), 400u32.to_le_bytes()].concat());
        f.extend([37500u16.to_le_bytes(), 7u16.to_le_bytes()].concat());
        f.extend([note_len.to_le_bytes(), note_at.to_le_bytes()].concat());
        f.extend(0u32.to_le_bytes());
        let ifd0 = put_ifd(
            &mut f,
            &[
                (256, 3, 1, shorts(&[3000])),
                (257, 3, 1, shorts(&[2000])),
                (271, 2, 6, b"Canon\0".to_vec()),
                (272, 2, 13, b"Canon EOS T1\0".to_vec()),
                (273, 4, 1, long(at[0])),
                (274, 3, 1, shorts(&[6])),
                (279, 4, 1, long(full.len())),
                (34665, 4, 1, long(exif)),
            ],
            at[3] as u32,
        );
        f[4..8].copy_from_slice(&ifd0.to_le_bytes());
        at[0] = f.len();
        f.extend(full);
        at[3] = put_ifd(
            &mut f,
            &[(513, 4, 1, long(at[1])), (514, 4, 1, long(thumbnail.len()))],
            at[4] as u32,
        ) as usize;
        at[1] = f.len();
        f.extend(&thumbnail);
        at[4] = put_ifd(&mut f, &[(259, 3, 1, shorts(&[1]))], at[5] as u32) as usize;
        f.resize(f.len() + (1 << 20), 0x55);
        let raw = [(273, 4, 1, long(at[2])), (279, 4, 1, long(coded.len()))];
        at[5] = put_ifd(&mut f, &raw, 0) as usize;
        f.resize(f.len() + gap, 0x55);
        at[2] = f.len();
        f.extend(&coded);
        f.resize(f.len() + (3 << 20), 0x55);
    }
    f
}

/// An older CR2's later directories each follow an image, megabytes into
/// the file, and the sensor data's size is in its strip's headers: each is
/// read once the directory before it says where it is.
#[test]
fn a_cr2_s_later_directories_and_strip_headers_are_read() {
    let full = jpeg(3000, 2000, 700_000);
    // The strip right after its directory, and 200 KB after it.
    for gap in [0, 200_000] {
        let file = cr2_of_an_older_body(&full, gap, &[]);
        let whole = crate::open(&file).unwrap();
        assert_eq!(whole.variant, crate::Variant::Cr2LosslessJpeg);
        assert_eq!([whole.width, whole.height], [8, 8]);
        assert_eq!((whole.lens.as_str(), whole.iso), ("EF 50mm f/1.8", 400.));
        assert_eq!(whole.previews.len(), 2);
        let header = header_from_file(&mut Cursor::new(&file)).unwrap();
        assert_eq!(facts(&header.info), facts(&whole));
        assert!(
            header.read < HEAD + TAIL + 5 * (64 << 10),
            "{}",
            header.read
        );
        let preview = crate::preview_reaching(&whole, &file, 640).unwrap();
        assert_eq!(preview.as_ref(), full);
        let partial = preview_from_file(&mut Cursor::new(&file), 640).unwrap();
        assert_eq!(partial.preview, preview.as_ref());
        assert_eq!(facts(&partial.info), facts(&whole));
        assert!(partial.read < file.len() as u64 / 2);
    }
}

#[test]
fn a_cr2_whose_strip_headers_are_longer_than_the_part_read_is_none() {
    // Two comments of 40 KB before the strip's tables and frame: read whole
    // they are skipped; cut at the part read, the headers would end in zeros.
    let mut comment = vec![0xff, 0xfe];
    comment.extend(40_000u16.to_be_bytes());
    comment.resize(40_002, b'c');
    let file = cr2_of_an_older_body(&jpeg(3000, 2000, 700_000), 0, &comment.repeat(2));
    assert_eq!([crate::open(&file).unwrap().width, 8], [8, 8]);
    assert!(header_from_file(&mut Cursor::new(&file)).is_none());
    assert!(preview_from_file(&mut Cursor::new(&file), 640).is_none());
    // One comment leaves them within the part read.
    let file = cr2_of_an_older_body(&jpeg(3000, 2000, 700_000), 0, &comment);
    assert!(header_from_file(&mut Cursor::new(&file)).is_some());
}

/// Some Olympus compacts store a JPEG after their two fields of sensor
/// data, listed by no directory: its first bytes are read where the fields
/// end, so that it is found as in the whole file.
#[test]
fn an_orf_s_jpeg_after_its_sensor_data_is_found() {
    use crate::container::orf::tests::orf;
    // 1000 × 1000 samples of 12 bits in two fields from byte 8, each from a
    // 2 KiB boundary, then the JPEG at the next one.
    let field = 500 * 1500usize;
    let at = (8 + field.next_multiple_of(2048) + field).next_multiple_of(2048);
    let mut strip = vec![0x55; at - 8];
    strip.extend(jpeg(640, 480, 100_000));
    let file = orf([1000, 1000], 12, &strip, vec![short(LE, 262, &[2])], None);
    let info = crate::open(&file).unwrap();
    assert_eq!(info.variant, crate::Variant::OrfPacked);
    assert_eq!(info.previews.len(), 1);
    assert_eq!(info.previews[0].offset, at as u64);
    let whole = crate::preview_reaching(&info, &file, 640).unwrap();
    let partial = in_pieces(&mut Cursor::new(&file), 640, |_| true).unwrap();
    assert_eq!(partial.preview, whole.as_ref());
    assert!(partial.read < file.len() as u64 / 2);
}

/// A CR3's white balance and levels are in its timed metadata, after the
/// images: the sample is read where the `moov` box's tables say, not taken
/// for a file without them.
#[test]
fn a_cr3_s_timed_metadata_after_its_images_is_read() {
    // Two timed tracks put the images and the metadata 1 MiB in; 200 KB
    // more stand for the rest of the file.
    let mut file = crate::container::cr3::tests::cr3_built(true, false, None, 2);
    file.resize(file.len() + 200_000, 0);
    let whole = crate::open(&file).unwrap();
    assert_eq!(whole.cam_mul, [2048., 1024., 1536., 1024.]);
    let header = header_from_file(&mut Cursor::new(&file)).unwrap();
    assert_eq!(facts(&header.info), facts(&whole));
    assert_eq!(header.info.presets.d65, whole.presets.d65);
    assert!(header.read < file.len() as u64);
}

/// A NEF is High Efficiency by the first bytes of its sensor data: they are
/// read where the raw image's directory says, not taken for another coding.
#[test]
fn a_nef_s_coding_is_read_from_the_start_of_its_sensor_data() {
    let stream = crate::codec::nikon_he::tests::blank_codestream(64, 64);
    // A maker note record of 600 KB puts the data past the file's start,
    // and 200 KB after it stand for the rest of a camera's.
    let note = [
        (0x003d, 3, shorts(&[1008; 4])),
        (0x0e10, 7, vec![0x55; 600_000]),
    ];
    let mut file = nef("NIKON Z 9", 64, 64, 14, 34713, &note, &stream);
    file.resize(file.len() + 200_000, 0x55);
    let whole = crate::open(&file).unwrap();
    assert_eq!(whole.variant, crate::Variant::NefHighEfficiency);
    let mut cursor = Cursor::new(&file);
    let mut sparse = Sparse::new(&mut cursor).unwrap();
    let read = super::header_in_pieces(&mut sparse, |_| true).unwrap();
    assert_eq!(facts(&read), facts(&whole));
    assert!(sparse.read < file.len() as u64);
}

/// Cut short or damaged, the files whose containers name parts of their own
/// make nothing panic, and the parts asked for end. What is read of them is
/// not looked at: a damaged file may still read, or be read whole.
#[test]
fn truncated_and_damaged_files_do_not_panic() {
    let stream = crate::codec::nikon_he::tests::blank_codestream(64, 64);
    let note = [(0x0e10, 7, vec![0x55; 600_000])];
    let mut high_efficiency = nef("NIKON Z 9", 64, 64, 14, 34713, &note, &stream);
    high_efficiency.resize(high_efficiency.len() + 200_000, 0x55);
    let files = [
        raf_like_a_camera_s(600_000, &[(0xf00b, vec![7; 20_000])]),
        cr2_of_an_older_body(&jpeg(3000, 2000, 600_000), 200_000, &[]),
        crate::container::cr3::tests::cr3_built(true, false, None, 2),
        high_efficiency,
        nef_with_previews(1 << 20, &sizes(), 1 << 20),
    ];
    let read = |file: &[u8]| {
        let _ = crate::variant::header_parts(file);
        let _ = header_from_file(&mut Cursor::new(file));
        let _ = preview_from_file(&mut Cursor::new(file), 640);
    };
    for file in &files {
        // Every length of its first bytes, then lengths across the file.
        let ends = (0..512).chain((1..=64).map(|i| file.len() * i / 64 - 1));
        for end in ends {
            read(&file[..end]);
        }
        // Pointers and counts turned to the largest values: in its first
        // bytes, and at the start of each part its container names in the
        // whole file (its records, directories and values, megabytes in).
        let named = crate::variant::header_parts(file).unwrap_or_default();
        let named = named
            .iter()
            .flat_map(|&(at, _)| (at as usize..at as usize + 32).step_by(4));
        for at in (0..256).step_by(3).chain(named.take(240)) {
            let mut damaged = file.clone();
            let end = (at + 4).min(damaged.len());
            damaged[at.min(end)..end].fill(0xff);
            read(&damaged);
        }
    }
}

/// A SubIFDs entry may name a quarter of a million places that hold no
/// directory: only so many are looked at, in a file read whole or in pieces.
#[test]
fn directories_that_are_not_there_are_followed_up_to_a_limit() {
    let nowhere: Vec<u32> = (0..100_000).map(|i| 0x4000_0000 + 16 * i).collect();
    let file = tiff(b"II*\0", LE, vec![long(LE, 330, &nowhere)], vec![], &[]);
    let t = crate::container::tiff::TiffFile::parse(&file, 0).unwrap();
    assert_eq!(t.ifds.len(), 1);
    // IFD0, its value, and the places looked at.
    let parts = crate::container::tiff::parts(&file, 0, u64::MAX);
    assert!(parts.len() <= 2 + 4 * 64, "{}", parts.len());
    assert!(header_from_file(&mut Cursor::new(&file)).is_none());
}

/// The largest preview read in pieces is the one the whole file gives: past
/// the head, behind a large EXIF segment, and where a smaller one of another
/// shape sits within the head before it (whose size the first open knows,
/// the largest's only once its frame is read).
#[test]
fn the_largest_preview_read_in_pieces_is_the_whole_file_s() {
    let files = [
        nef_with_previews(3 << 20, &sizes(), 0),
        nef_with_jpegs(
            3 << 20,
            vec![
                jpeg(640, 424, 40_000),
                jpeg_after_app(4928, 3264, 65_533, 900_000),
            ],
            0,
        ),
        nef_with_previews(0, &[([640, 480], 600_000), ([4928, 3264], 900_000)], 0),
    ];
    for file in files {
        let info = crate::open(&file).unwrap();
        let whole = crate::preview(&info, &file).unwrap();
        let partial = largest_preview_from_file(&mut Cursor::new(&file)).unwrap();
        assert_eq!(partial.preview, whole.as_ref());
        assert!(partial.read < file.len() as u64);
    }
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
/// format, read in pieces and whole. Its header: prints for each variant how
/// many gave the same header facts (all of [`crate::RawInfo`], the plan for
/// decoding included), how many others and how many only read whole. Its
/// preview for a 640-pixel thumbnail, where it has one: how many gave the
/// same preview, how many another and how many only read whole; and the
/// largest preview, as Develop's placeholder reads it. Then how much of the
/// files was read for the previews and for the headers alone. Fails if a
/// format [`checked`] accepts gives another preview or other header facts; a
/// format may be added there once none of its files does.
#[test]
#[ignore = "Set RAWMAKASE_PARTIAL_FIXTURES to a folder of RAWs"]
fn real_files_give_the_same_preview_read_in_pieces() {
    use std::collections::BTreeMap;
    let dir = std::env::var_os("RAWMAKASE_PARTIAL_FIXTURES").expect("RAWMAKASE_PARTIAL_FIXTURES");
    let mut todo = vec![std::path::PathBuf::from(dir)];
    // Per variant: previews the same, another, read whole; then headers the
    // same, other, read whole.
    let mut tally: BTreeMap<String, [u32; 6]> = BTreeMap::new();
    let mut differ = Vec::new();
    let mut other_facts = Vec::new();
    let mut largest_same = 0;
    let (mut total, mut read) = (0u64, 0u64);
    // What the headers alone take, as photo info reads them.
    let (mut header_total, mut header_read) = (0u64, 0u64);
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
        let counts = tally.entry(format!("{:?}", info.variant)).or_default();
        // The header, of every file that opens, with a preview or without.
        let mut cursor = Cursor::new(&file);
        let mut sparse = Sparse::new(&mut cursor).unwrap();
        match super::header_in_pieces(&mut sparse, |_| true) {
            Some(header) => {
                header_total += file.len() as u64;
                header_read += sparse.read;
                let (pieces, whole) = (format!("{header:#?}"), format!("{info:#?}"));
                if pieces == whole {
                    counts[3] += 1;
                } else {
                    counts[4] += 1;
                    eprintln!(
                        "other header facts: {} ({:?})",
                        path.display(),
                        info.variant
                    );
                    // The first lines that differ.
                    for (a, b) in pieces
                        .lines()
                        .zip(whole.lines())
                        .filter(|(a, b)| a != b)
                        .take(8)
                    {
                        eprintln!("  in pieces: {}\n  whole:     {}", a.trim(), b.trim());
                    }
                    if checked(info.variant) || checked(header.variant) {
                        other_facts.push(path.display().to_string());
                    }
                }
            }
            None => counts[5] += 1,
        }
        let Ok(whole) = crate::preview_reaching(&info, &file, 640) else {
            continue;
        };
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
        // The largest, as Develop's placeholder reads it, any format.
        if let Ok(whole) = crate::preview(&info, &file) {
            match largest_in_pieces(&mut Cursor::new(&file), |_| true) {
                Some(p) if p.preview == whole.as_ref() => largest_same += 1,
                Some(p) => {
                    eprintln!("another largest: {} ({:?})", path.display(), info.variant);
                    if checked(info.variant) || checked(p.info.variant) {
                        differ.push(path.display().to_string());
                    }
                }
                None => {}
            }
        }
    }
    eprintln!("largest previews the same: {largest_same}");
    for (variant, [same, other, whole, facts, other_facts, facts_whole]) in &tally {
        eprintln!(
            "{variant}: same {same}, another {other}, read whole {whole}; \
             headers: same {facts}, other {other_facts}, read whole {facts_whole}"
        );
    }
    eprintln!(
        "read {read} of {total} bytes ({:.1} %) for the same previews",
        read as f64 * 100. / total.max(1) as f64
    );
    eprintln!(
        "read {header_read} of {header_total} bytes ({:.1} %) for the headers",
        header_read as f64 * 100. / header_total.max(1) as f64
    );
    assert!(
        differ.is_empty(),
        "another preview in a checked format: {differ:?}"
    );
    assert!(
        other_facts.is_empty(),
        "other header facts in a checked format: {other_facts:?}"
    );
}
