//! Checks model-written tool arguments against the tool's parameter schema before the tool runs, so a missing or
//! mistyped argument comes back as one clear error the model can fix, instead of a tool failing in its own words (or
//! quietly using a default). Port of the original `validateJsonSchemaValue`.
//!
//! The subset is the one the original enforces: `type` (`string`, `number`, `integer`, `boolean`, `null`, `array`,
//! `object`), exact-one `oneOf`, `properties`, `required`, `additionalProperties: false`, array `items`, and scalar
//! `enum` and `const`. A schema with no `type` accepts any value. Other keywords are annotations and are ignored.

use serde_json::Value;

/// The violations of `value` against `schema`, in walk order, each naming its path. Empty means valid. `path` labels the
/// root: empty reads as `arguments`.
#[must_use]
pub fn violations(schema: &Value, value: &Value, path: &str) -> Vec<String> {
    let mut out = Vec::new();
    check(schema, value, path, &mut out);
    out
}

/// `invalid arguments: …` for a call that does not fit its schema; `None` when it does.
#[must_use]
pub fn invalid_arguments(schema: &Value, arguments: &Value) -> Option<String> {
    let found = violations(schema, arguments, "");
    (!found.is_empty()).then(|| format!("invalid arguments: {}", found.join("; ")))
}

fn shown(path: &str) -> &str {
    if path.is_empty() { "arguments" } else { path }
}

fn property(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
    }
}

fn is_integer(value: &Value) -> bool {
    value.is_i64()
        || value.is_u64()
        || value
            .as_f64()
            .is_some_and(|f| f.is_finite() && f.fract() == 0.0)
}

fn check(schema: &Value, value: &Value, path: &str, out: &mut Vec<String>) {
    if let Some(branches) = schema.get("oneOf").and_then(Value::as_array) {
        let matches = branches
            .iter()
            .filter(|branch| violations(branch, value, path).is_empty())
            .count();
        if matches != 1 {
            out.push(format!(
                "\"{}\" must match exactly one oneOf branch (matched {matches})",
                shown(path)
            ));
        }
        return;
    }
    let Some(kind) = schema.get("type").and_then(Value::as_str) else {
        return;
    };
    let label = shown(path);
    match kind {
        "object" => check_object(schema, value, path, out),
        "array" => match value.as_array() {
            None => out.push(format!("\"{label}\" must be an array")),
            Some(items) => {
                if let Some(item_schema) = schema.get("items") {
                    for (index, item) in items.iter().enumerate() {
                        check(item_schema, item, &format!("{path}[{index}]"), out);
                    }
                }
            }
        },
        "string" if !value.is_string() => out.push(format!("\"{label}\" must be a string")),
        "number" if !value.is_number() => out.push(format!("\"{label}\" must be a number")),
        "integer" if !is_integer(value) => out.push(format!("\"{label}\" must be an integer")),
        "boolean" if !value.is_boolean() => out.push(format!("\"{label}\" must be a boolean")),
        "null" if !value.is_null() => out.push(format!("\"{label}\" must be null")),
        "string" | "number" | "integer" | "boolean" | "null" => {
            check_scalar(schema, value, label, out);
        }
        _ => {}
    }
}

fn check_scalar(schema: &Value, value: &Value, label: &str, out: &mut Vec<String>) {
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array)
        && !allowed.contains(value)
    {
        out.push(format!(
            "\"{label}\" must be one of {}",
            Value::Array(allowed.clone())
        ));
        return;
    }
    if let Some(constant) = schema.get("const")
        && constant != value
    {
        out.push(format!("\"{label}\" must be {constant}"));
    }
}

