//! The computer's memory, for work sized to it, and giving back what the
//! allocator keeps of memory the app has freed ([`Release`]).
use std::time::{Duration, Instant};

/// The computer's physical memory in bytes, when the system says.
pub(crate) fn total() -> Option<u64> {
    let bytes = physical()?;
    (bytes > 0).then_some(bytes)
}

#[cfg(target_os = "macos")]
fn physical() -> Option<u64> {
    Some(objc2_foundation::NSProcessInfo::processInfo().physicalMemory())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn physical() -> Option<u64> {
    let info = rustix::system::sysinfo();
    // A count of `mem_unit`-byte units; 0 for bytes on kernels before 2.3.23.
    #[allow(clippy::useless_conversion)]
    let units = u64::from(info.totalram);
    Some(units.saturating_mul(u64::from(info.mem_unit.max(1))))
}

#[cfg(windows)]
fn physical() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: `status` is a MEMORYSTATUSEX with its size set, as the call asks.
    let filled = unsafe { GlobalMemoryStatusEx(&mut status) } != 0;
    filled.then_some(status.ullTotalPhys)
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "linux",
    target_os = "android",
    windows
)))]
fn physical() -> Option<u64> {
    None
}

/// How many large blocks macOS's allocator keeps after they are freed, twice
/// over: 64 on macOS 27.2, where 48 blocks pushed out all but the last 16.
#[cfg(target_os = "macos")]
const KEPT_BLOCKS: usize = 128;
/// A block large enough to be kept that way: blocks of 2 MB and less are not.
#[cfg(target_os = "macos")]
const KEPT_BLOCK: usize = 4 << 20;

/// Gives the memory the allocator keeps after the app freed it back to the
/// system, on macOS and on Linux with glibc. Elsewhere it does nothing.
///
/// macOS's allocator keeps the last large blocks freed, whatever their size, to
/// hand out again, and they count as the app's meanwhile. Measured on macOS
/// 27.2 with a release build (2026-11-18): the Library showing 60 edited RAWs
/// sat at 4.2 GB with 35 MB in use once their previews were rendered, and a
/// 61-megapixel photo opened in Develop reached 8.7 GB with 2.5 GB in use.
/// Sometimes the system took the blocks back within a minute, sometimes not for
/// many.
///
/// The allocator has no call that empties that cache
/// (`malloc_zone_pressure_relief` returned 0 and changed nothing), but it keeps
/// a fixed number of blocks, the newest: so this allocates more blocks than it
/// keeps, never written and so never the app's, and frees them, which pushes
/// out every block kept before. A standalone program holding 2.7 GB of freed
/// blocks was at 2 MB after it, in 27 ms. Where an allocator keeps blocks
/// another way this changes nothing.
///
/// Switching the cache off for the whole process instead (`MallocLargeCache=0`
/// in its environment) left the Library at 232 MB, but took 12% more CPU time
/// to render the same previews: so the blocks are kept while work goes on and
/// given back after it ([`Release`]).
///
/// glibc keeps freed memory another way: it hands blocks above its mmap
/// threshold straight back, but the threshold rises to 32 MB as large blocks
/// are freed, each of the worker threads takes memory from one of up to 16
/// arenas here, and an arena gives memory back from its top only. Measured on
/// glibc 2.44 with a release build (2026-11-18, 16 threads, 27 GB): after
/// thumbnails for 48 RAWs the heap held 792 MB with 168 MB in use; after
/// opening the 48 in Develop one after another, 1.2 GB with 260 MB in use,
/// level from the sixth photo on; after the Library rendered their edited
/// previews, 3.0 GB with 180 MB in use. All of it stayed through five idle
/// minutes. `malloc_trim(0)` gives back the free pages inside every arena, in
/// 1 to 100 ms: with it, the Library sat at 0.81 GB instead of 3.1 GB, and at
/// 1.0 GB instead of 1.76 GB after the 48 photos, for no more CPU time. Fewer
/// arenas (`MALLOC_ARENA_MAX=2`) kept nearly as much in a standalone program.
pub(crate) fn release_freed() {
    #[cfg(target_os = "macos")]
    {
        let blocks: Vec<Vec<u8>> = (0..KEPT_BLOCKS)
            .map(|_| Vec::with_capacity(KEPT_BLOCK))
            .collect();
        // Allocated and freed for what that does to the allocator: not to be
        // optimised away as unused.
        drop(std::hint::black_box(blocks));
    }
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: `malloc_trim` takes each arena's lock while it works, so it may
    // run beside other threads' allocations; 0 keeps no extra space at the top.
    unsafe {
        libc::malloc_trim(0);
    }
}

