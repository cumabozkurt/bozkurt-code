//! Event bus: a tokio broadcast channel carrying the API `Event` union
//! (`docs/API.md`, WebSocket `/events`).

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::models::{Job, Runtime, Service, Site};

/// One event as sent over the `/events` WebSocket (`{"type": "...", ...}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Event {
    #[serde(rename = "job.progress")]
    JobProgress { job: Job },
    #[serde(rename = "job.done")]
    JobDone { job: Job },
    #[serde(rename = "job.failed")]
    JobFailed { job: Job },
    #[serde(rename = "service.status")]
    ServiceStatus { service: Service },
    #[serde(rename = "site.changed")]
    SiteChanged { site: Site },
    #[serde(rename = "site.removed")]
    SiteRemoved { id: String },
    #[serde(rename = "runtime.changed")]
    RuntimeChanged { runtime: Runtime },
    #[serde(rename = "log")]
    Log { source: String, line: String },
    #[serde(rename = "share.url")]
    ShareUrl { site_id: String, url: String },
}

/// Cloneable handle to the broadcast bus. Sending never blocks; slow subscribers lag
/// (they receive `RecvError::Lagged`) instead of applying back-pressure.
#[derive(Debug, Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Event>,
}

impl Default for EventBus {
    fn default() -> Self {
        EventBus::new(1024)
    }
}

impl EventBus {
    pub fn new(capacity: usize) -> EventBus {
        let (tx, _) = broadcast::channel(capacity);
        EventBus { tx }
    }

    /// Publish (no-op when nobody listens).
    pub fn emit(&self, e: Event) {
        let _ = self.tx.send(e);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    pub fn sender(&self) -> broadcast::Sender<Event> {
        self.tx.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tagged_json() {
        let e = Event::SiteRemoved { id: "shop".into() };
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            serde_json::json!({"type": "site.removed", "id": "shop"})
        );
        let l: Event =
            serde_json::from_value(serde_json::json!({"type":"log","source":"caddy","line":"x"}))
                .unwrap();
        assert_eq!(
            l,
            Event::Log {
                source: "caddy".into(),
                line: "x".into()
            }
        );
        let s = Event::ShareUrl {
            site_id: "a".into(),
            url: "https://x".into(),
        };
        assert_eq!(serde_json::to_value(&s).unwrap()["type"], "share.url");
    }

    #[tokio::test]
    async fn broadcast() {
        let bus = EventBus::default();
        let mut rx = bus.subscribe();
        bus.emit(Event::SiteRemoved { id: "a".into() });
        assert_eq!(
            rx.recv().await.unwrap(),
            Event::SiteRemoved { id: "a".into() }
        );
    }
}
