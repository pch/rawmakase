//! The rendered file formats RAWmakase opens beside RAWs, as facts with no
//! codec: their extensions, MIME types and names. The decoders are
//! `native::raster`'s, the encoders `export::encode`'s, and which formats
//! export writes is `export_settings::Format`.
use std::path::Path;

/// A rendered photo's file format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RasterFormat {
    Jpeg,
    Tiff,
    Png,
    Webp,
    Avif,
    /// HEIF of HEVC: a HEIC, as phones write, or a camera's HEIF (`.hif`).
    Heif,
    Jxl,
}

impl RasterFormat {
    pub const ALL: [Self; 7] = [
        Self::Jpeg,
        Self::Tiff,
        Self::Png,
        Self::Webp,
        Self::Avif,
        Self::Heif,
        Self::Jxl,
    ];

    /// The extensions a file of this format has, lower case; the first is
    /// the one RAWmakase names files with.
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Jpeg => &["jpg", "jpeg"],
            Self::Tiff => &["tif", "tiff"],
            Self::Png => &["png"],
            Self::Webp => &["webp"],
            Self::Avif => &["avif"],
            Self::Heif => &["heic", "heif", "hif"],
            Self::Jxl => &["jxl"],
        }
    }
    /// The extension RAWmakase names a file of this format with.
    pub fn extension(self) -> &'static str {
        self.extensions()[0]
    }
    pub fn mime_type(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Tiff => "image/tiff",
            Self::Png => "image/png",
            Self::Webp => "image/webp",
            Self::Avif => "image/avif",
            Self::Heif => "image/heic",
            Self::Jxl => "image/jxl",
        }
    }
    /// The format's name, as the app shows it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Jpeg => "JPEG",
            Self::Tiff => "TIFF",
            Self::Png => "PNG",
            Self::Webp => "WebP",
            Self::Avif => "AVIF",
            Self::Heif => "HEIF",
            Self::Jxl => "JPEG XL",
        }
    }
    /// The format an extension names, in any case.
    pub fn from_extension(extension: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| {
            f.extensions()
                .iter()
                .any(|e| extension.eq_ignore_ascii_case(e))
        })
    }
    /// The format `path`'s extension names, in any case.
    pub fn of(path: &Path) -> Option<Self> {
        path.extension()
            .and_then(|e| e.to_str())
            .and_then(Self::from_extension)
    }
}

/// A file RAWmakase takes as a photo: a RAW, or a JPEG, TIFF, PNG, WebP, AVIF,
/// HEIF or JPEG XL, and not hidden.
pub fn is_photo(path: &Path) -> bool {
    super::is_raw(path) || (!super::is_hidden(path) && RasterFormat::of(path).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_format_is_known_by_any_of_its_extensions_in_any_case() {
        for (name, format) in [
            ("A.jpg", RasterFormat::Jpeg),
            ("A.JPEG", RasterFormat::Jpeg),
            ("A.Jpeg", RasterFormat::Jpeg),
            ("A.tif", RasterFormat::Tiff),
            ("A.TIFF", RasterFormat::Tiff),
            ("A.png", RasterFormat::Png),
            ("dir.png/A.PNG", RasterFormat::Png),
            ("A.webp", RasterFormat::Webp),
            ("A.AVIF", RasterFormat::Avif),
            ("A.heic", RasterFormat::Heif),
            ("A.HEIF", RasterFormat::Heif),
            ("A.HIF", RasterFormat::Heif),
            ("A.Jxl", RasterFormat::Jxl),
        ] {
            assert_eq!(RasterFormat::of(Path::new(name)), Some(format), "{name}");
        }
        for name in ["A.NEF", "A.dng", "A.jpg.xmp", "jpg", "A."] {
            assert_eq!(RasterFormat::of(Path::new(name)), None, "{name}");
        }
    }
    #[test]
    fn each_extension_is_lower_case_and_names_one_format_and_no_raw() {
        for format in RasterFormat::ALL {
            for extension in format.extensions() {
                assert_eq!(*extension, extension.to_ascii_lowercase());
                assert_eq!(RasterFormat::from_extension(extension), Some(format));
                assert!(!super::super::files::RAW_EXTENSIONS.contains(extension));
            }
        }
    }
    #[test]
    fn each_format_has_its_extension_mime_type_and_name() {
        let named = RasterFormat::ALL.map(|f| (f.extension(), f.mime_type(), f.name()));
        assert_eq!(
            named,
            [
                ("jpg", "image/jpeg", "JPEG"),
                ("tif", "image/tiff", "TIFF"),
                ("png", "image/png", "PNG"),
                ("webp", "image/webp", "WebP"),
                ("avif", "image/avif", "AVIF"),
                ("heic", "image/heic", "HEIF"),
                ("jxl", "image/jxl", "JPEG XL"),
            ]
        );
    }
    #[test]
    fn photos_are_raws_and_rendered_files_not_hidden() {
        for name in ["A.NEF", "A.jpeg", "A.TIF", "A.png", "A.webp", "A.HEIC"] {
            assert!(is_photo(Path::new(name)), "{name}");
        }
        for name in ["._A.jpg", ".A.png", "A.xmp", "A"] {
            assert!(!is_photo(Path::new(name)), "{name}");
        }
    }
}
