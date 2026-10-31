use super::*;
use crate::catalog::PhotoId;
use crate::metadata::{LangAlt, TextField, Value};

fn xmp(description: &str, body: &str) -> String {
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
        <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"
          xmlns:xmp="http://ns.adobe.com/xap/1.0/" {description}>{body}</rdf:Description></rdf:RDF></x:xmpmeta>"#
    )
}
/// A tiny JPEG with `packet` as its XMP.
fn jpeg_with(packet: &str) -> Result<Vec<u8>> {
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut jpeg).encode(
        &[0u8; 3],
        1,
        1,
        image::ExtendedColorType::Rgb8,
    )?;
    let mut payload = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
    payload.extend_from_slice(packet.as_bytes());
    let mut out = jpeg[..2].to_vec();
    out.extend([0xff, 0xe1]);
    out.extend(((payload.len() + 2) as u16).to_be_bytes());
    out.extend(payload);
    out.extend_from_slice(&jpeg[2..]);
    Ok(out)
}
fn title(cat: &Catalog, id: PhotoId) -> Result<Option<Value<LangAlt>>> {
    Ok(cat.descriptive(id)?.title)
}
fn set(text: &str) -> Option<Value<LangAlt>> {
    Some(Value::Set(LangAlt::new(text)))
}
fn id_of(cat: &Catalog, name: &str) -> Result<PhotoId> {
    Ok(cat
        .photos()?
        .into_iter()
        .find(|p| p.filename == name)
        .map(|p| p.id)
        .unwrap())
}

