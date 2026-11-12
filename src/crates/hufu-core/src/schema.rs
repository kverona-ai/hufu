//! A small JSON Schema validator for structured output.
//!
//! It covers the keywords task authors actually use: `type`, `enum`, `const`, `properties`,
//! `required`, `additionalProperties`, `items`, `minItems`, `maxItems`, `minLength`, `maxLength`,
//! `minimum`, `maximum`, `allOf`, `anyOf`, `oneOf`, `not` and local `$ref`. Keywords it does not
//! know are ignored, as the specification allows.

use serde_json::Value;

/// Validates `value` against `schema`. Returns every problem found, each with its JSON path.
pub fn validate(schema: &Value, value: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    check(schema, schema, value, "$", &mut errors, 0);
    errors
}

fn admits_null(schema: &Value) -> bool {
    match schema.get("type") {
        Some(Value::String(t)) => t == "null",
        Some(Value::Array(ts)) => ts.iter().any(|t| t == "null"),
        _ => false,
    }
}

/// Writes `null` for every required property that is absent and whose schema admits `null`.
///
/// Agents that are held to a schema by their own API send every property. Agents that are only
/// told about the schema tend to leave out what they have nothing to say about, and this keeps
/// their answer usable without loosening what the schema demands.
pub fn fill_nullable(schema: &Value, value: &mut Value) {
    fill(schema, schema, value, 0);
}

fn fill(root: &Value, schema: &Value, value: &mut Value, depth: usize) {
    if depth > 64 {
        return;
    }
    let schema = match schema.get("$ref").and_then(Value::as_str).and_then(|r| r.strip_prefix('#')).and_then(|p| root.pointer(p)) {
        Some(target) => target,
        None => schema,
    };
    match value {
        Value::Object(map) => {
            let Some(props) = schema.get("properties").and_then(Value::as_object) else { return };
            for name in schema.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
                if !map.contains_key(name) && props.get(name).map(admits_null).unwrap_or(false) {
                    map.insert(name.to_string(), Value::Null);
                }
            }
            for (key, child) in map.iter_mut() {
                if let Some(sub) = props.get(key) {
                    fill(root, sub, child, depth + 1);
                }
            }
        }
        Value::Array(items) => {
            if let Some(sub) = schema.get("items") {
                for item in items.iter_mut() {
                    fill(root, sub, item, depth + 1);
                }
            }
        }
        _ => {}
    }
}

