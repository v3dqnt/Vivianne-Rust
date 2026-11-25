//! The names every backend refuses, as one shared contract: a namespace list that is valid for one backend is valid for
//! all of them. Port of the original service definition's `RESERVED_*` sets and the checks the providers make with them.
//!
//! Vivianne has one backend, but the sets keep the original's union of ECMAScript and Python so a namespace list written
//! for either stays valid.

use std::collections::HashSet;

use super::types::BindingNamespace;

/// Binding globals refused because some backend owns the slot: `console` (output capture), the Python bootstrap's
/// `__dsh_main__`, `__builtins__` and `__name__`, and `__debug__` (which CPython compiles to a constant).
pub const RESERVED_BINDING_GLOBALS: [&str; 5] = [
    "console",
    "__dsh_main__",
    "__builtins__",
    "__name__",
    "__debug__",
];

/// Error member names every backend refuses: the JS `Error` fields and Python's exception-protocol members. Dunder
/// forms (`__x__`) are refused wholesale as well, see [`is_dunder`].
pub const RESERVED_ERROR_MEMBERS: [&str; 6] = [
    "name",
    "message",
    "stack",
    "args",
    "with_traceback",
    "add_note",
];

/// Reserved words of every portable target language (ECMAScript and Python), refused as namespace and error-class names.
pub const PORTABLE_RESERVED_WORDS: &[&str] = &[
    // ECMAScript reserved words and the ones reserved in strict mode.
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "new",
    "null",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
    "let",
    "static",
    "implements",
    "interface",
    "package",
    "private",
    "protected",
    "public",
    "arguments",
    "eval",
    // Python 3 keywords and soft keywords not already above (`type` and `_` are soft keywords, reserved here for safety).
    "False",
    "None",
    "True",
    "and",
    "as",
    "assert",
    "async",
    "def",
    "del",
    "elif",
    "except",
    "from",
    "global",
    "is",
    "lambda",
    "nonlocal",
    "not",
    "or",
    "pass",
    "raise",
    "match",
    "type",
    "_",
    // Names the program's own prelude needs for itself.
    "globalThis",
    "undefined",
    "NaN",
    "Infinity",
    "Object",
    "Array",
    "Promise",
    "Error",
    "JSON",
    "Symbol",
    "Function",
    "Math",
    "Reflect",
    "Proxy",
    "Number",
    "String",
    "Boolean",
    "Map",
    "Set",
    "Date",
    "BigInt",
];

/// Dunder form (`__x__`, non-empty middle): object-protocol slots in Python.
#[must_use]
pub fn is_dunder(name: &str) -> bool {
    name.len() > 4 && name.starts_with("__") && name.ends_with("__")
}

/// `[A-Za-z_][A-Za-z0-9_]*`, the identifier subset every language accepts.
#[must_use]
pub fn is_portable_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn check_identifier(what: &str, name: &str) -> Result<(), String> {
    if !is_portable_identifier(name) {
        return Err(format!("{what} {name:?} must match [A-Za-z_][A-Za-z0-9_]*"));
    }
    if PORTABLE_RESERVED_WORDS.contains(&name) {
        return Err(format!("{what} {name:?} is a reserved word"));
    }
    Ok(())
}