#[test]
fn folder_import_reads_sidecars_by_digikams_names_and_the_jpegs_own_xmp() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    let w = |name: &str, bytes: &[u8]| std::fs::write(folder.join(name), bytes);
    // A RAW with both sidecars: digiKam's NEF.xmp wins.
    w("A.NEF", b"synthetic raw a")?;
    w(
        "A.NEF.xmp",
        xmp(r#"dc:title="From A.NEF.xmp""#, "").as_bytes(),
    )?;
    w("A.xmp", xmp(r#"dc:title="From A.xmp""#, "").as_bytes())?;
    // A RAW+JPEG pair: B.xmp is the RAW's; the JPEG reads its own XMP and
    // B.JPG.xmp, field by field.
    w("B.NEF", b"synthetic raw b")?;
    w("B.xmp", xmp(r#"dc:title="RAW's""#, "").as_bytes())?;
    w(
        "B.JPG",
        &jpeg_with(&xmp(
            r#"dc:title="Embedded" xmp:Rating="2""#,
            "<dc:description><rdf:Alt><rdf:li xml:lang=\"x-default\">Embedded caption</rdf:li></rdf:Alt></dc:description>\
             <dc:rights><rdf:Alt><rdf:li xml:lang=\"x-default\">© Embedded</rdf:li></rdf:Alt></dc:rights>",
        ))?,
    )?;
    w(
        "B.JPG.xmp",
        xmp(
            r#"dc:title="Sidecar""#,
            "<dc:description><rdf:Alt/></dc:description>",
        )
        .as_bytes(),
    )?;
    // A malformed sidecar is reported, and the rest goes on.
    w("C.NEF", b"synthetic raw c")?;
    w("C.NEF.xmp", b"<x:xmpmeta><rdf:RDF>")?;
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    let (added, report) = cat.add_folder_with(&folder, &Default::default())?;
    // Photos are kept by their canonical paths.
    let folder = folder.canonicalize()?;
    assert_eq!(added, 4);
    let (a, b_raw, b_jpg) = (
        id_of(&cat, "A.NEF")?,
        id_of(&cat, "B.NEF")?,
        id_of(&cat, "B.JPG")?,
    );
    assert_eq!(title(&cat, a)?, set("From A.NEF.xmp"));
    assert_eq!(report.ignored, [folder.join("A.xmp")]);
    assert_eq!(title(&cat, b_raw)?, set("RAW's"));
    let jpg = cat.descriptive(b_jpg)?;
    assert_eq!(jpg.title, set("Sidecar"));
    assert_eq!(jpg.caption, Some(Value::Cleared));
    assert_eq!(jpg.copyright, set("© Embedded"));
    assert_eq!(
        cat.photos()?.iter().find(|p| p.id == b_jpg).unwrap().rating,
        2
    );
    assert_eq!(report.unreadable.len(), 1);
    assert_eq!(report.unreadable[0].0, folder.join("C.NEF.xmp"));
    assert_eq!(
        report.summary().as_deref(),
        Some("1 sidecar could not be read · 1 sidecar ignored")
    );
    // Adding the folder again reads nothing of photos already there.
    std::fs::write(folder.join("A.NEF.xmp"), xmp(r#"dc:title="Changed""#, ""))?;
    cat.add_folder_with(&folder, &Default::default())?;
    assert_eq!(title(&cat, a)?, set("From A.NEF.xmp"));
    Ok(())
}

#[test]
fn read_metadata_from_files_overwrites_what_the_file_has_and_skips_copies() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("A.NEF"), b"synthetic raw")?;
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    cat.add_folder(&folder)?;
    let id = id_of(&cat, "A.NEF")?;
    cat.set_text(&[id], TextField::Title, "My edit")?;
    cat.set_text(&[id], TextField::Caption, "My caption")?;
    let copy = cat.create_virtual_copy(id)?;
    std::fs::write(
        folder.join("A.NEF.xmp"),
        xmp(
            r#"dc:title="From the file" xmp:Rating="5""#,
            "<dc:subject><rdf:Bag><rdf:li>Read</rdf:li></rdf:Bag></dc:subject>",
        ),
    )?;
    let report = cat.read_metadata_from_files(&[id, copy])?;
    assert_eq!(report, SidecarReport::default());
    assert_eq!(title(&cat, id)?, set("From the file"));
    // A field the file lacks keeps the edit.
    assert_eq!(cat.descriptive(id)?.caption, set("My caption"));
    assert_eq!(cat.keywords(id)?[0].name, "Read");
    // The copy keeps its own.
    assert_eq!(title(&cat, copy)?, set("My edit"));
    assert!(cat.keywords(copy)?.is_empty());
    Ok(())
}

#[test]
fn utf16_sidecars_are_read() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("A.NEF");
    std::fs::write(&file, b"synthetic raw")?;
    let text = xmp(r#"dc:title="Zażółć""#, "");
    let mut bytes = vec![0xff, 0xfe];
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    std::fs::write(dir.path().join("A.NEF.xmp"), bytes)?;
    let (read, report) = read_file(&file, |_| None);
    assert_eq!(report, SidecarReport::default());
    assert_eq!(read.unwrap().title, set("Zażółć"));
    Ok(())
}

#[test]
fn a_photo_whose_values_cant_be_written_is_reported_and_the_rest_imported() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("A.NEF"), b"synthetic raw a")?;
    std::fs::write(folder.join("B.NEF"), b"synthetic raw b")?;
    cat.add_folder(&folder)?;
    let (a, b) = (id_of(&cat, "A.NEF")?, id_of(&cat, "B.NEF")?);
    let bad = crate::xmp::descriptive::Read {
        // Not a date the catalog can sort by.
        capture: Some(crate::metadata::Capture {
            captured: "garbage".into(),
            subsec: None,
            offset: None,
        }),
        title: set("Bad"),
        ..Default::default()
    };
    let good = crate::xmp::descriptive::Read {
        title: set("Good"),
        ..Default::default()
    };
    let report = cat.apply_file_metadata(
        &[
            (a, folder.join("A.NEF.xmp"), bad),
            (b, folder.join("B.NEF.xmp"), good),
        ],
        crate::catalog::Merge::FillEmpty,
    )?;
    assert_eq!(report.unreadable.len(), 1);
    assert_eq!(title(&cat, a)?, None);
    assert_eq!(title(&cat, b)?, set("Good"));
    Ok(())
}

#[test]
fn jpeg_fill_bytes_before_a_marker_are_skipped() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut jpeg = jpeg_with(&xmp(r#"dc:title="Filled""#, ""))?;
    // Fill bytes before the XMP segment's marker.
    jpeg.splice(2..2, [0xff, 0xff, 0xff]);
    let file = dir.path().join("A.JPG");
    std::fs::write(&file, jpeg)?;
    let (read, _) = read_file(&file, |_| None);
    assert_eq!(read.unwrap().title, set("Filled"));
    Ok(())
}

#[test]
fn defaults_fill_only_what_neither_file_nor_sidecar_has() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("A.NEF"), b"synthetic raw a")?;
    std::fs::write(folder.join("B.NEF"), b"synthetic raw b")?;
    std::fs::write(
        folder.join("B.NEF.xmp"),
        xmp(
            "",
            "<dc:creator><rdf:Seq><rdf:li>From sidecar</rdf:li></rdf:Seq></dc:creator>",
        ),
    )?;
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    let defaults = crate::catalog::NewPhotoDefaults::new(
        crate::catalog::MetadataDefaults {
            creator: "Default Creator".into(),
            copyright: "© Default".into(),
        },
        crate::exif::read,
        |_| None,
    );
    cat.add_folder_with(&folder, &defaults)?;
    let (a, b) = (id_of(&cat, "A.NEF")?, id_of(&cat, "B.NEF")?);
    assert_eq!(
        cat.descriptive(a)?.creator,
        Some(Value::Set(vec!["Default Creator".into()]))
    );
    assert_eq!(cat.descriptive(a)?.copyright, set("© Default"));
    assert_eq!(
        cat.descriptive(b)?.creator,
        Some(Value::Set(vec!["From sidecar".into()]))
    );
    Ok(())
}

/// A little-endian TIFF whose only directory holds `packet` as its XMP (tag 700).
fn tiff_with(packet: &str) -> Vec<u8> {
    let mut tiff = b"II*\0".to_vec();
    tiff.extend(8u32.to_le_bytes());
    tiff.extend(1u16.to_le_bytes());
    tiff.extend(700u16.to_le_bytes());
    tiff.extend(crate::tiff::kind::BYTE.to_le_bytes());
    tiff.extend((packet.len() as u32).to_le_bytes());
    tiff.extend(26u32.to_le_bytes());
    tiff.extend(0u32.to_le_bytes());
    tiff.extend(packet.as_bytes());
    tiff
}
/// A tiny PNG with `packet` as its XMP, in an "XML:com.adobe.xmp" iTXt chunk.
fn png_with(packet: &str) -> Result<Vec<u8>> {
    use image::ImageEncoder as _;
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png).write_image(
        &[0u8; 3],
        1,
        1,
        image::ExtendedColorType::Rgb8,
    )?;
    let mut chunk = b"iTXt".to_vec();
    chunk.extend(b"XML:com.adobe.xmp\0\0\0\0\0");
    chunk.extend(packet.as_bytes());
    let crc = !chunk.iter().fold(!0u32, |mut crc, &byte| {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
        crc
    });
    let iend = png.len() - 12;
    let mut out = png[..iend].to_vec();
    out.extend(((chunk.len() - 4) as u32).to_be_bytes());
    out.extend(chunk);
    out.extend(crc.to_be_bytes());
    out.extend_from_slice(&png[iend..]);
    Ok(out)
}

