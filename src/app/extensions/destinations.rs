//! Extensions' export destinations (`[[export-destination]]`,
//! docs/extensions/manifest.md) from the app's side: the destinations the
//! Export dialog's Send To offers, the requests of a delivery, and what a
//! batch photo's catalog facts become in them.
//!
//! The export itself stays the app's. A delivery begins when its export is
//! queued (`export.begin`), sends each file once the batch has it in place
//! (`export.file`), and ends after the batch (`export.end`); the export
//! module drives it, and this sends each request and keeps the delivery
//! open meanwhile, so an on-demand program isn't stopped while it waits for
//! the export. Each request's outcome comes back as
//! [`Outcome::Delivery`](super::Outcome::Delivery). A delivery ends with
//! `export.end`'s answer, a failed request, Stop, the extension disabled, or
//! its program gone: a request after its program's run ended is refused, not
//! sent to a program started again.
use super::{Host, Pending};
use crate::export::batch::BatchPhoto;
use rawmakase_extensions::protocol::manifest::Setting;
use rawmakase_extensions::protocol::method::{
    AppRequest, CatalogFacts, ExportBegin, ExportEnd, ExportFile, ExportStopped, LABELS,
};
use rawmakase_extensions::protocol::{ErrorCode, RemoteError};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::time::Duration;

/// How long `export.end` may go without a progress report: a destination
/// that uploads every file reports how far it is, and each report moves
/// its deadline this far ahead.
pub(in crate::app) const END_STALL: Duration = Duration::from_secs(2 * 60);

/// Which of a delivery's requests an outcome answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Step {
    Begin,
    /// The file at this place in the export.
    File(u32),
    End,
}

/// A delivery begun on an extension and not over.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Open {
    /// The program run it began on: a later run never gets its requests.
    pub(super) run: u64,
    /// The destination's id within its extension.
    pub(super) destination: String,
    /// What `export.end` last said of how far it is.
    pub(super) progress: Option<(Option<f64>, Option<String>)>,
}

/// One export destination for Send To.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) struct Destination {
    /// How the export settings name it: `<extension>/<destination>`.
    pub(in crate::app) key: String,
    /// Its name in the manifest.
    pub(in crate::app) name: String,
    /// Why it can't take a delivery now, if it can't.
    pub(in crate::app) unavailable: Option<String>,
    /// What the user chooses for each export, in the manifest's order.
    pub(in crate::app) options: Vec<Setting>,
}

/// Every option's value for a delivery: the one chosen, where it still fits
/// its control, else its default. An option no longer declared is left out.
pub(in crate::app) fn option_values(
    options: &[Setting],
    chosen: Option<&BTreeMap<String, Value>>,
) -> Map<String, Value> {
    options
        .iter()
        .map(|o| {
            let value = chosen
                .and_then(|c| c.get(&o.id))
                .filter(|v| o.control.accepts(v))
                .unwrap_or(&o.default);
            (o.id.clone(), value.clone())
        })
        .collect()
}

/// What a delivery's `export.file` says of a batch photo besides its file,
/// taken when the export is queued.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::app) struct FileFacts {
    captured: Option<String>,
    photo_uuid: Option<String>,
    catalog: Option<CatalogFacts>,
}

impl FileFacts {
    /// `photo`'s: its capture time as its name was made from, its UUID, and
    /// what the catalog says of it, its title, caption and keywords only
    /// when the export's Metadata includes them (`descriptive`). A photo
    /// outside the catalog has neither UUID nor catalog facts.
    pub(in crate::app) fn of(photo: &BatchPhoto, descriptive: bool) -> Self {
        let captured = photo.naming.captured.as_deref().and_then(capture_time);
        let Some(facts) = &photo.catalog else {
            return Self {
                captured,
                ..Self::default()
            };
        };
        let values = &photo.values;
        let text = |value: &Option<crate::metadata::Value<crate::metadata::LangAlt>>| match value {
            Some(crate::metadata::Value::Set(langs)) if descriptive => langs
                .default_text()
                .filter(|t| !t.is_empty())
                .map(str::to_string),
            _ => None,
        };
        let mut keywords: Vec<String> = Vec::new();
        if descriptive {
            for k in &values.keywords {
                for (name, exported) in k.path.iter().zip(&k.exported) {
                    if *exported && !keywords.contains(name) {
                        keywords.push(name.clone());
                    }
                }
            }
        }
        Self {
            captured,
            photo_uuid: facts.uuid.as_ref().map(ToString::to_string),
            catalog: Some(CatalogFacts {
                title: text(&values.descriptive.title),
                caption: text(&values.descriptive.caption),
                keywords,
                label: LABELS
                    .iter()
                    .find(|l| l.eq_ignore_ascii_case(&values.label))
                    .map(|l| l.to_string()),
                rating: u8::try_from(values.rating.clamp(0, 5)).unwrap_or(0),
            }),
        }
    }

