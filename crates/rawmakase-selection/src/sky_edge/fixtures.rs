//! Synthetic sky scenes shared by the edge modes' tests, and the run that turns a mode's
//! answer into coverage as `blend` does where a mode has full weight: the model's answer
//! resampled to the coverage's size, the mode, then the final luma guided filter.

use crate::auto::{colours, luma};
use crate::process::{Coverage, Rect, RgbImage, output_size, resample};
use crate::refine::guided;

/// An edge mode: sky coverage from the photo's colours and the model's answer.
pub(super) type Mode = fn(&[[f32; 3]], &[f32], usize, usize) -> Vec<f32>;

/// Coverage from `mode` for the model's `probability` (`model_width` x `model_height`).
pub(super) fn run(
    mode: impl Fn(&[[f32; 3]], &[f32], usize, usize) -> Vec<f32>,
    image: &RgbImage,
    probability: &[f32],
    model_width: usize,
    model_height: usize,
) -> Coverage {
    let (colours, soft, width, height) = inputs(image, probability, model_width, model_height);
    let decided = mode(&colours, &soft, width, height);
    let refined = guided(
        &luma(image, width, height),
        &decided,
        width,
        height,
        2,
        1e-3,
    );
    Coverage {
        width,
        height,
        data: refined
            .iter()
            .map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
            .collect(),
    }
}

/// What a mode is given for the model's `probability` (`model_width` x `model_height`):
/// the photo's colours and the model's answer at the coverage's size, and that size.
pub(super) fn inputs(
    image: &RgbImage,
    probability: &[f32],
    model_width: usize,
    model_height: usize,
) -> (Vec<[f32; 3]>, Vec<f32>, usize, usize) {
    let (width, height) = output_size(image);
    let model = Rect {
        x: 0,
        y: 0,
        width: model_width,
        height: model_height,
    };
    let soft = resample(probability, model_width, model, width, height);
    (colours(image, width, height), soft, width, height)
}

/// A photo of `w` x `h`: `paint(x, y)` colours each pixel.
pub(super) fn photo(w: usize, h: usize, paint: impl Fn(usize, usize) -> [u8; 3]) -> RgbImage {
    let mut data = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            data.extend(paint(x, y));
        }
    }
    RgbImage {
        width: w,
        height: h,
        data,
    }
}

/// `f(x, y)` over a `w` x `h` model answer, x and y as fractions of the frame.
pub(super) fn answer(w: usize, h: usize, f: impl Fn(f32, f32) -> f32) -> Vec<f32> {
    (0..w * h)
        .map(|i| f((i % w) as f32 / w as f32, (i / w) as f32 / h as f32))
        .collect()
}

/// The coverage at `fx`, `fy` (fractions of the frame).
pub(super) fn at(c: &Coverage, fx: f32, fy: f32) -> u8 {
    c.data[(fy * c.height as f32) as usize * c.width + (fx * c.width as f32) as usize]
}

/// A tree crown and a spire against a sky gradient, and a model answer that, like
/// UperNet's, calls the whole crown not sky and the spire's thin tip sky. `sky` and
/// `branch` colour the two; the crown's dark lines are 3 pixels wide every 12, with
/// 9 x 9 gaps of sky between them.
pub(super) fn crown_of(
    w: usize,
    h: usize,
    sky: impl Fn(usize) -> [u8; 3],
    branch: [u8; 3],
) -> (RgbImage, Vec<f32>) {
    let (sx, sy) = (w as f32 / 1024., h as f32 / 683.);
    let scaled = |x: usize, y: usize| ((x as f32 / sx) as usize, (y as f32 / sy) as usize);
    let in_crown = |x: usize, y: usize| (300..700).contains(&x) && (150..400).contains(&y);
    let in_spire = |x: usize, y: usize| (850..853).contains(&x) && (100..400).contains(&y);
    let img = photo(w, h, |px, py| {
        let (x, y) = scaled(px, py);
        if y >= 400 {
            [40, 60, 30]
        } else if in_spire(x, y) || (in_crown(x, y) && ((x - 300) % 12 < 3 || (y - 150) % 12 < 3)) {
            branch
        } else {
            sky(y)
        }
    });
    let probability = (0..w * h)
        .map(|i| {
            let (x, y) = scaled(i % w, i / w);
            if y >= 400 || in_crown(x, y) || (in_spire(x, y) && y >= 250) {
                0.01
            } else {
                0.99
            }
        })
        .collect();
    (img, probability)
}

