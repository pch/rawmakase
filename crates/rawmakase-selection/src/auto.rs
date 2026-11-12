//! Automatic Subject and Sky selection from what the models said.
//!
//! Segment Anything 2 answers a prompt with an object's outline but does not know which
//! object matters; the panoptic model knows where the people and animals are but draws
//! blobs. Subject keeps the outlines SAM 2 drew that lie inside the people and animals and
//! fills any gap with the label itself. Sky is what the sky model says, pixel by pixel.
//! Both end with their edges moved onto the photo's, Sky's in the way the kind of
//! horizon there calls for. Everything here is pure: what the models said comes in,
//! coverage in the photo's frame goes out.
use crate::kinds::Kinds;
use crate::panoptic::Instance;
use crate::process::{Coverage, Point, Prompt, Rect, RgbImage, output_size, resample};
use crate::refine::{guided, smooth};
use crate::sky_edge;
use rawmakase_cancel::Cancel;
use rawmakase_inference::InferenceError;

/// Side of the square grid the proposals and the labels live on: the photo
/// stretched to a square, as the models see it.
pub const SIDE: usize = 256;

/// Subject: `mask` (the union of what SAM 2 drew for each person and animal, 0..=1 on
/// the [`SIDE`] grid) with its edge moved onto the photo's. All zero when it is empty.
pub fn subject(image: &RgbImage, mask: &[f32]) -> Result<Coverage, InferenceError> {
    check(mask.len())?;
    let (width, height) = output_size(image);
    if mask.iter().sum::<f32>() < 0.002 * (SIDE * SIDE) as f32 {
        return Ok(empty(width, height));
    }
    let guide = luma(image, width, height);
    let soft = expand(mask, width, height);
    let refined = guided(
        &guide,
        &soft,
        width,
        height,
        (width.max(height) / 200).max(3),
        1e-3,
    );
    Ok(Coverage {
        width,
        height,
        data: refined
            .iter()
            .map(|v| (smooth(*v) * 255.0 + 0.5) as u8)
            .collect(),
    })
}

/// The prompt that makes SAM 2 draw `instance`: its box, a few points well inside it
/// and a point inside each other instance, which is not part of it.
pub fn aim(instance: &Instance, others: &[&Instance]) -> Prompt {
    let inside: Vec<bool> = instance.mask.iter().map(|v| *v > 0.5).collect();
    let inner = depth(&inside);
    let cell = |i: usize| ((i % SIDE) as f32 + 0.5) / SIDE as f32;
    let row = |i: usize| ((i / SIDE) as f32 + 0.5) / SIDE as f32;
    let deepest = |d: &[f32]| {
        d.iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, v)| (i, *v))
    };
    let mut points = Vec::new();
    let mut chosen: Vec<usize> = Vec::new();
    if let Some((first, peak)) = deepest(&inner) {
        chosen.push(first);
        // Two more, far from each other, still well inside.
        let well: Vec<usize> = (0..SIDE * SIDE)
            .filter(|i| inner[*i] > 0.5 * peak)
            .collect();
        for _ in 0..2 {
            let far = well.iter().copied().max_by_key(|c| {
                chosen
                    .iter()
                    .map(|p| {
                        let (dx, dy) = (
                            (c % SIDE) as i64 - (p % SIDE) as i64,
                            (c / SIDE) as i64 - (p / SIDE) as i64,
                        );
                        dx * dx + dy * dy
                    })
                    .min()
                    .unwrap_or(0)
            });
            if let Some(far) = far.filter(|f| !chosen.contains(f)) {
                chosen.push(far);
            }
        }
    }
    points.extend(chosen.iter().map(|i| Point {
        x: cell(*i),
        y: row(*i),
        positive: true,
    }));
    for other in others {
        let theirs: Vec<bool> = other.mask.iter().map(|v| *v > 0.5).collect();
        if let Some((i, _)) = deepest(&depth(&theirs)).filter(|(i, _)| !inside[*i]) {
            points.push(Point {
                x: cell(i),
                y: row(i),
                positive: false,
            });
        }
    }
    let (mut x0, mut y0, mut x1, mut y1) = (SIDE, SIDE, 0, 0);
    for (i, _) in inside.iter().enumerate().filter(|(_, m)| **m) {
        x0 = x0.min(i % SIDE);
        x1 = x1.max(i % SIDE);
        y0 = y0.min(i / SIDE);
        y1 = y1.max(i / SIDE);
    }
    let unit = |v: usize, up: bool| {
        let edge = if up { v + 1 } else { v };
        (edge as f32 / SIDE as f32).clamp(0.0, 1.0)
    };
    let bounds = (x1 >= x0 && y1 >= y0).then(|| {
        [
            unit(x0, false),
            unit(y0, false),
            unit(x1, true),
            unit(y1, true),
        ]
    });
    Prompt { points, bounds }
}

