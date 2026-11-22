//! Extensions' export destinations (`[[export-destination]]`,
//! docs/extensions/manifest.md) from the app's side: the destinations the
//! Export dialog's Send To offers, and the `export.deliver` request that
//! sends an export's written files to one.
//!
//! The export itself stays the app's: a delivery is sent only once the batch
//! has published its files, from the UI thread, and the program works on
//! them while the app goes on. Its one outcome comes back as
//! [`Outcome::Delivered`](super::Outcome::Delivered), however it ends: an
//! answer, a failure, its [`DELIVER_DEADLINE`], Stop on the extension's page,
//! the extension disabled or its program gone.
use super::{Host, Pending};
use rawmakase_extensions::protocol::manifest::Setting;
use rawmakase_extensions::protocol::method::{AppRequest, ExportDeliver};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::time::Duration;

/// How long `export.deliver` may take: a web gallery resizes and uploads
/// every file of the batch.
pub(in crate::app) const DELIVER_DEADLINE: Duration = Duration::from_secs(15 * 60);

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

    /// Sends `deliver` to the destination named `key`
    /// (`<extension>/<destination>`), starting its program first when it
    /// isn't running, with every option the destination declares: the value
    /// `chosen` holds where it fits, else the default ([`option_values`]).
    /// `exported` is what the export's own report said, which the outcome
    /// carries on. Returns the destination's name, or why it wasn't sent.
    pub(in crate::app) fn deliver(
        &mut self,
        key: &str,
        mut deliver: ExportDeliver,
        chosen: Option<&BTreeMap<String, Value>>,
        exported: String,
    ) -> Result<String, String> {
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
        deliver.options = option_values(&declared.options, chosen);
        deliver.destination = destination.to_string();
        deliver.validate().map_err(|e| format!("{name}: {e}"))?;
        self.send(
            extension,
            AppRequest::ExportDeliver(deliver),
            Pending::Delivery {
                name: name.clone(),
                exported,
            },
            None,
        )
        .map_err(|why| format!("{name}: {why}"))?;
        Ok(name)
    }
}
