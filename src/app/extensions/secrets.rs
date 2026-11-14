//! Extensions' secret settings (`control = "secret"`, such as an API key) in
//! the editor. Their values live in the credential store
//! ([`rawmakase_extensions::secrets`]); `extensions.json` records only which
//! are set, and this module keeps the values this session loaded, to send to
//! the extension's program in `hello` and `setting-changed` (an extension is
//! trusted with what it is given, as with any setting).
//!
//! Every credential operation, opening the store included, runs on one
//! thread of its own ([`Credentials`]), since a store may wait for the user
//! to unlock it; the host polls for what finished each frame. Each read
//! carries a ticket, and only the latest read of a setting applies, so a
//! read overtaken by a write, a newer read or a reloaded extension is
//! dropped. A value never reaches an error,
//! a log line, `Debug`, the page or diagnostics: [`Secret`] redacts itself
//! and failures carry only the store's reason.
//!
//! Quitting closes the thread's mailbox and hands it to the shared deadline;
//! a call blocked in the store (an unlock prompt) is not waited for past it.
use crate::app::task::Stopping;
use rawmakase_extensions::secrets::{Secret, SecretError, SecretStore, account};

/// Opens the store the thread uses, at its first call: opening the
/// system's may wait. Asked again after it fails.
pub(in crate::app) type Opener =
    Arc<dyn Fn() -> Result<Arc<dyn SecretStore>, SecretError> + Send + Sync>;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

/// What this session knows of one secret setting.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) enum State {
    /// Recorded as set, being read from the store.
    Loading,
    /// None is set.
    Unset,
    Set(Secret),
    /// Recorded as set, but the store couldn't give it: why.
    Unreadable(String),
}

/// A write the page started, while it runs or after it failed.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) enum Writing {
    Saving,
    Failed(String),
}

/// What became of a write, for the top bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Written {
    /// A value where there was none.
    Set,
    /// A value replacing another.
    Replaced,
    /// The value removed.
    Cleared,
}

enum Job {
    Load {
        extension: String,
        setting: String,
        ticket: u64,
    },
    Write {
        extension: String,
        setting: String,
        value: Option<Secret>,
    },
}

/// What a job ended in.
#[derive(Debug)]
pub(in crate::app) enum Done {
    Loaded {
        extension: String,
        setting: String,
        /// Which read this is: only the latest asked for a setting applies.
        ticket: u64,
        result: Result<Option<Secret>, SecretError>,
    },
    Written {
        extension: String,
        setting: String,
        value: Option<Secret>,
        result: Result<(), SecretError>,
    },
}

/// The credentials thread and its mailboxes. Started at the first job.
pub(in crate::app) struct Credentials {
    open: Opener,
    next_ticket: u64,
    jobs: Option<Sender<Job>>,
    done: Option<(Sender<Done>, Receiver<Done>)>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// Woken when a job finishes.
    wake: Option<rawmakase_extensions::session::Wake>,
}

impl Credentials {
    pub(in crate::app) fn new(
        open: Opener,
        wake: Option<rawmakase_extensions::session::Wake>,
    ) -> Self {
        Self {
            open,
            next_ticket: 1,
            jobs: None,
            done: Some(mpsc::channel()),
            thread: None,
            wake,
        }
    }
    /// Reads `setting` of `extension` from the store; returns the read's
    /// ticket, which its [`Done::Loaded`] carries.
    pub(in crate::app) fn load(&mut self, extension: &str, setting: &str) -> Result<u64, String> {
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        self.send(Job::Load {
            extension: extension.to_string(),
            setting: setting.to_string(),
            ticket,
        })?;
        Ok(ticket)
    }
    /// Keeps `value` for `setting` of `extension`, or forgets it with `None`.
    pub(in crate::app) fn write(
        &mut self,
        extension: &str,
        setting: &str,
        value: Option<Secret>,
    ) -> Result<(), String> {
        self.send(Job::Write {
            extension: extension.to_string(),
            setting: setting.to_string(),
            value,
        })
    }
    /// What finished since the last call.
    pub(in crate::app) fn finished(&mut self) -> Vec<Done> {
        self.done
            .as_ref()
            .map(|(_, rx)| rx.try_iter().collect())
            .unwrap_or_default()
    }
    fn send(&mut self, job: Job) -> Result<(), String> {
        let Some((done, _)) = &self.done else {
            return Err("RAWmakase is quitting".into());
        };
        if self.thread.as_ref().is_some_and(|t| t.is_finished()) {
            self.jobs = None;
            self.thread = None;
        }
        if self.jobs.is_none() {
            let (jobs, inbox) = mpsc::channel::<Job>();
            let (open, done, wake) = (self.open.clone(), done.clone(), self.wake.clone());
            let thread = std::thread::Builder::new()
                .name("credentials".into())
                .spawn(move || run(&open, inbox, done, wake))
                .map_err(|e| format!("could not start the credentials thread: {e}"))?;
            self.jobs = Some(jobs);
            self.thread = Some(thread);
        }
        self.jobs
            .as_ref()
            .and_then(|jobs| jobs.send(job).ok())
            .ok_or_else(|| "the credentials thread stopped".into())
    }
    /// Closes the mailboxes and returns the thread to wait for under the
    /// shutdown deadline; nothing more is read or written.
    pub(in crate::app) fn stop(&mut self) -> Stopping {
        self.jobs = None;
        self.done = None;
        Stopping::new(self.thread.take())
    }
}