fn type_matches(name: &str, v: &Value) -> bool {
    match name {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        "number" => v.is_number(),
        "integer" => v.as_f64().map(|f| f.fract() == 0.0).unwrap_or(false),
        _ => true,
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn check(root: &Value, schema: &Value, v: &Value, path: &str, errors: &mut Vec<String>, depth: usize) {
    if depth > 64 {
        errors.push(format!("{path}: schema nesting is too deep"));
        return;
    }
    let schema = match schema {
        Value::Bool(true) => return,
        Value::Bool(false) => {
            errors.push(format!("{path}: no value is allowed here"));
            return;
        }
        Value::Object(o) => o,
        _ => return,
    };
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        match reference.strip_prefix('#').and_then(|p| root.pointer(p)) {
            Some(target) => check(root, target, v, path, errors, depth + 1),
            None => errors.push(format!("{path}: cannot resolve $ref `{reference}` (only local references are supported)")),
        }
    }
    match schema.get("type") {
        Some(Value::String(t)) if !type_matches(t, v) => {
            errors.push(format!("{path}: expected {t}, found {}", type_name(v)));
            return;
        }
        Some(Value::Array(ts)) if !ts.iter().filter_map(Value::as_str).any(|t| type_matches(t, v)) => {
            let names: Vec<&str> = ts.iter().filter_map(Value::as_str).collect();
            errors.push(format!("{path}: expected one of {}, found {}", names.join(", "), type_name(v)));
            return;
        }
        _ => {}
    }
    if let Some(options) = schema.get("enum").and_then(Value::as_array) {
        if !options.contains(v) {
            let shown: Vec<String> = options.iter().map(Value::to_string).collect();
            errors.push(format!("{path}: must be one of {}", shown.join(", ")));
        }
    }
    if let Some(expected) = schema.get("const") {
        if expected != v {
            errors.push(format!("{path}: must be {expected}"));
        }
    }
    let num = |key: &str| schema.get(key).and_then(Value::as_f64);
    match v {
        Value::Object(map) => {
            for name in schema.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
                if !map.contains_key(name) {
                    errors.push(format!("{path}: missing required property `{name}`"));
                }
            }
            let props = schema.get("properties").and_then(Value::as_object);
            for (key, child) in map {
                let child_path = format!("{path}.{key}");
                match props.and_then(|p| p.get(key)) {
                    Some(sub) => check(root, sub, child, &child_path, errors, depth + 1),
                    None => match schema.get("additionalProperties") {
                        Some(Value::Bool(false)) => errors.push(format!("{path}: unexpected property `{key}`")),
                        Some(sub @ Value::Object(_)) => check(root, sub, child, &child_path, errors, depth + 1),
                        _ => {}
                    },
                }
            }
        }
        Value::Array(items) => {
            if let Some(sub) = schema.get("items") {
                for (i, item) in items.iter().enumerate() {
                    check(root, sub, item, &format!("{path}[{i}]"), errors, depth + 1);
                }
            }
            if let Some(min) = num("minItems") {
                if (items.len() as f64) < min {
                    errors.push(format!("{path}: needs at least {min} items, has {}", items.len()));
                }
            }
            if let Some(max) = num("maxItems") {
                if (items.len() as f64) > max {
                    errors.push(format!("{path}: allows at most {max} items, has {}", items.len()));
                }
            }
        }
        Value::String(s) => {
            let len = s.chars().count() as f64;
            if let Some(min) = num("minLength") {
                if len < min {
                    errors.push(format!("{path}: must be at least {min} characters long"));
                }
            }
            if let Some(max) = num("maxLength") {
                if len > max {
                    errors.push(format!("{path}: must be at most {max} characters long"));
                }
            }
        }
        Value::Number(n) => {
            let f = n.as_f64().unwrap_or(0.0);
            if let Some(min) = num("minimum") {
                if f < min {
                    errors.push(format!("{path}: must be at least {min}"));
                }
            }
            if let Some(max) = num("maximum") {
                if f > max {
                    errors.push(format!("{path}: must be at most {max}"));
                }
            }
        }
        _ => {}
    }
    let branch = |sub: &Value| {
        let mut local = Vec::new();
        check(root, sub, v, path, &mut local, depth + 1);
        local
    };
    for sub in schema.get("allOf").and_then(Value::as_array).into_iter().flatten() {
        errors.extend(branch(sub));
    }
    if let Some(options) = schema.get("anyOf").and_then(Value::as_array) {
        if !options.iter().any(|s| branch(s).is_empty()) {
            errors.push(format!("{path}: matches none of the `anyOf` alternatives"));
        }
    }
    if let Some(options) = schema.get("oneOf").and_then(Value::as_array) {
        let hits = options.iter().filter(|s| branch(s).is_empty()).count();
        if hits != 1 {
            errors.push(format!("{path}: must match exactly one `oneOf` alternative, matches {hits}"));
        }
    }
    if let Some(sub) = schema.get("not") {
        if branch(sub).is_empty() {
            errors.push(format!("{path}: must not match the `not` schema"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::validate;
    use serde_json::json;

    #[test]
    fn validates_objects_arrays_and_scalars() {
        let schema = json!({
            "type": "object",
            "required": ["v", "tasks"],
            "additionalProperties": false,
            "properties": {
                "v": {"const": 1},
                "summary": {"type": "string", "minLength": 3},
                "tasks": {"type": "array", "minItems": 1, "items": {"$ref": "#/$defs/task"}}
            },
            "$defs": {"task": {"type": "object", "required": ["id"], "properties": {"id": {"type": "string"}, "tier": {"enum": ["read-only", "workspace-write"]}}}}
        });
        assert!(validate(&schema, &json!({"v": 1, "tasks": [{"id": "t1", "tier": "read-only"}]})).is_empty());
        let errors = validate(&schema, &json!({"v": 2, "summary": "ab", "tasks": [{"tier": "root"}], "extra": true}));
        let text = errors.join("\n");
        assert!(text.contains("$.v: must be 1"), "{text}");
        assert!(text.contains("at least 3 characters"), "{text}");
        assert!(text.contains("$.tasks[0]: missing required property `id`"), "{text}");
        assert!(text.contains("$.tasks[0].tier: must be one of"), "{text}");
        assert!(text.contains("unexpected property `extra`"), "{text}");
        assert_eq!(validate(&json!({"type": "integer"}), &json!(1.5)).len(), 1);
        assert!(validate(&json!({"type": ["string", "null"]}), &json!(null)).is_empty());
        assert!(validate(&json!({"anyOf": [{"type": "string"}, {"type": "number"}]}), &json!(3)).is_empty());
        assert_eq!(validate(&json!({"oneOf": [{"type": "number"}, {"minimum": 1}]}), &json!(3)).len(), 1);
    }

    #[test]
    fn absent_nullable_properties_are_filled_in() {
        let schema = json!({
            "type": "object",
            "required": ["id", "note", "items"],
            "properties": {
                "id": {"type": "string"},
                "note": {"type": ["string", "null"]},
                "items": {"type": "array", "items": {"type": "object", "required": ["tag"], "properties": {"tag": {"type": ["string", "null"]}}}}
            }
        });
        let mut value = json!({"items": [{}]});
        super::fill_nullable(&schema, &mut value);
        assert_eq!(value, json!({"note": null, "items": [{"tag": null}]}));
        // What may not be null is still missing, and still reported.
        assert_eq!(validate(&schema, &value), vec!["$: missing required property `id`".to_string()]);
    }
}
