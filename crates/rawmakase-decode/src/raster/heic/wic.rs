//! A HEIC's pixels decoded by Windows Imaging Component, whose HEIF decoder
//! and HEVC decoder are Microsoft's, so RAWmakase ships none there.
//!
//! Both come from the Microsoft Store, as the HEIF Image Extensions and the
//! HEVC Video Extensions, and a PC may lack either: then WIC can't decode the
//! file. Which failure means that is not told apart here, so the error gives
//! WIC's reason and says what to install if no HEIC photo opens.
//!
//! WIC's frame is converted to 8- or 16-bit RGB, the depth the header says,
//! and read as it is: in the colour space of the file, which the profile the
//! header gives describes. As for ImageIO, RAWmakase turns the picture by the
//! header, and crops what WIC did not.
use super::{HeicFile, Pixels};
use anyhow::{Result, anyhow, ensure};
use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat24bppRGB, GUID_WICPixelFormat48bppRGB,
    IWICImagingFactory, WICConvertBitmapSource, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::core::HSTRING;

/// COM on this thread for as long as it lives: initialized here unless the
/// thread already was, in another mode, which serves as well.
struct Com(bool);

impl Com {
    fn init() -> Self {
        // SAFETY: no reserved pointer; balanced by `drop` when it succeeds.
        Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: balances the successful CoInitializeEx in `init`.
            unsafe { CoUninitialize() }
        }
    }
}

pub(super) fn pixels(file: HeicFile) -> Result<Pixels> {
    // Declared first, so dropped after every COM object below.
    let _com = Com::init();
    let missing = |e: windows::core::Error| {
        anyhow!(
            "Windows could not decode the HEIF image ({}). If no HEIC photo opens, install \
             Microsoft's HEIF Image Extensions and HEVC Video Extensions from the Microsoft \
             Store.",
            e.message()
        )
    };
    // SAFETY: COM is initialized on this thread; the factory is an
    // in-process server.
    let factory: IWICImagingFactory =
        unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
            .map_err(|e| anyhow!("Windows Imaging Component is unavailable: {}", e.message()))?;
    let path = HSTRING::from(file.path.as_path());
    // SAFETY: `path` outlives the call; no vendor is preferred.
    let decoder = unsafe {
        factory.CreateDecoderFromFilename(&path, None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)
    }
    .map_err(missing)?;
    // SAFETY: frame 0 is the primary image.
    let frame = unsafe { decoder.GetFrame(0) }.map_err(missing)?;
    let (format, pixel) = if file.deep {
        (GUID_WICPixelFormat48bppRGB, 6)
    } else {
        (GUID_WICPixelFormat24bppRGB, 3)
    };
    // SAFETY: the format is a WIC pixel format GUID that outlives the call.
    let rgb_source = unsafe { WICConvertBitmapSource(&format, &frame) }.map_err(missing)?;
    let (mut width, mut height) = (0, 0);
    // SAFETY: both pointers are to live locals.
    unsafe { rgb_source.GetSize(&mut width, &mut height) }.map_err(missing)?;
    ensure!(
        file.expects(width, height),
        "The HEIF image is not the size it says"
    );
    let stride = width
        .checked_mul(pixel)
        .ok_or_else(|| anyhow!("The HEIF image is too large"))?;
    let mut rgb = vec![0u8; stride as usize * height as usize];
    // SAFETY: a null rectangle copies the whole image, `height` rows of
    // `stride` bytes, which `rgb` holds.
    unsafe { rgb_source.CopyPixels(std::ptr::null(), stride, &mut rgb) }.map_err(missing)?;
    Ok(Pixels { width, height, rgb })
}
