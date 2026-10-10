use super::*;
use crate::model::recipe::Recipe;
#[test]
fn icc_export_and_sixteen_bit_precision() -> Result<()> {
    use image::ImageDecoder;
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("source.ARW");
    fs::write(&source, b"source")?;
    let image = Rendered {
        width: 1024,
        height: 1,
        pixels: (0..1024).map(|i| [i as f32 / 1023.; 3]).collect(),
    };
    let m = Metadata {
        make: "Sony".into(),
        model: "test".into(),
        iso: 100.,
        aperture: 2.8,
        shutter: 0.01,
        ..Default::default()
    };
    let jpg = dir.path().join("out.jpg");
    export(
        &jpg,
        &source,
        &image,
        &m,
        &ExportOptions::default(),
        Replace::NoClobber,
    )?;
    let mut decoder =
        image::codecs::jpeg::JpegDecoder::new(std::io::BufReader::new(fs::File::open(jpg)?))?;
    assert!(decoder.icc_profile()?.unwrap().len() > 100);
    assert!(
        decoder
            .exif_metadata()?
            .unwrap()
            .windows(4)
            .any(|w| w == b"Sony")
    );
    let tif = dir.path().join("out.tiff");
    export(
        &tif,
        &source,
        &image,
        &m,
        &ExportOptions::default(),
        Replace::NoClobber,
    )?;
    let out = image::open(tif)?.to_rgb16();
    let unique: std::collections::HashSet<_> = out.pixels().map(|p| p[0]).collect();
    assert_eq!(unique.len(), 1024);
    Ok(())
}
#[test]
fn develop_settings_round_trip_through_the_exported_xmp() -> Result<()> {
    use crate::model::recipe::Recipe;
    let m = Metadata {
        make: "Sony".into(),
        model: "ILCE-7M2".into(),
        lens_model: "FE 55mm F1.8 ZA".into(),
        wb: [2., 1., 1.5],
        daylight_wb: [2., 1., 1.5],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        cam_xyz: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    let mut r = Recipe {
        exposure: 0.4,
        contrast: 0.12,
        shadows: -0.3,
        vibrance: 0.25,
        straighten: 1.5,
        crop: [0.1, 0.05, 0.9, 0.95],
        sharpening: 0.4,
        lens_ca: true,
        lens_manual_distortion: -0.42,
        ..Default::default()
    };
    r.hsl[3][0] = 0.26;
    r.hsl[5][1] = -0.72;
    r.grading[0] = [220. / 360., 0.2, -0.1];
    r.effects.clarity = 0.15;
    r.curve.points = vec![[0., 0.1], [0.5, 0.55], [1., 1.]];
    r.curve_saturation = 0.35;
    let identity = [1., 0., 0., 0., 1., 0., 0., 0., 1.];
    r.upright = crate::model::transform::Upright {
        mode: crate::model::transform::UprightMode::Level,
        corrections: vec![
            identity,
            identity,
            identity,
            [1.02, -0.01, 0.005, 0.02, 1.02, -0.02, 0., 0., 1.],
        ],
        lightroom: [("UprightVersion".into(), "151388160".into())].into(),
        ..Default::default()
    };
    let photo = crate::xmp::write::Photo {
        raw_name: "DSC07924.ARW".into(),
        captured: Some("2018:08:26 10:39:33".into()),
        now: "2026-09-27T06:12:22Z".into(),
        rating: 3,
        keywords: vec![crate::xmp::write::KeywordPath::all(vec![
            "coffee & books".into(),
        ])],
        settings: true,
        format: "image/jpeg".into(),
        ..Default::default()
    };
    let packet = crate::xmp::write::packet(&r, &m, &photo);
    assert!(packet.contains("crs:Exposure2012=\"+0.40\""));
    assert!(packet.contains("crs:AutoLateralCA=\"1\""));
    assert!(packet.contains("xmp:CreateDate=\"2018-08-26T10:39:33\""));
    assert!(packet.contains("coffee &amp; books"));
    let preset = crate::xmp::parse(Path::new("export.xmp"), &packet)?;
    let back = preset.apply(&Recipe::default(), &m, &[], None)?;
    for (a, b) in [
        (back.exposure, r.exposure),
        (back.contrast, r.contrast),
        (back.shadows, r.shadows),
        (back.vibrance, r.vibrance),
        (back.straighten, r.straighten),
        (back.sharpening, r.sharpening),
        (back.hsl[3][0], r.hsl[3][0]),
        (back.hsl[5][1], r.hsl[5][1]),
        (back.grading[0][0], r.grading[0][0]),
        (back.grading[0][2], r.grading[0][2]),
        (back.effects.clarity, r.effects.clarity),
        (back.lens_manual_distortion, r.lens_manual_distortion),
        (back.curve_saturation, r.curve_saturation),
    ] {
        assert!((a - b).abs() < 0.006, "{a} != {b}");
    }
    assert_eq!(back.crop, r.crop);
    assert_eq!(back.curve.points.len(), 3);
    assert_eq!(back.upright, r.upright);
    assert!(back.lens_ca);
    // Panel switches: an off panel is written and read back; on ones are left out.
    use crate::model::panels::{Panel, PanelState};
    r.panels.set(Panel::Effects, PanelState::Off);
    let packet = crate::xmp::write::packet(&r, &m, &photo);
    assert!(packet.contains("crs:EnableEffects=\"False\""));
    assert!(!packet.contains("crs:EnableDetail"));
    let back = crate::xmp::parse(Path::new("export.xmp"), &packet)?.apply(
        &Recipe::default(),
        &m,
        &[],
        None,
    )?;
    assert_eq!(back.panels, r.panels);
    Ok(())
}
/// A crop turned and mirrored with the photo is written and read back as it is, and
/// changing the lens corrections afterwards leaves it where it was.
#[test]
fn a_turned_and_mirrored_crop_round_trips_through_xmp() -> Result<()> {
    let m = Metadata {
        width: 300,
        height: 200,
        wb: [2., 1., 1.5],
        daylight_wb: [2., 1., 1.5],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        cam_xyz: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    let mut r = Recipe {
        crop: [0.1, 0.25, 0.6, 0.875],
        straighten: 4.5,
        ..Default::default()
    };
    crate::develop::turn(&mut r, crate::develop::QuarterTurn::Right);
    crate::develop::mirror(&mut r, crate::develop::Mirror::Horizontal);
    assert_eq!(r.crop, [0.25, 0.1, 0.875, 0.6]);
    assert_eq!(r.straighten, -4.5);
    let photo = crate::xmp::write::Photo {
        settings: true,
        format: "image/jpeg".into(),
        ..Default::default()
    };
    let packet = crate::xmp::write::packet(&r, &m, &photo);
    let back = crate::xmp::parse(Path::new("export.xmp"), &packet)?.apply(
        &Recipe::default(),
        &m,
        &[],
        None,
    )?;
    assert_eq!(back.crop, r.crop);
    assert_eq!(back.straighten, r.straighten);
    // A lens preset applied on top keeps the crop and angle.
    let lens = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:LensManualDistortionAmount="20" crs:LensProfileEnable="1"/></rdf:RDF></x:xmpmeta>"#;
    let lensed = crate::xmp::parse(Path::new("lens.xmp"), lens)?.apply(&back, &m, &[], None)?;
    assert_ne!(lensed.lens_manual_distortion, back.lens_manual_distortion);
    assert_eq!(lensed.crop, r.crop);
    assert_eq!(lensed.straighten, r.straighten);
    Ok(())
}
/// With Constrain Crop the settings carry the crop as rendered, which Camera Raw renders
/// as stored, and reading them back renders the same crop.
#[test]
fn constrain_crop_writes_the_crop_as_rendered() -> Result<()> {
    let m = Metadata {
        width: 300,
        height: 200,
        wb: [2., 1., 1.5],
        daylight_wb: [2., 1., 1.5],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        cam_xyz: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    let mut r = Recipe {
        constrain_crop: true,
        crop: [0.1, 0., 1., 0.9],
        ..Default::default()
    };
    r.transform.vertical = 0.5;
    let rendered = crate::develop::Geometry::for_metadata(&m, &r).crop();
    assert_ne!(rendered, r.crop);
    // As the export job writes it.
    let mut photo = crate::xmp::write::Photo {
        settings: true,
        format: "image/jpeg".into(),
        crop: Some(crate::develop::rendered_crop(&r, &m)),
        ..Default::default()
    };
    let packet = crate::xmp::write::packet(&r, &m, &photo);
    assert!(packet.contains("crs:CropConstrainToWarp=\"1\""));
    assert!(packet.contains("crs:HasCrop=\"True\""));
    let back = crate::xmp::parse(Path::new("export.xmp"), &packet)?.apply(
        &Recipe::default(),
        &m,
        &[],
        None,
    )?;
    assert!(back.constrain_crop);
    for (a, b) in back.crop.iter().zip(rendered) {
        assert!((a - b).abs() < 2e-6, "{:?} {rendered:?}", back.crop);
    }
    let again = crate::develop::Geometry::for_metadata(&m, &back).crop();
    for (a, b) in again.iter().zip(back.crop) {
        assert!((a - b).abs() < 2e-6, "{again:?} {:?}", back.crop);
    }
    // Off, the crop is written as it is.
    r.constrain_crop = false;
    photo.crop = Some(crate::develop::rendered_crop(&r, &m));
    let packet = crate::xmp::write::packet(&r, &m, &photo);
    assert!(packet.contains("crs:CropConstrainToWarp=\"0\""));
    assert!(packet.contains("crs:CropLeft=\"0.100000\""));
    Ok(())
}
#[test]
fn jpeg_carries_camera_exif_gps_and_xmp() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("source.ARW");
    fs::write(&source, b"source")?;
    let image = Rendered {
        width: 8,
        height: 4,
        pixels: vec![[0.5; 3]; 32],
    };
    let camera = crate::exif::CameraExif {
        main: vec![
            crate::exif::Field::ascii(0x010f, "SONY"),
            crate::exif::Field::ascii(0x0110, "ILCE-7M2"),
        ],
        exif: vec![
            crate::exif::Field::ascii(0x9003, "2018:08:26 10:39:33"),
            crate::exif::Field::ascii(0xa434, "FE 55mm F1.8 ZA"),
            crate::exif::Field::ascii(0x927c, "maker note"),
        ],
        gps: vec![crate::exif::Field::rational(0x0002, 52, 1)],
    };
    let embed = Embed {
        camera: Some(camera.clone()),
        xmp: Some("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>".into()),
        ..Default::default()
    };
    let jpg = dir.path().join("out.jpg");
    let m = Metadata::default();
    export_with(
        &jpg,
        &source,
        &image,
        &m,
        &ExportOptions::default(),
        &embed,
        Replace::NoClobber,
    )?;
    let bytes = fs::read(&jpg)?;
    let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
    assert!(has(b"2018:08:26 10:39:33"));
    assert!(has(b"FE 55mm F1.8 ZA"));
    assert!(has(b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta"));
    assert!(image::open(&jpg).is_ok());
    let tif = dir.path().join("out.tif");
    let without_location = Embed {
        camera: Some(crate::exif::CameraExif {
            gps: Vec::new(),
            ..camera
        }),
        ..embed
    };
    export_with(
        &tif,
        &source,
        &image,
        &m,
        &ExportOptions::default(),
        &without_location,
        Replace::NoClobber,
    )?;
    assert!(image::open(&tif).is_ok());
    Ok(())
}
#[test]
fn a_tiff_export_writes_camera_text_tiff_cannot_hold_as_is() -> Result<()> {
    // Fujifilm X100F files pad Artist with spaces, and Copyright's photographer and
    // editor parts too, with a NUL between them, which a TIFF ASCII value cannot hold.
    let padded = |tag, text: &[u8]| crate::exif::Field {
        tag,
        kind: crate::tiff::kind::ASCII,
        count: text.len() as u32,
        bytes: text.to_vec(),
    };
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("source.RAF");
    fs::write(&source, b"source")?;
    let image = Rendered {
        width: 8,
        height: 4,
        pixels: vec![[0.5; 3]; 32],
    };
    let embed = Embed {
        camera: Some(crate::exif::CameraExif {
            main: vec![
                crate::exif::Field::ascii(0x010f, "FUJIFILM"),
                padded(0x013b, b"        \0"),
                padded(0x8298, b"   \0    \0"),
                padded(0x0110, b"Caf\xe9\0"),
            ],
            exif: Vec::new(),
            gps: Vec::new(),
        }),
        ..Default::default()
    };
    let tif = dir.path().join("out.tif");
    export_with(
        &tif,
        &source,
        &image,
        &Metadata::default(),
        &ExportOptions::default(),
        &embed,
        Replace::NoClobber,
    )?;
    assert!(image::open(&tif).is_ok());
    let bytes = fs::read(&tif)?;
    assert!(bytes.windows(4).any(|w| w == b"Caf?"));
    Ok(())
}
#[test]
fn capture_times_take_lightroom_form_with_three_digit_subseconds() {
    use crate::exif::lightroom_time;
    let t = |date, sub| lightroom_time(date, sub);
    assert_eq!(
        t("2018:08:26 10:39:33", Some("12")).as_deref(),
        Some("2018-08-26T10:39:33.120")
    );
    assert_eq!(
        t("2018:08:26 10:39:33", Some("1234")).as_deref(),
        Some("2018-08-26T10:39:33.123")
    );
    assert_eq!(
        t("2018:08:26 10:39:33", None).as_deref(),
        Some("2018-08-26T10:39:33.000")
    );
    assert_eq!(
        t("2018-08-26T10:39:33", Some(" 5 ")).as_deref(),
        Some("2018-08-26T10:39:33.500")
    );
    for blank in [
        "",
        "    :  :     :  :  ",
        "0000:00:00 00:00:00",
        "2018:08:26",
    ] {
        assert_eq!(t(blank, None), None, "{blank:?}");
    }
    // Lightroom's own values, with or without a fraction, sort with these.
    let mut times = [
        "2021-06-06T10:00:01",
        "2021-06-06T10:00:00.500",
        "2021-06-06T10:00:00.000",
        "2021-06-06T10:00:00",
    ];
    times.sort();
    assert_eq!(
        times,
        [
            "2021-06-06T10:00:00",
            "2021-06-06T10:00:00.000",
            "2021-06-06T10:00:00.500",
            "2021-06-06T10:00:01",
        ]
    );
}
#[test]
fn capture_time_is_read_from_tiff_and_jpeg_files() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    for (jpeg, name) in [(false, "a.tif"), (true, "b.jpg")] {
        let path = directory.path().join(name);
        std::fs::write(&path, exif::dated_file(jpeg, "2019:05:04 03:02:01", "7"))?;
        assert_eq!(
            crate::exif::capture_time(&path).as_deref(),
            Some("2019-05-04T03:02:01.700"),
            "{name}"
        );
    }
    let undated = directory.path().join("c.jpg");
    std::fs::write(&undated, [0xff, 0xd8, 0xff, 0xd9])?;
    assert_eq!(crate::exif::capture_time(&undated), None);
    Ok(())
}
#[test]
fn photo_info_is_read_from_exif() -> anyhow::Result<()> {
    use crate::exif::Field;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("a.tif");
    let block = exif::tiff_block(crate::exif::CameraExif {
        main: vec![
            Field::ascii(0x010f, "FUJIFILM"),
            Field::ascii(0x0110, "X100F"),
        ],
        exif: vec![
            Field::rational(0x829a, 1, 250),
            Field::rational(0x829d, 28, 10),
            Field::short(0x8827, 400),
            Field::rational(0x920a, 23, 1),
        ],
        gps: Vec::new(),
    });
    let mut bytes = block;
    bytes.resize(512, 0);
    std::fs::write(&path, bytes)?;
    let info = crate::exif::photo_info(&path).unwrap();
    assert_eq!(info.camera.as_deref(), Some("FUJIFILM X100F"));
    assert_eq!(info.exposure_text().as_deref(), Some("1/250 sec at f/2.8"));
    assert_eq!(info.iso_text().as_deref(), Some("ISO 400"));
    assert_eq!(info.focal_text().as_deref(), Some("23 mm"));
    Ok(())
}
#[test]
fn export_refuses_to_destroy_a_raw_the_source_or_an_existing_file() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("source.ARW");
    fs::write(&source, b"source")?;
    let image = Rendered {
        width: 8,
        height: 1,
        pixels: vec![[0.5; 3]; 8],
    };
    let m = Metadata::default();
    let options = ExportOptions::default();
    // Each of these is a refusal, so succeeding at all is the failure.
    let message = |r: anyhow::Result<()>| match r {
        Ok(()) => panic!("export unexpectedly succeeded"),
        Err(e) => e.to_string(),
    };

    // A RAW destination is refused outright, whatever the overwrite flag: these
    // are the pixels being developed, not somewhere to write.
    let raw_target = dir.path().join("out.ARW");
    assert_eq!(
        message(export(
            &raw_target,
            &source,
            &image,
            &m,
            &options,
            Replace::Overwrite
        )),
        "An export cannot overwrite a RAW file"
    );
    assert!(!raw_target.exists());

    // The source photo itself is never a destination. A RAW source trips the
    // check above first, so this uses a source without a RAW extension: the two
    // guards overlap and this is the one behind.
    let plain = dir.path().join("source.bin");
    fs::write(&plain, b"plain")?;
    for replace in [Replace::NoClobber, Replace::Overwrite] {
        let e = message(export(&plain, &plain, &image, &m, &options, replace));
        assert!(e.contains("Cannot overwrite source"), "{e}");
    }
    assert_eq!(fs::read(&plain)?, b"plain");

    // An existing destination needs the overwrite flag, and without it the file
    // is left exactly as it was.
    let target = dir.path().join("out.jpg");
    fs::write(&target, b"previous")?;
    assert_eq!(
        message(export(
            &target,
            &source,
            &image,
            &m,
            &options,
            Replace::NoClobber
        )),
        "Destination already exists"
    );
    assert_eq!(fs::read(&target)?, b"previous");

    // With it, the file is replaced and no temporary is left behind.
    export(&target, &source, &image, &m, &options, Replace::Overwrite)?;
    assert_ne!(fs::read(&target)?, b"previous");
    let mut left: Vec<String> = fs::read_dir(dir.path())?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, ["out.jpg", "source.ARW", "source.bin"]);

    // An extension the encoder does not write is refused before anything is created.
    let png = dir.path().join("out.png");
    assert_eq!(
        message(export(
            &png,
            &source,
            &image,
            &m,
            &options,
            Replace::NoClobber
        )),
        "Export extension must be .jpg, .jpeg, .tif or .tiff"
    );
    assert!(!png.exists());
    Ok(())
}

/// A packet of exactly `len` bytes, its size in a Camera Raw attribute.
fn packet_of(len: usize) -> String {
    let make = |pad: &str| {
        format!(
            "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
             <x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n \
             <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
             <rdf:Description rdf:about=\"\"\n    \
             xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n    \
             xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n    \
             xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n   \
             xmp:Rating=\"3\"\n   \
             crs:Exposure2012=\"+0.40\"\n   \
             crs:Description=\"{pad}\">\n   \
             <dc:subject>\n    <rdf:Bag>\n     <rdf:li>Kraków &amp; more</rdf:li>\n    </rdf:Bag>\n   </dc:subject>\n  \
             </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>"
        )
    };
    let base = make("").len();
    let packet = make(&"a".repeat(len - base));
    assert_eq!(packet.len(), len);
    packet
}

/// The XMP segments of a JPEG: (header, payload after it).
fn xmp_segments(jpeg: &[u8]) -> Vec<(&'static [u8], Vec<u8>)> {
    let headers: [&'static [u8]; 2] = [
        b"http://ns.adobe.com/xap/1.0/\0",
        b"http://ns.adobe.com/xmp/extension/\0",
    ];
    let mut out = Vec::new();
    let mut at = 2;
    while at + 4 <= jpeg.len() && jpeg[at] == 0xff && jpeg[at + 1] != 0xda {
        let len = u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]) as usize;
        let body = &jpeg[at + 4..at + 2 + len];
        if jpeg[at + 1] == 0xe1 {
            for h in headers {
                if let Some(rest) = body.strip_prefix(h) {
                    out.push((h, rest.to_vec()));
                }
            }
        }
        at += 2 + len;
    }
    out
}