/// The CPU time the process has used on all its threads, where
/// [`release_freed`] does something; elsewhere `None`, and nothing is timed.
fn cpu_time() -> Option<Duration> {
    #[cfg(any(target_os = "macos", all(target_os = "linux", target_env = "gnu")))]
    {
        // SAFETY: `usage` is a zeroed `rusage`, which `getrusage` fills.
        let usage = unsafe {
            let mut usage = std::mem::zeroed::<libc::rusage>();
            (libc::getrusage(libc::RUSAGE_SELF, &mut usage) == 0).then_some(usage)?
        };
        let time = |t: libc::timeval| {
            Duration::from_secs(t.tv_sec.max(0) as u64)
                + Duration::from_micros(t.tv_usec.max(0) as u64)
        };
        Some(time(usage.ru_utime) + time(usage.ru_stime))
    }
    #[cfg(not(any(target_os = "macos", all(target_os = "linux", target_env = "gnu"))))]
    None
}

/// How often the process's CPU time is looked at while a release is due.
const SAMPLE: Duration = Duration::from_secs(1);
/// The process counts as working when it used this share of one core since the
/// last look, or [`BURST`] of CPU time however long ago that was. Idle, with
/// its window drawn, the app uses under 1%.
const WORKING: f64 = 0.25;
const BURST: Duration = Duration::from_secs(2);
/// How long the process is quiet after work before what it freed is given
/// back: long enough that the next photo of a row opened one after another
/// still finds the blocks of the last.
const QUIET: Duration = Duration::from_secs(4);

/// What [`Release::step`] found.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    /// Nothing to give back, and nothing to wait for.
    Idle,
    /// Work was seen: look again after this long.
    Wait(Duration),
    /// The work has ended: give back what it freed.
    Release,
}

/// Gives freed memory back ([`release_freed`]) once the process has gone quiet
/// after a burst of work, whatever the work was: thumbnails, previews, a
/// photo opening, an export. It goes by the process's CPU time, so no worker
/// has to report in, and a worker added later is covered.
#[derive(Default)]
pub(crate) struct Release {
    /// When the CPU time was last looked at, and what it was.
    last: Option<(Instant, Duration)>,
    /// Work was seen since the last release.
    worked: bool,
    /// Since when the process has been quiet after that work.
    quiet_since: Option<Instant>,
}
impl Release {
    /// Looks at the process's CPU time, when the last look is [`SAMPLE`] old, and
    /// gives freed memory back when work has ended. Returns how long from now to
    /// call again while work is under way or has just ended; `None` when there
    /// is nothing to wait for, and the next call may come whenever the app next
    /// runs.
    pub(crate) fn poll(&mut self) -> Option<Duration> {
        match self.step(Instant::now(), cpu_time()?) {
            Step::Idle => None,
            Step::Wait(after) => Some(after),
            Step::Release => {
                // Off the UI thread: giving back gigabytes takes a moment. The
                // thread holds nothing and ends with the call (docs/shutdown.md).
                let _ = std::thread::Builder::new()
                    .name("memory-release".into())
                    .spawn(release_freed);
                None
            }
        }
    }
    /// [`Self::poll`]'s decision at `now`, the process having used `cpu` so far.
    fn step(&mut self, now: Instant, cpu: Duration) -> Step {
        let Some((then, before)) = self.last else {
            self.last = Some((now, cpu));
            return Step::Idle;
        };
        let elapsed = now.saturating_duration_since(then);
        if elapsed < SAMPLE {
            return match self.worked {
                true => Step::Wait(SAMPLE - elapsed),
                false => Step::Idle,
            };
        }
        self.last = Some((now, cpu));
        let used = cpu.saturating_sub(before);
        if used >= BURST.min(elapsed.mul_f64(WORKING)) {
            self.worked = true;
            self.quiet_since = None;
            return Step::Wait(SAMPLE);
        }
        if !self.worked {
            return Step::Idle;
        }
        let since = *self.quiet_since.get_or_insert(then);
        if now.saturating_duration_since(since) < QUIET {
            return Step::Wait(SAMPLE);
        }
        self.worked = false;
        self.quiet_since = None;
        Step::Release
    }
}

