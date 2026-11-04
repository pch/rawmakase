//! Preferences > Monitor profile: which profile the viewport converts its sRGB
//! pixels to, and the display profile the system assigns to the window.
//!
//! The choice (System, sRGB or a chosen ICC profile) is resolved here, with what
//! `platform::display` reports about the window's display, into the profile each
//! render job captures. `app::shell::display` installs a paint callback for the macOS
//! surface colour declaration; `platform::display` owns the AppKit/Metal layer
//! access and the Windows display profile and advanced-colour queries. Following the
//! system lets macOS's compositor match the display, and on Windows converts to the
//! display's assigned profile unless Windows manages colour itself. sRGB and a chosen
//! profile output device RGB and bypass compositor conversion. No choice changes the
//! renderer's output colour space or export profiles.
pub(crate) use crate::platform::display::SystemDisplay;
use std::path::{Path, PathBuf};

/// The monitor profile the user chose.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum MonitorProfile {
    /// The profile the system assigns to the display the window is on, as
    /// Lightroom does. Saved as no profile, which 0.4 and earlier showed as
    /// "sRGB" and, on macOS, already matched to the display.
    #[default]
    System,
    /// Show the sRGB pixels as they are, for a display set to sRGB.
    Srgb,
    /// A chosen ICC profile.
    Custom(PathBuf),
}

impl MonitorProfile {
    /// The choice saved as the session's `monitor` and `monitor_srgb`.
    pub(crate) fn from_session(monitor: Option<PathBuf>, srgb: bool) -> Self {
        match monitor {
            Some(path) => Self::Custom(path),
            None if srgb => Self::Srgb,
            None => Self::System,
        }
    }
    /// The session's `monitor`: a version without the sRGB choice reads that
    /// as its own "Use sRGB", which leaves pixels as they are outside macOS.
    pub(crate) fn session_path(&self) -> Option<PathBuf> {
        match self {
            Self::Custom(path) => Some(path.clone()),
            Self::System | Self::Srgb => None,
        }
    }
    pub(crate) fn session_srgb(&self) -> bool {
        *self == Self::Srgb
    }
    /// The ICC profile renders convert to, given what the system reports.
    pub(crate) fn transform<'a>(&'a self, system: &'a SystemDisplay) -> Option<&'a Path> {
        match (self, system) {
            (Self::Custom(path), _) | (Self::System, SystemDisplay::Profile(path)) => Some(path),
            _ => None,
        }
    }
    pub(crate) fn follows_system(&self) -> bool {
        *self == Self::System
    }
    /// Whether the window's pixels are device RGB, left unmatched by the
    /// compositor; only following the system lets macOS match them.
    pub(crate) fn device_rgb(&self) -> bool {
        *self != Self::System
    }
}

/// The status bar's and Preferences' account of what the viewport does.
pub(crate) fn describe(choice: &MonitorProfile, system: &SystemDisplay) -> String {
    let name = |path: &Path| {
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    };
    match (choice, system) {
        (MonitorProfile::Custom(path), _) => format!("{} (manual conversion)", name(path)),
        (MonitorProfile::Srgb, _) => "sRGB (no conversion)".into(),
        (MonitorProfile::System, SystemDisplay::Profile(path)) => {
            format!("System: {}", name(path))
        }
        (MonitorProfile::System, SystemDisplay::Managed) if cfg!(target_os = "macos") => {
            "System (matched by macOS)".into()
        }
        (MonitorProfile::System, SystemDisplay::Managed) => "System (managed by Windows)".into(),
        (MonitorProfile::System, SystemDisplay::Unknown) => {
            "System: none found, shown as sRGB".into()
        }
    }
}

/// The mode for the system report, without the profile's name.
pub(crate) fn report_mode(
    choice: &MonitorProfile,
    system: &SystemDisplay,
) -> crate::diagnostics::Monitor {
    use crate::diagnostics::{Monitor, SystemProfile};
    match (choice, system) {
        (MonitorProfile::Custom(_), _) => Monitor::Custom,
        (MonitorProfile::Srgb, _) => Monitor::Srgb,
        (MonitorProfile::System, SystemDisplay::Profile(_)) => {
            Monitor::System(SystemProfile::Found)
        }
        (MonitorProfile::System, SystemDisplay::Managed) => Monitor::System(SystemProfile::Managed),
        (MonitorProfile::System, SystemDisplay::Unknown) => {
            Monitor::System(SystemProfile::NotFound)
        }
    }
}

