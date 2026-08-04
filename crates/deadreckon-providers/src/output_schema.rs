use std::collections::BTreeSet;

use serde_json::Value;

use crate::{ProviderError, Result};

/// Validate the strict JSON Schema subset accepted by OpenAI response formats.
///
/// Every object must declare fixed properties, forbid additional properties,
/// and require every declared property. In particular, arbitrary-key maps are
/// not representable in this dialect; callers should use an array of fixed
/// `{key, value}` records instead. Validate locally so a controller-owned
/// schema defect never consumes a provider turn or masquerades as an operator
/// configuration problem.
pub fn validate_openai_strict_output_schema(provider: &str, schema: &Value) -> Result<()> {
    validate_node(provider, schema, "$")
}

fn validate_node(provider: &str, schema: &Value, path: &str) -> Result<()> {
    let object = schema
        .as_object()
        .ok_or_else(|| invalid(provider, path, "schema nodes must be JSON objects"))?;

    let object_shaped = object.get("type").and_then(Value::as_str) == Some("object")
        || object.contains_key("properties")
        || object.contains_key("additionalProperties")
        || object.contains_key("required");
    if object_shaped {
        let properties = object
            .get("properties")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                let detail = if object
                    .get("additionalProperties")
                    .is_some_and(Value::is_object)
                {
                    "dynamic object maps are not supported; encode maps as arrays of fixed records"
                } else {
                    "object schemas require fixed properties"
                };
                invalid(provider, path, detail)
            })?;
        if object.get("additionalProperties") != Some(&Value::Bool(false)) {
            return Err(invalid(
                provider,
                path,
                "object schemas must set additionalProperties to false; encode maps as arrays of fixed records",
            ));
        }
        let required = object
            .get("required")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid(provider, path, "object schemas require a required array"))?;
        let required = required
            .iter()
            .map(|value| {
                value.as_str().map(str::to_string).ok_or_else(|| {
                    invalid(provider, path, "required entries must be property names")
                })
            })
            .collect::<Result<BTreeSet<_>>>()?;
        let declared = properties.keys().cloned().collect::<BTreeSet<_>>();
        if required != declared {
            let missing = declared.difference(&required).cloned().collect::<Vec<_>>();
            let extra = required.difference(&declared).cloned().collect::<Vec<_>>();
            return Err(invalid(
                provider,
                path,
                format!(
                    "required must contain every property exactly once (missing: {}; extra: {})",
                    list_or_none(&missing),
                    list_or_none(&extra)
                ),
            ));
        }
        if required.len() != object["required"].as_array().map_or(0, Vec::len) {
            return Err(invalid(
                provider,
                path,
                "required must not contain duplicate property names",
            ));
        }
        for (name, child) in properties {
            validate_node(provider, child, &format!("{path}.properties.{name}"))?;
        }
    }

    if object.get("type").and_then(Value::as_str) == Some("array") {
        let items = object
            .get("items")
            .ok_or_else(|| invalid(provider, path, "array schemas require items"))?;
        validate_node(provider, items, &format!("{path}.items"))?;
    }
    for keyword in ["anyOf", "oneOf", "allOf"] {
        if let Some(branches) = object.get(keyword) {
            let branches = branches
                .as_array()
                .ok_or_else(|| invalid(provider, path, format!("{keyword} must be an array")))?;
            for (index, branch) in branches.iter().enumerate() {
                validate_node(provider, branch, &format!("{path}.{keyword}[{index}]"))?;
            }
        }
    }
    if let Some(definitions) = object.get("$defs") {
        let definitions = definitions
            .as_object()
            .ok_or_else(|| invalid(provider, path, "$defs must be an object"))?;
        for (name, definition) in definitions {
            validate_node(provider, definition, &format!("{path}.$defs.{name}"))?;
        }
    }
    Ok(())
}

fn invalid(provider: &str, path: &str, detail: impl Into<String>) -> ProviderError {
    ProviderError::InvalidOutputSchema {
        provider: provider.to_string(),
        path: path.to_string(),
        detail: detail.into(),
    }
}

fn list_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_string()
    } else {
        values.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn fixed_records_are_valid() {
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["files"],
            "properties": {
                "files": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["path", "contents"],
                        "properties": {
                            "path": {"type": "string"},
                            "contents": {"type": "string"}
                        }
                    }
                }
            }
        });
        validate_openai_strict_output_schema("cli:codex", &schema).expect("valid schema");
    }

    #[test]
    fn dynamic_maps_fail_before_the_provider() {
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["files"],
            "properties": {
                "files": {
                    "type": "object",
                    "additionalProperties": {"type": "string"}
                }
            }
        });
        let error = validate_openai_strict_output_schema("cli:codex", &schema)
            .expect_err("dynamic map must fail");
        assert!(error.to_string().contains("$.properties.files"), "{error}");
        assert!(
            error.to_string().contains("encode maps as arrays"),
            "{error}"
        );
    }

    #[test]
    fn required_must_equal_declared_properties() {
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["known", "ghost"],
            "properties": {"known": {"type": "string"}}
        });
        let error = validate_openai_strict_output_schema("openai", &schema)
            .expect_err("extra required key must fail");
        assert!(error.to_string().contains("extra: ghost"), "{error}");
    }
}
