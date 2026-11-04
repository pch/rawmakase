//! Runs the real AI Denoise model. Ignored by default: set `RAWMAKASE_TEST_DENOISE_MODEL`
//! to the folder holding its files and `RAWMAKASE_ORT_LIB` to an ONNX Runtime library.
//! `RAWMAKASE_TEST_DENOISE_TILES` names a folder of `inputN.f32` / `outputN.f32` pairs
//! (little-endian, `[1, 4, 512, 512]` in, `[1, 3, 1024, 1024]` out) written by the Python
//! reference with the same ONNX Runtime release, for the contract's tolerance check.
use rawmakase_cancel::Cancel;
use rawmakase_inference::denoise::TileModel;
use rawmakase_inference::{Denoiser, InferenceError, LoadOptions, Provider};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

fn model() -> PathBuf {
    PathBuf::from(
        std::env::var("RAWMAKASE_TEST_DENOISE_MODEL").expect("RAWMAKASE_TEST_DENOISE_MODEL"),
    )
}

fn read(path: &std::path::Path) -> Vec<f32> {
    std::fs::read(path)
        .unwrap()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect()
}

/// Rust (this crate, the app's ONNX Runtime) and Python on the same tile:
/// `max |a − b| ≤ 1e-5 × max |b|` and mean `|a − b| ≤ 1e-6 × max |b|`.
fn check(provider: Provider) {
    let dir = PathBuf::from(
        std::env::var("RAWMAKASE_TEST_DENOISE_TILES").expect("RAWMAKASE_TEST_DENOISE_TILES"),
    );
    let mut denoiser = Denoiser::load_with(
        &model(),
        &LoadOptions {
            provider: Some(provider),
            ..Default::default()
        },
    )
    .unwrap();
    let cancel = AtomicBool::new(false);
    for i in 0.. {
        let input = dir.join(format!("input{i}.f32"));
        if !input.exists() {
            assert!(i > 0, "no tiles in {}", dir.display());
            break;
        }
        let ours = denoiser.run(&read(&input), Cancel::new(&cancel)).unwrap();
        let theirs = read(&dir.join(format!("output{i}.f32")));
        assert_eq!(ours.len(), theirs.len());
        let peak = theirs.iter().fold(0f32, |m, v| m.max(v.abs()));
        let diff: Vec<f32> = ours
            .iter()
            .zip(&theirs)
            .map(|(a, b)| (a - b).abs())
            .collect();
        let max = diff.iter().fold(0f32, |m, v| m.max(*v));
        let mean = diff.iter().map(|v| f64::from(*v)).sum::<f64>() / diff.len() as f64;
        eprintln!(
            "{} tile {i}: max {:.3e} mean {:.3e} of peak {peak:.4e}",
            provider.name(),
            max / peak,
            mean / f64::from(peak)
        );
        assert!(max <= 1e-5 * peak, "max {max} peak {peak}");
        assert!(mean <= 1e-6 * f64::from(peak), "mean {mean} peak {peak}");
    }
}

#[test]
#[ignore = "needs the denoiser, an ONNX Runtime and the Python reference tiles"]
fn rust_and_python_agree_on_the_same_tile_on_the_cpu() {
    check(Provider::Cpu);
}

#[test]
#[cfg(target_os = "macos")]
#[ignore = "needs the denoiser, an ONNX Runtime and the Python reference tiles"]
fn coreml_agrees_with_python_on_the_same_tile() {
    check(Provider::CoreML);
}

#[test]
#[ignore = "needs the denoiser and an ONNX Runtime"]
fn a_raised_cancel_flag_stops_a_running_tile() {
    let mut denoiser = Denoiser::load_with(&model(), &LoadOptions::default()).unwrap();
    let cancel = AtomicBool::new(false);
    let input = vec![0.2; 4 * 512 * 512];
    std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(30));
            cancel.store(true, Ordering::Relaxed);
        });
        let started = std::time::Instant::now();
        let result = denoiser.run(&input, Cancel::new(&cancel));
        eprintln!("cancelled after {:?}", started.elapsed());
        assert_eq!(result.err(), Some(InferenceError::Cancelled));
    });
}

#[test]
#[ignore = "needs the denoiser and an ONNX Runtime"]
fn a_provider_the_manifest_does_not_allow_is_refused() {
    let refused = Denoiser::load_with(
        &model(),
        &LoadOptions {
            provider: Some(Provider::Cuda),
            ..Default::default()
        },
    );
    assert!(matches!(refused, Err(InferenceError::Failed(_))));
}