#[test]
fn a_files_own_xmp_is_read_by_its_extension() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let packet = xmp(r#"dc:title="Inside""#, "");
    let jpeg = jpeg_with(&packet)?;
    let tiff = tiff_with(&packet);
    let png = png_with(&packet)?;
    let title_of = |name: &str, bytes: &[u8]| -> Result<_> {
        let file = dir.path().join(name);
        std::fs::write(&file, bytes)?;
        let (read, report) = read_file(&file, |_| None);
        assert!(report.unreadable.is_empty(), "{name}: {report:?}");
        Ok(read.and_then(|r| r.title))
    };
    for name in ["A.jpg", "B.JPG", "C.jpeg", "D.JPEG"] {
        assert_eq!(title_of(name, &jpeg)?, set("Inside"), "{name}");
    }
    for name in ["E.tif", "F.TIF", "G.tiff", "H.TIFF", "I.dng", "J.DNG"] {
        assert_eq!(title_of(name, &tiff)?, set("Inside"), "{name}");
    }
    for name in ["K.png", "L.PNG"] {
        assert_eq!(title_of(name, &png)?, set("Inside"), "{name}");
    }
    // A compressed one, which the XMP standard does not allow, is passed over.
    let mut compressed = png.clone();
    let flag = compressed
        .windows(18)
        .position(|w| w == b"XML:com.adobe.xmp\0")
        .unwrap()
        + 18;
    compressed[flag] = 1;
    assert_eq!(title_of("K2.png", &compressed)?, None);
    // Nor is a JPEG's or a TIFF's under another extension.
    assert_eq!(title_of("M.webp", &jpeg)?, None);
    assert_eq!(title_of("N.NEF", &tiff)?, None);
    Ok(())
}

