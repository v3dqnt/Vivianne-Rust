//! Port of `src/api-registry.ts`.
//!
//! Custom API provider registry. Allows extensions to register streaming functions for custom API
//! types (e.g., "vertex-claude-api") that are not built into `stream.ts`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::error::validation::ConfigurationError;
use crate::types::{Context, Model, SimpleStreamOptions, StreamOptions, KNOWN_APIS};
use crate::utils::event_stream::AssistantMessageEventStream;

pub type CustomStreamFn =
    Arc<dyn Fn(&Model, &Context, Option<&StreamOptions>) -> AssistantMessageEventStream + Send + Sync>;
pub type CustomStreamSimpleFn =
    Arc<dyn Fn(&Model, &Context, Option<&SimpleStreamOptions>) -> AssistantMessageEventStream + Send + Sync>;

#[derive(Clone)]
pub struct RegisteredCustomApi {
    pub stream: CustomStreamFn,
    pub stream_simple: CustomStreamSimpleFn,
    pub source_id: Option<String>,
}

fn registry() -> &'static Mutex<HashMap<String, RegisteredCustomApi>> {
    static R: OnceLock<Mutex<HashMap<String, RegisteredCustomApi>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock() -> std::sync::MutexGuard<'static, HashMap<String, RegisteredCustomApi>> {
    registry().lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn assert_custom_api_name(api: &str) -> Result<(), ConfigurationError> {
    if KNOWN_APIS.contains(&api) {
        return Err(ConfigurationError::new(
            format!("Cannot register custom API \"{api}\": built-in API names are reserved."),
            None,
        ));
    }
    Ok(())
}

/// Register a custom API streaming function. When `stream` is omitted it falls back to
/// `stream_simple`, handing it the base options as simple options.
pub fn register_custom_api(
    api: &str,
    stream_simple: CustomStreamSimpleFn,
    source_id: Option<&str>,
    stream: Option<CustomStreamFn>,
) -> Result<(), ConfigurationError> {
    assert_custom_api_name(api)?;
    let stream = stream.unwrap_or_else(|| {
        let simple = stream_simple.clone();
        Arc::new(move |model: &Model, context: &Context, options: Option<&StreamOptions>| {
            let opts = options.map(|o| SimpleStreamOptions { base: o.clone(), ..Default::default() });
            simple(model, context, opts.as_ref())
        })
    });
    lock().insert(
        api.to_owned(),
        RegisteredCustomApi { stream, stream_simple, source_id: source_id.map(str::to_owned) },
    );
    Ok(())
}

/// Get a custom API provider by API identifier.
pub fn get_custom_api(api: &str) -> Option<RegisteredCustomApi> {
    lock().get(api).cloned()
}

/// Remove all custom APIs registered by a specific source (e.g., extension path).
pub fn unregister_custom_apis(source_id: &str) {
    lock().retain(|_, entry| entry.source_id.as_deref() != Some(source_id));
}

/// Clear all custom API registrations.
pub fn clear_custom_apis() {
    lock().clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simple() -> CustomStreamSimpleFn {
        Arc::new(|_, _, _| AssistantMessageEventStream::new())
    }

    // The registry is process-global; each test uses its own api names and source ids.
    #[test]
    fn built_in_names_are_reserved() {
        let err = register_custom_api("anthropic-messages", simple(), None, None).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Cannot register custom API \"anthropic-messages\": built-in API names are reserved."
        );
    }

    #[test]
    fn register_get_and_unregister_by_source() {
        register_custom_api("reg-test-a", simple(), Some("ext-reg-test"), None).unwrap();
        register_custom_api("reg-test-b", simple(), Some("ext-reg-test"), None).unwrap();
        register_custom_api("reg-test-c", simple(), Some("other-reg-test"), None).unwrap();
        assert!(get_custom_api("reg-test-a").is_some());
        unregister_custom_apis("ext-reg-test");
        assert!(get_custom_api("reg-test-a").is_none() && get_custom_api("reg-test-b").is_none());
        assert!(get_custom_api("reg-test-c").is_some());
        unregister_custom_apis("other-reg-test");
        assert!(get_custom_api("reg-test-c").is_none());
    }

    #[test]
    fn stream_defaults_to_the_simple_function() {
        let calls = Arc::new(Mutex::new(0));
        let c2 = calls.clone();
        let f: CustomStreamSimpleFn = Arc::new(move |_, _, _| {
            *c2.lock().unwrap() += 1;
            AssistantMessageEventStream::new()
        });
        register_custom_api("reg-test-d", f, Some("src-reg-d"), None).unwrap();
        let entry = get_custom_api("reg-test-d").unwrap();
        let _ = (entry.stream)(&serde_json::json!({}), &Context::default(), None);
        assert_eq!(*calls.lock().unwrap(), 1);
        unregister_custom_apis("src-reg-d");
    }
}
