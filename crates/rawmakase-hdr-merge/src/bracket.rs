//! Whether a selection of photos is a bracket Merge to HDR takes, the order
//! its frames merge in and its reference frame.
//!
//! A bracket is 2 to 9 RAWs from one camera, of one size, sensor and
//! orientation, taken one after another, whose exposures differ by at least
//! [`MIN_STEP`] and leave no gap wider than [`MAX_STEP`]. Its frames merge from
//! the darkest to the brightest; the reference, whose brightness the merge
//! renders at, is the frame nearest the bracket's median exposure.

/// The fewest and most frames a bracket has.
pub const MIN_FRAMES: usize = 2;
pub const MAX_FRAMES: usize = 9;
/// The least difference in exposure, in stops, between two frames.
pub const MIN_STEP: f64 = 0.3;
/// The widest gap in exposure, in stops, between neighbouring frames.
pub const MAX_STEP: f64 = 4.;
/// The most time between one frame's end and the next frame's capture, in
/// seconds.
pub const MAX_PAUSE: f64 = 2.;

/// What a bracket check reads of a frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Shot {
    /// The photo's file name, as refusals name it.
    pub name: String,
    /// The camera's make and model, as RAWmakase names them.
    pub camera: [String; 2],
    /// The body's serial number, when the photo records it.
    pub serial: Option<String>,
    /// The decoded image's width and height.
    pub size: [u32; 2],
    /// RAWmakase's flip code (`Metadata::flip`).
    pub flip: i32,
    /// Whether the sensor is X-Trans rather than Bayer.
    pub xtrans: bool,
    pub exposure: Exposure,
    /// When the photo was taken, in seconds from any fixed time, to the
    /// subsecond when the photo records it; `None` when it records no time.
    pub captured: Option<f64>,
}

/// A frame's exposure settings, as its EXIF records them; 0 for one it doesn't.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Exposure {
    /// Seconds.
    pub time: f64,
    pub f_number: f64,
    pub iso: f64,
}

impl Exposure {
    /// The light the frame's samples gather, in stops: log2(t · ISO / N²).
    /// Settings a frame doesn't record count as the same in every frame:
    /// `f_number` and `iso` are used only when every frame has them.
    fn ev(&self, f_number: bool, iso: bool) -> f64 {
        let n = if f_number { self.f_number } else { 1. };
        let s = if iso { self.iso } else { 100. };
        (self.time * s / (n * n)).log2()
    }
}

/// Why a selection is not a bracket.
#[derive(Clone, Debug, PartialEq)]
pub enum Refusal {
    /// Fewer than [`MIN_FRAMES`] or more than [`MAX_FRAMES`] photos.
    Count(usize),
    /// The photo is from another camera than the first.
    AnotherCamera(String),
    AnotherSize(String),
    AnotherOrientation(String),
    AnotherSensor(String),
    /// The photo records no exposure time.
    NoExposure(String),
    /// The photo was taken too long after the one before it.
    NotInSequence(String),
    /// The two photos have the same exposure.
    SameExposure(String, String),
    /// The gap in stops between the two photos is too wide.
    ExposureGap(String, String, f64),
    /// The photos' brightness differs from what their exposures say by
    /// this many stops: they don't show one scene.
    AnotherScene(String, String, f64),
    /// The photo is not a mosaic RAW, or could not be decoded.
    Unreadable(String, String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Count(n) => write!(
                f,
                "Merge to HDR takes {MIN_FRAMES} to {MAX_FRAMES} photos, not {n}"
            ),
            Self::AnotherCamera(p) => write!(f, "{p} is from another camera"),
            Self::AnotherSize(p) => write!(f, "{p} is another size"),
            Self::AnotherOrientation(p) => write!(f, "{p} is turned another way"),
            Self::AnotherSensor(p) => write!(f, "{p} has another kind of sensor"),
            Self::NoExposure(p) => write!(f, "{p} records no exposure time"),
            Self::NotInSequence(p) => {
                write!(f, "{p} was not taken straight after the photo before it")
            }
            Self::SameExposure(a, b) => write!(f, "{a} and {b} have the same exposure"),
            Self::ExposureGap(a, b, ev) => write!(
                f,
                "{a} and {b} are {ev:.1} stops apart; a bracket's frames can be at most {MAX_STEP} apart"
            ),
            Self::AnotherScene(a, b, ev) => write!(
                f,
                "{a} and {b} are not a bracket of the same scene: their brightness differs by {ev:.1} stops from what their exposures say"
            ),
            Self::Unreadable(p, why) => write!(f, "{p} can't be merged: {why}"),
        }
    }
}

/// A bracket's frames in merge order and its reference.
#[derive(Clone, Debug, PartialEq)]
pub struct Bracket {
    /// Indices into the shots, from the darkest exposure to the brightest.
    pub order: Vec<usize>,
    /// Each frame's exposure in stops, in merge order, relative to the
    /// darkest's (0).
    pub ev: Vec<f64>,
    /// The reference frame's place in [`Self::order`].
    pub reference: usize,
}