pub(super) fn crown() -> (RgbImage, Vec<f32>) {
    crown_of(
        1024,
        683,
        |y| [110 + (y / 8) as u8, 150 + (y / 10) as u8, 225],
        [30, 35, 25],
    )
}

/// A lake reflecting the sky, broken up by reflected trees, near the sky and where
/// the model is `faint`ly unsure: a large sky-coloured area, not gaps in a crown.
pub(super) fn reflection(faint: f32) -> (RgbImage, Vec<f32>) {
    let blob = |x: usize, y: usize| x % 60 < 24 && (220..370).contains(&y) && (y - 220) % 60 < 24;
    let img = photo(800, 600, |x, y| match y {
        0..150 => [200, 205, 212],
        150..200 => [50, 70, 40],
        200..300 if blob(x, y) => [40, 55, 35],
        200..300 => [190, 196, 205],
        _ => [60, 55, 45],
    });
    let probability = answer(800, 600, |x, y| {
        if y < 150. / 600. {
            0.99
        } else if (0.25..0.75).contains(&x) && (220. / 600.0..280. / 600.).contains(&y) {
            faint
        } else {
            0.01
        }
    });
    (img, probability)
}

/// A weathered trunk standing in a streaky sky, whose outline the model drew a few
/// pixels too wide: the sky along it is found, but the pale wood inside stays out
/// and the sky beside it stays entirely selected.
pub(super) fn trunk() -> (RgbImage, Vec<f32>) {
    let img = photo(640, 480, |x, y| {
        if (200..260).contains(&x) && y >= 80 {
            if (204..207).contains(&x) || (252..255).contains(&x) {
                [120, 112, 100]
            } else {
                [45, 40, 35]
            }
        } else if y >= 300 {
            [50, 45, 40]
        } else if x < 120 || (y / 12 + x / 50) % 2 == 0 {
            [200, 205, 212]
        } else {
            [165, 172, 185]
        }
    });
    let probability = answer(640, 480, |x, y| {
        let wide_trunk = (197. / 640.0..263. / 640.).contains(&x) && y >= 77. / 480.;
        if y >= 300. / 480. || wide_trunk {
            0.01
        } else {
            0.97
        }
    });
    (img, probability)
}

/// A sparse bare canopy over the ground, which the model, like UperNet, calls
/// foreground as a whole: thin twigs 2 pixels wide in a lattice of cells far larger
/// than a gap in a dense crown, with finer 1-pixel twigs across them, and the sky
/// behind them one large connected area. `sky(x, y)` colours the sky.
pub(super) fn sparse_canopy(sky: impl Fn(usize, usize) -> [u8; 3]) -> (RgbImage, Vec<f32>) {
    let in_canopy = |x: usize, y: usize| (300..700).contains(&x) && (150..400).contains(&y);
    let twig = |x: usize, y: usize| x % 48 < 2 || y % 40 < 2;
    let fine = |x: usize, y: usize| (x + 2 * y).is_multiple_of(57);
    let img = photo(1024, 683, |x, y| {
        if y >= 400 {
            [40, 60, 30]
        } else if in_canopy(x, y) && (twig(x, y) || fine(x, y)) {
            [35, 32, 30]
        } else {
            sky(x, y)
        }
    });
    let probability = answer(1024, 683, |fx, fy| {
        let (x, y) = ((fx * 1024.) as usize, (fy * 683.) as usize);
        if y >= 400 || in_canopy(x, y) {
            0.01
        } else {
            0.99
        }
    });
    (img, probability)
}

pub(super) type Pixels = Vec<(usize, usize)>;