#[cfg(test)]
mod tests {
    /// Every platform CI runs on says, and no computer has less than 512 MB.
    #[test]
    fn the_computer_s_memory_is_known() {
        let bytes = super::total().expect("memory size");
        assert!(bytes >= 512 << 20, "{bytes}");
    }

    use super::{QUIET, Release, SAMPLE, Step};
    use std::time::{Duration, Instant};

    /// A process at `cores` of CPU for each second of `seconds`, looked at once a
    /// second: the steps taken.
    fn run(
        release: &mut Release,
        start: &mut (Instant, Duration),
        cores: f64,
        seconds: u32,
    ) -> Vec<Step> {
        (0..seconds)
            .map(|_| {
                start.0 += SAMPLE;
                start.1 += SAMPLE.mul_f64(cores);
                release.step(start.0, start.1)
            })
            .collect()
    }

    /// Freed memory is given back once, a few seconds after work ends; an idle
    /// app is never asked to look again, and work that goes on puts it off.
    #[test]
    fn memory_is_released_once_work_has_gone_quiet() {
        let mut release = Release::default();
        let mut at = (Instant::now(), Duration::ZERO);
        assert_eq!(release.step(at.0, at.1), Step::Idle);
        // Idle, its window drawn: under 1% of a core.
        assert_eq!(run(&mut release, &mut at, 0.01, 30), vec![Step::Idle; 30]);
        // Rendering on eight cores, then quiet.
        assert_eq!(
            run(&mut release, &mut at, 8., 5),
            vec![Step::Wait(SAMPLE); 5]
        );
        let quiet = run(&mut release, &mut at, 0.01, 10);
        let released = quiet
            .iter()
            .position(|s| *s == Step::Release)
            .expect("a release");
        assert_eq!(Duration::from_secs(released as u64 + 1), QUIET);
        assert_eq!(quiet[..released], vec![Step::Wait(SAMPLE); released]);
        // Once: nothing more until work is seen again.
        assert_eq!(quiet[released + 1..], vec![Step::Idle; 10 - released - 1]);
        // Work that starts again before the quiet is long enough puts it off.
        run(&mut release, &mut at, 2., 2);
        assert_eq!(
            run(&mut release, &mut at, 0.01, 2),
            vec![Step::Wait(SAMPLE); 2]
        );
        assert_eq!(run(&mut release, &mut at, 2., 1), vec![Step::Wait(SAMPLE)]);
        assert_eq!(
            run(&mut release, &mut at, 0.01, 3),
            vec![Step::Wait(SAMPLE); 3]
        );
        assert_eq!(run(&mut release, &mut at, 0.01, 1), vec![Step::Release]);
    }

    /// Looked at again within a second, nothing is measured: the answer is to
    /// wait out the second while work is under way, and nothing otherwise.
    #[test]
    fn a_look_too_soon_measures_nothing() {
        let mut release = Release::default();
        let start = Instant::now();
        assert_eq!(release.step(start, Duration::ZERO), Step::Idle);
        let soon = start + Duration::from_millis(300);
        assert_eq!(release.step(soon, Duration::from_secs(5)), Step::Idle);
        // The work shows at the next look a second on.
        assert_eq!(
            release.step(start + SAMPLE, Duration::from_secs(5)),
            Step::Wait(SAMPLE)
        );
        assert_eq!(
            release.step(
                start + SAMPLE + Duration::from_millis(300),
                Duration::from_secs(5)
            ),
            Step::Wait(Duration::from_millis(700))
        );
    }

