use super::synthetic::*;
use super::*;

/// A RAW-like camera image `width` × `height` of `pixel(x, y)`, its as-shot white
/// balance neutral (so the sensor clips at 1).
pub(crate) fn image(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [f32; 3]) -> CameraImage {
    CameraImage {
        recovered: Default::default(),
        width,
        height,
        pixels: (0..width * height)
            .map(|i| pixel(i % width, i / width))
            .collect(),
        metadata: Metadata {
            width,
            height,
            wb: [1.; 3],
            daylight_wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    }
}

/// A grey ramp across the scene's range.
fn greys() -> impl Iterator<Item = [f32; 3]> {
    (0..=20).map(|i| [10f32.powf(-SCENE_DECADES * (1. - i as f32 / 20.)); 3])
}
/// Saturated scene colours within the film's range.
const PATCHES: [[f32; 3]; 5] = [
    [0.6, 0.05, 0.03],
    [0.04, 0.35, 0.05],
    [0.03, 0.06, 0.5],
    [0.5, 0.45, 0.02],
    [0.02, 0.3, 0.4],
];
fn relative_error(out: [f32; 3], want: [f32; 3]) -> f32 {
    (0..3)
        .map(|c| (out[c] / want[c] - 1.).abs())
        .fold(0., f32::max)
}

#[test]
fn the_film_base_converts_to_the_thin_end_in_every_channel() {
    for film in [FilmType::Colour, FilmType::BlackWhite] {
        let n = NegativeConversion {
            dmin: [0.; 3],
            dmax: [1.1, 1.4, 0.9],
            ..COLOUR.conversion(film)
        };
        let n = n.with_film(film);
        let out = Inversion::new(&n, false).apply(COLOUR.base);
        let thin = 10f32.powf(-decades(&n));
        for v in out {
            assert!((v / thin - 1.).abs() < 1e-4, "{film:?} {out:?}");
        }
    }
    // Film denser than `dmax` renders above white, unclamped.
    let n = COLOUR.conversion(FilmType::Colour);
    let dense = std::array::from_fn(|c| COLOUR.base[c] * 10f32.powf(-n.dmax[c] - 0.2));
    assert!(
        Inversion::new(&n, false)
            .apply(dense)
            .iter()
            .all(|v| *v > WHITE)
    );
}

/// With the base measured and the bounds at the scene's extremes, the conversion gives
/// back the scene the negative was exposed to: greys stay neutral whatever the layers'
/// gammas and the base's colour, and so do colours.
#[test]
fn a_grey_ramp_converts_to_a_neutral_ramp() {
    let inversion = Inversion::new(&COLOUR.conversion(FilmType::Colour), false);
    for e in greys().chain(PATCHES) {
        let out = inversion.apply(COLOUR.scan(e));
        assert!(relative_error(out, e) < 1e-3, "{e:?} -> {out:?}");
    }
    // The conversion's white is the scene's.
    let white = inversion.apply(COLOUR.scan([1.; 3]));
    assert!(relative_error(white, [WHITE; 3]) < 1e-4, "{white:?}");
}

/// A gain on the camera values before the conversion (another light, the scan's
/// exposure, a camera's channel sensitivities) cancels: the base is in the same values
/// and scales with them.
#[test]
fn a_gain_before_the_conversion_cancels() {
    let gain = [1.8, 1., 0.6];
    let n = COLOUR.conversion(FilmType::Colour);
    let plain = Inversion::new(&n, false);
    let lit = Inversion::new(
        &NegativeConversion {
            base: std::array::from_fn(|c| n.base[c] * gain[c]),
            ..n
        },
        false,
    );
    for e in greys().chain(PATCHES) {
        let v = COLOUR.scan(e);
        let out = plain.apply(v);
        let scaled = lit.apply(std::array::from_fn(|c| v[c] * gain[c]));
        assert!(relative_error(scaled, out) < 1e-4, "{out:?} {scaled:?}");
    }
}

/// Black & white film has one density, the channels' mean: no cast from a toned base
/// or the light, whatever the scan's colour.
#[test]
fn black_and_white_film_converts_to_equal_channels() {
    let n = COLOUR.conversion(FilmType::BlackWhite);
    let inversion = Inversion::new(&n, false);
    for e in greys().chain(PATCHES) {
        let out = inversion.apply(COLOUR.scan(e));
        assert!(out[0] == out[1] && out[1] == out[2], "{out:?}");
    }
    let film = BLACK_WHITE.conversion(FilmType::BlackWhite);
    let inversion = Inversion::new(&film, false);
    for e in greys() {
        let out = inversion.apply(BLACK_WHITE.scan(e));
        assert!(relative_error(out, e) < 1e-3, "{e:?} -> {out:?}");
    }
}

/// Camera values of 0, below it after black subtraction, or not numbers are the
/// densest film, never NaN.
#[test]
fn empty_samples_stay_finite() {
    let inversion = Inversion::new(&NegativeConversion::new(FilmType::Colour), false);
    for v in [
        [0.; 3],
        [-0.01, 0., 0.2],
        [f32::MIN_POSITIVE; 3],
        [f32::NAN; 3],
    ] {
        assert!(inversion.apply(v).iter().all(|x| x.is_finite()), "{v:?}");
    }
}

/// A rendered file's decoded values are its pseudo-scene: its own linear values, the
/// light the film let through, are what convert. A TIFF scan converts as the RAW of
/// the same scan does.
#[test]
fn a_rendered_scan_converts_its_own_linear_values() {
    let n = COLOUR.conversion(FilmType::Colour);
    let raw = Inversion::new(&n, false);
    let rendered = Inversion::new(&n, true);
    for e in greys().chain(PATCHES) {
        let linear = COLOUR.scan(e);
        let decoded = crate::develop::rendered::scene_from_rendered(linear);
        let want = raw.apply(linear);
        let got = rendered.apply(decoded);
        assert!(relative_error(got, want) < 2e-3, "{e:?}: {got:?} {want:?}");
        // Converting the pseudo-scene as it is would not.
        assert!(relative_error(raw.apply(decoded), want) > 0.02, "{e:?}");
    }
}

/// A scan for Auto, 300 × 200, of `film`'s negative of a grey ramp across the scene's
/// range, with flat ends, above colour patches. `highlight` replaces the last patch.
fn auto_scan(film: &Film, highlight: Option<[f32; 3]>) -> CameraImage {
    image(300, 200, |x, y| {
        let (u, v) = (x as f32 / 300., y as f32 / 200.);
        let e = if v < 0.6 {
            let t = ((u - 0.1) / 0.8).clamp(0., 1.);
            [10f32.powf(-SCENE_DECADES * (1. - t)); 3]
        } else {
            let patch = ((u * 5.) as usize).min(4);
            match highlight {
                Some(h) if patch == 4 => h,
                _ => PATCHES[patch],
            }
        };
        film.scan(e)
    })
}

/// The recipe Auto starts from: the film's base, not yet levelled.
fn unlevelled(film: &Film, kind: FilmType) -> Recipe {
    Recipe {
        negative: Some(NegativeConversion {
            dmin: [0.; 3],
            dmax: [1.5; 3],
            ..film.conversion(kind)
        }),
        ..Default::default()
    }
}

fn assert_bounds(got: &NegativeConversion, want: &NegativeConversion, tolerance: f32) {
    for c in 0..3 {
        assert!(
            (got.dmin[c] - want.dmin[c]).abs() < tolerance
                && (got.dmax[c] - want.dmax[c]).abs() < tolerance,
            "{got:?} against {want:?}"
        );
    }
}

/// Auto measures the frame's thin and dense ends: on a frame whose darkest and
/// brightest tones are greys, the densities of those greys. Only the bounds are
/// Auto's.
#[test]
fn auto_levels_the_frame_to_its_darkest_and_brightest_greys() {
    let r = unlevelled(&COLOUR, FilmType::Colour);
    let got = auto_negative(&auto_scan(&COLOUR, None), &r, Cancel::NEVER).unwrap();
    assert_bounds(&got, &COLOUR.conversion(FilmType::Colour), 1e-3);
    assert_eq!(
        NegativeConversion {
            dmin: [0.; 3],
            dmax: [1.5; 3],
            ..got
        },
        *r.negative.as_ref().unwrap()
    );
}

/// Black & white film has one density, so one pair of ends for every channel.
#[test]
fn auto_levels_black_and_white_film_alike_in_every_channel() {
    let r = unlevelled(&BLACK_WHITE, FilmType::BlackWhite);
    let got = auto_negative(&auto_scan(&BLACK_WHITE, None), &r, Cancel::NEVER).unwrap();
    assert!(got.dmin.iter().all(|d| *d == got.dmin[0]), "{got:?}");
    assert!(got.dmax.iter().all(|d| *d == got.dmax[0]), "{got:?}");
    assert_bounds(&got, &BLACK_WHITE.conversion(FilmType::BlackWhite), 1e-3);
    assert_eq!(got.film, FilmType::BlackWhite);
}

/// A coloured highlight denser in one layer than the frame's white does not set that
/// layer's dense end: it is read from the densest near-neutral tones, so greys still
/// render grey. Per-channel ends alone would have tinted them.
#[test]
fn a_coloured_highlight_does_not_cast_the_greys() {
    let r = unlevelled(&COLOUR, FilmType::Colour);
    let im = auto_scan(&COLOUR, Some([3., 0.4, 0.15]));
    let auto = auto_negative(&im, &r, Cancel::NEVER).unwrap();
    let inversion = Inversion::new(&auto, false);
    for e in greys() {
        let out = inversion.apply(COLOUR.scan(e));
        let spread = out.iter().copied().fold(0., f32::max)
            / out.iter().copied().fold(f32::INFINITY, f32::min);
        assert!(spread < 1.01, "{e:?} -> {out:?}");
    }
    let n = r.negative.as_ref().unwrap();
    let densities: Vec<[f32; 3]> = im
        .pixels
        .iter()
        .map(|p| Inversion::new(n, false).density(*p))
        .collect();
    let per_channel = Inversion::new(
        &NegativeConversion {
            dmin: percentiles(&densities, AUTO_PERCENTILE),
            dmax: percentiles(&densities, 1. - AUTO_PERCENTILE),
            ..n.clone()
        },
        false,
    );
    let out = per_channel.apply(COLOUR.scan([0.5; 3]));
    assert!(out[0] < out[1] * 0.95, "{out:?}");
}

/// A sliver of the film holder inside the crop, which recorded (nearly) no light,
/// along one edge with a blurred border, covering up to about 5% of the crop.
fn with_holder(im: &CameraImage, film: &Film, side: usize, share: f32) -> CameraImage {
    let (w, h) = (im.width as f32, im.height as f32);
    let holder = film.base.map(|b| b * 10f32.powf(-2.5));
    let mut out = im.clone();
    for (i, p) in out.pixels.iter_mut().enumerate() {
        let (x, y) = ((i as u32 % im.width) as f32, (i as u32 / im.width) as f32);
        // Distance into the holder from its edge, in pixels.
        let into = match side {
            0 => w * share - x,
            1 => x - w * (1. - share),
            2 => h * share - y,
            _ => y - h * (1. - share),
        };
        // Two pixels of blur, as a lens and the scan's reduction leave a holder's
        // edge: mixed light.
        let cover = ((into + 1.) / 2.).clamp(0., 1.);
        *p = std::array::from_fn(|c| p[c] * (1. - cover) + holder[c] * cover);
    }
    out
}

/// No-light pixels inside the crop are left out: a holder's sliver covering up to
/// about 5% of the crop, along any edge, leaves the bounds as the frame alone gives
/// them, for either film. Measured on a real scan, such a sliver (density 2.5 above
/// the base, against the frame's 0.8) was taken as the white and rendered the frame
/// black.
#[test]
fn auto_leaves_out_a_holder_sliver_inside_the_crop() {
    for (film, kind) in [
        (&BLACK_WHITE, FilmType::BlackWhite),
        (&COLOUR, FilmType::Colour),
    ] {
        let r = unlevelled(film, kind);
        let frame = auto_scan(film, None);
        let want = auto_negative(&frame, &r, Cancel::NEVER).unwrap();
        for side in 0..4 {
            for share in [0.035, 0.05] {
                let im = with_holder(&frame, film, side, share);
                let got = auto_negative(&im, &r, Cancel::NEVER).unwrap();
                assert_bounds(&got, &want, 0.02);
            }
        }
        // Without the gap, the sliver would set the white.
        let im = with_holder(&frame, film, 0, 0.05);
        let n = r.negative.as_ref().unwrap();
        let all: Vec<[f32; 3]> = im
            .pixels
            .iter()
            .map(|p| Inversion::new(n, false).density(*p))
            .collect();
        let naive = percentiles(&all, 1. - AUTO_PERCENTILE);
        assert!(naive[1] > want.dmax[1] + 0.5, "{naive:?}");
    }
}

/// A dense area of the frame's own away from its edges, apart from the rest (a bright
/// window, the sun), is part of the photo, not no light: Auto keeps it.
#[test]
fn auto_keeps_a_separate_dense_area_inside_the_frame() {
    let r = unlevelled(&BLACK_WHITE, FilmType::BlackWhite);
    let window = BLACK_WHITE.scan([20.; 3]);
    let mut im = auto_scan(&BLACK_WHITE, None);
    for (i, p) in im.pixels.iter_mut().enumerate() {
        let (x, y) = (i as u32 % 300, i as u32 / 300);
        if (120..180).contains(&x) && (70..130).contains(&y) {
            *p = window;
        }
    }
    let got = auto_negative(&im, &r, Cancel::NEVER).unwrap();
    let window_density = BLACK_WHITE.density([20.; 3])[0];
    assert!(
        (got.dmax[0] - window_density).abs() < 0.02,
        "{got:?} {window_density}"
    );
}

/// Auto needs a conversion to level, unclipped film inside the crop, and a frame with
/// a range of densities: a blank frame has none, and its levels would stretch its
/// noise. A cancelled Auto stops with an error.
#[test]
fn auto_refuses_what_it_cannot_level() {
    let r = unlevelled(&COLOUR, FilmType::Colour);
    let frame = auto_scan(&COLOUR, None);
    let error = auto_negative(&frame, &Recipe::default(), Cancel::NEVER).unwrap_err();
    assert!(error.to_string().contains("film type"), "{error}");
    let blank = image(300, 200, |_, _| [0.4, 0.2, 0.1]);
    let error = auto_negative(&blank, &r, Cancel::NEVER).unwrap_err();
    assert!(error.to_string().contains("too flat"), "{error}");
    let clipped = image(300, 200, |_, _| [1.; 3]);
    let error = auto_negative(&clipped, &r, Cancel::NEVER).unwrap_err();
    assert!(error.to_string().contains("unclipped"), "{error}");
    let flag = std::sync::atomic::AtomicBool::new(true);
    assert!(auto_negative(&frame, &r, Cancel::new(&flag)).is_err());
}

/// A scan whose left quarter is the film's clear rebate (the bare base) and the rest a
/// negative of a ramp.
fn scan_with_rebate(base: [f32; 3]) -> CameraImage {
    image(48, 32, |x, y| {
        if x < 12 {
            return base;
        }
        COLOUR.scan([10f32.powf(-1.9 + 1.85 * (x as f32 / 47. + y as f32 / 62.) / 1.5); 3])
    })
}

/// The film base picker reads the scan's own values, not the converted ones, averaged
/// over a patch: on the rebate it gives back the base the film was made with, whether
/// or not a conversion is on yet.
#[test]
fn the_film_base_picker_reads_the_rebate_as_scanned() {
    let im = scan_with_rebate(COLOUR.base);
    let (u, v) = (6. / 48., 0.5);
    for negative in [None, Some(COLOUR.conversion(FilmType::Colour))] {
        let r = Recipe {
            negative,
            ..Default::default()
        };
        let got = film_base_pick(&im, &r, u, v).unwrap();
        assert!(relative_error(got, COLOUR.base) < 1e-5, "{got:?}");
    }
    // Inside the frame it is the film there, averaged: thinner than nothing.
    let inside = film_base_pick(&im, &Recipe::default(), 0.6, 0.5).unwrap();
    assert!((0..3).all(|c| inside[c] < COLOUR.base[c]), "{inside:?}");
}

/// A clipped patch has no film base to read, only the light's colour; nor has a patch
/// with no light.
#[test]
fn the_film_base_picker_refuses_clipped_or_dark_patches() {
    let r = Recipe::default();
    let (u, v) = (6. / 48., 0.5);
    let pick = |im: &CameraImage| film_base_pick(im, &r, u, v);
    assert_eq!(
        pick(&scan_with_rebate([1.; 3])),
        Err(FilmBaseRefusal::Clipped)
    );
    assert_eq!(
        pick(&scan_with_rebate([0.99, 0.6, 0.3])),
        Err(FilmBaseRefusal::Clipped)
    );
    // At the sensor's clip only once the as-shot white balance is counted: camera
    // values clip at it.
    let mut im = scan_with_rebate([0.9, 0.55, 0.3]);
    assert!(pick(&im).is_ok());
    im.metadata.wb = [0.9, 1., 1.];
    assert_eq!(pick(&im), Err(FilmBaseRefusal::Clipped));
    // One clipped pixel in the patch is enough: the average would hide it.
    let mut im = scan_with_rebate([0.9, 0.55, 0.3]);
    im.pixels[16 * 48 + 4] = [1.; 3];
    assert_eq!(pick(&im), Err(FilmBaseRefusal::Clipped));
    let mut im = scan_with_rebate([0.9, 0.55, 0.3]);
    im.pixels[16 * 48 + 4] = [0.5, f32::NAN, 0.3];
    assert_eq!(pick(&im), Err(FilmBaseRefusal::Clipped));
    assert_eq!(
        pick(&scan_with_rebate([0., 0.2, 0.1])),
        Err(FilmBaseRefusal::Dark)
    );
}

/// A rendered file's base is its linear value, which the conversion reads.
#[test]
fn the_film_base_picker_reads_a_rendered_files_linear_values() {
    let mut im = scan_with_rebate(COLOUR.base);
    im.metadata.source = crate::camera_data::Source::Rendered;
    let im = crate::develop::rendered::to_scene(im);
    let got = film_base_pick(&im, &Recipe::default(), 6. / 48., 0.5).unwrap();
    assert!(relative_error(got, COLOUR.base) < 2e-3, "{got:?}");
}
