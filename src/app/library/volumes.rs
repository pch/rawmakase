//! Which drive each root folder is on, whether each drive is attached and
//! its free space, checked off the UI thread so a hung network mount can't
//! stall drawing; and Lightroom's volume header row that shows it.
use crate::app::ui::design::{Tone, radius, text};
use crate::app::ui::theme;
use crate::platform::volume::Volume;
use eframe::egui::{self, Color32, Vec2};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// Attached, and free/total bytes when known.
pub(super) type VolumeState = (bool, Option<(u64, u64)>);

/// The drive each root folder was last found on, by its location here.
pub(super) type Seen = HashMap<PathBuf, Volume>;

/// What a check found.
#[derive(Clone, Default, PartialEq, Debug)]
pub(super) struct Checked {
    /// Each root's drive, as last found: kept while the root can't be found,
    /// so a detached drive's folders stay under its name.
    pub seen: Seen,
    /// Per volume mount (the startup disk as "/").
    pub online: HashMap<PathBuf, VolumeState>,
}

/// Where [`Volumes`] keep the drives they found, across launches: set once
/// by the app, never in tests, which keep them in memory only.
static DRIVES_FILE: OnceLock<PathBuf> = OnceLock::new();

/// Keeps the drives each root folder was found on in `file`, so a drive
/// detached while RAWmakase was closed still holds its folders.
pub(crate) fn remember_drives_in(file: PathBuf) {
    // Set once, at launch; a second app in this process keeps the first file.
    let _ = DRIVES_FILE.set(file);
}

/// `drives.json`: each root folder's location and the drive it was on.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct DrivesFile {
    #[serde(default)]
    roots: std::collections::BTreeMap<String, SeenDrive>,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct SeenDrive {
    name: String,
    /// Where it was mounted; none for the startup disk.
    #[serde(default)]
    mount: Option<PathBuf>,
}
fn load(file: &Path) -> Seen {
    let drives: DrivesFile = crate::storage::read_json_or_default(file);
    drives
        .roots
        .into_iter()
        .map(|(root, d)| {
            // Kept by 0.4.6 or earlier, perhaps as \\?\D:, which a root
            // offline now would keep beside D: for a folder found there.
            let volume = Volume {
                name: d.name,
                mount: d.mount,
            }
            .plain();
            (PathBuf::from(root), volume)
        })
        .collect()
}
fn save(file: &Path, seen: &Seen) -> anyhow::Result<()> {
    let roots = seen
        .iter()
        // A location JSON can't spell is found again next launch instead.
        .filter_map(|(root, v)| {
            let drive = SeenDrive {
                name: v.name.clone(),
                mount: v.mount.clone(),
            };
            Some((root.to_str()?.to_owned(), drive))
        })
        .collect();
    crate::storage::atomic_json(file, &DrivesFile { roots })
}

