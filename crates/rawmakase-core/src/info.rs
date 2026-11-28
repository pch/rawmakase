//! What the library says of a photo before an app edits it: its file's
//! metadata, and the camera profiles that fit it, as plain values.
use rawmakase_model::camera_data;
use rawmakase_model::camera_profiles::CameraProfile;
use std::sync::Arc;

/// What a photo's file says of itself: camera, lens, exposure and size. A
/// number the file doesn't give is 0, and a name it doesn't give is empty.
///
/// Made from the decoder's own facts as the photo is opened; those the
/// renderer works from (the camera's matrices, its profile data) are the
/// library's and not shown.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Metadata {
    /// The camera's maker, as RAWmakase names it (`Nikon`, not `NIKON
    /// CORPORATION`).
    pub make: String,
    /// The camera's model, as RAWmakase names it.
    pub model: String,
    /// The lens, as the file or its imported lens profile names it.
    pub lens: String,
    /// The ISO speed.
    pub iso: f32,
    /// The exposure time, in seconds.
    pub shutter: f32,
    /// The aperture, as an f-number.
    pub aperture: f32,
    /// The focal length, in millimetres.
    pub focal: f32,
    /// The decoded image's width in pixels, before any crop or rotation:
    /// [`Image::output_size`](crate::Image::output_size) is what renders.
    pub width: u32,
    /// The decoded image's height in pixels, as `width` is.
    pub height: u32,
    /// A rendered file (a JPEG, a TIFF…) rather than a RAW: its Temp and
    /// Tint are relative, as in Lightroom.
    pub rendered: bool,
    /// A monochrome photo, developed as grey, with no white balance.
    pub monochrome: bool,
}

impl Metadata {
    pub(crate) fn of(m: &camera_data::Metadata) -> Self {
        Self {
            make: m.make.clone(),
            model: m.model.clone(),
            lens: m.lens_model.clone(),
            iso: m.iso,
            shutter: m.shutter,
            aperture: m.aperture,
            focal: m.focal,
            width: m.width,
            height: m.height,
            rendered: m.is_rendered(),
            monochrome: m.monochrome,
        }
    }
}

/// A camera profile that fits a photo: Adobe's, the camera maker's look or
/// one installed in the context's data folders. Its color data is the
/// renderer's.
#[derive(Clone)]
pub struct Profile(pub(crate) Arc<CameraProfile>);

impl Profile {
    /// Its name, which a recipe names it by.
    pub fn name(&self) -> &str {
        &self.0.name
    }

    /// The camera it is made for, as the profile names it.
    pub fn camera(&self) -> &str {
        &self.0.camera
    }

    /// Its copyright notice, if it has one.
    pub fn copyright(&self) -> &str {
        &self.0.copyright
    }

    /// Whether it is a high dynamic range profile.
    pub fn is_hdr(&self) -> bool {
        self.0.is_hdr()
    }

    /// Whether it takes an amount, as a Creative profile does.
    pub fn takes_amount(&self) -> bool {
        self.0.supports_amount()
    }
}

impl std::fmt::Debug for Profile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Profile")
            .field("name", &self.0.name)
            .finish_non_exhaustive()
    }
}
