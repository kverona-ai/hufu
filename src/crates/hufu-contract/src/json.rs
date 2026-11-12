//! Getting JSON out of free text, and asking for it.

use serde_json::Value;

/// Extracts the JSON value an agent meant as its answer.
///
/// Tried in order: the whole text, the last fenced code block that parses, and the outermost
/// balanced `{...}` or `[...]` span.
pub fn extract_json(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(trimmed)
        && (v.is_object() || v.is_array())
    {
        return Some(v);
    }
    let mut blocks = Vec::new();
    let mut rest = trimmed;
    while let Some(start) = rest.find("```") {
        let after = &rest[start + 3..];
        let body_start = after.find('\n').map(|i| i + 1).unwrap_or(0);
        let Some(end) = after[body_start..].find("```") else {
            break;
        };
        blocks.push(&after[body_start..body_start + end]);
        rest = &after[body_start + end + 3..];
    }
    for block in blocks.iter().rev() {
        if let Ok(v) = serde_json::from_str::<Value>(block.trim())
            && (v.is_object() || v.is_array())
        {
            return Some(v);
        }
    }
    balanced_span(trimmed)
}

fn balanced_span(text: &str) -> Option<Value> {
    let bytes = text.as_bytes();
    for (start, &open) in bytes.iter().enumerate() {
        if open != b'{' && open != b'[' {
            continue;
        }
        let mut depth = 0i32;
        let mut in_str = false;
        let mut escaped = false;
        for (off, &c) in bytes[start..].iter().enumerate() {
            if in_str {
                if escaped {
                    escaped = false;
                } else if c == b'\\' {
                    escaped = true;
                } else if c == b'"' {
                    in_str = false;
                }
                continue;
            }
            match c {
                b'"' => in_str = true,
                b'{' | b'[' => depth += 1,
                b'}' | b']' => {
                    depth -= 1;
                    if depth == 0 {
                        if let Ok(v) = serde_json::from_str::<Value>(&text[start..=start + off]) {
                            return Some(v);
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// Appends the instruction used when an agent has no schema flag of its own.
pub fn instruct_json(prompt: &str, schema: &Value) -> String {
    format!(
        "{prompt}\n\n---\nWhen you have finished, your final answer must be a single fenced ```json code block \
         containing one JSON value that validates against this JSON Schema, and nothing after it:\n\n{}\n",
        serde_json::to_string_pretty(schema).unwrap_or_else(|_| schema.to_string())
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_from_plain_fenced_and_embedded_text() {
        assert_eq!(extract_json("{\"a\":1}"), Some(json!({"a":1})));
        let fenced = "Here is the plan:\n```json\n{\"v\":1,\"tasks\":[]}\n```\nDone.";
        assert_eq!(extract_json(fenced), Some(json!({"v":1,"tasks":[]})));
        let two = "```\nnot json\n```\n```json\n[1,2]\n```";
        assert_eq!(extract_json(two), Some(json!([1, 2])));
        let embedded = "The verdict is {\"verdict\":\"accept\",\"reason\":\"a } brace\"} as requested";
        assert_eq!(extract_json(embedded).unwrap()["verdict"], "accept");
        assert_eq!(extract_json("no json here"), None);
        assert_eq!(extract_json("42"), None);
    }
}