fn jpeg_with(xmp: &str) -> Result<Vec<u8>> {
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut jpeg).encode(
        &[0u8; 3],
        1,
        1,
        image::ExtendedColorType::Rgb8,
    )?;
    encode::insert_xmp(jpeg, xmp)
}

#[test]
fn xmp_that_fits_one_segment_is_written_as_it_is() -> Result<()> {
    let packet = packet_of(65_504);
    let segments = xmp_segments(&jpeg_with(&packet)?);
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].1, packet.as_bytes());
    Ok(())
}

#[test]
fn larger_xmp_moves_camera_raw_settings_to_extended_segments() -> Result<()> {
    use md5::{Digest, Md5};
    let packet = packet_of(65_505);
    let jpeg = jpeg_with(&packet)?;
    let segments = xmp_segments(&jpeg);
    let (standard, extended): (Vec<_>, Vec<_>) = segments
        .iter()
        .partition(|(h, _)| h.starts_with(b"http://ns.adobe.com/xap"));
    let standard = String::from_utf8(standard[0].1.clone())?;
    assert!(standard.len() <= 65_504);
    assert!(standard.contains("xmp:Rating=\"3\""));
    assert!(standard.contains("Kraków &amp; more"));
    assert!(!standard.contains("crs:Exposure2012"));
    // Reassembled by offset, the extended part's MD5 is the GUID.
    let mut whole = Vec::new();
    let mut guid = Vec::new();
    for (_, body) in &extended {
        guid = body[..32].to_vec();
        let total = u32::from_be_bytes(body[32..36].try_into()?) as usize;
        let offset = u32::from_be_bytes(body[36..40].try_into()?) as usize;
        whole.resize(total, 0);
        whole[offset..offset + body.len() - 40].copy_from_slice(&body[40..]);
    }
    let digest: String = Md5::digest(&whole)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect();
    assert_eq!(digest.as_bytes(), guid.as_slice());
    assert!(standard.contains(&format!("xmpNote:HasExtendedXMP=\"{digest}\"")));
    let extended = String::from_utf8(whole)?;
    roxmltree::Document::parse(&extended)?;
    assert!(extended.contains("crs:Exposure2012=\"+0.40\""));
    Ok(())
}