/// Checks a list of namespaces before a program sees them: portable, unreserved, unique global names, and error classes
/// whose names and member property follow the same rules. An error says which rule a name broke.
pub fn validate_namespaces(namespaces: &[BindingNamespace]) -> Result<(), String> {
    let mut seen = HashSet::new();
    for namespace in namespaces {
        check_identifier("binding namespace", &namespace.global)?;
        if RESERVED_BINDING_GLOBALS.contains(&namespace.global.as_str()) {
            return Err(format!(
                "binding namespace {:?} is reserved by the runtime",
                namespace.global
            ));
        }
        if !seen.insert(namespace.global.as_str()) {
            return Err(format!(
                "binding namespace {:?} is declared twice",
                namespace.global
            ));
        }
        if let Some(class) = &namespace.error_class {
            check_identifier("error class", &class.name)?;
            if RESERVED_BINDING_GLOBALS.contains(&class.name.as_str())
                || seen.contains(class.name.as_str())
            {
                return Err(format!(
                    "error class {:?} collides with another global",
                    class.name
                ));
            }
            seen.insert(class.name.as_str());
            let property = class.member_name_property.as_str();
            if property.is_empty() {
                return Err("error class member name property must not be empty".into());
            }
            if RESERVED_ERROR_MEMBERS.contains(&property) || is_dunder(property) {
                return Err(format!(
                    "error class member name property {property:?} is reserved"
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::ptc::types::BindingErrorClass;

    fn namespace(global: &str) -> BindingNamespace {
        BindingNamespace {
            global: global.to_owned(),
            functions: BTreeMap::new(),
            error_class: None,
        }
    }

    fn with_class(global: &str, name: &str, property: &str) -> BindingNamespace {
        BindingNamespace {
            error_class: Some(BindingErrorClass {
                name: name.to_owned(),
                member_name_property: property.to_owned(),
            }),
            ..namespace(global)
        }
    }

    #[test]
    fn the_sets_hold_what_each_backend_owns() {
        for name in [
            "console",
            "__dsh_main__",
            "__builtins__",
            "__name__",
            "__debug__",
        ] {
            assert!(RESERVED_BINDING_GLOBALS.contains(&name), "{name}");
        }
        assert!(!RESERVED_BINDING_GLOBALS.contains(&"tools"));
        for name in [
            "name",
            "message",
            "stack",
            "args",
            "with_traceback",
            "add_note",
        ] {
            assert!(RESERVED_ERROR_MEMBERS.contains(&name), "{name}");
        }
        assert!(!RESERVED_ERROR_MEMBERS.contains(&"code"));
        for word in ["function", "lambda", "nonlocal", "class"] {
            assert!(PORTABLE_RESERVED_WORDS.contains(&word), "{word}");
        }
        assert!(!PORTABLE_RESERVED_WORDS.contains(&"tools"));
    }

    #[test]
    fn dunder_names_have_a_non_empty_middle() {
        for yes in ["__dict__", "__init__", "__x__"] {
            assert!(is_dunder(yes), "{yes}");
        }
        for no in ["_private", "name", "__mid", "__", "____", "__a_"] {
            assert!(!is_dunder(no), "{no}");
        }
    }

    #[test]
    fn only_portable_identifiers_pass() {
        for yes in ["tools", "_x", "A1", "a_b"] {
            assert!(is_portable_identifier(yes), "{yes}");
        }
        for no in ["", "1a", "$tools", "a-b", "a b", "é"] {
            assert!(!is_portable_identifier(no), "{no}");
        }
    }

    #[test]
    fn namespaces_must_be_portable_unreserved_and_unique() {
        assert!(validate_namespaces(&[namespace("tools"), namespace("host")]).is_ok());
        assert!(
            validate_namespaces(&[namespace("$tools")])
                .unwrap_err()
                .contains("must match")
        );
        assert!(
            validate_namespaces(&[namespace("lambda")])
                .unwrap_err()
                .contains("reserved word")
        );
        assert!(
            validate_namespaces(&[namespace("console")])
                .unwrap_err()
                .contains("reserved by the runtime")
        );
        assert!(
            validate_namespaces(&[namespace("__name__")]).is_err(),
            "a valid identifier, but a backend's slot"
        );
        assert!(
            validate_namespaces(&[namespace("tools"), namespace("tools")])
                .unwrap_err()
                .contains("twice")
        );
        assert!(
            validate_namespaces(&[namespace("Object")]).is_err(),
            "a global the program needs itself"
        );
    }

    #[test]
    fn error_classes_follow_the_same_rules_and_their_member_property_is_not_an_error_field() {
        assert!(validate_namespaces(&[with_class("tools", "ToolError", "tool")]).is_ok());
        assert!(validate_namespaces(&[with_class("tools", "class", "tool")]).is_err());
        assert!(
            validate_namespaces(&[with_class("tools", "tools", "tool")]).is_err(),
            "it would hide the namespace"
        );
        assert!(validate_namespaces(&[with_class("tools", "E", "")]).is_err());
        assert!(validate_namespaces(&[with_class("tools", "E", "message")]).is_err());
        assert!(validate_namespaces(&[with_class("tools", "E", "__dict__")]).is_err());
        assert!(
            validate_namespaces(&[with_class("tools", "E", "any thing")]).is_ok(),
            "any other name is accepted, identifier or not"
        );
    }
}
