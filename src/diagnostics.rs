//! What a bug report needs to know about this copy of RAWmakase and the
//! system it runs on: the plain-text system report the Report an Issue
//! dialog shows and `rawmakase system-info` prints. Nothing here is sent
//! anywhere; the user copies the text into an issue themselves.
//!
//! The report is built from plain snapshots: the build ([`Build`]), the
//! system, probed once per process ([`System`]), and what the window knew
//! when the dialog opened ([`Window`]). It names no path, user, file,
//! catalog or ICC profile: none of those are in the snapshots. Text from
//! outside the app (a driver's name, os-release, an environment variable) is
//! cleaned and bounded. A probe that fails leaves only its own line
//! unavailable.
use std::fmt;
use std::path::Path;
use std::time::Duration;

/// The repository's issues, searched before reporting.
pub const SEARCH_ISSUES: &str = "https://github.com/pch/rawmakase/issues?q=is%3Aissue";

/// How long finding the installation may take: it can run the package
/// manager.
pub const INSTALLATION_TIMEOUT: Duration = Duration::from_secs(10);

/// The longest a value from outside the app may be, in characters.
const UNTRUSTED_MAX: usize = 96;

/// A new bug report with `version` filled in. The system report is pasted
/// rather than put in the URL: GitHub refuses long URLs, and the browser
/// would be the only one to know.
pub fn new_issue_url(version: &str) -> String {
    format!(
        "https://github.com/pch/rawmakase/issues/new?template=bug_report.yml&version={}",
        percent_encode(version)
    )
}

/// RFC 3986's unreserved characters as they are; every other byte escaped.
fn percent_encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// One line's value, or why it is missing. A missing value never stops the
/// report.
#[derive(Clone, Debug, PartialEq)]
pub enum Field {
    Value(String),
    Unavailable(&'static str),
}

impl Field {
    /// Text from outside the app, on one line, without control characters
    /// and at most `max` characters long; unavailable when nothing is left.
    /// A value that looks like it names a path is withheld whole: cutting
    /// the path out of it can't be done reliably.
    pub fn untrusted(text: &str, max: usize) -> Self {
        let words: Vec<&str> = text
            .split(|c: char| c.is_whitespace() || c.is_control())
            .filter(|word| !word.is_empty())
            .collect();
        let line = words.join(" ");
        // Checked as it would be shown: a control character can't hide one.
        if names_a_path(&line) {
            return Self::Unavailable("withheld: looked like a path");
        }
        if line.is_empty() {
            return Self::Unavailable("empty");
        }
        if line.chars().count() <= max {
            return Self::Value(line);
        }
        let mut cut: String = line.chars().take(max.saturating_sub(1)).collect();
        cut.push('…');
        Self::Value(cut)
    }

    /// `untrusted`, for a value the user sets (the desktop, a custom
    /// kernel's suffix), which could also hold their login name: such a
    /// value is withheld whole.
    fn from_environment(text: &str, max: usize, user: Option<&str>) -> Self {
        if user.is_some_and(|user| holds_word(text, user)) {
            return Self::Unavailable("withheld: held the user name");
        }
        Self::untrusted(text, max)
    }

    fn known(value: Option<String>, missing: &'static str) -> Self {
        value.map_or(Self::Unavailable(missing), Self::Value)
    }
}

impl fmt::Display for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Value(value) => f.write_str(value),
            Self::Unavailable(why) => write!(f, "unavailable ({why})"),
        }
    }
}

/// Whether `text` may name a place on disk: a part that starts with `/`
/// (after a separator, unlike `GNU/Linux`), `./` or `../`, `~`, a
/// backslash, a URL, a drive letter (`C:`) or a percent-escape.
fn names_a_path(text: &str) -> bool {
    if text.contains(['\\', '~', '%']) || text.contains("://") || text.contains("./") {
        return true;
    }
    let chars: Vec<char> = text.chars().collect();
    let separator =
        |c: Option<&char>| c.is_none_or(|c| c.is_whitespace() || ":;,=([{\"'<>|".contains(*c));
    chars.iter().enumerate().any(|(i, c)| {
        let before = i.checked_sub(1).and_then(|i| chars.get(i));
        match c {
            '/' => separator(before),
            // C:Users, C:/x: a lone letter, then a colon and a name.
            ':' => {
                let letter = before.is_some_and(char::is_ascii_alphabetic);
                let lone = separator(i.checked_sub(2).and_then(|i| chars.get(i)));
                let next = chars
                    .get(i + 1)
                    .is_some_and(|c| c.is_alphanumeric() || *c == '/');
                letter && lone && next
            }
            _ => false,
        }
    })
}

