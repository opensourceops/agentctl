use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TemplateError {
    #[error("unclosed template expression")]
    Unclosed,
    #[error("empty template expression")]
    Empty,
    #[error("unsupported expression `{0}`")]
    Unsupported(String),
    #[error("undefined value `{0}`")]
    Undefined(String),
    #[error("embedded object or array `{0}` cannot be rendered into text")]
    NonScalar(String),
}

#[derive(Debug, Default, Clone)]
pub struct EvalContext {
    pub inputs: BTreeMap<String, Value>,
    pub vars: BTreeMap<String, Value>,
    pub memory: BTreeMap<String, Value>,
    pub tasks: BTreeMap<String, Value>,
}

pub fn validate_expression(template: &str) -> Result<(), TemplateError> {
    for expression in expressions(template)? {
        validate_path_or_comparison(expression)?;
    }
    Ok(())
}

#[must_use]
pub fn referenced_tasks(template: &str) -> BTreeSet<String> {
    expressions(template)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|expression| {
            let expression = expression
                .trim()
                .strip_prefix("not ")
                .unwrap_or(expression.trim());
            let path = split_comparison(expression)
                .map_or(expression, |(left, _, _)| left)
                .trim();
            let mut parts = path.split('.');
            (parts.next() == Some("tasks"))
                .then(|| parts.next().map(ToOwned::to_owned))
                .flatten()
        })
        .collect()
}

pub fn render(value: &Value, context: &EvalContext) -> Result<Value, TemplateError> {
    match value {
        Value::String(text) => render_string(text, context),
        Value::Array(items) => items
            .iter()
            .map(|item| render(item, context))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(map) => map
            .iter()
            .map(|(key, item)| render(item, context).map(|value| (key.clone(), value)))
            .collect::<Result<serde_json::Map<_, _>, _>>()
            .map(Value::Object),
        primitive => Ok(primitive.clone()),
    }
}

pub fn evaluate_when(expression: &str, context: &EvalContext) -> Result<bool, TemplateError> {
    let trimmed = expression.trim();
    let inner = trimmed
        .strip_prefix("${{")
        .and_then(|value| value.strip_suffix("}}"))
        .map(str::trim)
        .unwrap_or(trimmed);
    let negated = inner.starts_with("not ");
    let candidate = inner.strip_prefix("not ").unwrap_or(inner).trim();
    let result = if let Some((left, operator, right)) = split_comparison(candidate) {
        let left_value = resolve_path(left.trim(), context)?;
        let right_value: Value = serde_json::from_str(right.trim())
            .unwrap_or_else(|_| Value::String(right.trim().trim_matches(['\'', '"']).to_owned()));
        match operator {
            "==" => left_value == &right_value,
            "!=" => left_value != &right_value,
            "<" | "<=" | ">" | ">=" => {
                let left_number = left_value
                    .as_f64()
                    .ok_or_else(|| TemplateError::Unsupported(candidate.to_owned()))?;
                let right_number = right_value
                    .as_f64()
                    .ok_or_else(|| TemplateError::Unsupported(candidate.to_owned()))?;
                match operator {
                    "<" => left_number < right_number,
                    "<=" => left_number <= right_number,
                    ">" => left_number > right_number,
                    ">=" => left_number >= right_number,
                    _ => unreachable!(),
                }
            }
            _ => unreachable!(),
        }
    } else {
        truthy(resolve_path(candidate, context)?)
    };
    Ok(if negated { !result } else { result })
}

fn render_string(text: &str, context: &EvalContext) -> Result<Value, TemplateError> {
    let found = expressions_with_ranges(text)?;
    if found.is_empty() {
        return Ok(Value::String(text.to_owned()));
    }
    if found.len() == 1 && found[0].0 == 0 && found[0].1 == text.len() {
        if split_comparison(found[0].2).is_some() || found[0].2.trim_start().starts_with("not ") {
            return evaluate_when(found[0].2, context).map(Value::Bool);
        }
        return Ok(resolve_path(found[0].2, context)?.clone());
    }
    let mut output = String::new();
    let mut cursor = 0;
    for (start, end, expression) in found {
        output.push_str(&text[cursor..start]);
        let value = resolve_path(expression, context)?;
        match value {
            Value::Null => output.push_str("null"),
            Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
            Value::Number(value) => output.push_str(&value.to_string()),
            Value::String(value) => output.push_str(value),
            Value::Array(_) | Value::Object(_) => {
                return Err(TemplateError::NonScalar(expression.to_owned()));
            }
        }
        cursor = end;
    }
    output.push_str(&text[cursor..]);
    Ok(Value::String(output))
}

fn expressions(template: &str) -> Result<Vec<&str>, TemplateError> {
    expressions_with_ranges(template).map(|items| {
        items
            .into_iter()
            .map(|(_, _, expression)| expression)
            .collect()
    })
}

fn expressions_with_ranges(template: &str) -> Result<Vec<(usize, usize, &str)>, TemplateError> {
    let mut output = Vec::new();
    let mut cursor = 0;
    while let Some(relative_start) = template[cursor..].find("${{") {
        let start = cursor + relative_start;
        let inner_start = start + 3;
        let Some(relative_end) = template[inner_start..].find("}}") else {
            return Err(TemplateError::Unclosed);
        };
        let end_marker = inner_start + relative_end;
        let expression = template[inner_start..end_marker].trim();
        if expression.is_empty() {
            return Err(TemplateError::Empty);
        }
        output.push((start, end_marker + 2, expression));
        cursor = end_marker + 2;
    }
    Ok(output)
}