/// How far inside `mask` each cell is (city-block distance to the nearest outside cell,
/// 0 outside), by two sweeps.
fn depth(mask: &[bool]) -> Vec<f32> {
    let big = (2 * SIDE) as f32;
    let mut d: Vec<f32> = mask.iter().map(|m| if *m { big } else { 0.0 }).collect();
    for y in 0..SIDE {
        for x in 0..SIDE {
            let i = y * SIDE + x;
            if d[i] == 0.0 {
                continue;
            }
            let up = if y > 0 { d[i - SIDE] } else { 0.0 };
            let left = if x > 0 { d[i - 1] } else { 0.0 };
            d[i] = d[i].min(up + 1.0).min(left + 1.0);
        }
    }
    for y in (0..SIDE).rev() {
        for x in (0..SIDE).rev() {
            let i = y * SIDE + x;
            if d[i] == 0.0 {
                continue;
            }
            let down = if y + 1 < SIDE { d[i + SIDE] } else { 0.0 };
            let right = if x + 1 < SIDE { d[i + 1] } else { 0.0 };
            d[i] = d[i].min(down + 1.0).min(right + 1.0);
        }
    }
    d
}

/// How much two masks overlap, intersection over union.
pub fn iou(a: &[bool], b: &[bool]) -> f32 {
    let both = a.iter().zip(b).filter(|(x, y)| **x && **y).count();
    let either = a.iter().zip(b).filter(|(x, y)| **x || **y).count();
    if either == 0 {
        0.0
    } else {
        both as f32 / either as f32
    }
}

/// Below this fraction of the frame the sky model's answer is taken for no sky: a few
/// stray pixels are not a selection worth making.
const SKY_LEAST: f32 = 0.01;

/// Sky: `probability`, the sky model's answer (0..=1 at each of `model_width` x
/// `model_height` pixels), with its edge snapped onto the photo's, and moved by an edge
/// mode where a crown or a lattice calls for one ([`sky_edge::blend`]). All zero when it
/// finds none. `kinds` is what the model sees outside the sky, on its own grid; `cancel`
/// is checked before the edge is snapped, before and inside each edge mode, which takes
/// seconds at the sky's input size, and before the final mix.
#[cfg(test)]
pub(crate) fn sky(
    image: &RgbImage,
    probability: &[f32],
    model_width: usize,
    model_height: usize,
    kinds: &Kinds,
    cancel: Cancel<'_>,
) -> Result<Coverage, InferenceError> {
    sky_with_modes(image, probability, model_width, model_height, kinds, cancel)
        .map(|(coverage, _)| coverage)
}

/// [`sky`], and which edge modes it ran and how long they took.
pub(crate) fn sky_with_modes(
    image: &RgbImage,
    probability: &[f32],
    model_width: usize,
    model_height: usize,
    kinds: &Kinds,
    cancel: Cancel<'_>,
) -> Result<(Coverage, sky_edge::Modes), InferenceError> {
    if probability.len() != model_width * model_height
        || probability
            .iter()
            .any(|p| !p.is_finite() || !(0.0..=1.0).contains(p))
    {
        return Err(InferenceError::OutputInvalid(format!(
            "the sky model's answer has {} values, or values that are not probabilities, \
             for {model_width}x{model_height}",
            probability.len()
        )));
    }
    let (width, height) = output_size(image);
    let sure = probability.iter().filter(|p| **p > 0.5).count();
    if (sure as f32) < SKY_LEAST * probability.len() as f32 {
        return Ok((empty(width, height), sky_edge::Modes::default()));
    }
    let whole = Rect {
        x: 0,
        y: 0,
        width: model_width,
        height: model_height,
    };
    let soft = resample(probability, model_width, whole, width, height);
    let (refined, modes) = sky_edge::blend(
        &colours(image, width, height),
        &luma(image, width, height),
        &soft,
        width,
        height,
        kinds,
        cancel,
    )?;
    if cancel.is_cancelled() {
        return Err(InferenceError::Cancelled);
    }
    let coverage = Coverage {
        width,
        height,
        data: refined
            .iter()
            .map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
            .collect(),
    };
    Ok((coverage, modes))
}

fn check(len: usize) -> Result<(), InferenceError> {
    if len == SIDE * SIDE {
        Ok(())
    } else {
        Err(InferenceError::OutputInvalid(format!(
            "a label map has {len} values, expected {}",
            SIDE * SIDE
        )))
    }
}

