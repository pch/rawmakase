//! The one-off analyses of the open photo (Auto, Upright, Auto Straighten, Point
//! Color's sample) each run on a thread of their own. Should one panic, it still
//! sends its result, as an error, so the editor stops waiting for it. A photo whose
//! pixels are fewer than its size says makes each of them panic.
use crate::app::{Editor, worker::Event};
use crate::camera_data::{CameraImage, Metadata};
use eframe::egui;
use std::sync::Arc;

pub(in crate::app) fn broken_photo() -> Arc<CameraImage> {
    let metadata = Metadata {
        width: 64,
        height: 64,
        wb: [1.; 3],
        daylight_wb: [1.; 3],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    Arc::new(CameraImage {
        recovered: Default::default(),
        width: 64,
        height: 64,
        pixels: vec![[0.2, 0.1, 0.05]; 3],
        metadata,
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    })
}
fn editor(ctx: &egui::Context) -> Editor {
    let mut e = Editor::with_context(
        ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let photo = broken_photo();
    e.document.metadata = Some(photo.metadata.clone());
    e.document.set_image(photo);
    e
}
/// The first event `pick` takes, within half a minute.
fn next<T>(e: &Editor, mut pick: impl FnMut(Event) -> Option<T>) -> T {
    loop {
        match e.rx.recv_timeout(std::time::Duration::from_secs(30)) {
            Ok(event) => {
                if let Some(t) = pick(event) {
                    return t;
                }
            }
            Err(err) => panic!("no result: {err}"),
        }
    }
}

#[test]
fn a_panicking_auto_still_reports_and_stops_running() {
    let ctx = egui::Context::default();
    for kind in [
        crate::app::worker::AutoKind::Settings,
        crate::app::worker::AutoKind::WhiteBalance,
    ] {
        let mut e = editor(&ctx);
        e.start_auto(kind);
        assert!(e.document.auto.is_running());
        let (got, result) = next(&e, |event| match event {
            Event::Auto { kind, result, .. } => Some((kind, result)),
            _ => None,
        });
        assert_eq!(got, kind);
        assert_eq!(
            result.as_ref().map(|_| ()),
            Err(&"Auto: the estimate failed unexpectedly".to_string())
        );
        e.auto_ready(kind, result);
        assert!(!e.document.auto.is_running());
    }
}

#[test]
fn a_panicking_upright_analysis_still_reports_and_stops_running() {
    let ctx = egui::Context::default();
    let mut e = editor(&ctx);
    e.start_upright();
    assert!(e.document.upright.is_running());
    let (generation, analysed, result) = next(&e, |event| match event {
        Event::Upright {
            generation,
            analysed,
            result,
            ..
        } => Some((generation, analysed, result)),
        _ => None,
    });
    assert_eq!(*analysed, *e.document.edit.recipe());
    assert_eq!(
        result,
        Err("Upright: the analysis failed unexpectedly".to_string())
    );
    e.upright_ready(generation, &analysed, result);
    assert!(!e.document.upright.is_running());
}

#[test]
fn a_panicking_auto_straighten_still_reports_and_stops_running() {
    let ctx = egui::Context::default();
    let mut e = editor(&ctx);
    e.start_auto_straighten();
    assert!(e.document.straighten.is_running());
    let (generation, analysed, result) = next(&e, |event| match event {
        Event::Straighten {
            generation,
            analysed,
            result,
            ..
        } => Some((generation, analysed, result)),
        _ => None,
    });
    assert_eq!(*analysed, *e.document.edit.recipe());
    assert_eq!(
        result,
        Err("Auto straighten: the analysis failed unexpectedly".to_string())
    );
    e.auto_straighten_ready(generation, &analysed, result);
    assert!(!e.document.straighten.is_running());
}

#[test]
fn a_panicking_point_color_sample_still_reports_and_stops_running() {
    let ctx = egui::Context::default();
    let mut e = editor(&ctx);
    e.start_point_color_sample(0.5, 0.5);
    assert!(e.document.point_color_pick.is_running());
    let (sampled, result) = next(&e, |event| match event {
        Event::PointColorSample {
            sampled, result, ..
        } => Some((sampled, result)),
        _ => None,
    });
    assert_eq!(*sampled, *e.document.edit.recipe());
    assert_eq!(
        result,
        Err("Cannot sample a color: sampling failed unexpectedly".to_string())
    );
    e.point_color_sample_ready(&sampled, result);
    assert!(!e.document.point_color_pick.is_running());
}