fn validate_path_or_comparison(expression: &str) -> Result<(), TemplateError> {
    let candidate = expression
        .trim()
        .strip_prefix("not ")
        .unwrap_or(expression.trim());
    let comparison = split_comparison(candidate);
    let path = comparison.map_or(candidate, |(left, _, _)| left).trim();
    if let Some((_, operator, right)) = comparison {
        if right.trim().is_empty() {
            return Err(TemplateError::Unsupported(expression.to_owned()));
        }
        if matches!(operator, "<" | "<=" | ">" | ">=")
            && serde_json::from_str::<Value>(right.trim())
                .ok()
                .and_then(|value| value.as_f64())
                .is_none()
        {
            return Err(TemplateError::Unsupported(expression.to_owned()));
        }
    }
    let mut parts = path.split('.');
    match parts.next() {
        Some("inputs" | "vars" | "memory") if parts.next().is_some() => {}
        Some("tasks") if parts.next().is_some() && parts.next() == Some("output") => {}
        _ => return Err(TemplateError::Unsupported(expression.to_owned())),
    }
    if path.split('.').any(|part| {
        part.is_empty()
            || !part
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
    }) {
        return Err(TemplateError::Unsupported(expression.to_owned()));
    }
    Ok(())
}

fn split_comparison(expression: &str) -> Option<(&str, &str, &str)> {
    for operator in ["==", "!=", "<=", ">=", "<", ">"] {
        if let Some((left, right)) = expression.split_once(operator) {
            return Some((left, operator, right));
        }
    }
    None
}

fn resolve_path<'a>(path: &str, context: &'a EvalContext) -> Result<&'a Value, TemplateError> {
    validate_path_or_comparison(path)?;
    let parts: Vec<&str> = path.trim().split('.').collect();
    let (root, remainder): (&BTreeMap<String, Value>, &[&str]) = match parts.as_slice() {
        ["inputs", rest @ ..] => (&context.inputs, rest),
        ["vars", rest @ ..] => (&context.vars, rest),
        ["memory", rest @ ..] => (&context.memory, rest),
        ["tasks", task, "output", rest @ ..] => {
            let value = context
                .tasks
                .get(*task)
                .ok_or_else(|| TemplateError::Undefined(path.to_owned()))?;
            return descend(value, rest, path);
        }
        _ => return Err(TemplateError::Unsupported(path.to_owned())),
    };
    let first = remainder
        .first()
        .ok_or_else(|| TemplateError::Unsupported(path.to_owned()))?;
    let value = root
        .get(*first)
        .ok_or_else(|| TemplateError::Undefined(path.to_owned()))?;
    descend(value, &remainder[1..], path)
}

fn descend<'a>(
    mut value: &'a Value,
    parts: &[&str],
    path: &str,
) -> Result<&'a Value, TemplateError> {
    for part in parts {
        value = value
            .as_object()
            .and_then(|map| map.get(*part))
            .ok_or_else(|| TemplateError::Undefined(path.to_owned()))?;
    }
    Ok(value)
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|number| number != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn exact_template_preserves_object_type() {
        let inputs = BTreeMap::from([("config".to_owned(), serde_json::json!({"safe": true}))]);
        let context = EvalContext {
            inputs,
            ..EvalContext::default()
        };
        let rendered = render(&Value::String("${{ inputs.config }}".to_owned()), &context)
            .expect("template renders");
        assert_eq!(rendered, serde_json::json!({"safe": true}));
    }

    #[test]
    fn embedded_object_is_rejected() {
        let inputs = BTreeMap::from([("config".to_owned(), serde_json::json!({"safe": true}))]);
        let context = EvalContext {
            inputs,
            ..EvalContext::default()
        };
        assert!(matches!(
            render(
                &Value::String("config=${{ inputs.config }}".to_owned()),
                &context
            ),
            Err(TemplateError::NonScalar(_))
        ));
    }

    #[test]
    fn condition_supports_typed_comparisons() {
        let inputs = BTreeMap::from([
            ("deploy".to_owned(), Value::Bool(true)),
            ("iteration".to_owned(), Value::from(2)),
            ("label".to_owned(), Value::String("ready".to_owned())),
        ]);
        let context = EvalContext {
            inputs,
            ..EvalContext::default()
        };
        assert!(evaluate_when("${{ inputs.deploy == true }}", &context).expect("valid"));
        assert!(evaluate_when("${{ inputs.label != \"blocked\" }}", &context).expect("valid"));
        assert!(evaluate_when("${{ inputs.iteration < 3 }}", &context).expect("valid"));
        assert!(evaluate_when("${{ inputs.iteration <= 2 }}", &context).expect("valid"));
        assert!(evaluate_when("${{ inputs.iteration >= 2 }}", &context).expect("valid"));
        assert!(!evaluate_when("${{ inputs.iteration > 2 }}", &context).expect("valid"));
        assert_eq!(
            render(
                &Value::String("${{ inputs.iteration < 3 }}".to_owned()),
                &context
            ),
            Ok(Value::Bool(true))
        );
        assert!(validate_expression("${{ inputs.label < \"z\" }}").is_err());
        assert!(validate_expression("${{ inputs.x + 1 }}").is_err());
    }

    proptest! {
        #[test]
        fn arbitrary_templates_never_panic(template in ".{0,4096}") {
            let validation = validate_expression(&template);
            if validation.is_ok() {
                let _ = render(&Value::String(template), &EvalContext::default());
            }
        }

        #[test]
        fn scalar_exact_templates_preserve_json(value in any::<i64>()) {
            let context = EvalContext {
                inputs: BTreeMap::from([("value".to_owned(), Value::from(value))]),
                ..EvalContext::default()
            };
            prop_assert_eq!(
                render(&Value::String("${{ inputs.value }}".to_owned()), &context),
                Ok(Value::from(value))
            );
        }
    }
}
