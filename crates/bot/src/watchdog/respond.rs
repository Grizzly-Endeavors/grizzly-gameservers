//! The watchdog responder: the `Data`-holding consumer of fired incidents.
//!
//! One sequential loop per process — incidents are rare, and running them one
//! at a time is itself a guardrail (never two proactive Gary turns in flight).
//! Every incident is recorded to the product event log whatever its
//! disposition; that record is what makes shadow mode auditable with one query.

use tokio::sync::mpsc;
use tracing::{info, warn};

use super::Incident;
use crate::config::WatchdogMode;
use crate::discord::Data;
use crate::events::mapping;

/// Consume incidents until shutdown, on the tracked task set so an in-flight
/// response is drained rather than cut off. `None` (the watchdog is off, so no
/// channel exists) spawns nothing.
pub(crate) fn spawn_responder(
    data: Data,
    mode: WatchdogMode,
    incidents: Option<mpsc::Receiver<Incident>>,
) {
    let Some(mut incidents) = incidents else {
        return;
    };
    let tasks = data.tasks.clone();
    tasks.spawn(async move {
        loop {
            let incident = tokio::select! {
                () = data.shutdown.cancelled() => return,
                received = incidents.recv() => match received {
                    Some(incident) => incident,
                    None => return,
                },
            };
            handle_incident(&data, mode, &incident).await;
        }
    });
    info!(mode = mode.as_str(), "proactive watchdog responder started");
}

/// Dispose of one incident according to the watchdog mode.
async fn handle_incident(data: &Data, mode: WatchdogMode, incident: &Incident) {
    warn!(
        instance = %incident.instance,
        signal = incident.kind.as_str(),
        facts = ?incident.facts,
        "watchdog incident fired"
    );
    match mode {
        WatchdogMode::Shadow | WatchdogMode::On => {
            data.events
                .record(mapping::incident_event(incident, mapping::SHADOW, None))
                .await;
        }
        WatchdogMode::Off => {}
    }
}
