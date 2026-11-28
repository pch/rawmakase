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
    let got = auto(&auto_scan(&COLOUR, None), &r).conversion;
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
    let got = auto(&auto_scan(&BLACK_WHITE, None), &r).conversion;
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
    let levelled = auto(&im, &r).conversion;
    let inversion = Inversion::new(&levelled, false);
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

/// A sliver of the film holder inside the crop, which recorded no light (a thousandth
/// of the sensor's white, as camera scans of film in a holder record it: flare and the
/// black level's noise), along one edge with a blurred border, covering up to about 5%
/// of the crop.
fn with_holder(im: &CameraImage, _film: &Film, side: usize, share: f32) -> CameraImage {
    let (w, h) = (im.width as f32, im.height as f32);
    let holder = [0.001; 3];
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

/// Auto's result on `im`, which must measure.
fn auto(im: &CameraImage, r: &Recipe) -> AutoLevels {
    auto_negative(im, r, Cancel::NEVER).unwrap()
}

/// The levels of `im` as every sample inside the crop, less the 3% inset and clipped
/// samples, gives them, nothing else left out: what Auto must give whatever the crop
/// holds.
fn levels_as_measured(im: &CameraImage, r: &Recipe) -> NegativeConversion {
    use crate::develop::auto::{WHITE_BALANCE_EDGE, analysis_edge, crop_grid};
    let n = r.negative.as_ref().unwrap();
    let small = crate::develop::preview(im, analysis_edge(im, r, WHITE_BALANCE_EDGE));
    let inversion = Inversion::new(n, small.metadata.is_rendered());
    let densities: Vec<[f32; 3]> = crop_grid(&small, r, AUTO_INSET)
        .samples
        .into_iter()
        .filter(|(_, p)| !clipped(*p, &small.metadata))
        .map(|(_, p)| inversion.density(p))
        .collect();
    let (dmin, dmax) = match n.film {
        FilmType::BlackWhite => (
            percentiles(&densities, AUTO_PERCENTILE),
            percentiles(&densities, 1. - AUTO_PERCENTILE),
        ),
        FilmType::Colour => colour_ends(&densities),
    };
    NegativeConversion {
        dmin,
        dmax,
        ..n.clone()
    }
}

/// A holder's sliver inside the crop, along any edge, for either film: Auto measures
/// it as part of the frame (it can't be told reliably from the scene's own dense
/// areas), so the levels are the crop's as it is, darker than the frame alone gives
/// them, and Auto says the crop holds part of the holder. The frame alone gets no such
/// note.
#[test]
fn auto_says_the_crop_holds_a_holder_sliver_and_measures_it() {
    for (film, kind) in [
        (&BLACK_WHITE, FilmType::BlackWhite),
        (&COLOUR, FilmType::Colour),
    ] {
        let r = unlevelled(film, kind);
        let frame = auto_scan(film, None);
        let alone = auto(&frame, &r);
        assert!(!alone.holder_in_crop, "{kind:?}");
        for side in 0..4 {
            for share in [0.045, 0.05] {
                let im = with_holder(&frame, film, side, share);
                let got = auto(&im, &r);
                assert!(got.holder_in_crop, "{kind:?} side {side} {share}");
                assert_eq!(got.conversion, levels_as_measured(&im, &r), "{kind:?}");
                assert!(
                    got.conversion.dmax[1] > alone.conversion.dmax[1] + 0.5,
                    "{kind:?} side {side} {share}: {:?}",
                    got.conversion
                );
            }
        }
    }
}

/// A 256 × 192 frame of densities 0.1 to 0.7 above `base`, its top `sky` rows at
/// density 1.5, and its top `holder` rows a holder recording no light (0.001 of the
/// sensor's white) above the sky.
fn sky_frame(base: f32, sky: u32, holder: u32) -> CameraImage {
    image(256, 192, |x, y| {
        if y < holder {
            return [0.001; 3];
        }
        let d = if y < sky {
            1.5
        } else {
            0.1 + 0.6 * x as f32 / 255.
        };
        [base * 10f32.powf(-d); 3]
    })
}
fn on_base(base: f32, kind: FilmType) -> Recipe {
    Recipe {
        negative: Some(NegativeConversion {
            base: [base; 3],
            ..NegativeConversion::new(kind)
        }),
        ..Default::default()
    }
}

/// A sky along a whole edge is measured as the photo's own, for either film: its
/// density is the white, and it renders as white. On a bright base (0.8) Auto says
/// nothing of a holder. On a dark one (0.1) the sky lets through less than a holder's
/// light floor, so Auto may say the crop holds part of the holder: a needless note,
/// with the levels as they are. The review's cases: the sky was cut as holder, and
/// rendered at 23 times white.
#[test]
fn auto_measures_a_sky_along_an_edge_whatever_the_base() {
    for kind in [FilmType::BlackWhite, FilmType::Colour] {
        for (base, note) in [(0.8, false), (0.1, true)] {
            let r = on_base(base, kind);
            let im = sky_frame(base, 16, 0);
            let got = auto(&im, &r);
            assert_eq!(got.conversion, levels_as_measured(&im, &r), "{kind:?}");
            assert!(
                got.conversion.dmax.iter().all(|d| (d - 1.5).abs() < 0.02),
                "{kind:?} on {base}: {:?}",
                got.conversion
            );
            let sky = Inversion::new(&got.conversion, false).apply([base * 10f32.powf(-1.5); 3]);
            assert!(
                sky.iter().all(|v| (v - 1.).abs() < 0.05),
                "{kind:?} on {base}: the sky renders at {sky:?}"
            );
            assert_eq!(got.holder_in_crop, note, "{kind:?} on {base}");
        }
    }
}

/// A holder above a sky along the same edge: both are measured, nothing cut, and Auto
/// says the crop holds part of the holder. The review's case: the holder and the sky
/// formed one region, which was cut whole, and the sky rendered at 23 times white.
#[test]
fn auto_measures_a_holder_beside_a_sky_and_says_so() {
    for kind in [FilmType::BlackWhite, FilmType::Colour] {
        let r = on_base(0.8, kind);
        for holder in [8, 10, 12] {
            let im = sky_frame(0.8, 16, holder);
            let got = auto(&im, &r);
            assert_eq!(got.conversion, levels_as_measured(&im, &r), "{kind:?}");
            assert!(got.holder_in_crop, "{kind:?} {holder}");
            // The sky is measured: the white is no thinner than its density.
            assert!(
                got.conversion.dmax.iter().all(|d| *d >= 1.48),
                "{kind:?} {holder}: {:?}",
                got.conversion
            );
        }
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
    let got = auto(&im, &r).conversion;
    let window_density = BLACK_WHITE.density([20.; 3])[0];
    assert!(
        (got.dmax[0] - window_density).abs() < 0.02,
        "{got:?} {window_density}"
    );
}

/// A highlight near the crop's edge, apart from the rest of the frame by a gap in
/// density, is measured as the photo's own: one 20 pixels in from the left edge
/// of a 256 × 192 frame, and one touching that edge along a short stretch of it, are
/// kept as the one in the middle is, for either film (the review's case: the one
/// near the edge was cut as no light, and rendered at 23 times white).
#[test]
fn auto_keeps_a_highlight_near_or_briefly_touching_the_edge() {
    for (film, kind) in [
        (&BLACK_WHITE, FilmType::BlackWhite),
        (&COLOUR, FilmType::Colour),
    ] {
        let r = unlevelled(film, kind);
        let highlight = film.density([1.; 3]);
        for (x0, y0) in [(90, 70), (20, 70), (0, 70)] {
            let im = image(256, 192, |x, y| {
                let e = if (x0..x0 + 20).contains(&x) && (y0..y0 + 50).contains(&y) {
                    1.
                } else {
                    // A frame up to 0.3, its own white well below the highlight.
                    10f32.powf(-2. + 1.48 * x as f32 / 255.)
                };
                film.scan([e; 3])
            });
            let got = auto(&im, &r).conversion;
            for c in 0..3 {
                assert!(
                    (got.dmax[c] - highlight[c]).abs() < 0.02,
                    "{kind:?} at x {x0}: {got:?}, highlight {highlight:?}"
                );
            }
        }
    }
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

/// With a lens's distortion corrected, the picker reads the pixels shown where it is
/// clicked: the click is mapped through the correction, as rendering maps it. On a
/// scan that brightens to the right, the shown point maps outward, to brighter
/// pixels, than the uncorrected geometry alone gives.
#[test]
fn the_film_base_picker_reads_through_the_lens_correction() {
    use crate::optics::{LensCorrection, Radial};
    let mut im = image(200, 120, |x, _| [0.1 + 0.002 * x as f32; 3]);
    im.metadata.lens = Some(LensCorrection {
        source: "test".into(),
        default_on: true,
        distortion: Some(Radial {
            knots: vec![0., 1.],
            values: vec![1., 0.7],
        }),
        ..Default::default()
    });
    let r = Recipe::for_metadata(&im.metadata);
    assert!(r.lens_correction(&im.metadata).is_some());
    let (u, v) = (0.85, 0.5);
    let view = crate::develop::ViewMapping::new(&im, &r);
    let [x, _] = view.frame().to_source(view.to_image(u, v));
    let shown = 0.1 + 0.002 * x.round();
    let unmapped = 0.1 + 0.002 * crate::develop::Geometry::new(&im, &r, 0).source(u, v)[0].round();
    assert!((shown - unmapped).abs() > 0.005, "{shown} {unmapped}");
    let got = film_base_pick(&im, &r, u, v).unwrap();
    assert!((got[0] - shown).abs() < 1e-4, "{got:?}, shown {shown}");
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

/// Valid narrow-range recipes stay finite on unlit / zero pixels and never produce
/// `inf` or `NaN` in the per-pixel pipeline.
#[test]
fn narrow_range_conversion_stays_finite_on_unlit_samples() {
    let n = NegativeConversion {
        film: FilmType::Colour,
        base: [1.; 3],
        dmin: [0.; 3],
        dmax: [0.05, 5., 5.],
    };
    n.validate().expect("valid conversion");
    let inv = Inversion::new(&n, false);
    for sample in [[0.; 3], [1e-6; 3], [-0.1, 0., 0.5], [f32::NAN; 3]] {
        let out = inv.apply(sample);
        assert!(
            out.iter().all(|v| v.is_finite() && *v > 0.),
            "non-finite inversion for {sample:?}: {out:?}"
        );
    }
    let im = image(32, 32, |x, _| if x < 8 { [0.; 3] } else { [0.2; 3] });
    let r = Recipe {
        negative: Some(n),
        ..Default::default()
    };
    let rendered = crate::develop::render(&im, &r.checked().unwrap(), 32, Cancel::NEVER).unwrap();
    let bright = rendered.pixels.iter().filter(|p| p[0] > 0.5).count();
    assert!(
        bright > 0,
        "unlit samples should render bright, not NaN-black"
    );
    assert!(
        rendered
            .pixels
            .iter()
            .all(|p| p.iter().all(|v| v.is_finite())),
        "non-finite rendered pixel"
    );
}

/// For a rendered negative scan, `ColorSpread::measure` checks clipping in the scan's
/// linear values (`negative::clipped`), not on Camera Raw's pseudo-scene values: bright
/// unclipped scan pixels (`0.95`) stay included, while clipped light-table pixels
/// (`0.985`, whose pseudo-scene falls below `0.94` so the raw `peak > 0.94` check kept
/// them) are excluded.
#[test]
fn color_spread_checks_rendered_negative_clip_in_linear_scan_space() {
    let n = COLOUR.conversion(FilmType::Colour);
    // Clean unclipped scan pixels (linear r = 0.95 < 0.98) vs. same scan with a clipped
    // light-table border (linear 0.985 >= 0.98, pseudo-scene < 0.94).
    let clean_raw = image(32, 32, |x, y| {
        if x < 16 {
            [0.95, 0.70, 0.45]
        } else {
            let t = (x - 16) as f32 / 15.;
            let s = (y as f32) / 31.;
            COLOUR.scan([0.05 + 0.8 * t, 0.05 + 0.8 * s, 0.05 + 0.8 * (1. - t)])
        }
    });
    let with_clipped_raw = image(32, 32, |x, y| {
        if x < 8 {
            [0.985; 3]
        } else {
            clean_raw.pixels[(y * 32 + x) as usize]
        }
    });
    let to_rendered = |mut im: CameraImage| {
        im.metadata.source = crate::camera_data::Source::Rendered;
        crate::develop::rendered::to_scene(im)
    };
    let clean_rendered = to_rendered(clean_raw.clone());
    let with_clipped_rendered = to_rendered(with_clipped_raw.clone());
    // Sanity check: the clipped light-table pixel (linear 0.985) really has pseudo-scene
    // < 0.94 (where the old raw-value check failed to exclude it), while `clipped` catches it.
    let clipped_px = with_clipped_rendered.pixels[0];
    assert!(
        clipped_px[0] < 0.94 && clipped(clipped_px, &with_clipped_rendered.metadata),
        "expected pseudo-scene < 0.94 and clipped=true, got {clipped_px:?}"
    );
    // Rendered matches RAW on the clean bright scan.
    let raw_spread = crate::develop::ColorSpread::measure(&clean_raw, Some(&n));
    let rendered_spread = crate::develop::ColorSpread::measure(&clean_rendered, Some(&n));
    for c in 0..3 {
        assert!(
            rendered_spread.mean[c] > 0.
                && (rendered_spread.mean[c] / raw_spread.mean[c] - 1.).abs() < 0.02,
            "channel {c}: rendered={:?} raw={:?}",
            rendered_spread.mean,
            raw_spread.mean
        );
    }
    // And clipped light-table pixels in the rendered scan are excluded just as in the RAW scan.
    let raw_with_clip = crate::develop::ColorSpread::measure(&with_clipped_raw, Some(&n));
    let rendered_with_clip = crate::develop::ColorSpread::measure(&with_clipped_rendered, Some(&n));
    for c in 0..3 {
        assert!(
            (rendered_with_clip.mean[c] / raw_with_clip.mean[c] - 1.).abs() < 0.02,
            "clipped exclusion mismatch c={c}: rendered={:?} raw={:?}",
            rendered_with_clip.mean,
            raw_with_clip.mean
        );
    }
}

/// A black film holder cropped out of a negative scan must not distort Shadows,
/// Highlights, Dehaze, Contrast, Clarity, or `ColorSpread::for_recipe`.
#[test]
fn cropped_out_film_holder_does_not_corrupt_tone_haze_or_color_spread() {
    let (w, h) = (160u32, 120u32);
    let scene_at = |x: u32, y: u32| -> [f32; 3] {
        let u = (x as f32 / (w - 1) as f32).clamp(0., 1.);
        let v = (y as f32 / (h - 1) as f32).clamp(0., 1.);
        // Rich tonal gradient with midtones, shadows and a highlight band.
        let l = 10f32.powf(-1.8 + 1.75 * (0.75 * u + 0.25 * v));
        [l, l * (0.85 + 0.15 * v), l * (0.75 + 0.25 * (1. - u))]
    };
    let clean = image(w, h, |x, y| COLOUR.scan(scene_at(x, y)));
    let with_holder = image(w, h, |x, y| {
        if x < 16 || x >= w - 16 || y < 12 || y >= h - 12 {
            [0.0005; 3]
        } else {
            COLOUR.scan(scene_at(x, y))
        }
    });
    let n = COLOUR.conversion(FilmType::Colour);
    let base = Recipe {
        negative: Some(n),
        crop: [0.1, 0.1, 0.9, 0.9],
        ..Default::default()
    };

    // ColorSpread::for_recipe matches the clean crop.
    let spread_clean = crate::develop::ColorSpread::for_recipe(&clean, &base);
    let spread_holder = crate::develop::ColorSpread::for_recipe(&with_holder, &base);
    for c in 0..3 {
        assert!(
            (spread_holder.mean[c] - spread_clean.mean[c]).abs() < 1e-4,
            "ColorSpread mean mismatch c={c}: holder={:?} clean={:?}",
            spread_holder.mean,
            spread_clean.mean
        );
    }

    // Tone, local tone, Dehaze, Contrast and Clarity must agree closely between
    // clean and holder-framed scans once cropped.
    type Edit = fn(&mut Recipe);
    let cases: [(&str, Edit); 5] = [
        ("Shadows +50", |r| r.shadows = 0.5),
        ("Highlights -50", |r| r.highlights = -0.5),
        ("Dehaze -50", |r| r.effects.dehaze = -0.5),
        ("Dehaze +50", |r| r.effects.dehaze = 0.5),
        ("Contrast +50 & Clarity +50", |r| {
            r.contrast = 0.5;
            r.effects.clarity = 0.5;
        }),
    ];
    for (name, edit) in cases {
        let mut r = base.clone();
        edit(&mut r);
        let valid = r.checked().unwrap();
        let a = crate::develop::render(&clean, &valid, 256, Cancel::NEVER).unwrap();
        let b = crate::develop::render(&with_holder, &valid, 256, Cancel::NEVER).unwrap();
        assert_eq!(a.pixels.len(), b.pixels.len());
        let mean_diff: f32 = a
            .pixels
            .iter()
            .zip(&b.pixels)
            .map(|(p, q)| {
                (p[0] - q[0])
                    .abs()
                    .max((p[1] - q[1]).abs())
                    .max((p[2] - q[2]).abs())
            })
            .sum::<f32>()
            / a.pixels.len() as f32;
        assert!(
            mean_diff < 0.02,
            "{name}: cropped-out film holder shifted render (mean diff {mean_diff:.4})"
        );
    }
    // Rendering through a shared PreviewRenderer (StageCache) before and after cropping
    // must invalidate the cached measures rather than reusing the uncropped holder's.
    let mut renderer = crate::develop::PreviewRenderer::default();
    let uncropped = Recipe {
        shadows: 0.5,
        crop: [0., 0., 1., 1.],
        ..base.clone()
    };
    let cropped = Recipe {
        shadows: 0.5,
        ..base
    };
    let _ = renderer
        .render(&with_holder, &uncropped, 256, None, Cancel::NEVER)
        .unwrap();
    let cached_cropped = renderer
        .render(&with_holder, &cropped, 256, None, Cancel::NEVER)
        .unwrap();
    let fresh_clean =
        crate::develop::render(&clean, &cropped.checked().unwrap(), 256, Cancel::NEVER).unwrap();
    let diff: f32 = cached_cropped
        .pixels
        .iter()
        .zip(&fresh_clean.pixels)
        .map(|(p, q)| {
            (p[0] - q[0])
                .abs()
                .max((p[1] - q[1]).abs())
                .max((p[2] - q[2]).abs())
        })
        .sum::<f32>()
        / fresh_clean.pixels.len() as f32;
    assert!(
        diff < 0.02,
        "cached measures stayed stale after cropping out holder (mean diff {diff:.4})"
    );
}

/// While the Crop tool displays the full uncropped scan, `MeasureCrop` keeps measuring
/// inside the active crop so entering or leaving the Crop tool does not shift tone.
#[test]
fn measure_crop_keeps_negative_tone_steady_in_crop_tool() {
    let (w, h) = (160u32, 120u32);
    let with_holder = image(w, h, |x, y| {
        if x < 16 || x >= w - 16 || y < 12 || y >= h - 12 {
            [0.0005; 3]
        } else {
            let u = (x as f32 / (w - 1) as f32).clamp(0., 1.);
            let v = (y as f32 / (h - 1) as f32).clamp(0., 1.);
            let l = 10f32.powf(-1.8 + 1.75 * (0.75 * u + 0.25 * v));
            COLOUR.scan([l, l * 0.9, l * 0.8])
        }
    });
    let crop = [0.1, 0.1, 0.9, 0.9];
    let cropped_recipe = Recipe {
        negative: Some(COLOUR.conversion(FilmType::Colour)),
        crop,
        shadows: 0.5,
        sharpening: 0.,
        effects: crate::model::effects::Effects {
            dehaze: 0.5,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut crop_tool_recipe = cropped_recipe.clone();
    crop_tool_recipe.crop = [0., 0., 1., 1.];
    crop_tool_recipe.constrain_crop = false;

    let mut renderer = crate::develop::PreviewRenderer::default();
    let normal = renderer
        .render(&with_holder, &cropped_recipe, 0, None, Cancel::NEVER)
        .unwrap();
    let without_override = renderer
        .render(&with_holder, &crop_tool_recipe, 0, None, Cancel::NEVER)
        .unwrap();
    renderer.set_measure_crop(Some(crate::develop::MeasureCrop {
        crop,
        constrain_crop: cropped_recipe.constrain_crop,
    }));
    let in_crop_tool = renderer
        .render(&with_holder, &crop_tool_recipe, 0, None, Cancel::NEVER)
        .unwrap();

    // Without MeasureCrop the uncropped holder shifts tone; with it every pixel inside
    // the active crop matches the normal cropped view without a tone jump.
    assert_eq!((in_crop_tool.width, in_crop_tool.height), (w, h));
    assert_eq!((normal.width, normal.height), (128, 96));
    let mut max_diff_without = 0f32;
    let mut max_diff_with = 0f32;
    for y in 0..normal.height {
        for x in 0..normal.width {
            let a = normal.pixels[(y * normal.width + x) as usize];
            let idx = ((y + 12) * in_crop_tool.width + (x + 16)) as usize;
            let b_without = without_override.pixels[idx];
            let b_with = in_crop_tool.pixels[idx];
            for c in 0..3 {
                max_diff_without = max_diff_without.max((a[c] - b_without[c]).abs());
                max_diff_with = max_diff_with.max((a[c] - b_with[c]).abs());
            }
        }
    }
    assert!(
        max_diff_without > 0.05,
        "test setup must show a tone jump without MeasureCrop (got {max_diff_without:.6})"
    );
    assert!(
        max_diff_with < 1e-5,
        "Crop tool shifted negative tone inside active crop (max diff {max_diff_with:.6})"
    );
}

/// Switching the Transform panel off must make `ColorSpread::for_recipe` sample the
/// untransformed frame, matching what renders.
#[test]
fn color_spread_respects_switched_off_transform_panel() {
    use crate::model::panels::{Panel, PanelState};
    let (w, h) = (120u32, 90u32);
    let scan = image(w, h, |x, y| {
        let u = x as f32 / (w - 1) as f32;
        let v = y as f32 / (h - 1) as f32;
        let scene = if v < 0.35 {
            [0.8, 0.2, 0.1]
        } else {
            [0.1 + 0.3 * u, 0.5, 0.7]
        };
        COLOUR.scan(scene)
    });
    let base = Recipe {
        negative: Some(COLOUR.conversion(FilmType::Colour)),
        ..Default::default()
    };
    let mut transformed_on = base.clone();
    transformed_on.transform.vertical = 0.6;
    let mut transformed_off = transformed_on.clone();
    transformed_off
        .panels
        .set(Panel::Transform, PanelState::Off);

    let spread_base = crate::develop::ColorSpread::for_recipe(&scan, &base);
    let spread_on = crate::develop::ColorSpread::for_recipe(&scan, &transformed_on);
    let spread_off = crate::develop::ColorSpread::for_recipe(&scan, &transformed_off);
    assert_ne!(spread_base, spread_on, "active transform changes spread");
    assert_eq!(
        spread_base, spread_off,
        "switched-off Transform panel must match the untransformed frame"
    );
}