fn run(
    open: &Opener,
    inbox: Receiver<Job>,
    done: Sender<Done>,
    wake: Option<rawmakase_extensions::session::Wake>,
) {
    let mut opened: Option<Arc<dyn SecretStore>> = None;
    for job in inbox {
        // A store that panics fails the one job, as a failure would.
        let finished = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let store = match &opened {
                Some(store) => Ok(store.clone()),
                None => open().inspect(|store| opened = Some(store.clone())),
            };
            perform(store, &job)
        }))
        .unwrap_or_else(|_| {
            failed(
                job,
                SecretError::Unavailable("it stopped unexpectedly".into()),
            )
        });
        if done.send(finished).is_err() {
            return;
        }
        if let Some(wake) = &wake {
            wake();
        }
    }
}

/// `job` done with `store`, or failed as it couldn't be opened.
fn perform(store: Result<Arc<dyn SecretStore>, SecretError>, job: &Job) -> Done {
    let store = match store {
        Ok(store) => store,
        Err(why) => {
            return failed(
                match job {
                    Job::Load {
                        extension,
                        setting,
                        ticket,
                    } => Job::Load {
                        extension: extension.clone(),
                        setting: setting.clone(),
                        ticket: *ticket,
                    },
                    Job::Write {
                        extension,
                        setting,
                        value,
                    } => Job::Write {
                        extension: extension.clone(),
                        setting: setting.clone(),
                        value: value.clone(),
                    },
                },
                why,
            );
        }
    };
    match job {
        Job::Load {
            extension,
            setting,
            ticket,
        } => Done::Loaded {
            extension: extension.clone(),
            setting: setting.clone(),
            ticket: *ticket,
            result: store.get(&account(extension, setting)),
        },
        Job::Write {
            extension,
            setting,
            value,
        } => {
            let account = account(extension, setting);
            let result = match value {
                Some(value) => store.set(&account, value),
                None => store.delete(&account),
            };
            Done::Written {
                extension: extension.clone(),
                setting: setting.clone(),
                value: value.clone(),
                result,
            }
        }
    }
}

/// What `job` ends in when it fails with `why`.
fn failed(job: Job, why: SecretError) -> Done {
    match job {
        Job::Load {
            extension,
            setting,
            ticket,
        } => Done::Loaded {
            extension,
            setting,
            ticket,
            result: Err(why),
        },
        Job::Write {
            extension,
            setting,
            value,
        } => Done::Written {
            extension,
            setting,
            value,
            result: Err(why),
        },
    }
}

/// What the page says of a store's failure, with what to do about it.
pub(in crate::app) fn advice(error: &SecretError) -> String {
    match error {
        SecretError::Locked(_) => format!("{error}. Unlock it and try again."),
        SecretError::Unavailable(_) => format!(
            "{error}. The extension may read its key from an environment variable instead; \
             see its documentation."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rawmakase_extensions::secrets::MemoryStore;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// The store is opened on the thread, at its first call, and opened again
    /// after it failed.
    #[test]
    fn the_store_is_opened_on_its_thread_and_again_after_a_failure() {
        let tries = Arc::new(AtomicU32::new(0));
        let store = Arc::new(MemoryStore::default());
        let (t, s) = (tries.clone(), store);
        let main = std::thread::current().id();
        let open: Opener = Arc::new(move || {
            assert_ne!(std::thread::current().id(), main);
            match t.fetch_add(1, Ordering::SeqCst) {
                0 => Err(SecretError::Unavailable("not yet".into())),
                _ => Ok(s.clone() as Arc<dyn SecretStore>),
            }
        });
        let mut credentials = Credentials::new(open, None);
        assert_eq!(tries.load(Ordering::SeqCst), 0);
        let done = |c: &mut Credentials| loop {
            if let Some(d) = c.finished().pop() {
                return d;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        credentials
            .write("dev.x.y", "key", Some(Secret::new("k")))
            .unwrap();
        assert!(matches!(
            done(&mut credentials),
            Done::Written { result: Err(_), .. }
        ));
        credentials
            .write("dev.x.y", "key", Some(Secret::new("k")))
            .unwrap();
        assert!(matches!(
            done(&mut credentials),
            Done::Written { result: Ok(()), .. }
        ));
        let first = credentials.load("dev.x.y", "key").unwrap();
        let second = credentials.load("dev.x.y", "key").unwrap();
        assert!(second > first);
        let _ = done(&mut credentials);
        assert_eq!(tries.load(Ordering::SeqCst), 2);
        let _ = credentials.stop();
    }
}