fn empty(width: usize, height: usize) -> Coverage {
    Coverage {
        width,
        height,
        data: vec![0; width * height],
    }
}

/// The photo's luminance at `width` x `height` (the coverage's size, which is the
/// photo's own unless it was capped).
pub(crate) fn luma(image: &RgbImage, width: usize, height: usize) -> Vec<f32> {
    let full: Vec<f32> = image
        .data
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| {
            (0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2])) / 255.0
        })
        .collect();
    if (width, height) == (image.width, image.height) {
        return full;
    }
    let whole = Rect {
        x: 0,
        y: 0,
        width: image.width,
        height: image.height,
    };
    resample(&full, image.width, whole, width, height)
}

/// The photo's colours (sRGB / 255) at `width` x `height`, as [`luma`].
pub(crate) fn colours(image: &RgbImage, width: usize, height: usize) -> Vec<[f32; 3]> {
    let whole = Rect {
        x: 0,
        y: 0,
        width: image.width,
        height: image.height,
    };
    let channels: [Vec<f32>; 3] = std::array::from_fn(|k| {
        let full: Vec<f32> = image
            .data
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| f32::from(p[k]) / 255.0)
            .collect();
        if (width, height) == (image.width, image.height) {
            full
        } else {
            resample(&full, image.width, whole, width, height)
        }
    });
    (0..width * height)
        .map(|i| [channels[0][i], channels[1][i], channels[2][i]])
        .collect()
}

