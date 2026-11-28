//! What photos added get from their files where XMP has nothing: the
//! location their camera's EXIF gives.
use crate::catalog::{Catalog, NewPhotoDefaults, PhotoId};
use crate::exif::{CameraExif, gps_fixture};
use crate::metadata::Location;
use anyhow::Result;

const LAT: [(u32, u32); 3] = [(40, 1), (42, 1), (5363, 100)];
const LON: [(u32, u32); 3] = [(73, 1), (59, 1), (5480, 100)];

/// The app's camera EXIF reader, as a test gives it: a GPS position for
/// files named "gps", references alone for "nofix", nothing otherwise.
fn camera_exif(path: &std::path::Path) -> Option<CameraExif> {
    let name = path.file_name()?.to_string_lossy().to_string();
    let gps = if name.contains("gps") {
        gps_fixture::coordinates("N", LAT, "W", LON)
    } else if name.contains("nofix") {
        vec![
            crate::exif::Field::ascii(crate::exif::tag::GPS_LATITUDE_REF, "N"),
            crate::exif::Field::ascii(crate::exif::tag::GPS_LONGITUDE_REF, "W"),
        ]
    } else {
        Vec::new()
    };
    Some(CameraExif {
        main: vec![crate::exif::Field::ascii(crate::exif::tag::MAKE, "Test")],
        gps,
        ..Default::default()
    })
}

fn sidecar(attrs: &str) -> String {
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
        <rdf:Description rdf:about="" xmlns:exif="http://ns.adobe.com/exif/1.0/"
          xmlns:xmp="http://ns.adobe.com/xap/1.0/" {attrs}/></rdf:RDF></x:xmpmeta>"#
    )
}

fn location(cat: &Catalog, name: &str) -> Result<Option<Location>> {
    let id: PhotoId = cat
        .photos()?
        .into_iter()
        .find(|p| p.filename == name)
        .map(|p| p.id)
        .unwrap();
    Ok(cat.descriptive(id)?.location)
}

#[test]
fn a_photo_added_gets_its_cameras_gps_where_its_xmp_has_no_location() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    let w = |name: &str, bytes: &[u8]| std::fs::write(folder.join(name), bytes);
    w("gps.DNG", b"synthetic raw")?;
    // The sidecar's location, newer than the camera's, wins.
    w("gps-moved.NEF", b"synthetic raw")?;
    w(
        "gps-moved.NEF.xmp",
        sidecar(r#"exif:GPSLatitude="50,3.7302N" exif:GPSLongitude="19,56.2140E""#).as_bytes(),
    )?;
    // A sidecar with other metadata only leaves the camera's.
    w("gps-rated.NEF", b"synthetic raw")?;
    w("gps-rated.NEF.xmp", sidecar(r#"xmp:Rating="2""#).as_bytes())?;
    // A sidecar that can't be read may hold a location: none is guessed.
    w("gps-unread.NEF", b"synthetic raw")?;
    w("gps-unread.NEF.xmp", b"<x:xmpmeta><rdf:RDF>")?;
    w("nofix.NEF", b"synthetic raw")?;
    w("plain.NEF", b"synthetic raw")?;
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    // No default Creator or Copyright: the EXIF is read for the location alone.
    let defaults = NewPhotoDefaults::new(Default::default(), camera_exif, |_| None);
    cat.add_folder_with(&folder, &defaults)?;
    let at = |l: Option<Location>| match l {
        Some(Location::At { lat, lon, alt }) => Some((lat, lon, alt)),
        _ => None,
    };
    let (lat, lon, alt) = at(location(&cat, "gps.DNG")?).expect("the camera's GPS");
    assert!((lat - 40.7148972).abs() < 1e-6 && (lon + 73.9985556).abs() < 1e-6);
    assert_eq!(alt, None);
    let (lat, ..) = at(location(&cat, "gps-moved.NEF")?).expect("the sidecar's");
    assert!((lat - 50.06217).abs() < 1e-4, "{lat}");
    assert!(at(location(&cat, "gps-rated.NEF")?).is_some());
    assert_eq!(location(&cat, "gps-unread.NEF")?, None);
    assert_eq!(location(&cat, "nofix.NEF")?, None);
    assert_eq!(location(&cat, "plain.NEF")?, None);
    // Without the app's reader nothing is read.
    let mut other = Catalog::create(&dir.path().join("Other.rawmakase"))?;
    other.add_folder(&folder)?;
    assert_eq!(location(&other, "gps.DNG")?, None);
    Ok(())
}

#[test]
fn read_metadata_from_files_fills_the_cameras_gps_and_keeps_a_cleared_one() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    for name in ["gps-a.NEF", "gps-cleared.NEF", "gps-set.NEF"] {
        std::fs::write(folder.join(name), b"synthetic raw")?;
    }
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    cat.add_folder(&folder)?;
    let id = |cat: &Catalog, name: &str| -> Result<PhotoId> {
        Ok(cat
            .photos()?
            .into_iter()
            .find(|p| p.filename == name)
            .unwrap()
            .id)
    };
    let (a, cleared, set) = (
        id(&cat, "gps-a.NEF")?,
        id(&cat, "gps-cleared.NEF")?,
        id(&cat, "gps-set.NEF")?,
    );
    cat.clear_location(&[cleared])?;
    cat.db_for_tests().execute(
        "INSERT INTO photo_location(photo, lat, lon, alt, cleared) VALUES (?, 1.5, 2.5, NULL, 0)",
        [set.0],
    )?;
    let exif = |p: &std::path::Path| camera_exif(p).and_then(|e| e.location());
    let found: Vec<(PhotoId, Location)> = [a, cleared, set]
        .iter()
        .filter_map(|id| {
            let p = cat.photos().ok()?.into_iter().find(|p| p.id == *id)?;
            Some((*id, exif(&p.path)?))
        })
        .collect();
    assert_eq!(found.len(), 3);
    cat.fill_locations(&found)?;
    assert!(
        matches!(cat.descriptive(a)?.location, Some(Location::At { lat, .. }) if (lat - 40.7148972).abs() < 1e-6)
    );
    // A location removed or given in the catalog stays.
    assert_eq!(cat.descriptive(cleared)?.location, Some(Location::Cleared));
    assert_eq!(
        cat.descriptive(set)?.location,
        Some(Location::At {
            lat: 1.5,
            lon: 2.5,
            alt: None
        })
    );
    Ok(())
}