/// The [`sparse_canopy`]'s gaps (at least 4 pixels from any twig) and its twigs.
pub(super) fn canopy_pixels() -> (Pixels, Pixels) {
    let near_twig = |x: usize, y: usize| {
        (x.saturating_sub(4)..x + 5).any(|tx| tx % 48 < 2)
            || (y.saturating_sub(4)..y + 5).any(|ty| ty % 40 < 2)
            || (x.saturating_sub(4)..x + 5)
                .any(|tx| (y.saturating_sub(4)..y + 5).any(|ty| (tx + 2 * ty).is_multiple_of(57)))
    };
    let gaps = (160..390)
        .step_by(7)
        .flat_map(|y| (310..690).step_by(7).map(move |x| (x, y)))
        .filter(|(x, y)| !near_twig(*x, *y))
        .collect();
    let twigs = (165..390)
        .step_by(23)
        .flat_map(|y| [(336, y), (337, y), (481, y), (624, y)])
        .chain((310..690).step_by(17).map(|x| (x, 200)))
        .collect();
    (gaps, twigs)
}

/// Whether a pixel is a twig of the bare tree in front of [`building`]: a crown
/// with small gaps, half against the sky and half against the building.
pub(super) fn tree_twig(x: usize, y: usize) -> bool {
    (200..420).contains(&x) && (200..420).contains(&y) && (x % 14 < 2 || y % 12 < 2)
}

/// An overcast sky over a building with a bare tree in front ([`tree_twig`]), and a
/// model answer that calls the tree's crown not sky and fades over `ramp` rows from
/// sky (0.99) to not sky (0.01): a wide ramp reaching into the building. `wall`
/// colours the building from row 300 down: its face, its windows or courses.
pub(super) fn building(
    wall: impl Fn(usize, usize) -> [u8; 3],
    ramp: std::ops::Range<f32>,
) -> (RgbImage, Vec<f32>) {
    let img = photo(1024, 683, |x, y| {
        if tree_twig(x, y) {
            [38, 34, 30]
        } else if y >= 300 && (150..870).contains(&x) {
            wall(x, y)
        } else if y >= 500 {
            [50, 48, 45]
        } else {
            [176, 180, 189]
        }
    });
    let probability = answer(1024, 683, |fx, fy| {
        let (x, y) = (fx * 1024., fy * 683.);
        if (200.0..420.).contains(&x) && (200.0..420.).contains(&y) {
            return 0.01;
        }
        let start = if (150.0..870.).contains(&x) {
            ramp.start
        } else {
            500.
        };
        let end = if (150.0..870.).contains(&x) {
            ramp.end
        } else {
            500.
        };
        if y < start {
            0.99
        } else if y >= end {
            0.01
        } else {
            0.99 - 0.98 * (y - start) / (end - start)
        }
    });
    (img, probability)
}

/// The truss of a bridge's arch against a pale overcast sky: dark struts 8 pixels
/// wide crossing at angles, with the sky between them in gaps the size of a strut's
/// length. The truss lies within the band where the model's answer is questioned.
pub(super) fn truss_strut(x: usize, y: usize) -> bool {
    (200..800).contains(&x)
        && (250..390).contains(&y)
        && ((x + y) % 64 < 8 || (x + 2048 - y) % 64 < 8 || y % 48 < 8)
}

// The checks both modes pass, each run through a mode.

pub(super) fn sky_between_branches_is_found_and_the_branches_are_not(mode: Mode) {
    let (img, p) = crown();
    let cov = run(mode, &img, &p, 1024, 683);
    let px = |x: usize, y: usize| cov.data[y * cov.width + x];
    for (x, y) in [(307, 157), (499, 265), (691, 385), (403, 301)] {
        assert!(px(x, y) > 204, "gap {x},{y}: {}", px(x, y));
    }
    for (x, y) in [(301, 200), (500, 151), (601, 289)] {
        assert!(px(x, y) < 51, "branch {x},{y}: {}", px(x, y));
    }
    assert!(px(851, 150) < 51, "spire tip {}", px(851, 150));
    assert!(px(851, 320) < 51, "spire {}", px(851, 320));
    assert!(px(100, 50) > 240 && px(500, 600) < 10);
}

