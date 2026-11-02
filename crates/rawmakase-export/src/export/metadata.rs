//! The EXIF directories an export writes: the camera's own when it is included,
//! with the export's size, orientation, resolution and software.
use super::{Embed, exif::tiff_block};
use crate::build_info::SOFTWARE;
use crate::{
    camera_data::Metadata,
    exif::{
        CameraExif, Field,
        tag::{
            self, COLOR_SPACE, LENS_MODEL, MAKE, ORIENTATION, PIXEL_X_DIMENSION, PIXEL_Y_DIMENSION,
            RESOLUTION_UNIT, X_RESOLUTION, Y_RESOLUTION, YCBCR_POSITIONING,
        },
    },
};

pub(super) fn directories(m: &Metadata, embed: &Embed, width: u32, height: u32) -> CameraExif {
    let mut d = embed
        .camera
        .clone()
        .unwrap_or_else(|| CameraExif::from_libraw(m));
    if embed.camera_fallback && !d.main.iter().any(|f| f.tag == MAKE) {
        d.main.extend(CameraExif::from_libraw(m).main);
    }
    // A lens the camera did not name (Pentax records only its LensType) is named
    // from the lens profile that identified it; the camera's own name is kept.
    if embed.camera_fallback
        && !m.lens_model.is_empty()
        && !d.exif.iter().any(|f| f.tag == LENS_MODEL)
    {
        d.exif.push(Field::ascii(LENS_MODEL, &m.lens_model));
    }
    let ppi = embed.ppi.clamp(1, 10_000);
    d.main.retain(|f| {
        ![
            ORIENTATION,
            tag::SOFTWARE,
            X_RESOLUTION,
            Y_RESOLUTION,
            RESOLUTION_UNIT,
        ]
        .contains(&f.tag)
    });
    d.main.extend([
        Field::short(ORIENTATION, 1),
        Field::rational(X_RESOLUTION, ppi, 1),
        Field::rational(Y_RESOLUTION, ppi, 1),
        Field::short(RESOLUTION_UNIT, 2),
        Field::ascii(tag::SOFTWARE, SOFTWARE),
    ]);
    d.exif.retain(|f| f.tag != COLOR_SPACE);
    d.exif.extend([
        Field::short(COLOR_SPACE, 1),
        Field::long(PIXEL_X_DIMENSION, width),
        Field::long(PIXEL_Y_DIMENSION, height),
    ]);
    d
}

/// The eXIf payload of a PNG export: the TIFF stream alone.
pub(super) fn png_exif(d: CameraExif) -> Vec<u8> {
    tiff_block(d)
}

/// The most EXIF a JPEG's APP1 segment holds: its length counts itself (2
/// bytes) and the "Exif\0\0" header (6), and can't exceed 65535.
const JPEG_EXIF_MAX: usize = 65_535 - 2 - 6;

/// The APP1 EXIF payload of a JPEG export. EXIF is one segment in a JPEG, so
/// camera EXIF too large for it loses its largest values, the EXIF and GPS
/// directories' before the main directory's, until it fits; the file's own
/// fields (orientation, resolution, software, pixel size, colour space) stay.
/// TIFF and PNG exports keep all of it.
pub(super) fn jpeg_exif(mut d: CameraExif) -> Vec<u8> {
    // Required for JPEG by the EXIF standard: chroma samples are centered.
    d.main.push(Field::short(YCBCR_POSITIONING, 1));
    let block = tiff_block(d.clone());
    if block.len() <= JPEG_EXIF_MAX {
        return block;
    }
    fit(&mut d, block.len() - JPEG_EXIF_MAX);
    let block = tiff_block(d);
    // Never a segment the JPEG can't hold.
    if block.len() <= JPEG_EXIF_MAX {
        block
    } else {
        Vec::new()
    }
}

