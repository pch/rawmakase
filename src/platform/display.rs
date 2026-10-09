//! Declare the desktop surface's pixel color space to the macOS compositor.

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
    pub(crate) fn prepare(&self, device_rgb: bool) {
        let Some(main_thread) = objc2::MainThreadMarker::new() else {
            return;
        };
        // Look up the current layer; surface recovery can replace it.
        if let Some(layer) = self.0.get(main_thread).layer() {
            prepare_layer(&layer, device_rgb);
        }
    }
}

#[cfg(target_os = "macos")]
fn prepare_layer(layer: &objc2_quartz_core::CALayer, device_rgb: bool) {
    use objc2_core_graphics::{CGColorSpace, kCGColorSpaceSRGB};

    // wgpu-hal 30.0.1 resets sRGB surfaces to nil, which means device RGB,
    // not sRGB. Tagging the layer lets Core Animation match the current display.
    // A manual monitor transform already outputs device RGB, so keep nil there.
    let srgb = (!device_rgb).then(|| {
        // SAFETY: this constant is available on every supported macOS version.
        CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))
            .expect("macOS provides the standard sRGB color space")
    });
    tag_layers(layer, srgb.as_deref());
}

#[cfg(target_os = "macos")]
fn tag_layers(
    layer: &objc2_quartz_core::CALayer,
    space: Option<&objc2_core_graphics::CGColorSpace>,
) {
    use objc2_quartz_core::CAMetalLayer;

    // raw-window-metal normally installs a sublayer beneath the view's layer.
    if let Some(metal) = layer.downcast_ref::<CAMetalLayer>()
        && metal.colorspace().as_deref() != space
    {
        metal.setColorspace(space);
    }
    // SAFETY: AppKit layers are visited on the main thread, with no concurrent
    // hierarchy mutation. Tests use isolated layers owned by their test thread.
    if let Some(children) = unsafe { layer.sublayers() } {
        for child in children {
            tag_layers(&child, space);
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::prepare_layer;
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
        prepare_layer(&root, false);
        assert_srgb(&metal);
        // wgpu 30 resets this property when reconfiguring (e.g. on resize).
        metal.setColorspace(None);
        prepare_layer(&root, false);
        assert_srgb(&metal);
        metal.removeFromSuperlayer();
        let replacement = CAMetalLayer::new();
        root.addSublayer(&replacement);
        prepare_layer(&root, false);
        assert_srgb(&replacement);
    }

    #[test]
    fn manual_device_rgb_is_not_color_converted_twice() {
        let metal = CAMetalLayer::new();
        prepare_layer(&metal, false);
        assert_srgb(&metal);
        prepare_layer(&metal, true);
        assert!(metal.colorspace().is_none());
        prepare_layer(&metal, false);
        assert_srgb(&metal);
    }
}