/// `shots` checked as a bracket and put in merge order; the first reason it
/// isn't one otherwise.
pub fn check(shots: &[Shot]) -> Result<Bracket, Refusal> {
    if !(MIN_FRAMES..=MAX_FRAMES).contains(&shots.len()) {
        return Err(Refusal::Count(shots.len()));
    }
    let first = &shots[0];
    for s in &shots[1..] {
        let serials = first.serial.as_ref().zip(s.serial.as_ref());
        if !s
            .camera
            .iter()
            .zip(&first.camera)
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
            || serials.is_some_and(|(a, b)| a != b)
        {
            return Err(Refusal::AnotherCamera(s.name.clone()));
        }
        if s.xtrans != first.xtrans {
            return Err(Refusal::AnotherSensor(s.name.clone()));
        }
        if s.size != first.size {
            return Err(Refusal::AnotherSize(s.name.clone()));
        }
        if s.flip != first.flip {
            return Err(Refusal::AnotherOrientation(s.name.clone()));
        }
    }
    if let Some(s) = shots
        .iter()
        .find(|s| !(s.exposure.time.is_finite() && s.exposure.time > 0.))
    {
        return Err(Refusal::NoExposure(s.name.clone()));
    }
    in_sequence(shots)?;
    let known = |v: fn(&Exposure) -> f64| shots.iter().all(|s| v(&s.exposure) > 0.);
    let (f_number, iso) = (known(|e| e.f_number), known(|e| e.iso));
    let ev: Vec<f64> = shots.iter().map(|s| s.exposure.ev(f_number, iso)).collect();
    let mut order: Vec<usize> = (0..shots.len()).collect();
    order.sort_by(|a, b| ev[*a].total_cmp(&ev[*b]));
    for pair in order.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let step = ev[b] - ev[a];
        let names = (shots[a].name.clone(), shots[b].name.clone());
        if step < MIN_STEP {
            return Err(Refusal::SameExposure(names.0, names.1));
        }
        if step > MAX_STEP {
            return Err(Refusal::ExposureGap(names.0, names.1, step));
        }
    }
    let darkest = ev[order[0]];
    let ev: Vec<f64> = order.iter().map(|i| ev[*i] - darkest).collect();
    let reference = reference(&ev);
    Ok(Bracket {
        order,
        ev,
        reference,
    })
}

/// The frame of `ev` (ascending) nearest its median; of two as near, the
/// darker, whose highlights clip less.
pub(crate) fn reference(ev: &[f64]) -> usize {
    let n = ev.len();
    let median = if n % 2 == 1 {
        ev[n / 2]
    } else {
        (ev[n / 2 - 1] + ev[n / 2]) / 2.
    };
    let mut best = 0;
    for (i, v) in ev.iter().enumerate() {
        if (v - median).abs() < (ev[best] - median).abs() - 1e-9 {
            best = i;
        }
    }
    best
}

/// Whether `shots`, in the order they were taken, follow one another with no
/// more than [`MAX_PAUSE`] between one's end and the next's start. Photos
/// without a capture time are not held to it.
fn in_sequence(shots: &[Shot]) -> Result<(), Refusal> {
    let mut taken: Vec<(f64, &Shot)> = shots
        .iter()
        .filter_map(|s| s.captured.map(|t| (t, s)))
        .collect();
    if taken.len() < shots.len() {
        return Ok(());
    }
    taken.sort_by(|a, b| a.0.total_cmp(&b.0));
    for pair in taken.windows(2) {
        let ((t0, s0), (t1, s1)) = (pair[0], pair[1]);
        // Recorded times may count from a frame's start or its end, and
        // whole seconds round either way: the earlier frame's exposure and a
        // second of rounding are allowed beside the pause.
        if t1 - t0 > MAX_PAUSE + s0.exposure.time + 1. {
            return Err(Refusal::NotInSequence(s1.name.clone()));
        }
    }
    Ok(())
}