/// Removes fields other than the file's own until they free `excess`
/// bytes of the TIFF block: those of the EXIF and GPS directories first, the
/// largest first, the highest tag of equal sizes. A field takes its 12-byte
/// entry and, past 4 bytes, its value padded to an even length; a directory
/// left empty frees more besides.
fn fit(d: &mut CameraExif, excess: usize) {
    const OWN: [u16; 9] = [
        ORIENTATION,
        X_RESOLUTION,
        Y_RESOLUTION,
        RESOLUTION_UNIT,
        tag::SOFTWARE,
        YCBCR_POSITIONING,
        COLOR_SPACE,
        PIXEL_X_DIMENSION,
        PIXEL_Y_DIMENSION,
    ];
    let size = |f: &Field| {
        12 + if f.bytes.len() > 4 {
            f.bytes.len().next_multiple_of(2)
        } else {
            0
        }
    };
    // (main directory last, then largest, then highest tag), directory, index.
    let mut order: Vec<_> = [&d.exif, &d.gps, &d.main]
        .iter()
        .enumerate()
        .flat_map(|(dir, fields)| {
            fields
                .iter()
                .enumerate()
                .filter(|(_, f)| !OWN.contains(&f.tag))
                .map(move |(i, f)| ((dir == 2, std::cmp::Reverse((size(f), f.tag))), dir, i))
        })
        .collect();
    order.sort();
    let mut dropped = [&d.exif, &d.gps, &d.main].map(|fields| vec![false; fields.len()]);
    let mut freed = 0;
    for (_, dir, i) in order {
        if freed >= excess {
            break;
        }
        freed += size(&[&d.exif, &d.gps, &d.main][dir][i]);
        dropped[dir][i] = true;
    }
    for (fields, dropped) in [&mut d.exif, &mut d.gps, &mut d.main]
        .into_iter()
        .zip(dropped)
    {
        let mut dropped = dropped.into_iter();
        fields.retain(|_| !dropped.next().unwrap_or(false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exif::tag::MODEL;

    #[test]
    fn oversized_jpeg_exif_drops_the_largest_capture_values_first() {
        let text = |tag, len| Field::ascii(tag, &"x".repeat(len));
        let camera = || CameraExif {
            main: vec![Field::ascii(MAKE, "SONY"), Field::ascii(MODEL, "ILCE-7M2")],
            exif: vec![
                Field::ascii(0x9003, "2018:08:26 10:39:33"),
                text(0x9286, 30_000),
                text(0xa420, 30_000),
                text(0xa430, 9_000),
                Field::long(PIXEL_X_DIMENSION, 8),
                Field::short(COLOR_SPACE, 1),
            ],
            gps: vec![Field::rational(0x0002, 52, 1)],
        };
        // The equal largest go highest tag first; one is enough here.
        let mut d = camera();
        d.main.push(Field::short(YCBCR_POSITIONING, 1));
        let excess = tiff_block(d.clone()).len() - JPEG_EXIF_MAX;
        fit(&mut d, excess);
        let tags = |fields: &[Field]| fields.iter().map(|f| f.tag).collect::<Vec<_>>();
        assert_eq!(
            tags(&d.exif),
            [0x9003, 0x9286, 0xa430, PIXEL_X_DIMENSION, COLOR_SPACE]
        );
        assert_eq!(tags(&d.main), [MAKE, MODEL, YCBCR_POSITIONING]);
        assert_eq!(d.gps.len(), 1);
        assert_eq!(jpeg_exif(camera()), tiff_block(d));

        // Thousands of small entries are cut in one pass too, highest tags
        // first, so a crafted file can't make the export crawl. (Without GPS,
        // whose emptied directory would free a few bytes more.)
        let many = CameraExif {
            exif: (0x1000..0x1000 + 8000)
                .map(|tag| Field::short(tag, 1))
                .collect(),
            gps: Vec::new(),
            ..camera()
        };
        let block = jpeg_exif(many);
        assert!(block.len() <= JPEG_EXIF_MAX && block.len() > JPEG_EXIF_MAX - 12);
        let entry = |tag: u16| [tag.to_le_bytes(), 3u16.to_le_bytes()].concat();
        assert!(block.windows(4).any(|w| w == entry(0x1000)));
        assert!(!block.windows(4).any(|w| w == entry(0x1000 + 7999)));
    }

    #[test]
    fn a_lens_named_from_its_profile_is_exported_where_the_camera_wrote_none() {
        // A Pentax DNG's EXIF reads fine but has no LensModel; the lens is named
        // from the imported profile its LensType matched (issue #501).
        let m = Metadata {
            make: "Pentax".into(),
            model: "K-x".into(),
            lens_model: "smc PENTAX-DA 18-55mm F3.5-5.6 AL".into(),
            ..Default::default()
        };
        let embed = |exif: Vec<Field>, camera_fallback| Embed {
            camera: Some(CameraExif {
                main: vec![
                    Field::ascii(MAKE, "PENTAX"),
                    Field::ascii(MODEL, "PENTAX K-x"),
                ],
                exif,
                gps: Vec::new(),
            }),
            camera_fallback,
            ..Default::default()
        };
        let lens = |d: &CameraExif| {
            d.exif
                .iter()
                .filter(|f| f.tag == LENS_MODEL)
                .filter_map(Field::text)
                .collect::<Vec<_>>()
        };
        let d = directories(&m, &embed(Vec::new(), true), 4, 4);
        assert_eq!(lens(&d), ["smc PENTAX-DA 18-55mm F3.5-5.6 AL"]);
        // The camera's own name is kept.
        let d = directories(
            &m,
            &embed(vec![Field::ascii(LENS_MODEL, "DA 18-55")], true),
            4,
            4,
        );
        assert_eq!(lens(&d), ["DA 18-55"]);
        // Nothing of the camera's when its information is left out.
        let left_out = Embed {
            camera: Some(CameraExif::default()),
            camera_fallback: false,
            ..Default::default()
        };
        assert!(lens(&directories(&m, &left_out, 4, 4)).is_empty());
    }
}