fn formats_fixture(name: &str, dir: &Path) -> Result<PathBuf> {
    let from = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/data/formats")
        .join(name);
    let file = dir.join(name);
    std::fs::copy(from, &file)?;
    Ok(file)
}

#[test]
fn webp_avif_heif_and_jxl_files_own_xmp_is_read() -> Result<()> {
    let dir = tempfile::tempdir()?;
    // metadata-brob.jxl has its "xml " box brotli-compressed, as cjxl writes
    // by default.
    for name in [
        "metadata.webp",
        "metadata.avif",
        "metadata.heic",
        "metadata.jxl",
        "metadata-brob.jxl",
    ] {
        let file = formats_fixture(name, dir.path())?;
        let (read, report) = read_file(&file, |_| None);
        assert!(report.unreadable.is_empty(), "{name}: {report:?}");
        assert_eq!(read.and_then(|r| r.title), set("Fixture title"), "{name}");
    }
    Ok(())
}

#[test]
fn an_avifs_xmp_item_over_16_mib_is_not_read() -> Result<()> {
    use rawmakase_interop::isobmff::Heif;
    let dir = tempfile::tempdir()?;
    let file = formats_fixture("metadata.avif", dir.path())?;
    let mut bytes = std::fs::read(&file)?;
    let heif = Heif::read(&mut std::io::Cursor::new(&bytes)).unwrap();
    let xmp = heif
        .describing(*b"cdsc", heif.primary)
        .into_iter()
        .find(|id| heif.item(*id).unwrap().content_type == "application/rdf+xml")
        .unwrap();
    let (offset, len) = heif.extent(xmp).unwrap();
    // Point the XMP item at a copy of its packet at the end of the file,
    // padded with spaces past 16 MiB: still a readable packet, but too big.
    let mut location = (offset as u32).to_be_bytes().to_vec();
    location.extend((len as u32).to_be_bytes());
    let at = bytes
        .windows(8)
        .position(|w| w == location.as_slice())
        .unwrap();
    let big = (16 << 20) + 1;
    let mut packet = bytes[offset as usize..(offset + len) as usize].to_vec();
    packet.resize(big, b' ');
    let end = bytes.len() as u32;
    bytes[at..at + 4].copy_from_slice(&end.to_be_bytes());
    bytes[at + 4..at + 8].copy_from_slice(&(big as u32).to_be_bytes());
    bytes.extend(packet);
    std::fs::write(&file, bytes)?;
    // Skipped unread, as a WebP chunk or JPEG XL box that size is.
    let (read, report) = read_file(&file, |_| None);
    assert!(report.unreadable.is_empty(), "{report:?}");
    assert_eq!(read.and_then(|r| r.title), None);
    Ok(())
}

#[test]
fn webp_avif_heif_and_jxl_files_without_xmp_read_nothing_and_are_not_unreadable() -> Result<()> {
    let dir = tempfile::tempdir()?;
    for name in [
        "animated.webp",
        "ten-bit.avif",
        "grid.heic",
        "sixteen-bit.jxl",
    ] {
        let file = formats_fixture(name, dir.path())?;
        let (read, report) = read_file(&file, |_| None);
        assert!(report.unreadable.is_empty(), "{name}: {report:?}");
        assert_eq!(read.and_then(|r| r.title), None, "{name}");
    }
    Ok(())
}