    /// The `export.file` of the file at `path`, `size` pixels, written for
    /// the photo at `index` of delivery `delivery`.
    pub(in crate::app) fn file(
        &self,
        delivery: &str,
        index: usize,
        path: &std::path::Path,
        size: (u32, u32),
    ) -> ExportFile {
        ExportFile {
            delivery: delivery.to_string(),
            destination: String::new(),
            index: u32::try_from(index).unwrap_or(u32::MAX),
            path: path.to_path_buf(),
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            width: Some(size.0),
            height: Some(size.1),
            captured: self.captured.clone(),
            photo_uuid: self.photo_uuid.clone(),
            catalog: self.catalog.clone(),
        }
    }
}

/// A capture time as the catalog keeps it ("2026-05-04 10:21:33.000") or
/// EXIF writes it ("2026:05:04 10:21:33"), as a delivery sends it:
/// "2026-05-04T10:21:33". None for anything else.
pub(in crate::app) fn capture_time(text: &str) -> Option<String> {
    let bytes = text.as_bytes().get(..19)?;
    let fits = bytes.iter().enumerate().all(|(i, &c)| match i {
        4 | 7 => c == b'-' || c == b':',
        10 => c == b' ' || c == b'T',
        13 | 16 => c == b':',
        _ => c.is_ascii_digit(),
    });
    fits.then(|| {
        format!(
            "{}-{}-{}T{}",
            &text[..4],
            &text[5..7],
            &text[8..10],
            &text[11..19]
        )
    })
}

/// Tells `e`'s program that `delivery`, open on it as `open`, stopped, so it
/// stops what it still sends: only the run it began on, which is the one
/// that knows it.
pub(super) fn tell_stopped(
    e: &mut super::Extension,
    delivery: &str,
    open: &Open,
) -> Vec<super::Outcome> {
    let Some(run) = e.run.as_mut().filter(|r| r.number == open.run && r.ready()) else {
        return Vec::new();
    };
    let (name, params) =
        rawmakase_extensions::protocol::method::AppEvent::ExportStopped(ExportStopped {
            delivery: delivery.to_string(),
            destination: open.destination.clone(),
        })
        .into_parts();
    match run.session.urgent_event(&name, params) {
        // It no longer reads even the room kept for answers: it would never
        // stop what it sends, so it is stopped instead.
        Err(err) if err.code == ErrorCode::ChannelFull => Host::end(e, ErrorCode::Exited),
        // A program already gone is ended, and counted, by its session's
        // close.
        _ => Vec::new(),
    }
}

/// A delivery begun: where its later requests go.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) struct Begun {
    /// The extension's id.
    pub(in crate::app) extension: String,
    /// The destination's id within it.
    pub(in crate::app) destination: String,
    /// The destination's name.
    pub(in crate::app) name: String,
}

impl Host {
    /// The enabled extensions' export destinations, each with why it can't
    /// take a delivery now. Disabled extensions offer none.
    pub(in crate::app) fn export_destinations(&self) -> Vec<Destination> {
        self.extensions
            .iter()
            .filter(|e| e.enabled())
            .flat_map(|e| {
                let why = e.can_run().err();
                e.manifest
                    .export_destinations
                    .iter()
                    .map(move |d| Destination {
                        key: format!("{}/{}", e.id(), d.id),
                        name: d.name.clone(),
                        unavailable: why.clone(),
                        options: d.options.clone(),
                    })
            })
            .collect()
    }

    /// Prepares `begin`'s delivery to the destination named `key`
    /// (`<extension>/<destination>`), with every option the destination
    /// declares now: the value `chosen` holds where it fits, else the default
    /// ([`option_values`]). Returns where its requests go and its
    /// `export.begin`, for [`Host::begin_delivery`]; or why it can't go.
    pub(in crate::app) fn prepare_delivery(
        &self,
        key: &str,
        mut begin: ExportBegin,
        chosen: Option<&BTreeMap<String, Value>>,
    ) -> Result<(Begun, ExportBegin), String> {
        let (extension, destination) = key
            .rsplit_once('/')
            .ok_or_else(|| format!("Send To {key} names no export destination"))?;
        let e = self
            .get(extension)
            .ok_or_else(|| format!("{extension} isn't installed"))?;
        let declared = e
            .manifest
            .export_destinations
            .iter()
            .find(|d| d.id == destination)
            .ok_or_else(|| format!("{} has no export destination {destination}", e.name()))?;
        let name = declared.name.clone();
        begin.options = option_values(&declared.options, chosen);
        begin.destination = destination.to_string();
        begin.validate().map_err(|e| format!("{name}: {e}"))?;
        let to = Begun {
            extension: extension.to_string(),
            destination: destination.to_string(),
            name,
        };
        Ok((to, begin))
    }