pub(super) fn a_sky_coloured_wall_next_to_the_sky_stays_out_up_to_its_edge(mode: Mode) {
    let img = photo(640, 480, |_, y| {
        if y < 200 {
            [120, 160, 230]
        } else {
            [126, 158, 222]
        }
    });
    let probability = answer(640, 480, |_, y| if y < 200. / 480. { 0.99 } else { 0.01 });
    let cov = run(mode, &img, &probability, 640, 480);
    assert!(
        at(&cov, 0.5, 210. / 480.) < 51,
        "{}",
        at(&cov, 0.5, 210. / 480.)
    );
    assert!(at(&cov, 0.5, 0.7) < 10 && at(&cov, 0.5, 0.2) > 240);
}

/// Fine twigs against the sky are neither sky nor branch: where a crown has gaps,
/// a pixel that is partly sky keeps that part instead of being forced to one side.
pub(super) fn twigs_in_a_crown_with_gaps_are_partly_sky(mode: Mode) {
    let (sky_colour, branch) = ([130u8, 170, 230], [30u8, 35, 25]);
    let blend: [u8; 3] = std::array::from_fn(|k| {
        (0.35 * f32::from(sky_colour[k]) + 0.65 * f32::from(branch[k])) as u8
    });
    let in_box = |x: usize, y: usize, x0: usize, y0: usize, side: usize| {
        (x0..x0 + side).contains(&x) && (y0..y0 + side).contains(&y)
    };
    let img = photo(1024, 683, |x, y| {
        if y >= 400 {
            [40, 60, 30]
        } else if !in_box(x, y, 300, 150, 400)
            || in_box(x, y, 400, 200, 9)
            || in_box(x, y, 600, 300, 9)
        {
            sky_colour
        } else if in_box(x, y, 420, 200, 3) || in_box(x, y, 620, 300, 3) {
            blend
        } else {
            branch
        }
    });
    let probability = answer(1024, 683, |x, y| {
        let crown =
            (300. / 1024.0..700. / 1024.).contains(&x) && (150. / 683.0..400. / 683.).contains(&y);
        if y >= 400. / 683. || crown {
            0.01
        } else {
            0.99
        }
    });
    let cov = run(mode, &img, &probability, 1024, 683);
    let px = |x: usize, y: usize| cov.data[y * cov.width + x];
    assert!(px(404, 204) > 204, "gap {}", px(404, 204));
    for (x, y) in [(421, 201), (621, 301)] {
        assert!((51..153).contains(&px(x, y)), "twig {x},{y}: {}", px(x, y));
    }
    assert!(px(500, 350) < 26, "branch {}", px(500, 350));
}

pub(super) fn reflection_stays_out(mode: Mode, faint: f32) {
    let (img, probability) = reflection(faint);
    let cov = run(mode, &img, &probability, 800, 600);
    let px = |x: usize, y: usize| cov.data[y * cov.width + x];
    for (x, y) in [
        (27, 232),
        (45, 232),
        (45, 262),
        (330, 250),
        (230, 262),
        (500, 290),
    ] {
        assert!(px(x, y) < 26, "water {x},{y}: {}", px(x, y));
    }
    assert!(px(400, 50) > 240);
}

/// A faint wisp of "sky" the model draws across water, far from the sky: the colours
/// are not asked about it, so it stays as little sky as before.
pub(super) fn a_faint_wisp_on_the_water_away_from_the_sky_stays_out(mode: Mode) {
    let wisp = |x: f32, y: f32| (y - (350. + 20. * (x / 40.).sin())).abs() < 4.;
    let img = photo(800, 600, |_, y| match y {
        0..150 => [200, 205, 212],
        150..200 => [50, 70, 40],
        _ => [120, 125, 135],
    });
    let probability = answer(800, 600, |fx, fy| {
        if fy < 150. / 600. {
            0.99
        } else if wisp(fx * 800., fy * 600.) {
            0.3
        } else {
            0.01
        }
    });
    let cov = run(mode, &img, &probability, 800, 600);
    for x in (0..800).step_by(25) {
        let y = (350. + 20. * (x as f32 / 40.).sin()).round() as usize;
        let v = cov.data[y * cov.width + x];
        assert!(v < 26, "wisp {x},{y}: {v}");
    }
}