/// The camera's own Artist and Copyright, as the app's reader finds them
/// (a CR3's, which the catalog can't read itself), keep the defaults out;
/// a camera without them gets the defaults (#671).
#[test]
fn defaults_give_way_to_the_camera_exif_the_apps_reader_finds() -> Result<()> {
    use crate::exif::{CameraExif, Field, tag};
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("A.CR3"), b"synthetic raw a")?;
    std::fs::write(folder.join("B.CR3"), b"synthetic raw b")?;
    std::fs::write(folder.join("C.CR3"), b"synthetic raw c")?;
    fn reader(path: &Path) -> Option<CameraExif> {
        let main = match path.file_name()?.to_str()? {
            "A.CR3" => vec![
                Field::ascii(tag::ARTIST, "Ann Example"),
                Field::ascii(tag::COPYRIGHT, "Ann Example 2024"),
            ],
            // Canon writes them blank when the camera has none.
            "B.CR3" => vec![
                Field::ascii(tag::ARTIST, ""),
                Field::ascii(tag::COPYRIGHT, ""),
            ],
            _ => return None,
        };
        Some(CameraExif {
            main,
            ..Default::default()
        })
    }
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    let defaults = crate::catalog::NewPhotoDefaults::new(
        crate::catalog::MetadataDefaults {
            creator: "Default Creator".into(),
            copyright: "© Default".into(),
        },
        reader,
        |_| None,
    );
    cat.add_folder_with(&folder, &defaults)?;
    let a = cat.descriptive(id_of(&cat, "A.CR3")?)?;
    assert_eq!((a.creator, a.copyright), (None, None));
    for name in ["B.CR3", "C.CR3"] {
        let d = cat.descriptive(id_of(&cat, name)?)?;
        assert_eq!(
            d.creator,
            Some(Value::Set(vec!["Default Creator".into()])),
            "{name}"
        );
        assert_eq!(d.copyright, set("© Default"), "{name}");
    }
    Ok(())
}

/// The XMP a camera embeds in a CR3, as the app's reader finds it, is read
/// like a JPEG's: its rating and title fill a new photo, below a sidecar's
/// fields, and its creator keeps the default out. Read Metadata from Files
/// reads it too; XMP that isn't text is reported (#680).
#[test]
fn a_cr3s_own_xmp_the_apps_reader_finds_is_read_below_its_sidecar() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    for name in ["A.CR3", "B.CR3", "C.CR3", "D.NEF"] {
        std::fs::write(folder.join(name), b"synthetic raw")?;
    }
    std::fs::write(folder.join("B.CR3.xmp"), xmp(r#"xmp:Rating="2""#, ""))?;
    fn reader(path: &Path) -> Option<Vec<u8>> {
        match path.file_name()?.to_str()? {
            "A.CR3" | "B.CR3" => Some(
                xmp(
                    r#"xmp:Rating="4" dc:title="In camera""#,
                    "<dc:creator><rdf:Seq><rdf:li>Camera Owner</rdf:li></rdf:Seq></dc:creator>",
                )
                .into_bytes(),
            ),
            "C.CR3" => Some(vec![0xff, 0xfe, 0xfd]),
            _ => None,
        }
    }
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    let defaults = crate::catalog::NewPhotoDefaults::new(
        crate::catalog::MetadataDefaults {
            creator: "Default Creator".into(),
            copyright: String::new(),
        },
        |_| None,
        reader,
    );
    let (_, report) = cat.add_folder_with(&folder, &defaults)?;
    assert_eq!(
        report.unreadable.iter().map(|(p, _)| p).collect::<Vec<_>>(),
        [&folder.join("C.CR3").canonicalize()?]
    );
    let rating = |name: &str| -> Result<i32> {
        let id = id_of(&cat, name)?;
        Ok(cat
            .photos()?
            .into_iter()
            .find(|p| p.id == id)
            .unwrap()
            .rating)
    };
    let (a, b) = (id_of(&cat, "A.CR3")?, id_of(&cat, "B.CR3")?);
    assert_eq!((rating("A.CR3")?, title(&cat, a)?), (4, set("In camera")));
    assert_eq!(
        cat.descriptive(a)?.creator,
        Some(Value::Set(vec!["Camera Owner".into()]))
    );
    // The sidecar's rating wins; the title it lacks is the camera's.
    assert_eq!((rating("B.CR3")?, title(&cat, b)?), (2, set("In camera")));
    assert_eq!(rating("D.NEF")?, 0);
    // Read Metadata from Files, as the app reads a file.
    let (read, report) = read_file(&folder.join("A.CR3"), reader);
    assert_eq!(report, SidecarReport::default());
    assert_eq!(read.map(|r| r.rating), Some(Some(4)));
    // Without the app's reader, the catalog finds no XMP in a CR3.
    assert_eq!(read_file(&folder.join("A.CR3"), |_| None).0, None);
    Ok(())
}