/// Whether `text` holds `word` whole, in any case: not inside a longer
/// word, so the login `ian` is not found in `Debian`.
fn holds_word(text: &str, word: &str) -> bool {
    let (text, word) = (text.to_lowercase(), word.to_lowercase());
    if word.is_empty() {
        return false;
    }
    let edge = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric());
    text.match_indices(&word).any(|(at, _)| {
        edge(text[..at].chars().next_back()) && edge(text[at + word.len()..].chars().next())
    })
}

/// This build.
#[derive(Clone, Debug, PartialEq)]
pub struct Build {
    /// The build's own version, not `RAWMAKASE_PRETEND_VERSION`'s.
    pub version: &'static str,
    /// The commit it was built from, as build.rs found it.
    pub commit: &'static str,
    /// The target triple it was built for.
    pub target: &'static str,
    /// RAWmakase's RAW decoders.
    pub decoder: String,
}

impl Build {
    pub fn current() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION"),
            commit: env!("RAWMAKASE_COMMIT"),
            target: env!("RAWMAKASE_TARGET"),
            decoder: crate::raw::version(),
        }
    }
}

/// The system, the same for the whole process. Linux's own lines are None
/// elsewhere, and left out of the report.
#[derive(Clone, Debug, PartialEq)]
pub struct System {
    pub installation: Field,
    pub os: Field,
    pub kernel: Option<Field>,
    pub desktop: Option<Field>,
    pub display_server: Option<Field>,
    pub cpu_threads: Field,
}

impl System {
    /// Probes the system. This blocks, for up to [`INSTALLATION_TIMEOUT`]
    /// while the package manager is asked, so the app calls it off the UI
    /// thread.
    pub fn probe() -> Self {
        Self::probe_with(installation_channel, INSTALLATION_TIMEOUT)
    }

    fn probe_with(
        installation: impl FnOnce() -> &'static str + Send + 'static,
        timeout: Duration,
    ) -> Self {
        let linux = cfg!(target_os = "linux");
        Self {
            installation: within(timeout, installation),
            os: guarded(os).unwrap_or(Field::Unavailable("probe failed")),
            kernel: linux.then(|| guarded(kernel).unwrap_or(Field::Unavailable("probe failed"))),
            desktop: linux.then(|| {
                let var = |name: &str| std::env::var_os(name);
                guarded(|| desktop(var)).unwrap_or(Field::Unavailable("probe failed"))
            }),
            display_server: linux.then(|| {
                let var = |name: &str| std::env::var_os(name);
                guarded(|| display_server(var)).unwrap_or(Field::Unavailable("probe failed"))
            }),
            cpu_threads: Field::known(
                std::thread::available_parallelism()
                    .ok()
                    .map(|n| n.to_string()),
                "unknown",
            ),
        }
    }

    /// Every line unavailable for the same reason: while the probe runs, or
    /// after it stopped without an answer.
    pub fn unavailable(why: &'static str) -> Self {
        let linux = cfg!(target_os = "linux");
        let field = Field::Unavailable(why);
        Self {
            installation: field.clone(),
            os: field.clone(),
            kernel: linux.then(|| field.clone()),
            desktop: linux.then(|| field.clone()),
            display_server: linux.then(|| field.clone()),
            cpu_threads: field,
        }
    }
}

/// Runs a probe, a panic in which only loses its own answer.
fn guarded<T>(probe: impl FnOnce() -> T) -> Option<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(probe)).ok()
}

/// The installation, if found within `timeout`. A probe still running then
/// is left to finish on its own thread, which holds nothing of the app's.
fn within(timeout: Duration, probe: impl FnOnce() -> &'static str + Send + 'static) -> Field {
    let (tx, rx) = std::sync::mpsc::channel();
    let started = std::thread::Builder::new()
        .name("installation probe".into())
        .spawn(move || {
            let _ = tx.send(guarded(probe));
        });
    if started.is_err() {
        return Field::Unavailable("probe failed");
    }
    match rx.recv_timeout(timeout) {
        Ok(Some(channel)) => Field::Value(channel.to_string()),
        Ok(None) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Field::Unavailable("probe failed")
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Field::Unavailable("timed out"),
    }
}

/// The GPU the window draws with, from wgpu's description of it. Its name
/// and driver come from the driver and are treated as untrusted text.
#[derive(Clone, Debug, PartialEq)]
pub struct Gpu {
    pub name: Field,
    pub vendor: String,
    pub backend: &'static str,
    pub device_type: &'static str,
    pub driver: Field,
    /// The window surface's format, as [`surface`] names it.
    pub surface: &'static str,
}

