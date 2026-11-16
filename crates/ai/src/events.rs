//! Where the provider layer sends what it produces: stream tokens
//! (`ai-stream-token`), `models_refreshed`, `tool-defs-invalidated`.
//!
//! The old app sent these to its webview. Here they go onto an in-process bus:
//! the agent loop and the UI subscribe. `Events` is cheap to clone; all clones
//! share the bus and the kept services.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tokio::sync::broadcast;

/// How many events a slow subscriber may fall behind before it loses the oldest.
const CAPACITY: usize = 1024;

/// One event: its name and JSON payload.
pub type Event = (String, serde_json::Value);

#[derive(Debug)]
pub struct EmitError(pub String);

impl std::fmt::Display for EmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EmitError {}

type Services = Mutex<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>;

#[derive(Clone)]
pub struct Events {
    bus: broadcast::Sender<Event>,
    services: Arc<Services>,
}

impl Default for Events {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Events {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Events").finish_non_exhaustive()
    }
}

impl Events {
    #[must_use]
    pub fn new() -> Self {
        Self {
            bus: broadcast::channel(CAPACITY).0,
            services: Arc::default(),
        }
    }

    /// Put an event on the bus. Having no subscribers is fine: nobody is watching.
    pub fn emit<S: Serialize>(&self, event: &str, payload: S) -> Result<(), EmitError> {
        let value = serde_json::to_value(payload).map_err(|e| EmitError(e.to_string()))?;
        let _ = self.bus.send((event.to_owned(), value));
        Ok(())
    }

    /// Listen to everything emitted on this bus.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.bus.subscribe()
    }

    /// Keep a shared service (such as the credential store) for later lookup by type.
    /// `false` if one of that type was already kept.
    pub fn manage<T: Send + Sync + 'static>(&self, service: T) -> bool {
        let mut services = self
            .services
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if services.contains_key(&TypeId::of::<T>()) {
            return false;
        }
        services.insert(TypeId::of::<T>(), Arc::new(service));
        true
    }

    /// The kept service of type `T`, if there is one.
    #[must_use]
    pub fn try_state<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        let services = self
            .services
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        services
            .get(&TypeId::of::<T>())
            .cloned()?
            .downcast::<T>()
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_event_reaches_a_subscriber_on_a_clone() {
        let events = Events::new();
        let mut rx = events.subscribe();
        events
            .clone()
            .emit("tick", serde_json::json!({"n": 1}))
            .unwrap();
        let (name, payload) = rx.recv().await.unwrap();
        assert_eq!((name.as_str(), payload["n"].as_i64()), ("tick", Some(1)));
    }

    #[test]
    fn emitting_with_nobody_listening_is_fine() {
        assert!(Events::new().emit("x", 1).is_ok());
    }

    #[test]
    fn services_are_kept_per_type_and_shared_by_clones() {
        let events = Events::new();
        assert!(events.manage(7_u32));
        assert!(!events.manage(8_u32));
        assert_eq!(events.clone().try_state::<u32>().map(|v| *v), Some(7));
        assert!(events.try_state::<String>().is_none());
    }
}
