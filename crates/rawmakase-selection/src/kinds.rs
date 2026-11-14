//! What the sky model sees below and around the sky: its 150 ADE20K classes gathered
//! into the kinds of horizon Select Sky treats differently.

use rawmakase_inference::InferenceError;

/// The sky model's classes (ADE20K, as `openmmlab/upernet-convnext-base` numbers them).
const CLASSES: usize = 150;
/// ADE20K's sky.
const SKY_CLASS: usize = 2;
/// Walls, buildings, houses, towers and other solid structures.
const BUILT: &[usize] = &[
    0, 1, 8, 14, 25, 40, 42, 43, 48, 51, 79, 84, 86, 88, 104, 106, 132, 149,
];
/// Fences, railings, bridges, poles, lights and other open frameworks the sky shows through.
const LATTICE: &[usize] = &[32, 38, 61, 82, 87, 93, 95, 136, 140];
/// Trees, grass, plants, palms and flowers.
const VEGETATION: &[usize] = &[4, 9, 17, 66, 72];
/// Ground, mountains, rocks, sand and other land.
const TERRAIN: &[usize] = &[6, 11, 13, 16, 29, 34, 46, 52, 68, 91, 94];
/// Sea, rivers, lakes and other water.
const WATER: &[usize] = &[21, 26, 60, 76, 103, 109, 113, 128];

/// How likely each pixel of the model's logits grid is to be each kind (softmax over all
/// classes, summed per kind). Sky's own share is in none of them, so the six and the
/// sky's add up to one.
pub(crate) struct Kinds {
    pub width: usize,
    pub height: usize,
    pub built: Vec<f32>,
    pub lattice: Vec<f32>,
    pub vegetation: Vec<f32>,
    pub terrain: Vec<f32>,
    pub water: Vec<f32>,
    /// Every class that is neither sky nor in one of the other kinds.
    pub unclear: Vec<f32>,
}

/// One kind of horizon, for painting [`Kinds`] in tests.
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Built,
    Lattice,
    Vegetation,
    Terrain,
    Water,
    Unclear,
}

#[cfg(test)]
impl Kinds {
    /// Kinds on a `width` x `height` grid, each cell entirely `kind(x, y)` (the cell's
    /// centre, as fractions of the frame), or entirely sky where that is `None`.
    pub(crate) fn painted(
        width: usize,
        height: usize,
        kind: impl Fn(f32, f32) -> Option<Kind>,
    ) -> Kinds {
        let cell = |k: Kind| -> Vec<f32> {
            (0..width * height)
                .map(|i| {
                    let x = ((i % width) as f32 + 0.5) / width as f32;
                    let y = ((i / width) as f32 + 0.5) / height as f32;
                    f32::from(u8::from(kind(x, y) == Some(k)))
                })
                .collect()
        };
        Kinds {
            width,
            height,
            built: cell(Kind::Built),
            lattice: cell(Kind::Lattice),
            vegetation: cell(Kind::Vegetation),
            terrain: cell(Kind::Terrain),
            water: cell(Kind::Water),
            unclear: cell(Kind::Unclear),
        }
    }
}

/// The kinds in `logits`, the model's class scores laid out class by class over a
/// `width` x `height` grid.
pub(crate) fn kinds(
    logits: &[f32],
    classes: usize,
    width: usize,
    height: usize,
) -> Result<Kinds, InferenceError> {
    let pixels = width.checked_mul(height).filter(|n| *n > 0);
    let size = pixels.and_then(|n| n.checked_mul(classes));
    if classes != CLASSES || size != Some(logits.len()) || logits.iter().any(|l| !l.is_finite()) {
        return Err(InferenceError::OutputInvalid(format!(
            "the sky model's class scores are {} values for {classes} classes over \
             {width}x{height}, or not all finite",
            logits.len()
        )));
    }
    let pixels = width * height;
    let kind_of = kind_table();
    // built, lattice, vegetation, terrain, water, unclear; the sky's share is dropped.
    let mut kinds: [Vec<f32>; 6] = std::array::from_fn(|_| vec![0.0; pixels]);
    let mut shares = [0f32; 6];
    for p in 0..pixels {
        let score = |c: usize| logits[c * pixels + p];
        let max = (0..CLASSES).map(score).fold(f32::MIN, f32::max);
        let mut total = 0.0;
        shares.fill(0.0);
        for (c, kind) in kind_of.iter().enumerate() {
            let e = (score(c) - max).exp();
            total += e;
            if let Some(kind) = kind {
                shares[*kind] += e;
            }
        }
        for (kind, share) in kinds.iter_mut().zip(shares) {
            kind[p] = share / total;
        }
    }
    let [built, lattice, vegetation, terrain, water, unclear] = kinds;
    Ok(Kinds {
        width,
        height,
        built,
        lattice,
        vegetation,
        terrain,
        water,
        unclear,
    })
}

