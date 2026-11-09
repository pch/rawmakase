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
            prepare_and_remember(&root, layer);
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

#[cfg(target_os = "macos")]
thread_local! {
    /// The root layer and declaration [`Surface::prepare`] last applied, for
    /// [`prepare_again`]. Weak, so a closed window's layers are not kept alive.
    static LAST_PREPARED: std::cell::Cell<
        Option<(objc2::rc::Weak<objc2_quartz_core::CALayer>, Layer)>,
    > = const { std::cell::Cell::new(None) };
}

#[cfg(target_os = "macos")]
fn prepare_and_remember(root: &objc2::rc::Retained<objc2_quartz_core::CALayer>, layer: Layer) {
    prepare_layer(root, layer);
    LAST_PREPARED.set(Some((objc2::rc::Weak::from_retained(root), layer)));
}

/// Applies the last declaration [`Surface::prepare`] made again. Called right
/// after eframe handles a window event, before control returns to AppKit.
///
/// egui-wgpu reconfigures the surface on every resize with wgpu's `Auto` colour
/// space. For a float surface wgpu resolves that to extended *linear* sRGB and
/// turns EDR on. The paint callback puts the declaration back only in the next
/// frame, and during a live resize Core Animation can show the window in
/// between: once, with its sRGB-encoded pixels read as linear, so washed out.
#[cfg(target_os = "macos")]
pub(crate) fn prepare_again() {
    let Some((root, layer)) = LAST_PREPARED.take() else {
        return;
    };
    if let Some(live) = root.load() {
        prepare_layer(&live, layer);
    }
    LAST_PREPARED.set(Some((root, layer)));
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

/// What a Wayland compositor says about the light a window's display shows, through
/// the color-management protocol (`wp_color_manager_v1`).
///
/// Only reading: the compositor's image description of a surface (its preferred one)
/// or of an output. The window's own surface is tagged by Mesa's Vulkan WSI from the
/// swapchain's colour space (linear extended sRGB on a float surface). Mesa holds a
/// `wp_color_management_surface_v1` on the window for the swapchain's whole life, so
/// asking for a second one here would be a protocol error that ends the connection.
#[cfg(target_os = "linux")]
pub(crate) mod wayland {
    use wayland_client::{
        Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
        protocol::{wl_output::WlOutput, wl_registry::WlRegistry, wl_surface::WlSurface},
    };
    use wayland_protocols::wp::color_management::v1::client::{
        wp_color_management_output_v1::WpColorManagementOutputV1,
        wp_color_management_surface_feedback_v1::{
            self as feedback, WpColorManagementSurfaceFeedbackV1,
        },
        wp_color_manager_v1::{TransferFunction, WpColorManagerV1},
        wp_image_description_info_v1::{self as info, WpImageDescriptionInfoV1},
        wp_image_description_v1::{self as description, WpImageDescriptionV1},
    };

    /// The luminances of an image description, in cd/m².
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub(crate) struct Description {
        /// The primary colour volume's minimum: black.
        pub(crate) min: f32,
        /// Reference white: where the output shows SDR white.
        pub(crate) reference: u32,
        /// The primary colour volume's maximum.
        pub(crate) max: u32,
        /// The most the display reaches, when the description says.
        pub(crate) target_max: Option<u32>,
        /// PQ, whose nominal maximum is always 10000 cd/m², whatever the display.
        pub(crate) pq: bool,
    }

    /// Where a compositor shows a linear surface's 1.0, which is where the interface's
    /// SDR white lands: Mesa declares the window's linear surface with no luminances,
    /// and compositors read that differently.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Anchor {
        /// At the output's reference white, as the protocol's default luminances
        /// (reference white at the maximum) have it.
        Reference,
        /// At 80 cd/m², in absolute terms, whatever the output: Hyprland 0.56 takes an
        /// undescribed linear surface for Windows' scRGB. Its sRGB windows' white is at
        /// `sdr_max_luminance`, 80 cd/m² unless the monitor sets another, so the
        /// interface matches them then.
        Nits80,
    }

    impl Anchor {
        /// The anchor of the compositor this session runs under.
        pub(crate) fn of_session() -> Self {
            if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
                Self::Nits80
            } else {
                Self::Reference
            }
        }
    }

    impl Description {
        /// The interface's white in cd/m² (see [`Anchor`]).
        fn white(&self, anchor: Anchor) -> f32 {
            match anchor {
                Anchor::Reference => self.reference as f32,
                Anchor::Nits80 => 80.,
            }
        }

        /// The most the display reaches: its target luminance, or else the maximum
        /// unless that is PQ's nominal 10000 cd/m², which says nothing of the display.
        fn reach(&self) -> Option<f32> {
            match self.target_max.filter(|&t| t > 0) {
                Some(target) => Some(target as f32),
                None if self.pq && self.max >= 10000 => None,
                None => Some(self.max as f32),
            }
        }

        /// How far above the interface's white the display shows light, as a linear
        /// factor (1 = none).
        pub(crate) fn headroom(&self, anchor: Anchor) -> f32 {
            let white = self.white(anchor);
            match self.reach() {
                Some(reach) if white > 0. => (reach / white).max(1.),
                _ => 1.,
            }
        }

        /// The output's black as a fraction of the interface's white: where a linear
        /// surface's black has to be for the interface to look as it does on an sRGB
        /// surface.
        pub(crate) fn black(&self, anchor: Anchor) -> f32 {
            let white = self.white(anchor);
            if white > 0. { self.min / white } else { 0. }
        }
    }

    /// The preferred image description of a window's surface, which the compositor
    /// changes when the window moves to another output or the output changes (HDR
    /// turned on or off, a new SDR brightness). It is read on winit's connection, in
    /// a queue of its own that winit's reads fill and [`Surface::description`] drains.
    pub(crate) struct Surface {
        queue: EventQueue<State>,
        state: State,
    }

    #[derive(Default)]
    struct State {
        manager: Option<WpColorManagerV1>,
        feedback: Option<WpColorManagementSurfaceFeedbackV1>,
        /// The description being read; complete at its info's `done`.
        reading: Description,
        description: Option<Description>,
    }

    impl Surface {
        /// `None` off Wayland, or when the compositor has no color management.
        pub(crate) fn from_window(
            window: &(
                 impl winit::raw_window_handle::HasWindowHandle
                 + winit::raw_window_handle::HasDisplayHandle
             ),
        ) -> Option<Self> {
            use winit::raw_window_handle::{RawDisplayHandle, RawWindowHandle};
            let RawDisplayHandle::Wayland(display) = window.display_handle().ok()?.as_raw() else {
                return None;
            };
            let RawWindowHandle::Wayland(handle) = window.window_handle().ok()?.as_raw() else {
                return None;
            };
            // SAFETY: winit's wl_display outlives the window, and the app (which holds
            // this) is dropped before winit's event loop. A foreign backend never
            // closes the display.
            let backend = unsafe {
                wayland_backend::client::Backend::from_foreign_display(
                    display.display.as_ptr().cast(),
                )
            };
            let connection = Connection::from_backend(backend);
            // SAFETY: the window handle's surface is a live wl_surface of this display.
            let id = unsafe {
                wayland_backend::client::ObjectId::from_ptr(
                    WlSurface::interface(),
                    handle.surface.as_ptr().cast(),
                )
            }
            .ok()?;
            let surface = WlSurface::from_id(&connection, id).ok()?;
            let mut queue = connection.new_event_queue();
            let handle = queue.handle();
            connection.display().get_registry(&handle, ());
            let mut state = State::default();
            queue.roundtrip(&mut state).ok()?;
            let manager = state.manager.clone()?;
            // Feedback, unlike the surface's colour management, may exist more than
            // once per surface: Mesa's own doesn't conflict with it.
            let feedback = manager.get_surface_feedback(&surface, &handle, ());
            feedback.get_preferred(&handle, ());
            state.feedback = Some(feedback);
            queue.roundtrip(&mut state).ok()?;
            queue.roundtrip(&mut state).ok()?;
            Some(Self { queue, state })
        }

        /// The window's display's description now, as far as the compositor has said.
        pub(crate) fn description(&mut self) -> Option<Description> {
            let _ = self.queue.dispatch_pending(&mut self.state);
            self.state.description
        }
    }

    /// Whether any output shows light above SDR white now, read on a connection of
    /// its own: the window's surface format is chosen once, before it is shown.
    pub(crate) fn any_output_has_headroom() -> bool {
        let Ok(connection) = Connection::connect_to_env() else {
            return false;
        };
        let mut queue = connection.new_event_queue();
        let handle = queue.handle();
        connection.display().get_registry(&handle, ());
        let mut outputs = Outputs::default();
        if queue.roundtrip(&mut outputs).is_err() {
            return false;
        }
        let Some(manager) = outputs.manager.clone() else {
            return false;
        };
        for output in &outputs.outputs {
            let output = manager.get_output(output, &handle, ());
            output.get_image_description(&handle, ());
        }
        // The descriptions get ready, then their information arrives.
        for _ in 0..3 {
            if queue.roundtrip(&mut outputs).is_err() {
                return false;
            }
        }
        outputs
            .described
            .iter()
            .any(|d| d.headroom(Anchor::of_session()) > 1.05)
    }

    #[derive(Default)]
    struct Outputs {
        manager: Option<WpColorManagerV1>,
        outputs: Vec<WlOutput>,
        reading: Description,
        described: Vec<Description>,
    }

    /// Records an image description's information into `reading`; `true` at its end.
    fn read(reading: &mut Description, event: info::Event) -> bool {
        match event {
            info::Event::Luminances {
                min_lum,
                max_lum,
                reference_lum,
            } => {
                // min_lum is in 0.0001 cd/m².
                reading.min = min_lum as f32 / 10000.;
                reading.max = max_lum;
                reading.reference = reference_lum;
            }
            info::Event::TargetLuminance { max_lum, .. } => reading.target_max = Some(max_lum),
            info::Event::TfNamed { tf } => {
                reading.pq = tf == WEnum::Value(TransferFunction::St2084Pq);
            }
            info::Event::Done => return true,
            _ => {}
        }
        false
    }

    fn bind_manager<D>(
        registry: &WlRegistry,
        event: &wayland_client::protocol::wl_registry::Event,
        qh: &QueueHandle<D>,
    ) -> Option<WpColorManagerV1>
    where
        D: Dispatch<WpColorManagerV1, ()> + 'static,
    {
        match event {
            wayland_client::protocol::wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == WpColorManagerV1::interface().name => {
                Some(registry.bind(*name, (*version).min(1), qh, ()))
            }
            _ => None,
        }
    }

    impl Dispatch<WlRegistry, ()> for State {
        fn event(
            state: &mut Self,
            registry: &WlRegistry,
            event: wayland_client::protocol::wl_registry::Event,
            _: &(),
            _: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            if let Some(manager) = bind_manager(registry, &event, qh) {
                state.manager = Some(manager);
            }
        }
    }

    impl Dispatch<WpColorManagementSurfaceFeedbackV1, ()> for State {
        fn event(
            _: &mut Self,
            feedback: &WpColorManagementSurfaceFeedbackV1,
            event: feedback::Event,
            _: &(),
            _: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            if let feedback::Event::PreferredChanged { .. } = event {
                feedback.get_preferred(qh, ());
            }
        }
    }

    impl Dispatch<WpImageDescriptionV1, ()> for State {
        fn event(
            state: &mut Self,
            description: &WpImageDescriptionV1,
            event: description::Event,
            _: &(),
            _: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            match event {
                description::Event::Ready { .. } => {
                    state.reading = Description::default();
                    description.get_information(qh, ());
                    description.destroy();
                }
                description::Event::Failed { .. } => {
                    state.description = None;
                    description.destroy();
                }
                _ => {}
            }
        }
    }

    impl Dispatch<WpImageDescriptionInfoV1, ()> for State {
        fn event(
            state: &mut Self,
            _: &WpImageDescriptionInfoV1,
            event: info::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if read(&mut state.reading, event) {
                state.description = Some(state.reading);
            }
        }
    }

    impl Dispatch<WlRegistry, ()> for Outputs {
        fn event(
            outputs: &mut Self,
            registry: &WlRegistry,
            event: wayland_client::protocol::wl_registry::Event,
            _: &(),
            _: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            if let Some(manager) = bind_manager(registry, &event, qh) {
                outputs.manager = Some(manager);
            } else if let wayland_client::protocol::wl_registry::Event::Global {
                name,
                interface,
                ..
            } = event
                && interface == WlOutput::interface().name
            {
                outputs.outputs.push(registry.bind(name, 1, qh, ()));
            }
        }
    }

    impl Dispatch<WpImageDescriptionV1, ()> for Outputs {
        fn event(
            _: &mut Self,
            description: &WpImageDescriptionV1,
            event: description::Event,
            _: &(),
            _: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            if let description::Event::Ready { .. } = event {
                description.get_information(qh, ());
            }
        }
    }

    impl Dispatch<WpImageDescriptionInfoV1, ()> for Outputs {
        fn event(
            outputs: &mut Self,
            _: &WpImageDescriptionInfoV1,
            event: info::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if read(&mut outputs.reading, event) {
                outputs.described.push(std::mem::take(&mut outputs.reading));
            }
        }
    }

    wayland_client::delegate_noop!(State: ignore WpColorManagerV1);
    wayland_client::delegate_noop!(Outputs: ignore WpColorManagerV1);
    wayland_client::delegate_noop!(Outputs: ignore WlOutput);
    wayland_client::delegate_noop!(Outputs: ignore WpColorManagementOutputV1);
}

