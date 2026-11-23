//! The source-checkout hint: guidance with no value in it, and the value only as runtime context.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::pedantic)]

use vivianne_context::dev::{source_context, source_prompt};

#[test]
fn the_prompt_never_holds_a_path() {
    let prompt = source_prompt();
    assert!(prompt.contains("separate values") && !prompt.contains('\\'));
}

#[test]
fn the_checkout_is_named_as_a_json_string_or_not_at_all() {
    assert_eq!(
        source_context(Some("C:\\src\\viv")).as_deref(),
        Some("Current Vivianne implementation checkout: \"C:\\\\src\\\\viv\".")
    );
    assert!(source_context(Some("")).is_none() && source_context(None).is_none());
}