fn check_object(schema: &Value, value: &Value, path: &str, out: &mut Vec<String>) {
    let Some(map) = value.as_object() else {
        out.push(format!("\"{}\" must be an object", shown(path)));
        return;
    };
    let properties = schema.get("properties").and_then(Value::as_object);
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    for key in required.iter().copied() {
        if map.get(key).is_none() {
            out.push(format!(
                "missing required property \"{}\"",
                property(path, key)
            ));
        }
    }
    if let Some(properties) = properties {
        for (key, child) in properties {
            // Deviation: an optional property written as `null` is read as left out; models often do that, and the
            // tools already treat an absent one as "use the default".
            if let Some(present) = map
                .get(key)
                .filter(|v| !v.is_null() || required.contains(&key.as_str()))
            {
                check(child, present, &property(path, key), out);
            }
        }
    }
    if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
        for key in map.keys() {
            if !properties.is_some_and(|p| p.contains_key(key)) {
                out.push(format!(
                    "\"{}\" is not a declared property (additionalProperties: false)",
                    property(path, key)
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn read_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "offset": { "type": "number" },
                "limit": { "type": "integer" },
                "ocr": { "type": "boolean" },
                "mode": { "type": "string", "enum": ["fast", "slow"] }
            },
            "required": ["path"]
        })
    }

    #[test]
    fn a_call_that_fits_is_valid_and_unknown_properties_are_allowed_unless_forbidden() {
        assert!(
            violations(
                &read_schema(),
                &json!({ "path": "a.rs", "offset": 1.5, "limit": 3, "extra": true }),
                ""
            )
            .is_empty()
        );
        let strict = json!({ "type": "object", "properties": { "a": { "type": "string" } }, "additionalProperties": false });
        assert_eq!(
            violations(&strict, &json!({ "a": "x", "b": 1 }), ""),
            ["\"b\" is not a declared property (additionalProperties: false)"]
        );
    }

    #[test]
    fn a_missing_required_property_and_wrong_types_are_each_named_by_their_path() {
        let mut found = violations(
            &read_schema(),
            &json!({ "offset": "5", "limit": 2.5, "ocr": "yes" }),
            "",
        );
        // Each problem is named once. Their order follows the schema's, which depends on how the JSON library was built
        // (a build that keeps insertion order differs from one that sorts), so the order is not what is checked.
        found[1..].sort();
        assert_eq!(
            found,
            [
                "missing required property \"path\"",
                "\"limit\" must be an integer",
                "\"ocr\" must be a boolean",
                "\"offset\" must be a number",
            ]
        );
        assert_eq!(
            invalid_arguments(&read_schema(), &json!({})).as_deref(),
            Some("invalid arguments: missing required property \"path\"")
        );
        assert_eq!(
            invalid_arguments(&read_schema(), &json!({ "path": "x" })),
            None
        );
    }

    #[test]
    fn a_whole_number_written_with_a_fraction_part_still_counts_as_an_integer() {
        assert!(violations(&json!({ "type": "integer" }), &json!(5.0), "n").is_empty());
        assert!(!violations(&json!({ "type": "integer" }), &json!(5.5), "n").is_empty());
    }

    #[test]
    fn enums_and_constants_name_what_is_allowed() {
        assert_eq!(
            violations(&read_schema(), &json!({ "path": "x", "mode": "turbo" }), ""),
            ["\"mode\" must be one of [\"fast\",\"slow\"]"]
        );
        assert_eq!(
            violations(&json!({ "type": "string", "const": "a" }), &json!("b"), "k"),
            ["\"k\" must be \"a\""]
        );
    }

    #[test]
    fn nested_objects_and_array_items_carry_their_full_path() {
        let schema = json!({
            "type": "object",
            "properties": { "files": { "type": "array", "items": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] } } },
            "required": ["files"]
        });
        let found = violations(
            &schema,
            &json!({ "files": [{ "path": "a" }, { "path": 3 }, {}] }),
            "",
        );
        assert_eq!(
            found,
            [
                "\"files[1].path\" must be a string",
                "missing required property \"files[2].path\""
            ]
        );
        assert_eq!(
            violations(&schema, &json!({ "files": "a.txt" }), ""),
            ["\"files\" must be an array"]
        );
    }

    #[test]
    fn exactly_one_branch_of_a_one_of_must_match() {
        let schema = json!({ "oneOf": [{ "type": "string" }, { "type": "integer" }] });
        assert!(
            violations(&schema, &json!("a"), "v").is_empty()
                && violations(&schema, &json!(3), "v").is_empty()
        );
        assert_eq!(
            violations(&schema, &json!(true), "v"),
            ["\"v\" must match exactly one oneOf branch (matched 0)"]
        );
        let overlapping = json!({ "oneOf": [{ "type": "number" }, { "type": "integer" }] });
        assert_eq!(
            violations(&overlapping, &json!(3), "v"),
            ["\"v\" must match exactly one oneOf branch (matched 2)"]
        );
    }

    #[test]
    fn a_schema_without_a_type_accepts_anything_and_arguments_that_are_not_an_object_are_refused() {
        assert!(violations(&json!({}), &json!([1, "a", null]), "").is_empty());
        assert_eq!(
            violations(&read_schema(), &json!("just text"), ""),
            ["\"arguments\" must be an object"]
        );
        assert_eq!(
            violations(&read_schema(), &Value::Null, ""),
            ["\"arguments\" must be an object"]
        );
    }

    #[test]
    fn a_required_property_set_to_null_is_checked_but_an_optional_one_reads_as_left_out() {
        assert_eq!(
            violations(&read_schema(), &json!({ "path": null }), ""),
            ["\"path\" must be a string"]
        );
        assert!(
            violations(
                &read_schema(),
                &json!({ "path": "x", "offset": null, "ocr": null }),
                ""
            )
            .is_empty()
        );
    }
}
