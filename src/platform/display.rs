//! Declare the desktop surface's pixel color space to the macOS compositor,
//! and find the display profile the system assigns to a window's display.

/// What the system says about the display a window is on.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum SystemDisplay {
    /// No profile is known; sRGB pixels are shown as they are (Linux).
    #[default]
    Unknown,
    /// The compositor converts sRGB to the display itself: macOS, and Windows
    /// with automatic color management or HDR on.
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    Managed,
    /// The display's ICC profile, which nothing else applies (Windows).
    #[cfg_attr(not(windows), allow(dead_code))]
    Profile(std::path::PathBuf),
}

/// The display a window is on, compared to notice the window reaching another.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Monitor(isize);

#[cfg(windows)]
pub(crate) fn monitor(window: &impl winit::raw_window_handle::HasWindowHandle) -> Monitor {
    use windows_sys::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromWindow};
    use winit::raw_window_handle::RawWindowHandle;
    let Ok(handle) = window.window_handle() else {
        return Monitor(0);
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return Monitor(0);
    };
    // SAFETY: the caller holds the live window handle.
    Monitor(unsafe { MonitorFromWindow(handle.hwnd.get() as _, MONITOR_DEFAULTTONEAREST) } as isize)
}

#[cfg(not(windows))]
pub(crate) fn monitor(_window: &impl winit::raw_window_handle::HasWindowHandle) -> Monitor {
    Monitor(0)
}

/// The system's color management of `monitor`.
pub(crate) fn system_display(monitor: Monitor) -> SystemDisplay {
    #[cfg(target_os = "macos")]
    {
        let _ = monitor;
        SystemDisplay::Managed
    }
    #[cfg(windows)]
    {
        windows::system_display(monitor.0)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = monitor;
        SystemDisplay::Unknown
    }
}

#[cfg(windows)]
mod windows {
    use super::SystemDisplay;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Devices::Display::{
        DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
        DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER,
        DISPLAYCONFIG_DEVICE_INFO_TYPE, DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO,
        DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME,
        DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS,
        QueryDisplayConfig,
    };
    use windows_sys::Win32::Foundation::LUID;
    use windows_sys::Win32::Graphics::Gdi::{
        CreateDCW, DeleteDC, GetMonitorInfoW, HMONITOR, MONITORINFO, MONITORINFOEXW,
    };
    use windows_sys::Win32::UI::ColorSystem::GetICMProfileW;

    pub(super) fn system_display(monitor: isize) -> SystemDisplay {
        let monitor = monitor as HMONITOR;
        if monitor.is_null() {
            return SystemDisplay::Unknown;
        }
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        // SAFETY: `info` is a MONITORINFOEXW with its size set, as asked.
        if unsafe { GetMonitorInfoW(monitor, (&raw mut info).cast::<MONITORINFO>()) } == 0 {
            return SystemDisplay::Unknown;
        }
        let device = info.szDevice;
        if advanced_color(&device) {
            SystemDisplay::Managed
        } else {
            profile(&device).map_or(SystemDisplay::Unknown, SystemDisplay::Profile)
        }
    }

