//! The one policy for orientation, visible area and crops.

/// EXIF orientation (1 to 8) as RAWmakase's flip code: bit 0 mirrors
/// columns, bit 1 mirrors rows, bit 2 swaps rows and columns (applied last);
/// 0 for an unknown value.
pub(crate) fn flip(orientation: u32) -> i32 {
    match orientation {
        2 => 1,
        3 => 3,
        4 => 2,
        5 => 4,
        6 => 6,
        7 => 7,
        8 => 5,
        _ => 0,
    }
}

/// A crop `[left, top, width, height]` starting on even rows and columns,
/// as RAWmakase has always placed a maker note's crop: an odd start moves on
/// by one, keeping the end.
pub(crate) fn even([left, top, width, height]: [u32; 4]) -> [u32; 4] {
    let (l, t) = (left & 1, top & 1);
    [
        left.saturating_add(l),
        top.saturating_add(t),
        width.saturating_sub(l),
        height.saturating_sub(t),
    ]
}

/// The visible area the camera table gives for this file, where the file's
/// own differs from the area RAWmakase has always shown, and for a DNG the
/// default crop it gives (in sensor coordinates, after the visible area).
pub(crate) fn apply(info: &mut crate::info::RawInfo) {
    // A headerless file's own table row gives its geometry already.
    if info.variant == crate::variant::Variant::HeaderlessPacked {
        return;
    }
    visible(info);
    holds_sony_frame(info);
    let dng = info.dng_version > 0;
    let size = [info.raw_width, info.raw_height];
    if dng
        && let Some(frame) =
            crate::facts::camera(&info.make, &info.model, dng, size).find_map(|c| c.frame)
    {
        info.frame = frame;
    }
}

/// The default crop of a camera file that gives none: the image size the
/// camera records for the shot (EXIF's, else its largest embedded JPEG's),
/// centred in the visible area on even rows and columns, where that size is
/// the visible area's within a tenth on each side (either way round, for a
/// size recorded turned). A size further off is another crop (an aspect
/// ratio, a smaller preview), and the visible area stays the frame.
pub(crate) fn camera_frame(info: &mut crate::info::RawInfo, bytes: &[u8]) {
    let none = [crate::info::UNSET, crate::info::UNSET, 0, 0];
    if info.dng_version > 0 || info.frame != none || info.super_ccd.is_some() {
        return;
    }
    let Some([mut w, mut h]) = info.image_size.or_else(|| {
        let (at, len) = crate::preview::largest(&info.previews, bytes)?;
        crate::preview::jpeg_size(bytes, at, len)
    }) else {
        return;
    };
    let (vw, vh) = (info.width, info.height);
    if (w > vw || h > vh) && h <= vw && w <= vh {
        (w, h) = (h, w);
    }
    let close =
        |size: u32, visible: u32| size <= visible && u64::from(size) * 10 >= u64::from(visible) * 9;
    if !close(w, vw) || !close(h, vh) {
        return;
    }
    let [left, top] = info.margins;
    info.frame = [
        left + (((vw - w) / 2) & !1),
        top + (((vh - h) / 2) & !1),
        w,
        h,
    ];
}

/// A Sony crop-mode file whose crop runs past the table's visible area (the a7 IV's
/// APS-C lossless M and S, the a7 III's APS-C): the area grows to as much past the crop
/// as before it, the sensor area those files record (SonyRawImageSize).
fn holds_sony_frame(info: &mut crate::info::RawInfo) {
    if info.dng_version > 0 || !info.make.eq_ignore_ascii_case("sony") {
        return;
    }
    let [left, top, width, height] = info.frame;
    if left == crate::info::UNSET || top == crate::info::UNSET || width == 0 || height == 0 {
        return;
    }
    let grown = |at: u32, len: u32, margin: u32, shown: u32, raw: u32| {
        let Some(before) = at.checked_sub(margin) else {
            return shown;
        };
        let end = before.saturating_add(len);
        let wanted = end.saturating_add(before);
        if end > shown && margin.saturating_add(wanted) <= raw {
            wanted
        } else {
            shown
        }
    };
    let [mx, my] = info.margins;
    info.width = grown(left, width, mx, info.width, info.raw_width);
    info.height = grown(top, height, my, info.height, info.raw_height);
}

