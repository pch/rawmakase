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

/// The visible area the camera table gives for this file, where the file's
/// own differs from the area RAWmakase has always shown.
pub(crate) fn apply(info: &mut crate::info::RawInfo) {
    let dng = info.dng_version > 0;
    let size = [info.raw_width, info.raw_height];
    let Some([left, top, width, height]) =
        crate::facts::camera(&info.make, &info.model, dng, size).and_then(|c| c.visible)
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
        // Crops are placed from the visible area's corner.
        for rect in [&mut info.frame, &mut info.aspect] {
            for (v, (new, old)) in rect
                .iter_mut()
                .zip([left, top].into_iter().zip(info.margins))
            {
                if *v != crate::info::UNSET {
                    *v = (*v + new).saturating_sub(old);
                }
            }
        }
        info.margins = [left, top];
        info.width = width;
        info.height = height;
    }
}