    /// The ICC profile Windows assigns to the display `device`, e.g. `\\.\DISPLAY1`.
    fn profile(device: &[u16; 32]) -> Option<std::path::PathBuf> {
        // SAFETY: `device` is the NUL-terminated name GetMonitorInfoW returned.
        let dc = unsafe {
            CreateDCW(
                device.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if dc.is_null() {
            return None;
        }
        let mut name = vec![0u16; 1024];
        let mut size = name.len() as u32;
        // SAFETY: `size` is the buffer's length in characters; the DC is ours.
        let found = unsafe {
            let found = GetICMProfileW(dc, &mut size, name.as_mut_ptr()) != 0;
            DeleteDC(dc);
            found
        };
        let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        (found && len > 0).then(|| std::ffi::OsString::from_wide(&name[..len]).into())
    }

    /// Windows 11 24H2's DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO_2, which
    /// windows-sys 0.61 does not have yet.
    #[repr(C)]
    #[derive(Default)]
    struct AdvancedColor2 {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
        value: u32,
        color_encoding: i32,
        bits_per_color_channel: u32,
        active_color_mode: i32,
    }
    const GET_ADVANCED_COLOR_INFO_2: DISPLAYCONFIG_DEVICE_INFO_TYPE = 15;

    /// Whether Windows converts sRGB to `device` itself: automatic color
    /// management or HDR, on a display in advanced color.
    fn advanced_color(device: &[u16; 32]) -> bool {
        let Some(paths) = active_paths() else {
            return false;
        };
        let name = |s: &[u16]| s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())].to_vec();
        let header = |kind, size: usize, adapter: LUID, id| DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: kind,
            size: size as u32,
            adapterId: adapter,
            id,
        };
        for path in paths {
            let source = &path.sourceInfo;
            let mut source_name = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
                header: header(
                    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                    size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>(),
                    source.adapterId,
                    source.id,
                ),
                ..Default::default()
            };
            // SAFETY: each request is the structure its header names, with its size.
            if unsafe { DisplayConfigGetDeviceInfo(&mut source_name.header) } != 0
                || name(&source_name.viewGdiDeviceName) != name(device)
            {
                continue;
            }
            let target = &path.targetInfo;
            let mut info = AdvancedColor2 {
                header: header(
                    GET_ADVANCED_COLOR_INFO_2,
                    size_of::<AdvancedColor2>(),
                    target.adapterId,
                    target.id,
                ),
                ..Default::default()
            };
            // SAFETY: as above.
            if unsafe { DisplayConfigGetDeviceInfo(&mut info.header) } == 0 {
                // advancedColorActive: the display is in WCG or HDR mode.
                return info.value & 0b10 != 0;
            }
            // Before 24H2: advancedColorEnabled (HDR) or wideColorEnforced (ACM).
            let mut info = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO {
                header: header(
                    DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
                    size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>(),
                    target.adapterId,
                    target.id,
                ),
                ..Default::default()
            };
            // SAFETY: as above; the union is a plain bit field.
            return unsafe { DisplayConfigGetDeviceInfo(&mut info.header) } == 0
                && unsafe { info.Anonymous.value } & 0b110 != 0;
        }
        false
    }

    fn active_paths() -> Option<Vec<DISPLAYCONFIG_PATH_INFO>> {
        let (mut path_count, mut mode_count) = (0, 0);
        // SAFETY: the counts are written; then the buffers have those lengths.
        unsafe {
            if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
                != 0
            {
                return None;
            }
            let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
            let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
            if QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut path_count,
                paths.as_mut_ptr(),
                &mut mode_count,
                modes.as_mut_ptr(),
                std::ptr::null_mut(),
            ) != 0
            {
                return None;
            }
            paths.truncate(path_count as usize);
            Some(paths)
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) struct Surface(dispatch2::MainThreadBound<objc2::rc::Retained<objc2_app_kit::NSView>>);

#[cfg(target_os = "macos")]
impl Surface {
    pub(crate) fn from_window(
        window: &impl winit::raw_window_handle::HasWindowHandle,
    ) -> Option<Self> {
        use objc2::MainThreadMarker;
        use objc2_app_kit::NSView;
        use winit::raw_window_handle::RawWindowHandle;

        let main_thread = MainThreadMarker::new()?;
        let handle = window.window_handle().ok()?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return None;
        };
        // SAFETY: the caller holds the live window handle and we checked that this
        // is AppKit's main thread. The view is borrowed only for this call.
        let view = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
        use objc2::Message;
        Some(Self(dispatch2::MainThreadBound::new(
            view.retain(),
            main_thread,
        )))
    }

    /// Called while painting, after wgpu has had its last chance to reconfigure.
    /// `layer` says how the surface's pixels are to be read (see [`Layer`]).
    pub(crate) fn prepare(&self, layer: Layer) {
        let Some(main_thread) = objc2::MainThreadMarker::new() else {
            return;
        };
        // Look up the current layer; surface recovery can replace it.
        if let Some(root) = self.0.get(main_thread).layer() {
            prepare_layer(&root, layer);
        }
    }
    /// How far above SDR white the window's display can show light, as linear
    /// factors over SDR white (1 on an SDR display): `(now, potential)`. `now` is
    /// macOS's `maximumExtendedDynamicRangeColorComponentValue`, which stays 1 until a
    /// layer asks for EDR and then follows the display's brightness and other HDR
    /// content; `potential` is `maximumPotentialExtendedDynamicRangeColorComponentValue`,
    /// what the display can reach at all.
    pub(crate) fn headroom(&self) -> (f32, f32) {
        let Some(main_thread) = objc2::MainThreadMarker::new() else {
            return (1., 1.);
        };
        let view = self.0.get(main_thread);
        view.window()
            .and_then(|w| w.screen())
            .map_or((1., 1.), |s| {
                (
                    (s.maximumExtendedDynamicRangeColorComponentValue() as f32).max(1.),
                    (s.maximumPotentialExtendedDynamicRangeColorComponentValue() as f32).max(1.),
                )
            })
    }
}

/// How the window surface's pixels are read on macOS.
#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Layer {
    /// The pixels are already in the monitor's device RGB (a manual monitor
    /// transform): no colour space, so nothing converts them twice.
    pub(crate) device_rgb: bool,
    /// The surface holds 16-bit floats: tagged extended sRGB, whose values above 1
    /// are light above SDR white; egui writes sRGB-encoded values to it either way.
    pub(crate) extended: bool,
    /// Asks the compositor for extended dynamic range: an HDR photo is shown.
    pub(crate) edr: bool,
}