#[cfg(all(test, target_os = "linux"))]
mod wayland_tests {
    use super::wayland::{Anchor, Description};

    /// Hyprland 0.56's SDR output: sRGB, black at 0.2 cd/m², reference white and
    /// maximum at 80 cd/m².
    const SDR: Description = Description {
        min: 0.2,
        reference: 80,
        max: 80,
        target_max: Some(80),
        pq: false,
    };
    /// Hyprland 0.56's HDR output (`cm = "hdr"`): PQ, reference white at 203 cd/m²,
    /// and the display's 600 cd/m² from its EDID as the target.
    const HDR: Description = Description {
        min: 0.005,
        reference: 203,
        max: 10000,
        target_max: Some(600),
        pq: true,
    };

    /// An SDR output shows nothing above SDR white, whichever way the compositor
    /// reads a linear surface.
    #[test]
    fn an_sdr_output_has_no_headroom() {
        assert_eq!(SDR.headroom(Anchor::Nits80), 1.);
        assert_eq!(SDR.headroom(Anchor::Reference), 1.);
    }

    /// Hyprland shows a linear surface's 1.0 at 80 cd/m², where its sRGB windows'
    /// white is (`sdr_max_luminance`'s default), not at the output's 203 cd/m²
    /// reference: the display's 600 cd/m² is 7.5 times the interface's white.
    #[test]
    fn hyprland_measures_headroom_from_80_nits() {
        assert!((HDR.headroom(Anchor::Nits80) - 7.5).abs() < 1e-6);
    }