/// A [`SIDE`] grid mask bilinearly enlarged to `width` x `height`.
fn expand(grid: &[f32], width: usize, height: usize) -> Vec<f32> {
    let whole = Rect {
        x: 0,
        y: 0,
        width: SIDE,
        height: SIDE,
    };
    resample(grid, SIDE, whole, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kinds::Kind;

    /// A photo of `w` x `h`: `paint(x, y)` colours each pixel.
    fn photo(w: usize, h: usize, paint: impl Fn(usize, usize) -> [u8; 3]) -> RgbImage {
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
    fn rect(x0: usize, y0: usize, x1: usize, y1: usize) -> Vec<bool> {
        (0..SIDE * SIDE)
            .map(|i| (x0..x1).contains(&(i % SIDE)) && (y0..y1).contains(&(i / SIDE)))
            .collect()
    }
    fn at(c: &Coverage, fx: f32, fy: f32) -> u8 {
        c.data[(fy * c.height as f32) as usize * c.width + (fx * c.width as f32) as usize]
    }

    #[test]
    fn the_subject_is_the_drawn_mask_with_its_edge_on_the_photos() {
        // A bright block (pixels 250..470 across); the mask drawn for it, on the coarse
        // grid, is a cell too wide on every side.
        let img = photo(1000, 1000, |x, y| {
            if (250..470).contains(&x) && (300..780).contains(&y) {
                [220, 200, 60]
            } else {
                [30, 40, 50]
            }
        });
        let loose: Vec<f32> = rect(63, 76, 121, 200)
            .iter()
            .map(|m| f32::from(u8::from(*m)))
            .collect();
        let cov = subject(&img, &loose).unwrap();
        let px = |x: usize, y: usize| cov.data[y * cov.width + x];
        assert!(px(350, 500) > 240 && px(800, 200) < 10);
        // Just outside the block, where the loose mask reached, the edge has moved in.
        assert!(px(245, 500) < 100, "{}", px(245, 500));
        assert!(px(475, 500) < 100, "{}", px(475, 500));
    }

    fn instance(mask: Vec<bool>) -> Instance {
        Instance {
            class: 1,
            score: 0.99,
            mask: mask.iter().map(|m| f32::from(u8::from(*m))).collect(),
        }
    }

    #[test]
    fn a_person_is_asked_for_with_a_box_points_inside_and_points_on_the_others() {
        let left = instance(rect(20, 40, 100, 200));
        let right = instance(rect(110, 40, 200, 200));
        let prompt = aim(&left, &[&right]);
        let [l, t, r, b] = prompt.bounds.unwrap();
        assert!((l - 20. / 256.).abs() < 0.01 && (r - 101. / 256.).abs() < 0.01);
        assert!((t - 40. / 256.).abs() < 0.01 && (b - 201. / 256.).abs() < 0.01);
        let (pos, neg): (Vec<&Point>, Vec<&Point>) = prompt.points.iter().partition(|p| p.positive);
        assert!((1..=3).contains(&pos.len()) && neg.len() == 1);
        assert!(pos.iter().all(|p| p.x > 20. / 256. && p.x < 100. / 256.));
        assert!(neg[0].x > 110. / 256.);
        prompt.validate().unwrap();
        // Alone, there is nothing to leave out.
        assert!(aim(&left, &[]).points.iter().all(|p| p.positive));
    }

    #[test]
    fn masks_are_compared_by_overlap() {
        assert!((iou(&rect(0, 0, 100, 100), &rect(0, 0, 100, 50)) - 0.5).abs() < 1e-6);
        assert_eq!(iou(&rect(0, 0, 10, 10), &rect(100, 100, 110, 110)), 0.0);
    }

    #[test]
    fn without_anything_salient_nothing_is_selected() {
        let img = photo(64, 48, |_, _| [100, 100, 100]);
        let cov = subject(&img, &vec![0.0; SIDE * SIDE]).unwrap();
        assert!(cov.data.iter().all(|v| *v == 0));
        assert!(subject(&img, &[0.0; 3]).is_err());
    }

    /// What the model sees outside the sky: `kind(x, y)` (fractions of the frame) on a
    /// coarse grid.
    fn kinds(kind: impl Fn(f32, f32) -> Option<Kind>) -> Kinds {
        Kinds::painted(64, 48, kind)
    }

    /// `f(x, y)` over a `w` x `h` model answer, x and y as fractions of the frame.
    fn answer(w: usize, h: usize, f: impl Fn(f32, f32) -> f32) -> Vec<f32> {
        (0..w * h)
            .map(|i| f((i % w) as f32 / w as f32, (i / w) as f32 / h as f32))
            .collect()
    }

    #[test]
    fn sky_is_where_the_model_says_with_its_edge_on_the_photos() {
        // The horizon is at row 267; the model's answer is a few pixels too low.
        let img = photo(1024, 683, |_, y| {
            if y < 267 {
                [120, 160, 230]
            } else {
                [40, 70, 30]
            }
        });
        let probability = answer(1024, 683, |_, y| if y < 272. / 683. { 0.99 } else { 0.01 });
        let ground = kinds(|_, y| (y >= 267. / 683.).then_some(Kind::Terrain));
        let cov = sky(&img, &probability, 1024, 683, &ground, Cancel::NEVER).unwrap();
        assert_eq!((cov.width, cov.height), (1024, 683));
        assert!(at(&cov, 0.5, 0.1) > 240 && at(&cov, 0.5, 0.8) < 10);
        // Under the true edge, where the model's answer still says sky, it has moved up.
        let below = at(&cov, 0.5, 271.5 / 683.);
        assert!(below < 200, "{below}");
    }

    #[test]
    fn water_under_the_sky_is_left_out_when_the_model_leaves_it_out() {
        // Sky-coloured water below a band of ground: nothing here looks at where it lies.
        let img = photo(640, 480, |_, y| match y {
            0..=199 => [120, 160, 230],
            200..=279 => [40, 60, 30],
            _ => [115, 155, 225],
        });
        let probability = answer(640, 480, |_, y| if y < 200. / 480. { 0.99 } else { 0.01 });
        let lake = kinds(|_, y| match y {
            ..0.4167 => None,
            ..0.5833 => Some(Kind::Terrain),
            _ => Some(Kind::Water),
        });
        let cov = sky(&img, &probability, 640, 480, &lake, Cancel::NEVER).unwrap();
        assert!(at(&cov, 0.5, 0.2) > 240 && at(&cov, 0.5, 0.8) < 10);
    }

    /// A sea horizon the model answers at a quarter of the photo, with a ramp three of
    /// its rows wide over a sharp step at row 200, on time or (in haze) three photo rows
    /// late. Over water no edge mode runs: the model's answer, snapped as main snaps it,
    /// puts the edge within a few rows of the step (later in haze, as main's), and
    /// nothing well above it is lost.
    #[test]
    fn a_sea_horizon_the_model_sees_blurred_keeps_mains_edge() {
        let img = photo(640, 480, |_, y| {
            if y < 200 {
                [120, 160, 230]
            } else {
                [60, 90, 140]
            }
        });
        for late in [48.5, 49.25] {
            let probability = answer(160, 120, |_, y| {
                let row = y * 120.;
                (1. - ((row - late) / 3.).clamp(0., 1.)) * 0.98 + 0.01
            });
            let sea = kinds(|_, y| (y >= 200. / 480.).then_some(Kind::Water));
            let cov = sky(&img, &probability, 160, 120, &sea, Cancel::NEVER).unwrap();
            let rows: Vec<u8> = (190..212).map(|y| at(&cov, 0.5, y as f32 / 480.)).collect();
            let (above, below) = (at(&cov, 0.5, 195. / 480.), at(&cov, 0.5, 208. / 480.));
            assert!(
                above > 230,
                "{late}: sky five rows above the horizon: {rows:?}"
            );
            assert!(
                below < 15,
                "{late}: sea eight rows below the horizon: {rows:?}"
            );
        }
    }

    /// Where no kind calls for a mode, the sky is main's edge to the byte: the model's
    /// answer at the coverage's size, snapped by the luma guided filter and its curve.
    #[test]
    fn without_a_crown_or_a_lattice_the_sky_is_mains_formula_exactly() {
        let img = photo(640, 480, |_, y| {
            if y < 200 {
                [120, 160, 230]
            } else {
                [60, 90, 140]
            }
        });
        for late in [48.5, 49.25] {
            let probability = answer(160, 120, |_, y| {
                let row = y * 120.;
                (1. - ((row - late) / 3.).clamp(0., 1.)) * 0.98 + 0.01
            });
            let ground = kinds(|_, _| Some(Kind::Terrain));
            let cov = sky(&img, &probability, 160, 120, &ground, Cancel::NEVER).unwrap();
            let (w, h) = output_size(&img);
            let whole = Rect {
                x: 0,
                y: 0,
                width: 160,
                height: 120,
            };
            let soft = resample(&probability, 160, whole, w, h);
            let mains: Vec<u8> = guided(
                &luma(&img, w, h),
                &soft,
                w,
                h,
                (w.max(h) / 200).max(3),
                1e-3,
            )
            .iter()
            .map(|v| smooth(*v))
            .map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
            .collect();
            assert_eq!((cov.width, cov.height), (w, h), "{late}");
            assert_eq!(cov.data.len(), mains.len(), "{late}");
            let differs = (0..mains.len()).find(|i| cov.data[*i] != mains[*i]);
            assert_eq!(
                differs, None,
                "{late}: the first pixel unlike main's formula"
            );
        }
    }

    #[test]
    fn sky_below_leaves_along_the_top_is_kept() {
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
        let leaves = kinds(|_, y| match y {
            ..0.125 => Some(Kind::Vegetation),
            ..0.5833 => None,
            _ => Some(Kind::Terrain),
        });
        let cov = sky(&img, &probability, 640, 480, &leaves, Cancel::NEVER).unwrap();
        assert!(at(&cov, 0.5, 0.3) > 240 && at(&cov, 0.5, 0.05) < 10 && at(&cov, 0.5, 0.8) < 10);
    }

    #[test]
    fn no_sky_is_exactly_nothing() {
        let img = photo(320, 240, |_, _| [90, 90, 90]);
        let unclear = kinds(|_, _| Some(Kind::Unclear));
        let cov = sky(
            &img,
            &vec![0.001; 160 * 120],
            160,
            120,
            &unclear,
            Cancel::NEVER,
        )
        .unwrap();
        assert_eq!((cov.width, cov.height), (320, 240));
        assert!(cov.data.iter().all(|v| *v == 0));
    }

    #[test]
    fn a_sky_answer_of_the_wrong_size_or_with_bad_values_is_refused() {
        let img = photo(320, 240, |_, _| [90, 90, 90]);
        let unclear = kinds(|_, _| Some(Kind::Unclear));
        let Err(InferenceError::OutputInvalid(why)) =
            sky(&img, &[0.5; 3], 160, 120, &unclear, Cancel::NEVER)
        else {
            panic!("a wrong-sized answer is not refused as invalid");
        };
        assert_eq!(
            why,
            "the sky model's answer has 3 values, or values that are not probabilities, \
             for 160x120"
        );
        let mut bad = vec![0.5; 160 * 120];
        bad[7] = f32::NAN;
        assert!(sky(&img, &bad, 160, 120, &unclear, Cancel::NEVER).is_err());
    }

    #[test]
    fn a_raised_cancel_flag_cancels_the_sky() {
        let img = photo(320, 240, |_, y| {
            if y < 120 {
                [120, 160, 230]
            } else {
                [40, 70, 30]
            }
        });
        let probability = answer(320, 240, |_, y| if y < 0.5 { 0.99 } else { 0.01 });
        let ground = kinds(|_, y| (y >= 0.5).then_some(Kind::Terrain));
        let result = sky(
            &img,
            &probability,
            320,
            240,
            &ground,
            Cancel::new(&std::sync::atomic::AtomicBool::new(true)),
        );
        assert!(matches!(result, Err(InferenceError::Cancelled)));
    }
}