/// The window surface's format and colour space, for a report: whether the
/// window presents through a 16-bit float surface tells apart the HDR path
/// from the plain one (#1126).
pub fn surface(format: wgpu::TextureFormat, color_space: wgpu::SurfaceColorSpace) -> &'static str {
    match (format, color_space) {
        (wgpu::TextureFormat::Rgba16Float, wgpu::SurfaceColorSpace::ExtendedSrgbLinear) => {
            "16-bit float, linear extended sRGB"
        }
        (wgpu::TextureFormat::Rgba16Float, _) => "16-bit float",
        _ => "8-bit",
    }
}

impl Gpu {
    pub fn from_adapter(adapter: &wgpu::AdapterInfo, surface: &'static str) -> Self {
        let vendor = match adapter.vendor {
            0x1002 => "AMD",
            0x10de => "NVIDIA",
            0x8086 => "Intel",
            0x106b => "Apple",
            0x13b5 => "Arm",
            0x5143 => "Qualcomm",
            0x1414 => "Microsoft",
            0x10005 => "Mesa",
            _ => "",
        };
        let id = format!("0x{:04x}", adapter.vendor);
        let driver = [adapter.driver.trim(), adapter.driver_info.trim()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
        Self {
            name: Field::untrusted(&adapter.name, UNTRUSTED_MAX),
            vendor: if vendor.is_empty() {
                id
            } else {
                format!("{vendor} ({id})")
            },
            backend: match adapter.backend {
                wgpu::Backend::Vulkan => "Vulkan",
                wgpu::Backend::Metal => "Metal",
                wgpu::Backend::Dx12 => "DirectX 12",
                wgpu::Backend::Gl => "OpenGL",
                wgpu::Backend::BrowserWebGpu => "WebGPU",
                wgpu::Backend::Noop => "none",
            },
            device_type: match adapter.device_type {
                wgpu::DeviceType::IntegratedGpu => "integrated",
                wgpu::DeviceType::DiscreteGpu => "discrete",
                wgpu::DeviceType::VirtualGpu => "virtual",
                wgpu::DeviceType::Cpu => "software",
                wgpu::DeviceType::Other => "other",
            },
            driver: Field::untrusted(&driver, UNTRUSTED_MAX),
            surface,
        }
    }
}

/// The Monitor profile mode, without the profile's name.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Monitor {
    /// Following the system, with what it reports for the window's display.
    System(SystemProfile),
    Srgb,
    /// A profile the user chose.
    Custom,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SystemProfile {
    /// The system named a display profile, which renders convert to.
    Found,
    /// The system matches colours itself (macOS, Windows' advanced color).
    Managed,
    /// None was found: shown as sRGB.
    NotFound,
}

impl fmt::Display for Monitor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::System(SystemProfile::Found) => "System, display profile found",
            Self::System(SystemProfile::Managed) => "System, managed by the OS",
            Self::System(SystemProfile::NotFound) => "System, no profile found (shown as sRGB)",
            Self::Srgb => "sRGB",
            Self::Custom => "Custom ICC profile",
        })
    }
}

/// What the window knew when the report was made.
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    /// None when the window draws without a GPU.
    pub gpu: Option<Gpu>,
    /// Physical pixels per point.
    pub scale: f32,
    pub monitor: Monitor,
    /// The open catalog's photos and folders; None with no catalog open.
    pub catalog: Option<(usize, usize)>,
    pub background_task: bool,
}

