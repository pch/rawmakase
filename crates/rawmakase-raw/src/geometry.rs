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

/// Whether `[left, top, width, height]` `r` is a non-empty rectangle inside
/// `outer`.
pub(crate) fn inside(r: [u32; 4], outer: [u32; 4]) -> bool {
    let end = |start: u32, len: u32| start.checked_add(len);
    r[2] > 0
        && r[3] > 0
        && r[0] >= outer[0]
        && r[1] >= outer[1]
        && end(r[0], r[2])
            .zip(end(outer[0], outer[2]))
            .is_some_and(|(a, b)| a <= b)
        && end(r[1], r[3])
            .zip(end(outer[1], outer[3]))
            .is_some_and(|(a, b)| a <= b)
}

/// Whether `info`'s framing is the shot's, which keeps the frame RAWmakase
/// has always read: the file records an in-camera aspect-ratio crop (of
/// another shape than the frame), or the frame is one (Adobe's `crop` has
/// another shape, as the whole sensor around a square shot has).
fn shot_framing(info: &crate::info::RawInfo, crop: [u32; 4]) -> bool {
    let frame = if info.frame[2] > 0 && info.frame[3] > 0 {
        info.frame
    } else {
        [info.margins[0], info.margins[1], info.width, info.height]
    };
    let shape = |r: [u32; 4]| f64::from(r[2]) / f64::from(r[3].max(1));
    let other = |a: [u32; 4], b: [u32; 4]| (shape(a) / shape(b) - 1.).abs() > 0.03;
    let aspect = info.aspect;
    (aspect[2] > 0 && aspect[3] > 0 && other(aspect, frame)) || other(crop, frame)
}

/// `info` in Adobe's frame (see [`crate::in_adobe_frame`]). The visible area
/// moves only where Adobe's crop needs it, so the sensor data RAWmakase reads
/// (and the masked borders levels are measured beside) stays as it was
/// wherever it can.
pub(crate) fn adobe(info: &crate::info::RawInfo) -> Option<crate::info::RawInfo> {
    let f = info.adobe_frame?;
    if info.super_ccd.is_some()
        || !inside(f.active, [0, 0, info.raw_width, info.raw_height])
        || !inside(f.default_crop, f.active)
        || shot_framing(info, f.default_crop)
    {
        return None;
    }
    let mut out = info.clone();
    let visible = [info.margins[0], info.margins[1], info.width, info.height];
    if !inside(f.default_crop, visible) {
        let [left, top, width, height] = f.active;
        if let Some(cfa) = &info.cfa {
            let (dy, dx) = (
                (i64::from(top) - i64::from(info.margins[1])).rem_euclid(cfa.rows as i64) as usize,
                (i64::from(left) - i64::from(info.margins[0])).rem_euclid(cfa.cols as i64) as usize,
            );
            out.cfa = Some(cfa.shifted(dy, dx));
        }
        out.margins = [left, top];
        out.width = width;
        out.height = height;
    }
    out.frame = f.default_crop;
    // Adobe's crop is the frame: no other rule widens or replaces it.
    out.sensor_borders = None;
    out.recommended_crop = None;
    Some(out)
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
    let dng = info.dng_version > 0;
    let size = [info.raw_width, info.raw_height];
    if dng
        && let Some(frame) =
            crate::facts::camera(&info.make, &info.model, dng, size).find_map(|c| c.frame)
    {
        info.frame = frame;
    }
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
    use crate::info::{AdobeFrame, Cfa, SuperCcd, base};
    use crate::variant::Variant;

    fn info(frame: AdobeFrame) -> crate::info::RawInfo {
        let mut i = base(Variant::NefPacked);
        (i.raw_width, i.raw_height) = (100, 80);
        (i.margins, i.width, i.height) = ([0, 0], 96, 80);
        i.frame = [crate::info::UNSET, crate::info::UNSET, 0, 0];
        i.recommended_crop = Some([2, 2, 90, 70]);
        i.cfa = Some(Cfa {
            rows: 2,
            cols: 2,
            colors: vec![0, 1, 3, 2],
        });
        i.adobe_frame = Some(frame);
        i
    }

    /// Adobe's crop inside the visible area moves only the frame.
    #[test]
    fn a_frame_inside_the_visible_area_keeps_it() {
        let i = info(AdobeFrame {
            active: [0, 0, 96, 80],
            default_crop: [8, 6, 80, 68],
        });
        let a = super::adobe(&i).unwrap();
        assert_eq!(a.frame, [8, 6, 80, 68]);
        assert_eq!((a.margins, a.width, a.height), ([0, 0], 96, 80));
        assert_eq!(a.cfa, i.cfa);
        assert_eq!(a.recommended_crop, None);
    }

    /// A crop reaching past the visible area makes Adobe's active area the
    /// visible one, its colour filter read from its own corner.
    #[test]
    fn a_frame_beyond_the_visible_area_takes_adobes_active_area() {
        let i = info(AdobeFrame {
            active: [1, 0, 99, 80],
            default_crop: [2, 1, 96, 79],
        });
        let a = super::adobe(&i).unwrap();
        assert_eq!((a.margins, a.width, a.height), ([1, 0], 99, 80));
        // RGgB moved one column on: GRBg.
        assert_eq!(a.cfa.unwrap().colors, vec![1, 0, 2, 3]);
        assert_eq!(a.frame, [2, 1, 96, 79]);
    }

    /// A shot framed in the camera keeps the frame RAWmakase has always read:
    /// an aspect-ratio crop the maker notes record (a PowerShot G12's 3:2 shot
    /// of its 4:3 sensor), or a frame that is one (a DC-GH6 high-resolution
    /// square shot, whose Adobe crop is the whole 4:3 sensor). A frame of the
    /// camera's own shape moves to Adobe's.
    #[test]
    fn a_shot_framed_in_the_camera_keeps_its_frame() {
        let mut g12 = info(AdobeFrame {
            active: [0, 0, 96, 80],
            default_crop: [4, 4, 88, 66],
        });
        g12.frame = [2, 2, 92, 69];
        g12.aspect = [2, 9, 92, 61];
        assert!(super::adobe(&g12).is_none());
        let mut gh6 = info(AdobeFrame {
            active: [0, 0, 96, 80],
            default_crop: [2, 2, 92, 69],
        });
        gh6.frame = [12, 2, 69, 69];
        assert!(super::adobe(&gh6).is_none());
        // The same camera's 4:3 shot, with its aspect the frame, moves.
        let mut plain = g12.clone();
        plain.aspect = plain.frame;
        assert_eq!(super::adobe(&plain).unwrap().frame, [4, 4, 88, 66]);
    }

    #[test]
    fn frames_that_do_not_fit_are_not_used() {
        let outside = info(AdobeFrame {
            active: [0, 0, 101, 80],
            default_crop: [0, 0, 90, 80],
        });
        assert!(super::adobe(&outside).is_none());
        let past_active = info(AdobeFrame {
            active: [0, 0, 96, 80],
            default_crop: [8, 6, 90, 68],
        });
        assert!(super::adobe(&past_active).is_none());
        let mut turned = info(AdobeFrame {
            active: [0, 0, 96, 80],
            default_crop: [8, 6, 80, 68],
        });
        turned.super_ccd = Some(SuperCcd {
            stored: [96, 80],
            wide: true,
        });
        assert!(super::adobe(&turned).is_none());
        let mut unknown = turned.clone();
        unknown.super_ccd = None;
        unknown.adobe_frame = None;
        assert!(super::adobe(&unknown).is_none());
    }
}