    /// A burst of work between two looks far apart counts, though it is a small
    /// share of the time between them: a window not drawn for a minute while a
    /// worker ran for a few seconds.
    #[test]
    fn a_burst_between_two_distant_looks_counts() {
        let mut release = Release::default();
        let start = Instant::now();
        release.step(start, Duration::ZERO);
        let later = start + Duration::from_secs(60);
        assert_eq!(
            release.step(later, Duration::from_secs(3)),
            Step::Wait(SAMPLE)
        );
    }

    /// What the app frees goes back to the system when asked: 512 MB allocated,
    /// written and freed is no longer the process's afterwards. Without the
    /// call, macOS 27.2's allocator kept it (the footprint stayed 512 MB up), as
    /// it did with `malloc_zone_pressure_relief` in its place.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "Measures the whole process's memory: run alone, `cargo test --lib -- --ignored freed_memory`"]
    fn freed_memory_goes_back_to_the_system() {
        fn footprint() -> u64 {
            // SAFETY: `info` is a zeroed `rusage_info_v2`, the flavor asked for,
            // which the call fills for this process.
            unsafe {
                let mut info = std::mem::zeroed::<libc::rusage_info_v2>();
                let filled = libc::proc_pid_rusage(
                    libc::getpid(),
                    libc::RUSAGE_INFO_V2,
                    (&raw mut info).cast(),
                );
                assert_eq!(filled, 0);
                info.ri_phys_footprint
            }
        }
        const BLOCK: usize = 32 << 20;
        let before = footprint();
        let blocks: Vec<Vec<u8>> = (0..16).map(|i| vec![i as u8 + 1; BLOCK]).collect();
        let held = footprint();
        assert!(held >= before + 15 * BLOCK as u64, "{before} to {held}");
        drop(blocks);
        let freed = footprint();
        super::release_freed();
        let after = footprint();
        eprintln!(
            "before {} MB, holding {} MB, freed {} MB, released {} MB",
            before >> 20,
            held >> 20,
            freed >> 20,
            after >> 20
        );
        assert!(after < before + 4 * BLOCK as u64, "{before} to {after}");
    }

    /// What the app frees goes back to the system when asked, on glibc too:
    /// 512 MB of medium blocks, written and freed below a block still held, is
    /// no longer the process's afterwards. Without the call, glibc 2.44 kept it
    /// (the process stayed 512 MB up).
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[test]
    #[ignore = "Measures the whole process's memory: run alone, `cargo test --lib -- --ignored freed_memory`"]
    fn freed_memory_goes_back_to_the_system_on_glibc() {
        /// The process's anonymous memory in bytes: its heap and mappings.
        fn anonymous() -> u64 {
            let status = std::fs::read_to_string("/proc/self/status").expect("status");
            let kb = status
                .lines()
                .find_map(|l| l.strip_prefix("RssAnon:"))
                .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
                .expect("RssAnon");
            kb << 10
        }
        const BLOCK: usize = 8 << 20;
        // A 16 MB block freed raises glibc's mmap threshold to its size, as the
        // app's images do, so the 8 MB blocks below come from the heap rather
        // than mappings of their own, which are returned when freed.
        drop(std::hint::black_box(vec![1u8; 16 << 20]));
        let before = anonymous();
        let blocks: Vec<Vec<u8>> = (0..64).map(|i| vec![i as u8 + 1; BLOCK]).collect();
        // Kept above them, as long-lived allocations are: the heap can only
        // shrink from its top, which this pins.
        let kept = std::hint::black_box(vec![1u8; 4096]);
        let held = anonymous();
        assert!(held >= before + 60 * BLOCK as u64, "{before} to {held}");
        drop(blocks);
        let freed = anonymous();
        // Kept after the free, or the test proves nothing: glibc tunables such
        // as a fixed `mmap_threshold` return the blocks at once.
        assert!(freed >= before + 60 * BLOCK as u64, "{before} to {freed}");
        super::release_freed();
        let after = anonymous();
        drop(kept);
        eprintln!(
            "before {} MB, holding {} MB, freed {} MB, released {} MB",
            before >> 20,
            held >> 20,
            freed >> 20,
            after >> 20
        );
        assert!(after < before + 8 * BLOCK as u64, "{before} to {after}");
    }
}