/// Seconds since 1970 of an EXIF date ("2023:09:13 07:07:46") and its
/// subseconds ("33" is 0.33 s), as local time read as UTC: only differences
/// between one camera's times matter.
pub fn seconds(date: &str, subseconds: Option<&str>) -> Option<f64> {
    let date = date.trim().trim_end_matches('\0');
    let number = |s: &str| s.trim().parse::<i64>().ok();
    let (day, time) = date.split_once(' ')?;
    let mut d = day.split(':').map(number);
    let (y, m, dd) = (d.next()??, d.next()??, d.next()??);
    let mut t = time.split(':').map(number);
    let (h, mi, s) = (t.next()??, t.next()??, t.next()??);
    if !(1..=12).contains(&m) || !(1..=31).contains(&dd) || h > 24 || mi > 59 || s > 60 {
        return None;
    }
    // Days from 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's
    // `days_from_civil`).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + dd - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let fraction = subseconds
        .map(|s| s.trim().trim_end_matches('\0'))
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| format!("0.{s}").parse::<f64>().ok())
        .unwrap_or(0.);
    Some((days * 86400 + h * 3600 + mi * 60 + s) as f64 + fraction)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shot(name: &str, time: f64, captured: f64) -> Shot {
        Shot {
            name: name.into(),
            camera: ["Canon".into(), "EOS 850D".into()],
            serial: Some("102032000023".into()),
            size: [6000, 4000],
            flip: 0,
            xtrans: false,
            exposure: Exposure {
                time,
                f_number: 11.,
                iso: 100.,
            },
            captured: Some(captured),
        }
    }

    /// The T8i bracket of discuss.pixls.us #39443: 1/5, 1/20 and 0.8 s.
    fn bracket() -> Vec<Shot> {
        vec![
            shot("IMG_9814.CR3", 0.2, 0.33),
            shot("IMG_9815.CR3", 0.05, 0.65),
            shot("IMG_9816.CR3", 0.8, 0.84),
        ]
    }

    #[test]
    fn a_bracket_merges_darkest_first_around_its_middle_frame() {
        let b = check(&bracket()).unwrap();
        assert_eq!(b.order, [1, 0, 2]);
        assert_eq!(b.reference, 1);
        for (got, want) in b.ev.iter().zip([0., 2., 4.]) {
            assert!((got - want).abs() < 1e-9, "{:?}", b.ev);
        }
    }

    #[test]
    fn the_reference_is_nearest_the_median_and_the_darker_of_two() {
        assert_eq!(reference(&[0., 1., 2., 3.]), 1);
        assert_eq!(reference(&[0., 2., 2.5, 5.]), 1);
        assert_eq!(reference(&[0., 3., 4., 4.5, 8.]), 2);
        assert_eq!(reference(&[0., 4.]), 0);
    }

    #[test]
    fn refusals_name_the_photo_and_the_reason() {
        let refused = |change: fn(&mut Vec<Shot>)| {
            let mut shots = bracket();
            change(&mut shots);
            check(&shots).unwrap_err().to_string()
        };
        assert_eq!(
            refused(|s| s.truncate(1)),
            "Merge to HDR takes 2 to 9 photos, not 1"
        );
        assert_eq!(
            refused(|s| s.extend(std::iter::repeat_n(s[0].clone(), 7))),
            "Merge to HDR takes 2 to 9 photos, not 10"
        );
        assert_eq!(
            refused(|s| s[1].camera[1] = "EOS 90D".into()),
            "IMG_9815.CR3 is from another camera"
        );
        assert_eq!(
            refused(|s| s[2].serial = Some("1".into())),
            "IMG_9816.CR3 is from another camera"
        );
        assert_eq!(
            refused(|s| s[1].size = [4000, 6000]),
            "IMG_9815.CR3 is another size"
        );
        assert_eq!(
            refused(|s| s[1].flip = 6),
            "IMG_9815.CR3 is turned another way"
        );
        assert_eq!(
            refused(|s| s[2].xtrans = true),
            "IMG_9816.CR3 has another kind of sensor"
        );
        assert_eq!(
            refused(|s| s[0].exposure.time = 0.),
            "IMG_9814.CR3 records no exposure time"
        );
        assert_eq!(
            refused(|s| s[2].captured = Some(60.)),
            "IMG_9816.CR3 was not taken straight after the photo before it"
        );
        assert_eq!(
            refused(|s| s[2].exposure.time = 0.22),
            "IMG_9814.CR3 and IMG_9816.CR3 have the same exposure"
        );
        assert_eq!(
            refused(|s| s[2].exposure.time = 8.),
            "IMG_9814.CR3 and IMG_9816.CR3 are 5.3 stops apart; a bracket's frames can be at most 4 apart"
        );
    }

    /// A serial number one photo doesn't record, and a time none do, aren't
    /// held against the bracket; an ISO bracket counts its ISO speeds.
    #[test]
    fn what_photos_dont_record_is_not_held_against_them() {
        let mut shots = bracket();
        shots[0].serial = None;
        for s in &mut shots {
            s.captured = None;
        }
        assert!(check(&shots).is_ok());
        let mut iso = bracket();
        for (s, iso_speed) in iso.iter_mut().zip([200., 800., 3200.]) {
            s.exposure.time = 0.01;
            s.exposure.iso = iso_speed;
        }
        let b = check(&iso).unwrap();
        assert_eq!(b.order, [0, 1, 2]);
        // Without f-numbers, they count as the same.
        let mut no_aperture = bracket();
        no_aperture[0].exposure.f_number = 0.;
        assert_eq!(check(&no_aperture).unwrap().order, [1, 0, 2]);
    }

    #[test]
    fn exif_dates_count_seconds() {
        let a = seconds("2023:09:13 07:07:46", Some("33")).unwrap();
        let b = seconds("2023:09:13 07:07:46", Some("84")).unwrap();
        assert!((b - a - 0.51).abs() < 1e-6);
        assert_eq!(seconds("1970:01:01 00:00:00", None), Some(0.));
        assert_eq!(seconds("2000:03:01 00:00:00", None), Some(951868800.));
        let midnight = seconds("2023:12:31 23:59:59", None).unwrap();
        assert_eq!(seconds("2024:01:01 00:00:00", None), Some(midnight + 1.));
        assert_eq!(seconds("0000:00:00 00:00:00", None), None);
        assert_eq!(seconds("not a date", None), None);
    }
}