/// The report, one `Label: value` per line.
pub fn report(build: &Build, system: &System, window: Option<&Window>) -> String {
    let mut lines: Vec<(&str, String)> = vec![
        ("RAWmakase", build.version.to_string()),
        ("Commit", build.commit.to_string()),
        ("Target", build.target.to_string()),
        ("RAW decoder", build.decoder.clone()),
        ("Installation", system.installation.to_string()),
        ("OS", system.os.to_string()),
    ];
    for (label, field) in [
        ("Kernel", &system.kernel),
        ("Desktop", &system.desktop),
        ("Display server", &system.display_server),
    ] {
        if let Some(field) = field {
            lines.push((label, field.to_string()));
        }
    }
    lines.push(("Architecture", std::env::consts::ARCH.to_string()));
    lines.push(("CPU threads", system.cpu_threads.to_string()));
    match window {
        Some(window) => {
            match &window.gpu {
                Some(gpu) => lines.extend([
                    ("GPU", gpu.name.to_string()),
                    ("GPU vendor", gpu.vendor.clone()),
                    ("GPU backend", gpu.backend.to_string()),
                    ("GPU type", gpu.device_type.to_string()),
                    ("GPU driver", gpu.driver.to_string()),
                    ("GPU surface", gpu.surface.to_string()),
                ]),
                None => lines.push(("GPU", "none (drawn without a GPU)".into())),
            }
            lines.push(("Display scale", format!("{:.2}", window.scale)));
            lines.push(("Monitor profile", window.monitor.to_string()));
            lines.push((
                "Catalog",
                match window.catalog {
                    Some((photos, folders)) => format!(
                        "{photos} {}, {folders} {}",
                        plural(photos, "photo"),
                        plural(folders, "folder")
                    ),
                    None => "none open".into(),
                },
            ));
            lines.push((
                "Background task",
                if window.background_task {
                    "running"
                } else {
                    "none"
                }
                .into(),
            ));
        }
        None => {
            let outside = Field::Unavailable("not in the app window").to_string();
            for label in [
                "GPU",
                "Display scale",
                "Monitor profile",
                "Catalog",
                "Background task",
            ] {
                lines.push((label, outside.clone()));
            }
        }
    }
    let mut text = String::new();
    for (label, value) in lines {
        text.push_str(label);
        text.push_str(": ");
        text.push_str(&value);
        text.push('\n');
    }
    text
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        word.to_string()
    } else {
        format!("{word}s")
    }
}

/// How this copy was installed: an AppImage, else as fastframe-update finds
/// it to decide whether it may update itself: the release download it came
/// from, or the package manager that owns it. Inspecting the install can run
/// the package manager, so this runs off the UI thread.
pub fn installation_channel() -> &'static str {
    use fastframe_update::{Kind, PackageManager, Unsupported};
    if runs_from_appimage() {
        return "linux-appimage";
    }
    match crate::updates::installation() {
        Ok(installation) => match installation.kind {
            Kind::AppImage => "linux-appimage",
            Kind::MacBundle => "macos-dmg",
            Kind::WindowsInstaller => "windows-installer",
            Kind::Portable if cfg!(windows) => "windows-zip",
            Kind::Portable => "linux-tarball",
        },
        Err(reason) => match reason {
            // Run from the disk image or a quarantine copy: still the DMG.
            Unsupported::MoveToApplications => "macos-dmg",
            Unsupported::Homebrew => "homebrew",
            Unsupported::SystemPackage(PackageManager::Apt) => "deb",
            Unsupported::SystemPackage(PackageManager::Dnf) => "rpm",
            // pacman can't tell the release package from an AUR build.
            Unsupported::SystemPackage(PackageManager::Pacman) => "arch-package",
            Unsupported::Flatpak => "flatpak",
            Unsupported::Snap => "snap",
            Unsupported::Nix => "nix",
            Unsupported::Cargo => "cargo",
            _ => "unknown",
        },
    }
}

/// The AppImage runtime sets `APPDIR` to where it mounted or extracted the
/// image. Programs started from the app inherit it, so only an executable
/// inside that directory is the AppImage itself.
fn runs_from_appimage() -> bool {
    let Some(app_dir) = std::env::var_os("APPDIR") else {
        return false;
    };
    std::env::current_exe().is_ok_and(|executable| is_inside(&executable, Path::new(&app_dir)))
}

/// Whether `executable`, already resolved, is inside `directory`.
fn is_inside(executable: &Path, directory: &Path) -> bool {
    std::fs::canonicalize(directory).is_ok_and(|directory| executable.starts_with(directory))
}

/// SystemVersion.plist's ProductVersion, e.g. `26.0.1`.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn macos_product_version(plist: &str) -> Option<String> {
    let after = &plist[plist.find("<key>ProductVersion</key>")?..];
    let start = after.find("<string>")? + "<string>".len();
    let end = start + after[start..].find('<')?;
    let version = after[start..end].trim();
    (!version.is_empty()).then(|| version.to_string())
}

#[cfg(target_os = "macos")]
pub(crate) fn macos_version() -> Option<String> {
    let plist = std::fs::read_to_string("/System/Library/CoreServices/SystemVersion.plist").ok()?;
    macos_product_version(&plist)
}

/// Windows 11 still reports major version 10; its builds start at 22000.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn windows_release(build: u32) -> u32 {
    if build >= 22_000 { 11 } else { 10 }
}

/// The registry's `CurrentBuildNumber`, e.g. 26100.
#[cfg(windows)]
fn windows_build() -> Option<u32> {
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let key = wide(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
    let value = wide("CurrentBuildNumber");
    let mut buffer = [0u16; 32];
    let mut size = std::mem::size_of_val(&buffer) as u32;
    // SAFETY: the key and value names are NUL-terminated and the buffer's
    // size in bytes is passed with it.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != 0 {
        return None;
    }
    String::from_utf16_lossy(&buffer)
        .trim_end_matches('\0')
        .parse()
        .ok()
}