/// Asks the system for the window's display profile, again when the window
/// reaches another display and every few seconds for a newly assigned profile.
#[derive(Default)]
pub(crate) struct SystemProbe {
    display: SystemDisplay,
    last: Option<(crate::platform::display::Monitor, std::time::Instant)>,
}

pub(crate) const RECHECK: std::time::Duration = std::time::Duration::from_secs(2);

impl SystemProbe {
    pub(crate) fn display(&self) -> &SystemDisplay {
        &self.display
    }
    /// Refreshes the display, returning whether it changed.
    pub(crate) fn update(
        &mut self,
        window: &impl winit::raw_window_handle::HasWindowHandle,
    ) -> bool {
        use crate::platform::display;
        let monitor = display::monitor(window);
        let now = std::time::Instant::now();
        if self
            .last
            .as_ref()
            .is_some_and(|(m, at)| *m == monitor && now.duration_since(*at) < RECHECK)
        {
            return false;
        }
        self.last = Some((monitor, now));
        self.set(display::system_display(monitor))
    }
    /// Records the display, returning whether it changed.
    pub(crate) fn set(&mut self, display: SystemDisplay) -> bool {
        let changed = self.display != display;
        self.display = display;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::preferences::session::Session;

    #[test]
    fn the_default_converts_to_the_system_display_profile() {
        // Issue 550: on Windows with automatic color management off, the default
        // showed sRGB pixels unconverted on a wide-gamut display.
        let panel = PathBuf::from(r"C:\Windows\System32\spool\drivers\color\panel.icm");
        let system = SystemDisplay::Profile(panel.clone());
        assert_eq!(
            MonitorProfile::default().transform(&system),
            Some(panel.as_path())
        );
        assert!(!MonitorProfile::default().device_rgb());
    }

    #[test]
    fn a_managed_or_unknown_display_is_left_to_the_system() {
        for system in [SystemDisplay::Managed, SystemDisplay::Unknown] {
            assert_eq!(MonitorProfile::System.transform(&system), None);
        }
    }

    #[test]
    fn srgb_and_custom_ignore_the_system_profile() {
        let system = SystemDisplay::Profile("panel.icm".into());
        assert_eq!(MonitorProfile::Srgb.transform(&system), None);
        assert!(MonitorProfile::Srgb.device_rgb());
        let custom = MonitorProfile::Custom("calibrated.icc".into());
        assert_eq!(custom.transform(&system), Some(Path::new("calibrated.icc")));
        assert_eq!(
            custom.transform(&SystemDisplay::Managed),
            Some(Path::new("calibrated.icc"))
        );
        assert!(custom.device_rgb());
    }

    #[test]
    fn saved_choices_keep_their_meaning() {
        let read = |json: &str| {
            let session: Session = serde_json::from_str(json).unwrap();
            MonitorProfile::from_session(session.monitor, session.monitor_srgb)
        };
        // Sessions from 0.4 and earlier: no profile follows the system now, as
        // it always did on macOS; a chosen profile stays chosen.
        assert_eq!(
            read(r#"{"last_path":null,"monitor":null}"#),
            MonitorProfile::System
        );
        assert_eq!(
            read(r#"{"last_path":null,"monitor":"/p/display.icc"}"#),
            MonitorProfile::Custom("/p/display.icc".into())
        );
        for choice in [
            MonitorProfile::System,
            MonitorProfile::Srgb,
            MonitorProfile::Custom("/p/display.icc".into()),
        ] {
            let session = Session {
                monitor: choice.session_path(),
                monitor_srgb: choice.session_srgb(),
                ..Default::default()
            };
            let json = serde_json::to_string(&session).unwrap();
            assert_eq!(read(&json), choice);
            // Only the new choice is saved with the new field.
            assert_eq!(
                json.contains("monitor_srgb"),
                choice == MonitorProfile::Srgb
            );
        }
    }

    #[test]
    fn the_probe_reports_changes_once() {
        let mut probe = SystemProbe::default();
        assert!(probe.set(SystemDisplay::Profile("a.icm".into())));
        assert!(!probe.set(SystemDisplay::Profile("a.icm".into())));
        assert!(probe.set(SystemDisplay::Managed));
    }
}
