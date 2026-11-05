//! The EXIF an export writes into a JPEG: the camera's directories (see
//! `crate::exif`) as one little-endian TIFF block.
use crate::exif::{
    CameraExif, Field,
    tag::{EXIF_IFD, GPS_IFD, INTEROPERABILITY_IFD},
};

/// A little-endian TIFF block for a JPEG APP1 segment: the main directory, then
/// the EXIF and GPS directories it points to.
#[cfg(any(test, feature = "test-support"))]
pub(super) fn tiff_block(d: CameraExif) -> Vec<u8> {
    tiff_block_with(d, &[])
}

/// [`tiff_block`] with an interoperability directory of `interop`'s fields after
/// the EXIF directory, which points to it, when there are any.
pub(super) fn tiff_block_with(d: CameraExif, interop: &[Field]) -> Vec<u8> {
    fn directory(mut fields: Vec<Field>, offset: u32) -> Vec<u8> {
        fields.sort_by_key(|f| f.tag);
        let mut out = (fields.len() as u16).to_le_bytes().to_vec();
        let mut payload = Vec::new();
        let start = offset + 2 + fields.len() as u32 * 12 + 4;
        for f in fields {
            out.extend(f.tag.to_le_bytes());
            out.extend(f.kind.to_le_bytes());
            out.extend(f.count.to_le_bytes());
            let mut bytes = f.bytes;
            if bytes.len() <= 4 {
                bytes.resize(4, 0);
                out.extend(bytes);
            } else {
                out.extend((start + payload.len() as u32).to_le_bytes());
                payload.extend(bytes);
                if payload.len() % 2 != 0 {
                    payload.push(0);
                }
            }
        }
        out.extend(0u32.to_le_bytes());
        out.extend(payload);
        out
    }
    let CameraExif {
        mut main,
        mut exif,
        gps,
    } = d;
    exif.retain(|f| f.tag != INTEROPERABILITY_IFD);
    if !interop.is_empty() {
        exif.push(Field::long(INTEROPERABILITY_IFD, 0));
    }
    let has_exif = !exif.is_empty();
    let has_gps = !gps.is_empty();
    main.retain(|f| f.tag != EXIF_IFD && f.tag != GPS_IFD);
    if has_exif {
        main.push(Field::long(EXIF_IFD, 0));
    }
    if has_gps {
        main.push(Field::long(GPS_IFD, 0));
    }
    // Directories follow each other; the pointers depend only on sizes.
    let exif_at = 8 + directory(main.clone(), 8).len() as u32;
    let interop_at = exif_at + directory(exif.clone(), exif_at).len() as u32;
    for f in &mut exif {
        if f.tag == INTEROPERABILITY_IFD {
            f.bytes = interop_at.to_le_bytes().to_vec();
        }
    }
    let exif_block = directory(exif, exif_at);
    let interop_block = if interop.is_empty() {
        Vec::new()
    } else {
        directory(interop.to_vec(), interop_at)
    };
    let gps_at = exif_at
        + if has_exif {
            (exif_block.len() + interop_block.len()) as u32
        } else {
            0
        };
    for f in &mut main {
        match f.tag {
            EXIF_IFD => f.bytes = exif_at.to_le_bytes().to_vec(),
            GPS_IFD => f.bytes = gps_at.to_le_bytes().to_vec(),
            _ => {}
        }
    }
    let mut out = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
    out.extend(directory(main, 8));
    if has_exif {
        out.extend(exif_block);
        out.extend(interop_block);
    }
    if has_gps {
        out.extend(directory(gps, gps_at));
    }
    out
}

/// A `width` × `height` PNG whose eXIf chunk names its camera and lens,
/// as RAWmakase's PNG exports carry them, for tests.
#[cfg(any(test, feature = "test-support"))]
pub fn camera_png(width: u32, height: u32, camera: (&str, &str), lens: &str) -> Vec<u8> {
    use crate::exif::tag::{LENS_MODEL, MAKE, MODEL};
    let mut info = png::Info::with_size(width, height);
    info.color_type = png::ColorType::Rgb;
    info.bit_depth = png::BitDepth::Eight;
    info.exif_metadata = Some(
        tiff_block(CameraExif {
            main: vec![Field::ascii(MAKE, camera.0), Field::ascii(MODEL, camera.1)],
            exif: vec![Field::ascii(LENS_MODEL, lens)],
            gps: Vec::new(),
        })
        .into(),
    );
    let mut out = Vec::new();
    let mut w = png::Encoder::with_info(&mut out, info)
        .and_then(|e| e.write_header())
        .expect("a PNG header in memory");
    w.write_image_data(&vec![128; (width * height * 3) as usize])
        .expect("PNG data in memory");
    w.finish().expect("a PNG in memory");
    out
}

/// A small TIFF, or a JPEG when `jpeg`, taken at `original` with these
/// subseconds, for tests.
#[cfg(any(test, feature = "test-support"))]
pub fn dated_file(jpeg: bool, original: &str, subseconds: &str) -> Vec<u8> {
    use crate::exif::tag::{DATE_TIME_ORIGINAL, MAKE, SUBSEC_TIME_ORIGINAL};
    let mut exif = vec![Field::ascii(DATE_TIME_ORIGINAL, original)];
    if !subseconds.is_empty() {
        exif.push(Field::ascii(SUBSEC_TIME_ORIGINAL, subseconds));
    }
    let block = tiff_block(CameraExif {
        main: vec![Field::ascii(MAKE, "Test")],
        exif,
        gps: Vec::new(),
    });
    if jpeg {
        let mut out = vec![0xff, 0xd8, 0xff, 0xe1];
        out.extend(((block.len() + 8) as u16).to_be_bytes());
        out.extend(b"Exif\0\0");
        out.extend(block);
        out.extend([0xff, 0xd9]);
        out
    } else {
        block
    }
}