#[cfg(target_os = "macos")]
fn os() -> Field {
    Field::known(
        macos_version().map(|version| match Field::untrusted(&version, 32) {
            Field::Value(version) => format!("macOS {version}"),
            Field::Unavailable(_) => "macOS".into(),
        }),
        "SystemVersion.plist unreadable",
    )
}

#[cfg(windows)]
fn os() -> Field {
    Field::known(
        windows_build().map(|build| format!("Windows {} (build {build})", windows_release(build))),
        "build number unreadable",
    )
}

#[cfg(target_os = "linux")]
fn os() -> Field {
    ["/etc/os-release", "/usr/lib/os-release"]
        .into_iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| linux_name(&text))
        .unwrap_or(Field::Unavailable("os-release unreadable"))
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
fn os() -> Field {
    Field::Value(std::env::consts::OS.into())
}

/// os-release's `PRETTY_NAME`, else its `NAME` and `VERSION_ID`.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn linux_name(os_release: &str) -> Option<Field> {
    let field = |name: &str| {
        os_release.lines().find_map(|line| {
            let value = line.strip_prefix(name)?.strip_prefix('=')?;
            let value = value.trim().trim_matches(['"', '\'']);
            (!value.is_empty()).then_some(value)
        })
    };
    let name = field("PRETTY_NAME").map(str::to_string).or_else(|| {
        let name = field("NAME")?;
        Some(match field("VERSION_ID") {
            Some(version) => format!("{name} {version}"),
            None => name.to_string(),
        })
    })?;
    Some(Field::untrusted(&name, UNTRUSTED_MAX))
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn kernel() -> Field {
    let var = |name: &str| std::env::var_os(name);
    std::fs::read_to_string("/proc/sys/kernel/osrelease").map_or(
        Field::Unavailable("unreadable"),
        |release| {
            // A custom kernel's suffix (CONFIG_LOCALVERSION) is free text.
            Field::from_environment(&release, 64, login(var).as_deref())
        },
    )
}

/// The login name, from the first of these variables that is set.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn login(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<String> {
    ["USER", "USERNAME", "LOGNAME"]
        .into_iter()
        .find_map(|name| {
            var(name)
                .map(|u| u.to_string_lossy().into_owned())
                .filter(|u| !u.is_empty())
        })
}

/// `XDG_CURRENT_DESKTOP`, e.g. `GNOME` or `Hyprland`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn desktop(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Field {
    let user = login(&var);
    match var("XDG_CURRENT_DESKTOP") {
        Some(value) => Field::from_environment(&value.to_string_lossy(), 64, user.as_deref()),
        None => Field::Unavailable("not set"),
    }
}