    /// A compositor that anchors a linear surface's 1.0 to reference white, as the
    /// protocol's default luminances have it, gives the target over reference white.
    #[test]
    fn otherwise_headroom_is_over_reference_white() {
        assert!((HDR.headroom(Anchor::Reference) - 600. / 203.).abs() < 1e-6);
    }

    /// PQ's nominal maximum is always 10000 cd/m²: without a target nothing is known
    /// of the display, unless the output's maximum was set to something else.
    #[test]
    fn a_pq_output_without_a_target_reaches_its_maximum_only_when_it_says_one() {
        let unknown = Description {
            target_max: None,
            ..HDR
        };
        assert_eq!(unknown.headroom(Anchor::Nits80), 1.);
        let set = Description {
            max: 400,
            ..unknown
        };
        assert_eq!(set.headroom(Anchor::Nits80), 5.);
    }

    /// A non-PQ description without a target reaches its own maximum; a degenerate
    /// one shows no HDR rather than dividing by zero, and never less than SDR.
    #[test]
    fn otherwise_the_maximum_and_never_below_one() {
        let linear = Description {
            target_max: None,
            max: 320,
            ..SDR
        };
        assert_eq!(linear.headroom(Anchor::Reference), 4.);
        let none = Description {
            reference: 0,
            ..linear
        };
        assert_eq!(none.headroom(Anchor::Reference), 1.);
        assert_eq!(none.black(Anchor::Reference), 0.);
        let dim = Description { max: 40, ..linear };
        assert_eq!(dim.headroom(Anchor::Reference), 1.);
    }