/// exiftool, an independent reader, reads the split packet back whole. CI
/// installs it and sets RAWMAKASE_REQUIRE_EXIFTOOL; elsewhere the test is
/// skipped without it.
#[test]
fn exiftool_reads_extended_xmp_back() -> Result<()> {
    let found = std::process::Command::new("exiftool").arg("-ver").output();
    if found.is_err() {
        assert!(
            std::env::var_os("RAWMAKASE_REQUIRE_EXIFTOOL").is_none(),
            "exiftool is required"
        );
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let r = Recipe {
        exposure: 0.4,
        ..Default::default()
    };
    // Enough keywords that, after the Camera Raw settings, the subject
    // moves too.
    let keywords: Vec<String> = (0..6000).map(|i| format!("keyword {i:05}")).collect();
    let photo = crate::xmp::write::Photo {
        rating: 3,
        keywords: keywords
            .iter()
            .map(|k| crate::xmp::write::KeywordPath::all(vec![k.clone()]))
            .collect(),
        settings: true,
        format: "image/jpeg".into(),
        ..Default::default()
    };
    let xmp = crate::xmp::write::packet(&r, &Metadata::default(), &photo);
    assert!(xmp.len() > 65_504);
    let path = dir.path().join("big.jpg");
    std::fs::write(&path, jpeg_with(&xmp)?)?;
    let out = std::process::Command::new("exiftool")
        // -m: every one of the many keywords, not the first thousand.
        .args([
            "-j",
            "-m",
            "-XMP-crs:Exposure2012",
            "-XMP-dc:Subject",
            "-XMP-xmp:Rating",
        ])
        .arg(&path)
        .output()?;
    let read: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let read = &read[0];
    assert_eq!(read["Exposure2012"].as_str(), Some("+0.40"));
    assert_eq!(read["Rating"].as_i64(), Some(3));
    assert_eq!(
        read["Subject"].as_array().map(Vec::len),
        Some(keywords.len())
    );
    Ok(())
}
#[test]
fn descriptive_fields_are_written_as_lightroom_does() -> Result<()> {
    let photo = crate::xmp::write::Photo {
        title: vec![
            ("x-default".into(), "Pier <at> dusk".into()),
            ("pl".into(), "Molo".into()),
        ],
        caption: vec![("x-default".into(), "Two\nlines".into())],
        rights: vec![("x-default".into(), "© Example".into())],
        creators: vec!["Zoë".into(), "A & B".into()],
        keywords: vec![crate::xmp::write::KeywordPath::all(vec![
            "Places".into(),
            "Kraków".into(),
        ])],
        created: Some("2024-05-01T12:30:15.120456+02:00".into()),
        captured: Some("2020:01:01 00:00:00".into()),
        format: "image/jpeg".into(),
        ..Default::default()
    };
    let packet = crate::xmp::write::packet(&Recipe::default(), &Metadata::default(), &photo);
    let doc = roxmltree::Document::parse(&packet)?;
    const DC: &str = "http://purl.org/dc/elements/1.1/";
    let items = |ns: &str, name: &str| -> Vec<(Option<String>, String)> {
        doc.descendants()
            .find(|n| n.has_tag_name((ns, name)))
            .map(|n| {
                n.descendants()
                    .filter(|li| li.has_tag_name("li") || li.tag_name().name() == "li")
                    .map(|li| {
                        (
                            li.attribute(("http://www.w3.org/XML/1998/namespace", "lang"))
                                .map(String::from),
                            li.text().unwrap_or("").to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    assert_eq!(
        items(DC, "title"),
        [
            (Some("x-default".into()), "Pier <at> dusk".into()),
            (Some("pl".into()), "Molo".into())
        ]
    );
    assert_eq!(items(DC, "description")[0].1, "Two\nlines");
    assert_eq!(items(DC, "rights")[0].1, "© Example");
    let creators: Vec<String> = items(DC, "creator").into_iter().map(|i| i.1).collect();
    assert_eq!(creators, ["Zoë", "A & B"]);
    let subject: Vec<String> = items(DC, "subject").into_iter().map(|i| i.1).collect();
    assert_eq!(subject, ["Places", "Kraków"]);
    let paths: Vec<String> = items("http://ns.adobe.com/lightroom/1.0/", "hierarchicalSubject")
        .into_iter()
        .map(|i| i.1)
        .collect();
    assert_eq!(paths, ["Places|Kraków"]);
    assert!(packet.contains("xmp:CreateDate=\"2024-05-01T12:30:15.120456+02:00\""));
    Ok(())
}
/// An exported XMP reads back with the settings it was written with, and names no
/// operators: one engine renders every recipe.
#[test]
fn exported_xmp_reads_back_its_settings_without_operator_markers() -> Result<()> {
    use crate::model::recipe::Recipe;
    let m = Metadata {
        wb: [2., 1., 1.5],
        daylight_wb: [2., 1., 1.5],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        cam_xyz: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    let photo = crate::xmp::write::Photo {
        settings: true,
        format: "image/jpeg".into(),
        ..Default::default()
    };
    let mut r = Recipe::with_profiles(&m, &[]);
    r.sharpening = 0.4;
    r.effects.lens_vignette = -0.3;
    r.effects.grain = 0.4;
    r.effects.clarity = 0.3;
    r.effects.texture = 0.3;
    r.noise_chroma = 0.3;
    r.saturation = -0.3;
    r.vibrance = 0.4;
    // Black & white, so the packet carries the mix.
    r.effects.monochrome = true;
    r.effects.gray_mix[2] = 0.3;
    let packet = crate::xmp::write::packet(&r, &m, &photo);
    for marker in [
        "RAWmakaseOriginal",
        "RAWmakaseMarkers",
        "RAWmakaseWhiteBalanceModel",
    ] {
        assert!(!packet.contains(marker), "{marker}: {packet}");
    }
    let back = crate::xmp::parse(Path::new("export.xmp"), &packet)?.apply(
        &Recipe::default(),
        &m,
        &[],
        None,
    )?;
    let near = |a: f32, b: f32| (a - b).abs() < 1e-2;
    assert!(near(back.sharpening, r.sharpening));
    assert!(near(back.effects.lens_vignette, r.effects.lens_vignette));
    assert!(near(back.effects.grain, r.effects.grain));
    assert!(near(back.effects.clarity, r.effects.clarity));
    assert!(near(back.effects.texture, r.effects.texture));
    assert!(near(back.noise_chroma, r.noise_chroma));
    assert!(near(back.saturation, r.saturation));
    assert!(near(back.vibrance, r.vibrance));
    assert!(near(back.effects.gray_mix[2], r.effects.gray_mix[2]));
    assert!(back.unknown.is_empty(), "{:?}", back.unknown);
    Ok(())
}
#[test]
fn a_temporary_file_cut_off_at_exit_is_removed_and_one_done_with_forgotten() -> Result<()> {
    static WRITING: Writing = Writing(Mutex::new(Vec::new()));
    let dir = tempfile::tempdir()?;
    // A worker still writing when the process ends: its file is never dropped.
    let cut_off = NamedTempFile::new_in(dir.path())?;
    let _unfinished = WRITING.track(cut_off.path());
    let (_, cut_off) = cut_off.keep()?;
    // One done with, either way, is forgotten.
    let done = NamedTempFile::new_in(dir.path())?;
    drop(WRITING.track(done.path()));
    assert_eq!(*WRITING.paths(), std::slice::from_ref(&cut_off));
    WRITING.remove_all();
    assert!(!cut_off.exists());
    assert!(done.path().exists());
    Ok(())
}