    /// Sends `to`'s `export.begin`, starting its program first when it isn't
    /// running, and keeps the delivery open from then on.
    pub(in crate::app) fn begin_delivery(
        &mut self,
        to: &Begun,
        begin: ExportBegin,
    ) -> Result<(), RemoteError> {
        let delivery = begin.delivery.clone();
        self.send(
            &to.extension,
            AppRequest::ExportBegin(begin),
            Pending::Delivery {
                name: to.name.clone(),
                delivery: delivery.clone(),
                step: Step::Begin,
            },
            None,
        )
        .map_err(|why| RemoteError::new(ErrorCode::Exited, why))?;
        let index = self.index(&to.extension).expect("sent to it above");
        let e = &mut self.extensions[index];
        let run = e.run.as_ref().map_or(0, |r| r.number);
        e.delivering.insert(
            delivery,
            Open {
                run,
                destination: to.destination.clone(),
                progress: None,
            },
        );
        Ok(())
    }

    /// Sends a file of `to`'s delivery: refused once the delivery is over
    /// or its program's run has ended.
    pub(in crate::app) fn send_file(
        &mut self,
        to: &Begun,
        mut file: ExportFile,
    ) -> Result<(), RemoteError> {
        self.check_open(to, &file.delivery)?;
        file.destination = to.destination.clone();
        let step = Step::File(file.index);
        let delivery = file.delivery.clone();
        self.send(
            &to.extension,
            AppRequest::ExportFile(file),
            Pending::Delivery {
                name: to.name.clone(),
                delivery,
                step,
            },
            None,
        )
        .map_err(|why| RemoteError::new(ErrorCode::Exited, why))
    }

    /// Ends `to`'s delivery `delivery`, which sent `files` files; `cancelled`
    /// when its export was.
    pub(in crate::app) fn end_delivery(
        &mut self,
        to: &Begun,
        delivery: &str,
        files: u32,
        cancelled: bool,
    ) -> Result<(), RemoteError> {
        self.check_open(to, delivery)?;
        self.send(
            &to.extension,
            AppRequest::ExportEnd(ExportEnd {
                delivery: delivery.to_string(),
                destination: to.destination.clone(),
                files,
                cancelled,
            }),
            Pending::Delivery {
                name: to.name.clone(),
                delivery: delivery.to_string(),
                step: Step::End,
            },
            None,
        )
        .map_err(|why| RemoteError::new(ErrorCode::Exited, why))
    }

    /// Stops `to`'s delivery `delivery`: its requests in flight are
    /// cancelled at the program, and settled as cancelled in the outcomes
    /// returned.
    pub(in crate::app) fn stop_delivery(
        &mut self,
        to: &Begun,
        delivery: &str,
    ) -> Vec<super::Outcome> {
        let mut outcomes = Vec::new();
        let Some(index) = self.index(&to.extension) else {
            return outcomes;
        };
        let e = &mut self.extensions[index];
        if let Some(open) = e.delivering.remove(delivery) {
            outcomes.extend(tell_stopped(e, delivery, &open));
        }
        Self::cancel_where(
            e,
            &mut outcomes,
            |p| matches!(p, Pending::Delivery { delivery: d, .. } if d == delivery),
        );
        outcomes
    }

    /// Tells `to`'s program that delivery `delivery`, whose end it answered,
    /// stopped after all: the app refused the answer.
    pub(in crate::app) fn refuse_end(&mut self, to: &Begun, delivery: &str) -> Vec<super::Outcome> {
        let Some(index) = self.index(&to.extension) else {
            return Vec::new();
        };
        let e = &mut self.extensions[index];
        let Some(run) = e.run.as_ref().map(|r| r.number) else {
            return Vec::new();
        };
        let open = Open {
            run,
            destination: to.destination.clone(),
            progress: None,
        };
        tell_stopped(e, delivery, &open)
    }

    /// What `to`'s delivery `delivery` last said of how far its end is: how
    /// much is done, 0 to 1, and a line.
    pub(in crate::app) fn delivery_progress(
        &self,
        to: &Begun,
        delivery: &str,
    ) -> Option<(Option<f64>, Option<String>)> {
        self.get(&to.extension)?
            .delivering
            .get(delivery)?
            .progress
            .clone()
    }

    /// Why `to`'s delivery `delivery` can't take another request, if it
    /// can't: stopped (`cancelled`), or its program's run over (`exited`).
    fn check_open(&self, to: &Begun, delivery: &str) -> Result<(), RemoteError> {
        let stopped = || {
            RemoteError::new(
                ErrorCode::Cancelled,
                format!("sending to {} stopped", to.name),
            )
        };
        let e = self.get(&to.extension).ok_or_else(stopped)?;
        let open = e.delivering.get(delivery).ok_or_else(stopped)?;
        match &e.run {
            Some(run) if run.number == open.run => Ok(()),
            _ => Err(RemoteError::new(
                ErrorCode::Exited,
                format!("{} ended while sending", e.name()),
            )),
        }
    }
}