/// A streaky sky beside a crown, where the model's answer fades over a wide ramp:
/// the darker streaks are sky as much as the bright ones.
pub(super) fn a_streaky_sky_where_the_model_fades_stays_entirely_selected(mode: Mode) {
    let img = photo(640, 480, |x, y| {
        if (300..400).contains(&x) && (100..300).contains(&y) || y >= 300 {
            [35, 45, 30]
        } else if x < 120 || (y / 12 + x / 50) % 2 == 0 {
            [200, 205, 212]
        } else {
            [165, 172, 185]
        }
    });
    let probability = answer(640, 480, |fx, fy| {
        let (x, y) = (fx * 640., fy * 480.);
        if y >= 300. || (300.0..400.).contains(&x) && (100.0..300.).contains(&y) {
            return 0.01;
        }
        let dx = (300. - x).max(x - 400.).max(0.);
        let dy = (100. - y).max(0.);
        (0.3 + dx.max(dy) / 40. * 0.67).min(0.97)
    });
    let cov = run(mode, &img, &probability, 640, 480);
    let low = (110..290)
        .flat_map(|y| (250..290).chain(410..450).map(move |x| (x, y)))
        .map(|(x, y)| cov.data[y * cov.width + x])
        .min()
        .unwrap();
    assert!(low > 240, "{low}");
}

pub(super) fn pale_wood_in_a_trunk_beside_the_sky_stays_out(mode: Mode) {
    let (img, p) = trunk();
    let cov = run(mode, &img, &p, 640, 480);
    let px = |x: usize, y: usize| cov.data[y * cov.width + x];
    for (x, y) in [(205, 150), (253, 250), (230, 200)] {
        assert!(px(x, y) < 26, "trunk {x},{y}: {}", px(x, y));
    }
    assert!(px(198, 150) > 153, "sky along the trunk {}", px(198, 150));
}

pub(super) fn a_streaky_sky_beside_a_trunk_stays_entirely_selected(mode: Mode) {
    let (img, p) = trunk();
    let cov = run(mode, &img, &p, 640, 480);
    let low = (20..280)
        .flat_map(|y| (150..190).chain(270..310).map(move |x| (x, y)))
        .map(|(x, y)| cov.data[y * cov.width + x])
        .min()
        .unwrap();
    assert!(low > 240, "{low}");
}

/// A patch of duller sky right beside a crown, all of it where the model's answer
/// fades: it is measured against its own sky, not the brighter sky farther off.
pub(super) fn a_duller_patch_of_sky_where_the_model_fades_stays_entirely_selected(mode: Mode) {
    let img = photo(640, 480, |x, y| {
        if (300..400).contains(&x) && (100..300).contains(&y) || y >= 300 {
            [35, 45, 30]
        } else if (262..300).contains(&x) && (150..250).contains(&y) {
            [170, 176, 188]
        } else {
            [200, 205, 212]
        }
    });
    let probability = answer(640, 480, |fx, fy| {
        let (x, y) = (fx * 640., fy * 480.);
        if y >= 300. || (300.0..400.).contains(&x) && (100.0..300.).contains(&y) {
            return 0.01;
        }
        let dx = (300. - x).max(x - 400.).max(0.);
        let dy = (100. - y).max(0.);
        (0.3 + dx.max(dy) / 40. * 0.67).min(0.97)
    });
    let cov = run(mode, &img, &probability, 640, 480);
    let low = (160..240)
        .flat_map(|y| (266..295).map(move |x| (x, y)))
        .map(|(x, y)| cov.data[y * cov.width + x])
        .min()
        .unwrap();
    assert!(low > 240, "{low}");
}