    /// Hyprland reads a linear surface's 0 as no light at all, below the 0.2 cd/m²
    /// its sRGB windows' black shows at on an SDR output: the interface's black is
    /// lifted to the output's, as a fraction of the interface's white.
    #[test]
    fn black_is_the_outputs_minimum_over_the_interfaces_white() {
        assert!((SDR.black(Anchor::Nits80) - 0.0025).abs() < 1e-7);
        assert!((HDR.black(Anchor::Nits80) - 0.005 / 80.).abs() < 1e-9);
        assert!((HDR.black(Anchor::Reference) - 0.005 / 203.).abs() < 1e-9);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::{Layer, prepare_again, prepare_and_remember, prepare_layer};
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

    /// egui-wgpu reconfigures the surface on every resize with wgpu's `Auto`
    /// colour space, which tags a float surface extended linear sRGB and turns
    /// EDR on. Applying the declaration again after the window event has to undo
    /// both before Core Animation shows the window washed out.
    #[test]
    fn a_resize_reconfigure_is_undone_before_the_next_paint() {
        use objc2_core_graphics::{
            CGColorSpace, kCGColorSpaceExtendedLinearSRGB, kCGColorSpaceExtendedSRGB,
        };
        let root = CALayer::new();
        let metal = CAMetalLayer::new();
        root.addSublayer(&metal);
        let extended = Layer {
            extended: true,
            ..SDR
        };
        prepare_and_remember(&root, extended);
        // What wgpu-hal 30.0.1's `configure` does for `ExtendedSrgbLinear`.
        // SAFETY: this framework constant is available on every supported macOS.
        let linear =
            CGColorSpace::with_name(Some(unsafe { kCGColorSpaceExtendedLinearSRGB })).unwrap();
        metal.setColorspace(Some(&linear));
        metal.setWantsExtendedDynamicRangeContent(true);
        prepare_again();
        let name = CGColorSpace::name(metal.colorspace().as_deref()).unwrap();
        // SAFETY: as above.
        assert_eq!(&*name, unsafe { kCGColorSpaceExtendedSRGB });
        assert!(!metal.wantsExtendedDynamicRangeContent());
        // A window that is gone leaves nothing to tag.
        drop((root, metal));
        prepare_again();
    }
}
