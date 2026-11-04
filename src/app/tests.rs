use crate::app::*;

#[cfg(windows)]
#[test]
fn windows_icon_file_contains_the_256_png() {
    let png =
        crate::app::largest_png_in_ico(include_bytes!("../../packaging/windows/rawmakase.ico"))
            .expect("icon");
    let image = image::load_from_memory(png).expect("png").into_rgba8();
    assert_eq!([image.width(), image.height()], [256, 256]);
}

#[test]
fn only_size_changes_hold_previews_gpu_work() {
    use winit::{dpi::PhysicalSize, event::WindowEvent};
    assert!(reconfigures_surface(&WindowEvent::Resized(
        PhysicalSize::new(1512, 491)
    )));
    assert!(!reconfigures_surface(&WindowEvent::RedrawRequested));
    assert!(!reconfigures_surface(&WindowEvent::Focused(true)));
}

#[cfg(target_os = "linux")]
#[test]
fn the_window_draws_on_the_gpu_the_system_lists_first() {
    use wgpu::{DeviceType::*, PowerPreference};
    // An Optimus laptop (#375): Mesa's device-select layer lists the Intel GPU,
    // which drives the display, before the render-offload-only NVIDIA GPU.
    let optimus = [
        ("Intel(R) Iris(R) Xe Graphics", IntegratedGpu),
        ("NVIDIA GeForce MX450", DiscreteGpu),
        ("llvmpipe (LLVM 20.1.8, 256 bits)", Cpu),
    ];
    assert_eq!(display_adapter(&optimus, None, None), Some(0));
    // prime-run lists the NVIDIA GPU first.
    let offloaded = [optimus[1], optimus[0], optimus[2]];
    assert_eq!(display_adapter(&offloaded, None, None), Some(0));
    // Software rendering only when no GPU can draw the window.
    assert_eq!(
        display_adapter(&[optimus[2], optimus[0]], None, None),
        Some(1)
    );
    assert_eq!(display_adapter(&[optimus[2]], None, None), Some(0));
    assert_eq!(display_adapter(&[], None, None), None);
    // WGPU_POWER_PREF and WGPU_ADAPTER_NAME still choose.
    let high = Some(PowerPreference::HighPerformance);
    assert_eq!(display_adapter(&optimus, high, None), Some(1));
    let low = Some(PowerPreference::LowPower);
    assert_eq!(display_adapter(&offloaded, low, None), Some(1));
    let none = Some(PowerPreference::None);
    assert_eq!(display_adapter(&offloaded, none, None), Some(0));
    assert_eq!(display_adapter(&optimus, None, Some("NVIDIA")), Some(1));
    assert_eq!(display_adapter(&optimus, None, Some("radeon")), Some(0));
}

#[test]
fn eframes_reason_for_giving_up_is_kept() {
    use log::Log;
    let record = |target: &str, level, message: &str| {
        EframeErrors.log(
            &log::Record::builder()
                .target(target)
                .level(level)
                .args(format_args!("{message}"))
                .build(),
        );
    };
    record(
        "wgpu_core",
        log::Level::Error,
        "Exiting because of error: other",
    );
    record(
        "eframe::native::run",
        log::Level::Warn,
        "Exiting because of error: warn",
    );
    assert_eq!(EframeErrors::take(), None);
    record(
        "eframe::native::run",
        log::Level::Error,
        "Exiting because of error: No suitable GPU adapter",
    );
    assert_eq!(
        EframeErrors::take().as_deref(),
        Some("No suitable GPU adapter")
    );
    assert_eq!(EframeErrors::take(), None);

    // wgpu only logs the driver call behind "Parent device is lost" (#768).
    record(
        "wgpu_hal::dx12",
        log::Level::Error,
        "Queue creation failed: E_INVALIDARG",
    );
    record(
        "eframe::native::run",
        log::Level::Error,
        "Exiting because of error: WGPU error: Parent device is lost",
    );
    assert_eq!(
        EframeErrors::take().as_deref(),
        Some("WGPU error: Parent device is lost\n\nQueue creation failed: E_INVALIDARG")
    );
}