/// The RAW containers whose camera EXIF is not a TIFF at the file's start.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Copy, Debug)]
pub enum Container {
    /// Canon's CR3: `CMT1`, `CMT2` and `CMT4` in Canon's box in `moov`.
    Cr3,
    /// Minolta's MRW: the TIFF in its TTW block.
    Mrw,
    /// Sigma's X3F: its JPEG preview's EXIF.
    X3f,
}

/// A CR3, MRW or X3F holding `camera` where the camera keeps its EXIF, and
/// no sensor data RAWmakase could decode, for tests.
#[cfg(any(test, feature = "test-support"))]
pub fn camera_raw(container: Container, camera: CameraExif) -> Vec<u8> {
    // An ISO base media box.
    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend(kind);
        out.extend(body);
        out
    }
    match container {
        Container::Cr3 => {
            // Each a TIFF whose first directory is IFD0, the EXIF directory
            // or the GPS directory.
            let tiff = |fields: Vec<Field>| {
                tiff_block(CameraExif {
                    main: fields,
                    ..Default::default()
                })
            };
            let mut canon = vec![
                0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b,
                0x6a, 0x48,
            ];
            canon.extend(bx(b"CMT1", &tiff(camera.main)));
            canon.extend(bx(b"CMT2", &tiff(camera.exif)));
            if !camera.gps.is_empty() {
                canon.extend(bx(b"CMT4", &tiff(camera.gps)));
            }
            let mut out = bx(b"ftyp", b"crx \0\0\0\x01crx isom");
            out.extend(bx(b"moov", &bx(b"uuid", &canon)));
            out.extend(bx(b"mdat", &[0; 64]));
            out
        }
        Container::Mrw => {
            let block = |name: &[u8; 4], body: &[u8]| {
                let mut out = name.to_vec();
                out.extend((body.len() as u32).to_be_bytes());
                out.extend(body);
                out
            };
            let mut out = block(b"\0MRM", &block(b"\0TTW", &tiff_block(camera)));
            out.extend([0; 64]);
            out
        }
        Container::X3f => {
            let tiff = tiff_block(camera);
            let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1];
            jpeg.extend(((tiff.len() + 8) as u16).to_be_bytes());
            jpeg.extend(b"Exif\0\0");
            jpeg.extend(tiff);
            jpeg.extend([0xff, 0xd9]);
            // An image section: its mark, then its version, type, format,
            // columns, rows and row size.
            let mut image = b"SECi".to_vec();
            for v in [0x0001_0000u32, 2, 18, 4, 4, 0] {
                image.extend(v.to_le_bytes());
            }
            image.extend(&jpeg);
            x3f(&[(b"IMA2", image)])
        }
    }
}

/// An X3F of an SD9's or SD10's kind: no preview EXIF, its camera and
/// exposure in its property list alone, for tests.
#[cfg(any(test, feature = "test-support"))]
pub fn x3f_with_properties(properties: &[(&str, &str)]) -> Vec<u8> {
    // Each property's name and value offsets into UTF-16 text.
    let mut text: Vec<u16> = Vec::new();
    let mut entries = Vec::new();
    for (name, value) in properties {
        for s in [name, value] {
            entries.extend((text.len() as u32).to_le_bytes());
            text.extend(s.encode_utf16());
            text.push(0);
        }
    }
    // The mark, a version, the count, the text's format (UTF-16), a
    // reserved word and the text's length.
    let mut prop = b"SECp".to_vec();
    for v in [
        0x0001_0000,
        properties.len() as u32,
        0,
        0,
        text.len() as u32,
    ] {
        prop.extend(v.to_le_bytes());
    }
    prop.extend(entries);
    prop.extend(text.iter().flat_map(|c| c.to_le_bytes()));
    x3f(&[(b"PROP", prop)])
}

/// An X3F's header, then `sections` of their types, then the directory.
#[cfg(any(test, feature = "test-support"))]
fn x3f(sections: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut out = b"FOVb".to_vec();
    out.extend(0x0002_0003u32.to_le_bytes());
    out.resize(232, 0);
    let mut entries = Vec::new();
    for (kind, body) in sections {
        entries.extend((out.len() as u32).to_le_bytes());
        entries.extend((body.len() as u32).to_le_bytes());
        entries.extend(*kind);
        out.extend(body);
    }
    let directory = out.len() as u32;
    out.extend(b"SECd");
    out.extend(0x0002_0000u32.to_le_bytes());
    out.extend((sections.len() as u32).to_le_bytes());
    out.extend(entries);
    out.extend(directory.to_le_bytes());
    out
}

/// A CRW (CIFF) whose only record is its TimeStamp, `seconds` since 1970
/// on the camera's clock, for tests.
#[cfg(any(test, feature = "test-support"))]
pub fn dated_crw(seconds: u32) -> Vec<u8> {
    // The header: byte order, its length, "HEAPCCDR", a version and a
    // reserved word.
    let mut out = b"II".to_vec();
    out.extend(26u32.to_le_bytes());
    out.extend(b"HEAPCCDR");
    out.extend([0; 12]);
    // The heap: the TimeStamp (seconds, time zone, its information), then
    // the directory, one entry: the record's ID, size and offset.
    let heap = out.len();
    for v in [seconds, 0, 0] {
        out.extend(v.to_le_bytes());
    }
    let directory = (out.len() - heap) as u32;
    out.extend(1u16.to_le_bytes());
    out.extend(0x180eu16.to_le_bytes());
    out.extend(12u32.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(directory.to_le_bytes());
    out
}