fn visible(info: &mut crate::info::RawInfo) {
    let dng = info.dng_version > 0;
    let size = [info.raw_width, info.raw_height];
    if crate::facts::camera(&info.make, &info.model, dng, size)
        .any(|c| c.aspect_crop == Some(false))
    {
        info.aspect = [crate::info::UNSET, crate::info::UNSET, 0, 0];
    }
    let Some([left, top, width, height]) =
        crate::facts::camera(&info.make, &info.model, dng, size).find_map(|c| c.visible)
    else {
        return;
    };
    let fits =
        |start: u32, len: u32, total: u32| start.checked_add(len).is_some_and(|end| end <= total);
    if width > 0
        && height > 0
        && fits(left, width, info.raw_width)
        && fits(top, height, info.raw_height)
    {
        if let Some(cfa) = &info.cfa {
            // The filter is described from the visible area's corner.
            let (dy, dx) = (
                (top as i64 - i64::from(info.margins[1])).rem_euclid(cfa.rows as i64) as usize,
                (left as i64 - i64::from(info.margins[0])).rem_euclid(cfa.cols as i64) as usize,
            );
            info.cfa = Some(cfa.shifted(dy, dx));
        }
        // Crops are placed from the visible area's corner, except Canon's,
        // which the maker notes place on the sensor.
        let sensor_crops = matches!(
            info.variant,
            crate::variant::Variant::Cr2LosslessJpeg
                | crate::variant::Variant::Cr2Sraw
                | crate::variant::Variant::CrwCompressed
        );
        let crops = if !sensor_crops {
            [&mut info.frame, &mut info.aspect].into_iter().collect()
        } else {
            Vec::new()
        };
        for rect in crops {
            for (v, (new, old)) in rect
                .iter_mut()
                .zip([left, top].into_iter().zip(info.margins))
            {
                if *v != crate::info::UNSET {
                    *v = (*v + new).saturating_sub(old);
                    // A DNG's crops stay on even rows and columns (floored),
                    // as they are placed in the file's own area.
                    if dng {
                        *v &= !1;
                    }
                }
            }
        }
        info.margins = [left, top];
        info.width = width;
        info.height = height;
    }
}

#[cfg(test)]
mod tests {
    use crate::info::{UNSET, base};
    use crate::variant::Variant;

    #[test]
    fn a_sony_crop_past_the_visible_area_grows_it() {
        // The a7 IV's APS-C lossless M: the table's row is the full-frame M's.
        let mut info = base(Variant::ArwLosslessJpeg);
        info.make = "Sony".into();
        info.model = "ILCE-7M4".into();
        (info.raw_width, info.raw_height) = (5120, 3584);
        (info.width, info.height) = (5120, 3584);
        info.frame = [44, 30, 4608, 3072];
        super::apply(&mut info);
        assert_eq!((info.width, info.height), (4696, 3132));
        // The full-frame M's crop fits the row, which stays.
        info.frame = [8, 4, 4608, 3072];
        (info.width, info.height) = (5120, 3584);
        info.margins = [0, 0];
        super::apply(&mut info);
        assert_eq!((info.width, info.height), (4624, 3080));
    }

    /// A 6048 × 4024 sensor whose visible area starts at (8, 6).
    fn info() -> crate::info::RawInfo {
        let mut info = base(Variant::ArwUncompressed);
        (info.raw_width, info.raw_height) = (6048, 4024);
        (info.width, info.height, info.margins) = (6024, 4012, [8, 6]);
        info
    }

    /// A file that gives no crop is framed at the size the camera records,
    /// centred on even rows and columns; a size given turned is turned back.
    #[test]
    fn the_cameras_image_size_is_centred_where_the_file_gives_no_crop() {
        for size in [[6000, 4000], [4000, 6000]] {
            let mut info = info();
            info.image_size = Some(size);
            super::camera_frame(&mut info, &[]);
            // (6024 - 6000) / 2 = 12, (4012 - 4000) / 2 = 6 floored to even.
            assert_eq!(info.frame, [8 + 12, 6 + 6, 6000, 4000]);
        }
        let mut info = info();
        info.image_size = Some([6002, 4006]);
        super::camera_frame(&mut info, &[]);
        assert_eq!(info.frame, [8 + 10, 6 + 2, 6002, 4006]);
    }

    /// The file's own crop, a DNG's, a size more than a tenth smaller (an
    /// aspect ratio or a small preview) or larger than the visible area
    /// leave the frame as it was.
    #[test]
    fn other_sizes_and_files_with_a_crop_keep_their_frame() {
        let none = [UNSET, UNSET, 0, 0];
        for (size, frame, dng) in [
            ([6000, 3376], none, 0),
            ([5000, 4000], none, 0),
            ([6030, 4000], none, 0),
            ([6000, 4000], [10, 10, 6004, 3992], 0),
            ([6000, 4000], [UNSET, UNSET, 6000, 4000], 0),
            ([6000, 4000], none, 1),
        ] {
            let mut info = info();
            (info.image_size, info.frame, info.dng_version) = (Some(size), frame, dng);
            super::camera_frame(&mut info, &[]);
            assert_eq!(info.frame, frame, "{size:?} {frame:?} {dng}");
        }
    }

    /// Without an EXIF size, the largest embedded JPEG's size frames the
    /// photo.
    #[test]
    fn the_largest_previews_size_stands_in_for_the_exif_size() {
        let jpeg = |w: u16, h: u16| {
            let mut j = vec![0xff, 0xd8, 0xff, 0xe0, 0, 4, 0, 0, 0xff, 0xc0, 0, 17, 8];
            j.extend(h.to_be_bytes());
            j.extend(w.to_be_bytes());
            j.extend([3; 10]);
            j.extend([0xff, 0xd9]);
            j
        };
        let (small, large) = (jpeg(160, 120), jpeg(6000, 4000));
        let mut bytes = small.clone();
        bytes.extend(&large);
        let mut info = info();
        info.previews = vec![
            crate::preview::Candidate {
                offset: 0,
                len: small.len() as u64,
                pixels: 160 * 120,
            },
            crate::preview::Candidate {
                offset: small.len() as u64,
                len: large.len() as u64,
                pixels: 6000 * 4000,
            },
        ];
        super::camera_frame(&mut info, &bytes);
        assert_eq!(info.frame, [20, 12, 6000, 4000]);
    }
}
