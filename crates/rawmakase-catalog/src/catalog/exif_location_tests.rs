//! The location a camera's EXIF GPS gives a photo: read in the background
//! with its capture time once it is added, never on adding it, and by Read
//! Metadata from Files, each only where the catalog has no location.
use crate::catalog::{Catalog, Merge, NewPhotoDefaults, PhotoId};
use crate::exif::{CameraExif, gps_fixture};
use crate::metadata::Location;
use anyhow::Result;

const LAT: [(u32, u32); 3] = [(40, 1), (42, 1), (5363, 100)];
const LON: [(u32, u32); 3] = [(73, 1), (59, 1), (5480, 100)];

/// A camera's EXIF with a GPS position.
fn camera_exif(_: &std::path::Path) -> Option<CameraExif> {
    Some(CameraExif {
        main: vec![crate::exif::Field::ascii(crate::exif::tag::MAKE, "Test")],
        gps: gps_fixture::coordinates("N", LAT, "W", LON),
        ..Default::default()
    })
}
fn camera_location() -> Location {
    camera_exif(std::path::Path::new(""))
        .unwrap()
        .location()
        .unwrap()
}

fn photos(names: &[&str]) -> Result<(tempfile::TempDir, Catalog, Vec<PhotoId>)> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    for name in names {
        std::fs::write(folder.join(name), b"synthetic raw")?;
    }
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    // The app's EXIF reader: what adding reads of it is for the default
    // Creator and Copyright alone.
    let defaults = NewPhotoDefaults::new(
        crate::catalog::MetadataDefaults {
            creator: "Default Creator".into(),
            copyright: String::new(),
        },
        camera_exif,
        |_| None,
    );
    cat.add_folder_with(&folder, &defaults)?;
    let ids = names
        .iter()
        .map(|name| {
            cat.photos()
                .unwrap()
                .into_iter()
                .find(|p| p.filename == *name)
                .unwrap()
                .id
        })
        .collect();
    Ok((dir, cat, ids))
}

#[test]
fn adding_a_photo_leaves_its_cameras_gps_to_the_background_read() -> Result<()> {
    let (_dir, mut cat, ids) = photos(&["a.DNG", "b.DNG", "c.DNG"])?;
    let (a, b, c) = (ids[0], ids[1], ids[2]);
    // Adding read no location, as before the camera's GPS was read.
    assert_eq!(cat.descriptive(a)?.location, None);
    let copy = cat.create_virtual_copy(a)?;
    // Removed, or given from elsewhere, meanwhile.
    cat.clear_location(&[b])?;
    let given = Location::At {
        lat: 1.5,
        lon: 2.5,
        alt: None,
    };
    cat.db_for_tests().execute(
        "INSERT INTO photo_location(photo, lat, lon, alt, cleared) VALUES (?, 1.5, 2.5, NULL, 0)",
        [c.0],
    )?;
    let reads = [(a, None), (b, None), (c, None)];
    let locations: Vec<_> = [a, b, c]
        .iter()
        .map(|id| (*id, camera_location()))
        .collect();
    let located = cat.record_capture_reads(&reads, &locations)?;
    assert_eq!(located, [a, copy]);
    assert_eq!(cat.descriptive(a)?.location, Some(camera_location()));
    // Its virtual copy, made before, gets it too, as it gets its date.
    assert_eq!(cat.descriptive(copy)?.location, Some(camera_location()));
    assert_eq!(cat.descriptive(b)?.location, Some(Location::Cleared));
    assert_eq!(cat.descriptive(c)?.location, Some(given));
    // The import facts are read, as without a location.
    assert!(cat.photos_to_read_exif_capture()?.is_empty());
    Ok(())
}

#[test]
fn read_metadata_from_files_fills_the_cameras_gps_with_the_rest() -> Result<()> {
    let (_dir, mut cat, ids) = photos(&["a.NEF", "b.NEF", "c.NEF"])?;
    let (a, b, c) = (ids[0], ids[1], ids[2]);
    cat.clear_location(&[b])?;
    // c's XMP gives one: it wins, in the same transaction.
    let from_xmp = Location::At {
        lat: 50.,
        lon: 19.,
        alt: None,
    };
    let reads = [(
        c,
        std::path::PathBuf::from("c.NEF"),
        crate::xmp::descriptive::Read {
            location: Some(from_xmp.clone()),
            ..Default::default()
        },
    )];
    let locations: Vec<_> = [a, b, c]
        .iter()
        .map(|id| (*id, camera_location()))
        .collect();
    cat.apply_file_metadata(&reads, &locations, Merge::Overwrite)?;
    assert_eq!(cat.descriptive(a)?.location, Some(camera_location()));
    assert_eq!(cat.descriptive(b)?.location, Some(Location::Cleared));
    assert_eq!(cat.descriptive(c)?.location, Some(from_xmp));
    Ok(())
}