#[cfg(target_os = "macos")]
fn prepare_layer(layer: &objc2_quartz_core::CALayer, mode: Layer) {
    use objc2_core_graphics::{CGColorSpace, kCGColorSpaceExtendedSRGB, kCGColorSpaceSRGB};

    // wgpu-hal 30.0.1 resets sRGB surfaces to nil, which means device RGB,
    // not sRGB. Tagging the layer lets Core Animation match the current display.
    // A manual monitor transform already outputs device RGB, so keep nil there.
    let space = (!mode.device_rgb).then(|| {
        // SAFETY: these constants are available on every supported macOS version.
        let name = if mode.extended {
            unsafe { kCGColorSpaceExtendedSRGB }
        } else {
            unsafe { kCGColorSpaceSRGB }
        };
        CGColorSpace::with_name(Some(name)).expect("macOS provides the sRGB color spaces")
    });
    tag_layers(
        layer,
        space.as_deref(),
        mode.edr && mode.extended && !mode.device_rgb,
    );
}

#[cfg(target_os = "macos")]
fn tag_layers(
    layer: &objc2_quartz_core::CALayer,
    space: Option<&objc2_core_graphics::CGColorSpace>,
    edr: bool,
) {
    use objc2_quartz_core::CAMetalLayer;

    // raw-window-metal normally installs a sublayer beneath the view's layer.
    if let Some(metal) = layer.downcast_ref::<CAMetalLayer>() {
        if metal.colorspace().as_deref() != space {
            metal.setColorspace(space);
        }
        if metal.wantsExtendedDynamicRangeContent() != edr {
            metal.setWantsExtendedDynamicRangeContent(edr);
        }
    }
    // SAFETY: AppKit layers are visited on the main thread, with no concurrent
    // hierarchy mutation. Tests use isolated layers owned by their test thread.
    if let Some(children) = unsafe { layer.sublayers() } {
        for child in children {
            tag_layers(&child, space, edr);
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::{Layer, prepare_layer};
    const SDR: Layer = Layer {
        device_rgb: false,
        extended: false,
        edr: false,
    };
    const DEVICE: Layer = Layer {
        device_rgb: true,
        ..SDR
    };
    use objc2_core_graphics::{CGColorSpace, kCGColorSpaceSRGB};
    use objc2_quartz_core::{CALayer, CAMetalLayer};

    fn assert_srgb(layer: &CAMetalLayer) {
        let space = layer
            .colorspace()
            .expect("Metal surface must be color managed");
        let name = CGColorSpace::name(Some(&space)).unwrap();
        // SAFETY: this framework constant is available on every supported macOS.
        assert_eq!(&*name, unsafe { kCGColorSpaceSRGB });
    }

    #[test]
    fn srgb_surface_is_tagged_after_creation_reset_and_replacement() {
        // wgpu installs a Metal sublayer, not necessarily the NSView's root.
        let root = CALayer::new();
        let container = CALayer::new();
        let metal = CAMetalLayer::new();
        root.addSublayer(&container);
        container.addSublayer(&metal);
        metal.setColorspace(None);
        prepare_layer(&root, SDR);
        assert_srgb(&metal);
        // wgpu 30 resets this property when reconfiguring (e.g. on resize).
        metal.setColorspace(None);
        prepare_layer(&root, SDR);
        assert_srgb(&metal);
        metal.removeFromSuperlayer();
        let replacement = CAMetalLayer::new();
        root.addSublayer(&replacement);
        prepare_layer(&root, SDR);
        assert_srgb(&replacement);
    }

    #[test]
    fn manual_device_rgb_is_not_color_converted_twice() {
        let metal = CAMetalLayer::new();
        prepare_layer(&metal, SDR);
        assert_srgb(&metal);
        prepare_layer(&metal, DEVICE);
        assert!(metal.colorspace().is_none());
        prepare_layer(&metal, SDR);
        assert_srgb(&metal);
    }

    /// A float surface is tagged extended sRGB, and asks for EDR only while an HDR
    /// photo is shown; a device-RGB monitor transform never does.
    #[test]
    fn a_float_surface_is_extended_srgb_and_edr_follows_the_photo() {
        use objc2_core_graphics::{CGColorSpace, kCGColorSpaceExtendedSRGB};
        let metal = CAMetalLayer::new();
        let extended = Layer {
            extended: true,
            ..SDR
        };
        prepare_layer(&metal, extended);
        let name = CGColorSpace::name(metal.colorspace().as_deref()).unwrap();
        // SAFETY: this framework constant is available on every supported macOS.
        assert_eq!(&*name, unsafe { kCGColorSpaceExtendedSRGB });
        assert!(!metal.wantsExtendedDynamicRangeContent());
        prepare_layer(
            &metal,
            Layer {
                edr: true,
                ..extended
            },
        );
        assert!(metal.wantsExtendedDynamicRangeContent());
        prepare_layer(
            &metal,
            Layer {
                edr: true,
                ..DEVICE
            },
        );
        assert!(!metal.wantsExtendedDynamicRangeContent());
        assert!(metal.colorspace().is_none());
    }
}