pub(super) struct Volumes {
    checked: Arc<Mutex<Checked>>,
    last: Option<Instant>,
    /// A check is still running, perhaps stuck on a stalled mount: start no other.
    busy: Arc<AtomicBool>,
    file: Option<PathBuf>,
}
impl Default for Volumes {
    fn default() -> Self {
        let file = DRIVES_FILE.get().cloned();
        let seen = file.as_deref().map(load).unwrap_or_default();
        Self {
            checked: Arc::new(Mutex::new(Checked {
                seen,
                online: HashMap::new(),
            })),
            last: None,
            busy: Default::default(),
            file,
        }
    }
}
impl Volumes {
    /// Re-checks every few seconds, on a background thread, which drive each
    /// of `roots` is on and whether it is attached, and asks for a repaint
    /// when the answer changes.
    pub(super) fn check(&mut self, ctx: &egui::Context, roots: Vec<PathBuf>) {
        let due = self
            .last
            .is_none_or(|t| t.elapsed() > Duration::from_secs(3));
        if !due {
            return;
        }
        self.last = Some(Instant::now());
        ctx.request_repaint_after(Duration::from_secs(3));
        let ctx = ctx.clone();
        let seen = self.checked.lock().unwrap().seen.clone();
        let file = self.file.clone();
        spawn_volume_check(
            &self.checked,
            &self.busy,
            move || ctx.request_repaint(),
            move || {
                let checked = survey(
                    &roots,
                    &seen,
                    crate::platform::volume::locate,
                    crate::platform::volume::is_mounted,
                    crate::platform::volume::space,
                );
                if checked.seen != seen
                    && let Some(file) = file
                    && let Err(e) = save(&file, &checked.seen)
                {
                    log::warn!(
                        "Could not keep the folders' drives in {}: {e}",
                        file.display()
                    );
                }
                checked
            },
        );
    }
    /// The last check's result.
    pub(super) fn snapshot(&self) -> Checked {
        self.checked.lock().unwrap().clone()
    }
    /// The drive each of `roots` is on: as last found, else as its path says.
    pub(super) fn drives<'a>(&self, roots: impl Iterator<Item = &'a Path>) -> Vec<Volume> {
        let checked = self.checked.lock().unwrap();
        roots.map(|root| drive_of(&checked.seen, root)).collect()
    }
    /// Waits for a running check to finish.
    #[cfg(test)]
    pub(super) fn wait(&self) {
        let started = Instant::now();
        while self.busy.load(Ordering::Acquire) {
            assert!(started.elapsed() < Duration::from_secs(10), "a stuck check");
            std::thread::yield_now();
        }
    }
    #[cfg(test)]
    pub(super) fn set_seen(&self, seen: Seen) {
        self.checked.lock().unwrap().seen = seen;
    }
}
/// `root`'s drive: as last found, else as its path says.
fn drive_of(seen: &Seen, root: &Path) -> Volume {
    seen.get(root)
        .cloned()
        .unwrap_or_else(|| crate::platform::volume::volume_of(root))
}
/// Finds which drive each of `roots` is on (`locate`, `None` for one not
/// there) and whether each drive is attached (`mounted`) with its free
/// space, given the drives they were on before (`seen`). A root not found
/// keeps the drive it was on; so does one found where its drive, detached,
/// was mounted, as the empty folder left there is not the disk.
pub(super) fn survey(
    roots: &[PathBuf],
    seen: &Seen,
    locate: impl Fn(&Path) -> Option<Volume>,
    mounted: impl Fn(&Path) -> bool,
    space: impl Fn(&Path) -> Option<(u64, u64)>,
) -> Checked {
    let mut attached: HashMap<PathBuf, bool> = HashMap::new();
    let is_attached = |attached: &mut HashMap<PathBuf, bool>, mount: &Path| {
        *attached
            .entry(mount.to_path_buf())
            .or_insert_with(|| mounted(mount))
    };
    let mut seen = seen.clone();
    for root in roots {
        let Some(found) = locate(root) else {
            continue;
        };
        // A root was just found on it: attached, though its folder may be
        // on its parent's device (a GVFS share, a bind mount).
        if let Some(mount) = &found.mount {
            attached.insert(mount.clone(), true);
        }
        // The folder a detached drive was mounted at is on the drive the
        // root is found on now: the root is that folder or inside it. (By
        // the drive rather than the path, which may name it through a link,
        // as /tmp is /private/tmp on macOS.)
        let left_behind = seen
            .get(root)
            .and_then(|v| v.mount.clone())
            .is_some_and(|m| {
                found.mount.as_ref() != Some(&m)
                    && !is_attached(&mut attached, &m)
                    && locate(&m).as_ref() == Some(&found)
            });
        if !left_behind {
            seen.insert(root.clone(), found);
        }
    }
    let mut online = HashMap::new();
    for root in roots {
        let volume = drive_of(&seen, root);
        let mount = volume.mount.clone().unwrap_or_else(|| PathBuf::from("/"));
        if online.contains_key(&mount) {
            continue;
        }
        let on = volume.mount.is_none() || is_attached(&mut attached, &mount);
        let free = if on { space(&mount) } else { None };
        online.insert(mount, (on, free));
    }
    Checked { seen, online }
}
/// Runs `job` on a background thread into `shared`, calling `changed` when the
/// result differs, unless the previous check is still running: probing a stalled
/// mount can hang, and new threads would pile up behind it. Returns whether a check
/// started.
pub(super) fn spawn_volume_check<T: PartialEq + Send + 'static>(
    shared: &Arc<Mutex<T>>,
    busy: &Arc<AtomicBool>,
    changed: impl FnOnce() + Send + 'static,
    job: impl FnOnce() -> T + Send + 'static,
) -> bool {
    if busy.swap(true, Ordering::Acquire) {
        return false;
    }
    /// Clears the flag however the check ends.
    struct Done(Arc<AtomicBool>);
    impl Drop for Done {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
    let done = Done(busy.clone());
    let shared = shared.clone();
    std::thread::spawn(move || {
        let _done = done;
        let state = job();
        let mut shared = shared.lock().unwrap();
        if *shared != state {
            *shared = state;
            changed();
        }
    });
    true
}
/// A volume's header in the folder tree: a dot lit green when the drive is
/// attached, its name, its free space (or Offline) and a disclosure chevron
/// on the right. Clicking it folds its folders away.
pub(super) fn volume_row(
    ui: &mut egui::Ui,
    volume: &Volume,
    attached: Option<bool>,
    space: Option<(u64, u64)>,
    photos: usize,
    open: bool,
) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.), egui::Sense::click());
    let painter = ui.painter();
    let hovered = response.hovered();
    if hovered {
        painter.rect_filled(rect, radius::CONTROL, palette.tone(Tone::CARD));
    }
    let y = rect.center().y;
    // The dot over the folder tree's chevrons, the name over its folder
    // icons; the chevron ends where the folders' counts do.
    let dot = egui::pos2(rect.left() + 13.5, y);
    if attached == Some(true) {
        painter.circle_filled(dot, 3.5, Color32::from_rgb(110, 200, 90));
    } else {
        let ring = palette.tone(if attached == Some(false) {
            Tone::SECONDARY
        } else {
            Tone::OUTLINE_STRONG
        });
        painter.circle_stroke(dot, 3., egui::Stroke::new(1., ring));
    }
    let chevron = if open {
        crate::app::ui::icons::Icon::ChevronDown
    } else {
        crate::app::ui::icons::Icon::ChevronRight
    };
    crate::app::ui::icons::paint_at(
        painter,
        chevron,
        egui::pos2(rect.right() - 15.5, y),
        11.,
        palette.tone(if hovered { Tone::SOFT } else { Tone::DISABLED }),
    );
    let gb = |bytes: u64| bytes as f64 / 1e9;
    let detail = match (attached, space) {
        (Some(false), _) => "Offline".to_string(),
        (_, Some((free, _))) => format!("{:.0} GB free", gb(free)),
        _ => String::new(),
    };
    // The free space turns to the warning colour once the drive is over 90% full.
    let full = attached != Some(false)
        && space.is_some_and(|(free, total)| total > 0 && (free as f64) < total as f64 * 0.1);
    let detail_color = if full {
        palette.warning()
    } else {
        palette.tone(Tone::FAINT)
    };
    let detail = painter.layout_no_wrap(
        detail,
        egui::FontId::proportional(text::SMALL),
        detail_color,
    );
    let detail_left = rect.right() - 27. - detail.size().x;
    painter.galley(
        egui::pos2(detail_left, y - detail.size().y / 2.),
        detail,
        detail_color,
    );
    let name_left = rect.left() + 22.;
    let name = painter.layout_no_wrap(
        volume.name.clone(),
        egui::FontId::proportional(text::LABEL),
        palette.tone(if hovered { Tone::STRONG } else { Tone::TEXT }),
    );
    painter
        .with_clip_rect(rect.with_min_x(name_left).with_max_x(detail_left - 8.))
        .galley(
            egui::pos2(name_left, y - name.size().y / 2.),
            name,
            palette.tone(Tone::TEXT),
        );
    let space_text = space
        .map(|(free, total)| format!(" · {:.0} of {:.0} GB free", gb(free), gb(total)))
        .unwrap_or_default();
    response.on_hover_text(match (&volume.mount, attached) {
        (None, _) => format!("Startup disk · {photos} photos{space_text}"),
        (Some(mount), Some(false)) => format!("{} is not attached", mount.display()),
        (Some(mount), _) => format!("{} · {photos} photos{space_text}", mount.display()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(name: &str, mount: &str) -> Volume {
        Volume {
            name: name.into(),
            mount: Some(mount.into()),
        }
    }
    fn startup() -> Volume {
        Volume {
            name: "Macintosh HD".into(),
            mount: None,
        }
    }
    /// The free space each mount reports, for a check that reads it.
    fn space(mount: &Path) -> Option<(u64, u64)> {
        match mount.to_str()? {
            "/" => Some((87, 1000)),
            "/tmp/rmk590-mnt" => Some((40, 50)),
            _ => None,
        }
    }

    /// A drive mounted outside /Volumes holds its folders, with its own free
    /// space; a folder of the startup disk stays there.
    #[test]
    fn a_root_is_on_the_drive_it_is_found_on() {
        let card = PathBuf::from("/tmp/rmk590-mnt/card");
        let pictures = PathBuf::from("/Users/me/Pictures");
        let checked = survey(
            &[card.clone(), pictures.clone()],
            &Seen::new(),
            |root| {
                Some(if root == card {
                    drive("RMK590", "/tmp/rmk590-mnt")
                } else {
                    startup()
                })
            },
            |_| true,
            space,
        );
        assert_eq!(checked.seen[&card], drive("RMK590", "/tmp/rmk590-mnt"));
        assert_eq!(checked.seen[&pictures], startup());
        assert_eq!(
            checked.online,
            HashMap::from([
                (PathBuf::from("/tmp/rmk590-mnt"), (true, Some((40, 50)))),
                (PathBuf::from("/"), (true, Some((87, 1000)))),
            ])
        );
    }

    /// A drive a root was just found on is attached, though its folder is
    /// on its parent's device: a GVFS share inside the one FUSE mount, a
    /// bind mount.
    #[test]
    fn a_drive_a_root_is_found_on_is_attached() {
        let share = PathBuf::from("/run/user/1000/gvfs/smb-share:server=nas,share=photos");
        let root = share.join("2024");
        let checked = survey(
            std::slice::from_ref(&root),
            &Seen::new(),
            |_| Some(drive("photos", share.to_str().unwrap())),
            |_| false,
            |_| Some((1, 2)),
        );
        assert_eq!(checked.online[&share], (true, Some((1, 2))));
    }

    /// A root that can't be found stays on the drive it was last seen on,
    /// offline; one never seen goes by its path, as before.
    #[test]
    fn an_offline_root_keeps_its_drive() {
        let card = PathBuf::from("/tmp/rmk590-mnt/card");
        let usb = PathBuf::from("/Volumes/USB/photos");
        let seen = Seen::from([(card.clone(), drive("RMK590", "/tmp/rmk590-mnt"))]);
        let checked = survey(&[card, usb.clone()], &seen, |_| None, |_| false, space);
        assert_eq!(checked.seen, seen);
        assert_eq!(checked.online[Path::new("/tmp/rmk590-mnt")], (false, None));
        assert!(!checked.seen.contains_key(&usb));
        let by_path = crate::platform::volume::volume_of(&usb);
        let mount = by_path.mount.clone().unwrap_or_else(|| "/".into());
        let expected = if by_path.mount.is_some() {
            (false, None)
        } else {
            (true, Some((87, 1000)))
        };
        assert_eq!(checked.online[&mount], expected);
    }

    /// The empty folder a detached drive leaves where it was mounted is not
    /// the drive: a root found there on the startup disk keeps its drive
    /// until a drive is mounted there again; a root moved elsewhere follows.
    #[test]
    fn a_left_mount_point_is_not_the_drive() {
        let card = PathBuf::from("/tmp/rmk590-mnt");
        let seen = Seen::from([(card.clone(), drive("RMK590", "/tmp/rmk590-mnt"))]);
        let detached = survey(
            std::slice::from_ref(&card),
            &seen,
            |_| Some(startup()),
            |_| false,
            space,
        );
        assert_eq!(detached.seen, seen);
        assert_eq!(detached.online[Path::new("/tmp/rmk590-mnt")], (false, None));
        // Named through a link, as /tmp is /private/tmp.
        let linked = Seen::from([(card.clone(), drive("RMK590", "/private/tmp/rmk590-mnt"))]);
        let detached = survey(
            std::slice::from_ref(&card),
            &linked,
            |_| Some(startup()),
            |_| false,
            space,
        );
        assert_eq!(detached.seen, linked);
        // Another drive mounted there is that drive.
        let other = drive("OTHER", "/tmp/rmk590-mnt");
        let replaced = survey(
            std::slice::from_ref(&card),
            &seen,
            |_| Some(other.clone()),
            |_| true,
            space,
        );
        assert_eq!(replaced.seen[&card], other);
        // A root whose drive's mount point is gone follows what it's on.
        let mut moved = seen;
        moved.insert(card.clone(), drive("RMK590", "/elsewhere"));
        let locate = |p: &Path| (p == card).then(startup);
        let followed = survey(
            std::slice::from_ref(&card),
            &moved,
            locate,
            |_| false,
            space,
        );
        assert_eq!(followed.seen[&card], startup());
    }

    /// The drives found are kept across launches, and a file from a release
    /// that kept none, or that doesn't read, keeps none.
    #[test]
    fn the_drives_found_survive_a_restart() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("drives.json");
        assert!(load(&file).is_empty());
        let seen = Seen::from([
            (
                PathBuf::from("/tmp/rmk590-mnt/card"),
                drive("RMK590", "/tmp/rmk590-mnt"),
            ),
            (PathBuf::from("/Users/me/Pictures"), startup()),
        ]);
        save(&file, &seen)?;
        assert_eq!(load(&file), seen);
        std::fs::write(&file, b"{\"roots\": 7}")?;
        assert!(load(&file).is_empty());
        Ok(())
    }
    /// A Windows drive an earlier release kept as `\\?\D:` is `D:`, as
    /// it is found now: a root offline keeps it, and another folder found
    /// on that drive is on the same drive (#678).
    #[test]
    #[cfg(windows)]
    fn a_windows_drive_kept_with_its_prefix_is_found_as_one() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("drives.json");
        let (offline, found) = (
            PathBuf::from(r"\\?\D:\Card"),
            PathBuf::from(r"D:\Elsewhere\Party"),
        );
        let old = Volume {
            name: r"\\?\D:".into(),
            mount: Some(r"\\?\D:\".into()),
        };
        save(&file, &Seen::from([(offline.clone(), old)]))?;
        let d = crate::platform::volume::volume_of(&found);
        let checked = survey(
            &[offline.clone(), found.clone()],
            &load(&file),
            |p| (p == found.as_path()).then(|| d.clone()),
            |_| true,
            |_| None,
        );
        assert_eq!(drive_of(&checked.seen, &offline), d);
        assert_eq!(drive_of(&checked.seen, &found), d);
        assert_eq!(checked.online.len(), 1);
        Ok(())
    }
}