/// A cloudy sky the model is fairly but not entirely sure of is all sky: darkening
/// it must not leave a blotchy sky behind.
pub(super) fn a_cloudy_sky_away_from_its_edge_is_entirely_selected(mode: Mode) {
    let img = photo(640, 480, |x, y| {
        if y >= 300 {
            [40, 60, 30]
        } else if (x / 40 + y / 30) % 2 == 0 {
            [235, 238, 242]
        } else {
            [150, 165, 190]
        }
    });
    let probability = answer(640, 480, |_, y| if y < 300. / 480. { 0.93 } else { 0.01 });
    let cov = run(mode, &img, &probability, 640, 480);
    let low = (0..250 * 640).map(|i| cov.data[i]).min().unwrap();
    assert!(low > 250, "{low}");
}

pub(super) fn dark_sky_behind_dark_trees_keeps_the_models_answer(mode: Mode) {
    let (img, p) = crown_of(1024, 683, |_| [22, 24, 34], [18, 20, 22]);
    let cov = run(mode, &img, &p, 1024, 683);
    let px = |x: usize, y: usize| cov.data[y * cov.width + x];
    for (x, y) in [(499, 265), (403, 301), (601, 289), (500, 199)] {
        assert!(px(x, y) < 26, "crown {x},{y}: {}", px(x, y));
    }
    assert!(px(100, 50) > 229);
}

pub(super) fn tiny_and_extreme_photos_do_not_break(mode: Mode) {
    for (w, h) in [(64, 43), (2560, 64), (64, 2560)] {
        let (img, p) = crown_of(w, h, |_| [120, 160, 230], [30, 35, 25]);
        let cov = run(mode, &img, &p, w, h);
        assert_eq!((cov.width, cov.height), (w, h));
    }
}

pub(super) fn all_sky_is_the_models_answer(mode: Mode) {
    let img = photo(320, 240, |_, _| [120, 160, 230]);
    let cov = run(mode, &img, &vec![0.99; 320 * 240], 320, 240);
    assert!(cov.data.iter().all(|v| *v > 240));
}

pub(super) fn sky_is_where_the_model_says_with_its_edge_on_the_photos(mode: Mode) {
    // The horizon is at row 267; the model's answer is a few pixels too low.
    let img = photo(1024, 683, |_, y| {
        if y < 267 {
            [120, 160, 230]
        } else {
            [40, 70, 30]
        }
    });
    let probability = answer(1024, 683, |_, y| if y < 272. / 683. { 0.99 } else { 0.01 });
    let cov = run(mode, &img, &probability, 1024, 683);
    assert_eq!((cov.width, cov.height), (1024, 683));
    assert!(at(&cov, 0.5, 0.1) > 240 && at(&cov, 0.5, 0.8) < 10);
    // Under the true edge, where the model's answer still says sky, it has moved up.
    let below = at(&cov, 0.5, 271.5 / 683.);
    assert!(below < 200, "{below}");
}

pub(super) fn water_under_the_sky_is_left_out_when_the_model_leaves_it_out(mode: Mode) {
    // Sky-coloured water below a band of ground: nothing here looks at where it lies.
    let img = photo(640, 480, |_, y| match y {
        0..=199 => [120, 160, 230],
        200..=279 => [40, 60, 30],
        _ => [115, 155, 225],
    });
    let probability = answer(640, 480, |_, y| if y < 200. / 480. { 0.99 } else { 0.01 });
    let cov = run(mode, &img, &probability, 640, 480);
    assert!(at(&cov, 0.5, 0.2) > 240 && at(&cov, 0.5, 0.8) < 10);
}

pub(super) fn sky_below_leaves_along_the_top_is_kept(mode: Mode) {
    let img = photo(640, 480, |_, y| match y {
        0..=59 => [20, 50, 25],
        60..=279 => [120, 160, 230],
        _ => [60, 45, 30],
    });
    let probability = answer(640, 480, |_, y| {
        if (60. / 480.0..280. / 480.).contains(&y) {
            0.99
        } else {
            0.01
        }
    });
    let cov = run(mode, &img, &probability, 640, 480);
    assert!(at(&cov, 0.5, 0.3) > 240 && at(&cov, 0.5, 0.05) < 10 && at(&cov, 0.5, 0.8) < 10);
}
