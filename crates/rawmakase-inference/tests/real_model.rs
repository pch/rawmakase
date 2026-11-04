//! Runs the real model. Ignored by default: set `RAWMAKASE_TEST_MODEL` to the folder
//! holding the model's files and `RAWMAKASE_ORT_LIB` to an ONNX Runtime library.
use rawmakase_cancel::Cancel;
use rawmakase_inference::{InferenceError, Point, Prompt, RgbImage, Subject};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

fn model() -> PathBuf {
    PathBuf::from(std::env::var("RAWMAKASE_TEST_MODEL").expect("RAWMAKASE_TEST_MODEL"))
}
/// A bright disc on a dark, slightly textured ground.
fn scene() -> RgbImage {
    let (w, h) = (320usize, 240usize);
    let mut data = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let inside = ((x as f32 - 200.).powi(2) + (y as f32 - 120.).powi(2)).sqrt() < 60.;
            let v = if inside {
                [230, 60, 50]
            } else {
                [30 + (x % 7) as u8, 40, 60]
            };
            data.extend(v);
        }
    }
    RgbImage {
        width: w,
        height: h,
        data,
    }
}

#[test]
#[ignore = "needs the model and an ONNX Runtime"]
fn a_click_selects_the_object_under_it_and_a_click_elsewhere_does_not() {
    let cancel = AtomicBool::new(false);
    let mut session = Subject::load(&model()).unwrap();
    let image = scene();
    let embedding = session.embed(&image, Cancel::new(&cancel)).unwrap();
    let click = |x: f32, y: f32| Prompt {
        points: vec![Point {
            x,
            y,
            positive: true,
        }],
        bounds: None,
    };
    let on = session
        .segment(&embedding, &click(200. / 320., 0.5), Cancel::new(&cancel))
        .unwrap();
    assert_eq!((on.width, on.height), (320, 240));
    let at = |c: &rawmakase_inference::Coverage, x: usize, y: usize| c.data[y * c.width + x];
    assert!(at(&on, 200, 120) > 200, "{}", at(&on, 200, 120));
    assert!(at(&on, 20, 20) < 40, "{}", at(&on, 20, 20));
    // A box does the same.
    let boxed = Prompt {
        points: vec![],
        bounds: Some([130. / 320., 50. / 240., 270. / 320., 190. / 240.]),
    };
    let by_box = session
        .segment(&embedding, &boxed, Cancel::new(&cancel))
        .unwrap();
    assert!(at(&by_box, 200, 120) > 200 && at(&by_box, 20, 20) < 40);
    // A raised flag never yields a result.
    cancel.store(true, Ordering::Relaxed);
    assert_eq!(
        session
            .segment(&embedding, &click(0.5, 0.5), Cancel::new(&cancel))
            .unwrap_err(),
        InferenceError::Cancelled
    );
    assert_eq!(
        session.embed(&image, Cancel::new(&cancel)).err(),
        Some(InferenceError::Cancelled)
    );
}

#[test]
#[ignore = "needs the model and an ONNX Runtime"]
fn automatic_selection_runs_and_a_disc_is_neither_a_person_nor_sky() {
    let cancel = AtomicBool::new(false);
    let mut session = Subject::load(&model()).unwrap();
    let image = scene();
    let analysis = session.analyze(&image, Cancel::new(&cancel)).unwrap();
    // The panoptic model knows people and animals: a red disc is neither.
    let subject = analysis.subject().unwrap();
    assert_eq!((subject.width, subject.height), (320, 240));
    assert!(subject.data.iter().all(|v| *v < 40));
    // Clicks work on the same analysis.
    let at = |c: &rawmakase_inference::Coverage, x: usize, y: usize| c.data[y * c.width + x];
    let click = Prompt {
        points: vec![Point {
            x: 200. / 320.,
            y: 0.5,
            positive: true,
        }],
        bounds: None,
    };
    assert!(
        at(
            &session
                .segment(analysis.embedding(), &click, Cancel::new(&cancel))
                .unwrap(),
            200,
            120
        ) > 200
    );
    cancel.store(true, Ordering::Relaxed);
    assert_eq!(
        session.analyze(&image, Cancel::new(&cancel)).err(),
        Some(InferenceError::Cancelled)
    );
}

fn sky_model() -> PathBuf {
    PathBuf::from(std::env::var("RAWMAKASE_TEST_SKY_MODEL").expect("RAWMAKASE_TEST_SKY_MODEL"))
}

/// A blue sky with white clouds over a textured ground, as a thin and a tall frame too.
fn landscape(w: usize, h: usize) -> RgbImage {
    let mut data = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let v = if fy < 0.55 {
                let cloud = ((fx * 21.).sin() + (fy * 37.).sin() > 1.3) as u8 as f32;
                let shade = 150. + 60. * fy;
                [
                    (90. + 130. * cloud) as u8,
                    (shade - 10. + 40. * cloud) as u8,
                    (shade + 45.).min(255.) as u8,
                ]
            } else {
                let grain = ((x * 7 + y * 13) % 41) as u8;
                [50 + grain, 70 + grain, 35]
            };
            data.extend(v);
        }
    }
    RgbImage {
        width: w,
        height: h,
        data,
    }
}

#[test]
#[ignore = "needs the sky model and an ONNX Runtime"]
fn the_sky_model_answers_at_any_shape_and_a_raised_flag_stops_it() {
    use rawmakase_inference::{LoadOptions, Sky};
    let cancel = AtomicBool::new(false);
    let mut session = Sky::load_with(&sky_model(), &LoadOptions::default()).unwrap();
    for (w, h) in [
        (1280, 853),
        (853, 1280),
        (1280, 720),
        (801, 801),
        (300, 200),
    ] {
        let cov = session
            .select(&landscape(w, h), Cancel::new(&cancel))
            .unwrap();
        assert_eq!((cov.width, cov.height), (w, h));
        // How much of the top and the bottom of the frame was chosen. It is a drawing, not
        // a photo, so this says the frame's shape is honoured, not how good the sky is.
        let share = |from: f32, to: f32| {
            let (a, b) = ((from * h as f32) as usize, (to * h as f32) as usize);
            let band = &cov.data[a * w..b * w];
            band.iter().filter(|v| **v >= 128).count() as f32 / band.len() as f32
        };
        println!("{w}x{h}: {:.2} {:.2}", share(0.0, 0.4), share(0.7, 1.0));
        assert!(share(0.0, 0.4) > 0.25, "{w}x{h}: {}", share(0.0, 0.4));
        assert!(share(0.7, 1.0) < 0.1, "{w}x{h}: {}", share(0.7, 1.0));
    }
    // Not a photo, but it must answer and not fail for its shape.
    for (w, h) in [(1280, 20), (20, 1280), (16, 16)] {
        let cov = session
            .select(&landscape(w, h), Cancel::new(&cancel))
            .unwrap();
        assert_eq!((cov.width, cov.height), (w, h));
    }
    let disc = session.select(&scene(), Cancel::new(&cancel)).unwrap();
    let centre = disc.data[120 * disc.width + 200];
    assert!(centre < 40, "a red disc is not sky: {centre}");
    cancel.store(true, Ordering::Relaxed);
    assert_eq!(
        session
            .select(&landscape(640, 480), Cancel::new(&cancel))
            .err(),
        Some(InferenceError::Cancelled)
    );
}
