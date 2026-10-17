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
/// own differs from the area RAWmakase has always shown.
pub(crate) fn apply(info: &mut crate::info::RawInfo) {
    // A headerless file's own table row gives its geometry already.
    if info.variant == crate::variant::Variant::HeaderlessPacked {
        return;
    }
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