/// The display server winit opens windows through.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn display_server(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Field {
    let set = |name| var(name).is_some_and(|value| !value.is_empty());
    if crate::platform::wayland_in(&var) {
        Field::Value("Wayland".into())
    } else if set("DISPLAY") {
        Field::Value("X11".into())
    } else {
        Field::Unavailable("no display found")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let vars: Vec<(String, OsString)> = vars
            .iter()
            .map(|(name, value)| (name.to_string(), OsString::from(value)))
            .collect();
        move |name| {
            vars.iter()
                .find(|(set, _)| set == name)
                .map(|(_, value)| value.clone())
        }
    }

    fn build() -> Build {
        Build {
            version: "0.9.3",
            commit: "0123456789abcdef0123456789abcdef01234567",
            target: "x86_64-unknown-linux-gnu",
            decoder: "rawmakase-raw 0.9.3".into(),
        }
    }

    fn system() -> System {
        System {
            installation: Field::Value("arch-package".into()),
            os: Field::Value("Arch Linux".into()),
            kernel: Some(Field::Value("6.17.1-arch1-1".into())),
            desktop: Some(Field::Value("Hyprland".into())),
            display_server: Some(Field::Value("Wayland".into())),
            cpu_threads: Field::Value("16".into()),
        }
    }

    fn adapter() -> wgpu::AdapterInfo {
        let mut adapter =
            wgpu::AdapterInfo::new(wgpu::DeviceType::IntegratedGpu, wgpu::Backend::Vulkan);
        adapter.name = "AMD Radeon 780M Graphics (RADV PHOENIX)".into();
        adapter.vendor = 0x1002;
        adapter.driver = "radv".into();
        adapter.driver_info = "Mesa 25.2.4-arch1.1".into();
        adapter
    }

    fn window() -> Window {
        Window {
            gpu: Some(Gpu::from_adapter(&adapter(), "8-bit")),
            scale: 1.5,
            monitor: Monitor::System(SystemProfile::Found),
            catalog: Some((1234, 1)),
            background_task: false,
        }
    }

    #[test]
    fn formats_the_report_from_its_snapshots() {
        let arch = std::env::consts::ARCH;
        assert_eq!(
            report(&build(), &system(), Some(&window())),
            format!(
                "RAWmakase: 0.9.3
Commit: 0123456789abcdef0123456789abcdef01234567
Target: x86_64-unknown-linux-gnu
RAW decoder: rawmakase-raw 0.9.3
Installation: arch-package
OS: Arch Linux
Kernel: 6.17.1-arch1-1
Desktop: Hyprland
Display server: Wayland
Architecture: {arch}
CPU threads: 16
GPU: AMD Radeon 780M Graphics (RADV PHOENIX)
GPU vendor: AMD (0x1002)
GPU backend: Vulkan
GPU type: integrated
GPU driver: radv, Mesa 25.2.4-arch1.1
GPU surface: 8-bit
Display scale: 1.50
Monitor profile: System, display profile found
Catalog: 1234 photos, 1 folder
Background task: none
"
            )
        );
    }

    #[test]
    fn leaves_out_the_lines_of_other_platforms() {
        let system = System {
            os: Field::Value("macOS 26.0.1".into()),
            kernel: None,
            desktop: None,
            display_server: None,
            ..system()
        };
        let text = report(&build(), &system, Some(&window()));
        assert!(text.contains("OS: macOS 26.0.1\nArchitecture: "));
        for line in ["Kernel", "Desktop", "Display server"] {
            assert!(!text.contains(line), "{line}");
        }
    }

    #[test]
    fn marks_what_is_missing_as_unavailable() {
        let system = System {
            installation: Field::Unavailable("timed out"),
            os: Field::Unavailable("os-release unreadable"),
            ..system()
        };
        let window = Window {
            gpu: None,
            monitor: Monitor::System(SystemProfile::NotFound),
            catalog: None,
            background_task: true,
            ..window()
        };
        let text = report(&build(), &system, Some(&window));
        for line in [
            "Installation: unavailable (timed out)\n",
            "OS: unavailable (os-release unreadable)\n",
            "GPU: none (drawn without a GPU)\n",
            "Monitor profile: System, no profile found (shown as sRGB)\n",
            "Catalog: none open\n",
            "Background task: running\n",
        ] {
            assert!(text.contains(line), "{line}");
        }
        assert!(!text.contains("GPU driver"));
        // Before the probe answers, and outside the window.
        let text = report(&build(), &System::unavailable("collecting"), None);
        assert!(text.contains("Installation: unavailable (collecting)\n"));
        assert!(text.contains("CPU threads: unavailable (collecting)\n"));
        for label in [
            "GPU",
            "Display scale",
            "Monitor profile",
            "Catalog",
            "Background task",
        ] {
            assert!(
                text.contains(&format!("{label}: unavailable (not in the app window)\n")),
                "{label}"
            );
        }
    }

    #[test]
    fn untrusted_text_stays_on_one_bounded_line() {
        assert_eq!(
            Field::untrusted("  NVIDIA\tGeForce\nRTX\u{1b}[31m 4090\r\n", 96),
            Field::Value("NVIDIA GeForce RTX [31m 4090".into())
        );
        let long = "x".repeat(500);
        let Field::Value(cut) = Field::untrusted(&long, 96) else {
            panic!("a value")
        };
        assert_eq!(cut.chars().count(), 96);
        assert!(cut.ends_with('…'));
        assert_eq!(Field::untrusted(" \n\t", 96), Field::Unavailable("empty"));
        // A driver can't add lines to the report.
        let mut adapter = adapter();
        adapter.name = "GPU\nOS: forged".into();
        adapter.driver_info = "x".repeat(4000);
        let window = Window {
            gpu: Some(Gpu::from_adapter(&adapter, "8-bit")),
            ..window()
        };
        let text = report(&build(), &system(), Some(&window));
        assert_eq!(text.lines().filter(|l| l.starts_with("OS:")).count(), 1);
        assert!(text.lines().all(|line| line.chars().count() < 120));
    }

    #[test]
    fn withholds_values_that_may_name_a_path() {
        for text in [
            "Hyprland:/home/alice/Private Photos/secret.ARW",
            "Hyprland:/home/alice/private(Secret).conf",
            "GNOME ~/x",
            "radv (loaded from /home/bob/lib/driver.so)",
            r"C:\Users\Bob\driver.dll",
            r"\\server\share\x",
            "C:Users",
            "file:///srv/x",
            "%USERPROFILE%",
            "%2Fhome%2Falice",
            "Hyprland:./Private Photos/secret.ARW",
            "GNOME|/srv/secret.ARW",
            "GNOME>/srv/secret.ARW",
            "GNOME\u{1b}/srv/secret.ARW",
            "../x",
        ] {
            assert_eq!(
                Field::untrusted(text, 96),
                Field::Unavailable("withheld: looked like a path"),
                "{text}"
            );
        }
        // What isn't a path stays as it is.
        for text in [
            "Debian GNU/Linux 13 (trixie)",
            "Mesa 25.2.4-arch1.1",
            "NVIDIA 580.95.05",
            "ubuntu:GNOME",
            "AMD Radeon 780M Graphics (RADV PHOENIX)",
            "7.2.8-9-omarchy",
            "Intel(R) Arc(TM) A770 Graphics",
        ] {
            assert_eq!(
                Field::untrusted(text, 96),
                Field::Value(text.into()),
                "{text}"
            );
        }
        // Through the adapter.
        let mut adapter = adapter();
        adapter.driver_info = "Mesa /home/alice/Private Photos/driver.so".into();
        let driver = Gpu::from_adapter(&adapter, "8-bit").driver.to_string();
        assert!(!driver.contains("Photos"), "{driver}");
    }

    #[test]
    fn withholds_a_desktop_that_holds_the_login_name() {
        let desktop_for = |user: &str, value: &str| {
            desktop(env(&[("USER", user), ("XDG_CURRENT_DESKTOP", value)]))
        };
        let withheld = Field::Unavailable("withheld: held the user name");
        assert_eq!(desktop_for("al", "Hyprland:al"), withheld);
        assert_eq!(desktop_for("alice", "Alice's-session"), withheld);
        assert_eq!(desktop_for("john.doe", "GNOME:John.Doe"), withheld);
        // An empty variable falls through to the next.
        assert_eq!(
            desktop(env(&[
                ("USER", ""),
                ("LOGNAME", "alice"),
                ("XDG_CURRENT_DESKTOP", "Hyprland:alice")
            ])),
            withheld
        );
        // A custom kernel's suffix is checked the same way, Arch's isn't hit.
        let kernel = |release| Field::from_environment(release, 64, Some("alice"));
        assert_eq!(kernel("6.17.0-alice"), withheld);
        assert_eq!(
            Field::from_environment("6.17.1-arch1-1", 64, Some("arch")),
            Field::Value("6.17.1-arch1-1".into())
        );
        // Only whole words: the login ian isn't in Debian.
        assert_eq!(
            desktop_for("ian", "Debian:GNOME"),
            Field::Value("Debian:GNOME".into())
        );
        assert_eq!(
            desktop_for("alice", "Hyprland:/home/x"),
            Field::Unavailable("withheld: looked like a path")
        );
        // System files aren't the user's: the login arch leaves Arch Linux.
        assert_eq!(
            linux_name("PRETTY_NAME=\"Arch Linux\"\n")
                .map(|f| f.to_string())
                .as_deref(),
            Some("Arch Linux")
        );
    }

    #[test]
    fn names_known_gpu_vendors_and_keeps_others_as_ids() {
        assert_eq!(
            Gpu::from_adapter(&adapter(), "8-bit").vendor,
            "AMD (0x1002)"
        );
        let mut other = adapter();
        other.vendor = 0xabcd;
        other.driver = String::new();
        other.driver_info = String::new();
        let gpu = Gpu::from_adapter(&other, "8-bit");
        assert_eq!(gpu.vendor, "0xabcd");
        assert_eq!(gpu.driver, Field::Unavailable("empty"));
    }

    #[test]
    fn names_the_window_surface_the_hdr_path_presents_through() {
        use wgpu::{SurfaceColorSpace as Space, TextureFormat as Format};
        assert_eq!(
            surface(Format::Rgba16Float, Space::ExtendedSrgbLinear),
            "16-bit float, linear extended sRGB"
        );
        assert_eq!(surface(Format::Rgba16Float, Space::Auto), "16-bit float");
        assert_eq!(surface(Format::Bgra8Unorm, Space::Auto), "8-bit");
    }

    #[test]
    fn a_failed_probe_loses_only_its_own_line() {
        let system = System::probe_with(|| panic!("package manager crashed"), INSTALLATION_TIMEOUT);
        assert_eq!(system.installation, Field::Unavailable("probe failed"));
        assert_ne!(system.cpu_threads, Field::Unavailable("probe failed"));
        let (_release, stalled) = std::sync::mpsc::channel::<()>();
        let stalled = std::sync::Mutex::new(stalled);
        let started = std::time::Instant::now();
        let system = System::probe_with(
            move || {
                let _ = stalled.lock().unwrap().recv();
                "never"
            },
            Duration::from_millis(50),
        );
        assert_eq!(system.installation, Field::Unavailable("timed out"));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(matches!(system.cpu_threads, Field::Value(_)));
    }

    #[test]
    fn probes_this_system() {
        let system = System::probe_with(|| "linux-tarball", INSTALLATION_TIMEOUT);
        assert_eq!(system.installation, Field::Value("linux-tarball".into()));
        let linux = cfg!(target_os = "linux");
        assert_eq!(system.kernel.is_some(), linux);
        assert_eq!(system.desktop.is_some(), linux);
        assert_eq!(system.display_server.is_some(), linux);
    }

    #[test]
    fn reads_the_linux_distribution_name() {
        let name = |text| linux_name(text).map(|f| f.to_string());
        assert_eq!(
            name("NAME=\"Arch Linux\"\nPRETTY_NAME=\"Arch Linux\"\nID=arch\n").as_deref(),
            Some("Arch Linux")
        );
        assert_eq!(
            name("NAME=Fedora\nVERSION_ID=43\n").as_deref(),
            Some("Fedora 43")
        );
        assert_eq!(name("ID=gentoo\n"), None);
        assert_eq!(name(""), None);
    }

    #[test]
    fn reads_the_desktop_and_display_server() {
        assert_eq!(
            desktop(env(&[("XDG_CURRENT_DESKTOP", "ubuntu:GNOME")])),
            Field::Value("ubuntu:GNOME".into())
        );
        assert_eq!(desktop(env(&[])), Field::Unavailable("not set"));
        assert_eq!(
            display_server(env(&[("WAYLAND_DISPLAY", "wayland-1"), ("DISPLAY", ":0")])),
            Field::Value("Wayland".into())
        );
        assert_eq!(
            display_server(env(&[("DISPLAY", ":0")])),
            Field::Value("X11".into())
        );
        assert_eq!(
            display_server(env(&[])),
            Field::Unavailable("no display found")
        );
    }

    #[test]
    fn reads_the_full_macos_version() {
        let plist = "<dict>\n\t<key>ProductName</key>\n\t<string>macOS</string>\n\t<key>ProductVersion</key>\n\t<string>26.0.1</string>\n</dict>";
        assert_eq!(macos_product_version(plist).as_deref(), Some("26.0.1"));
        assert_eq!(macos_product_version("<dict></dict>"), None);
    }

    #[test]
    fn names_windows_by_its_build() {
        assert_eq!(windows_release(19045), 10);
        assert_eq!(windows_release(22000), 11);
        assert_eq!(windows_release(26100), 11);
    }

    #[test]
    fn the_build_names_its_version_commit_and_target() {
        let build = Build::current();
        assert_eq!(build.version, env!("CARGO_PKG_VERSION"));
        assert!(
            (build.commit.len() == 40 && build.commit.bytes().all(|b| b.is_ascii_hexdigit()))
                || build.commit.starts_with("unknown"),
            "{}",
            build.commit
        );
        assert!(
            build.target.contains(std::env::consts::ARCH),
            "{}",
            build.target
        );
    }

    #[test]
    fn the_issue_url_stays_short_and_encodes_the_version() {
        assert_eq!(
            new_issue_url("0.9.3"),
            "https://github.com/pch/rawmakase/issues/new?template=bug_report.yml&version=0.9.3"
        );
        assert_eq!(
            new_issue_url("1.0.0-rc.1+g a&b=c"),
            "https://github.com/pch/rawmakase/issues/new?template=bug_report.yml&version=1.0.0-rc.1%2Bg%20a%26b%3Dc"
        );
        assert!(new_issue_url(env!("CARGO_PKG_VERSION")).len() < 200);
    }

    #[test]
    fn only_an_executable_inside_the_app_dir_is_an_appimage() {
        let dir = tempfile::tempdir().unwrap();
        let app_dir = dir.path().join("app dir");
        std::fs::create_dir(&app_dir).unwrap();
        let app_dir = std::fs::canonicalize(app_dir).unwrap();
        let inside = app_dir.join("usr/lib/rawmakase/rawmakase");
        assert!(is_inside(&inside, &app_dir));
        // A tarball or package copy started from an AppImage inherits APPDIR.
        let outside = app_dir.with_file_name("app dir2").join("rawmakase");
        assert!(!is_inside(&outside, &app_dir));
        assert!(!is_inside(&inside, &dir.path().join("gone")));
    }
}