/// For each class, the index of its kind in [`kinds`]'s order; `None` for sky.
fn kind_table() -> [Option<usize>; CLASSES] {
    const UNCLEAR: usize = 5;
    let mut table = [Some(UNCLEAR); CLASSES];
    table[SKY_CLASS] = None;
    for (kind, group) in [BUILT, LATTICE, VEGETATION, TERRAIN, WATER]
        .into_iter()
        .enumerate()
    {
        for c in group {
            table[*c] = Some(kind);
        }
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 3 x 2 grid where `class` scores high at every pixel and the rest low.
    fn dominant(class: usize) -> Vec<f32> {
        let pixels = 6;
        let mut logits = vec![-4.0f32; CLASSES * pixels];
        for p in 0..pixels {
            // Some variation so the other classes are not all equal.
            for c in 0..CLASSES {
                logits[c * pixels + p] += (c % 5) as f32 * 0.1;
            }
            logits[class * pixels + p] = 20.0;
        }
        logits
    }

    fn all(kinds: &Kinds) -> [(&'static str, &Vec<f32>); 6] {
        [
            ("built", &kinds.built),
            ("lattice", &kinds.lattice),
            ("vegetation", &kinds.vegetation),
            ("terrain", &kinds.terrain),
            ("water", &kinds.water),
            ("unclear", &kinds.unclear),
        ]
    }

    #[test]
    fn a_dominant_class_puts_its_kind_near_one() {
        for (class, expected) in [
            (1, Some("built")),
            (61, Some("lattice")),
            (4, Some("vegetation")),
            (16, Some("terrain")),
            (21, Some("water")),
            (12, Some("unclear")),
            (SKY_CLASS, None),
        ] {
            let kinds = kinds(&dominant(class), CLASSES, 3, 2).unwrap();
            assert_eq!((kinds.width, kinds.height), (3, 2));
            for (name, values) in all(&kinds) {
                assert_eq!(values.len(), 6);
                for v in values {
                    if Some(name) == expected {
                        assert!(*v > 0.99, "class {class}: {name} = {v}");
                    } else {
                        assert!(*v < 0.01, "class {class}: {name} = {v}");
                    }
                }
            }
        }
    }

    #[test]
    fn the_kinds_and_sky_add_up_to_one() {
        let pixels = 4;
        let logits: Vec<f32> = (0..CLASSES * pixels)
            .map(|i| ((i * 37) % 23) as f32 * 0.3 - 3.0)
            .collect();
        let kinds = kinds(&logits, CLASSES, 2, 2).unwrap();
        for p in 0..pixels {
            let column: Vec<f32> = (0..CLASSES).map(|c| logits[c * pixels + p]).collect();
            let max = column.iter().copied().fold(f32::MIN, f32::max);
            let total: f32 = column.iter().map(|l| (l - max).exp()).sum();
            let sky = (column[SKY_CLASS] - max).exp() / total;
            let sum: f32 = all(&kinds).iter().map(|(_, v)| v[p]).sum::<f32>() + sky;
            assert!((sum - 1.0).abs() < 1e-4, "pixel {p}: {sum}");
        }
    }

    #[test]
    fn every_class_but_sky_is_in_exactly_one_kind() {
        let mut seen = [0; CLASSES];
        for group in [BUILT, LATTICE, VEGETATION, TERRAIN, WATER] {
            for c in group {
                seen[*c] += 1;
            }
        }
        assert_eq!(seen[SKY_CLASS], 0);
        assert!(seen.iter().all(|n| *n <= 1));
    }

    #[test]
    fn the_wrong_classes_sizes_or_values_are_invalid() {
        let good = dominant(1);
        let invalid =
            |r: Result<Kinds, InferenceError>| matches!(r, Err(InferenceError::OutputInvalid(_)));
        assert!(invalid(kinds(&good[..149 * 6], 149, 3, 2)));
        let Err(InferenceError::OutputInvalid(why)) = kinds(&good, CLASSES, 3, 3) else {
            panic!("class scores of the wrong size are not refused as invalid");
        };
        assert_eq!(
            why,
            format!(
                "the sky model's class scores are {} values for {CLASSES} classes over \
                 3x3, or not all finite",
                good.len()
            )
        );
        assert!(invalid(kinds(&good, CLASSES, 0, 0)));
        let mut nan = good.clone();
        nan[7] = f32::NAN;
        assert!(invalid(kinds(&nan, CLASSES, 3, 2)));
        let mut infinite = good;
        infinite[0] = f32::INFINITY;
        assert!(invalid(kinds(&infinite, CLASSES, 3, 2)));
    }
}
