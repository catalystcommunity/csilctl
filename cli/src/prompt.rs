//! Interactively prompts for an operation's required request fields when
//! data is missing, walking the same resolved type structure
//! `crate::payload` encodes from.

use std::io::{BufRead, Write};

use anyhow::{Context, Result, bail};
use csilgen_core::{ControlOperator, GroupKey, LiteralValue, Occurrence, TypeExpression};
use serde_json::Value as JsonValue;

use crate::list::{File, entry_name, resolve};

/// Returns the `.default(v)` literal for `type_expr`, if it (once resolved)
/// carries one, converted to JSON so it can stand in for an absent field
/// without prompting.
fn default_value(type_expr: &TypeExpression, f: &File) -> Option<JsonValue> {
    let TypeExpression::Constrained { constraints, .. } = resolve(type_expr, f) else {
        return None;
    };
    constraints.iter().find_map(|c| match c {
        ControlOperator::Default(lit) => Some(literal_to_json(lit)),
        _ => None,
    })
}

fn literal_to_json(lit: &LiteralValue) -> JsonValue {
    match lit {
        LiteralValue::Integer(n) => JsonValue::from(*n),
        LiteralValue::Float(n) => JsonValue::from(*n),
        LiteralValue::Text(s) => JsonValue::String(s.clone()),
        LiteralValue::Bytes(b) => JsonValue::String(format!("0x{}", hex_encode(b))),
        LiteralValue::Bool(b) => JsonValue::Bool(*b),
        LiteralValue::Null => JsonValue::Null,
        LiteralValue::Array(items) => JsonValue::Array(items.iter().map(literal_to_json).collect()),
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Prompts on stdin/stdout for every required field of type_expr (resolved
/// through f) that isn't already present in `existing`, and returns the
/// assembled JSON. Fields already present in `existing` (including nested
/// ones) are kept as-is rather than prompted for. Optional fields and
/// catch-all map entries are left absent rather than prompted for, since
/// there's no fixed set of keys to ask about for the latter.
pub fn prompt_for_request(
    type_expr: &TypeExpression,
    f: &File,
    existing: Option<&JsonValue>,
) -> Result<JsonValue> {
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let mut stdout = std::io::stdout();
    prompt_value(type_expr, f, "", existing, &mut reader, &mut stdout)
}

fn prompt_value<R: BufRead, W: Write>(
    type_expr: &TypeExpression,
    f: &File,
    path: &str,
    existing: Option<&JsonValue>,
    reader: &mut R,
    writer: &mut W,
) -> Result<JsonValue> {
    let resolved = resolve(type_expr, f);
    match &resolved {
        TypeExpression::Builtin(name) => match existing {
            Some(v) => Ok(v.clone()),
            None => prompt_builtin(name, path, reader, writer),
        },
        TypeExpression::Group(g) | TypeExpression::Tuple(g) => {
            let existing_obj = existing.and_then(JsonValue::as_object);
            let mut obj = existing_obj.cloned().unwrap_or_default();
            for entry in &g.entries {
                if matches!(&entry.key, Some(GroupKey::Type(_))) {
                    // Catch-all map entry: no fixed key set to prompt for.
                    continue;
                }
                let name = entry_name(entry);
                let child_existing = existing_obj.and_then(|o| o.get(&name));
                if child_existing.is_none() {
                    if let Some(default) = default_value(&entry.value_type, f) {
                        obj.insert(name, default);
                        continue;
                    }
                    if matches!(entry.occurrence, Some(Occurrence::Optional)) {
                        continue;
                    }
                }
                let child_path = if path.is_empty() {
                    name.clone()
                } else {
                    format!("{path}.{name}")
                };
                let value = prompt_value(
                    &entry.value_type,
                    f,
                    &child_path,
                    child_existing,
                    reader,
                    writer,
                )?;
                obj.insert(name, value);
            }
            Ok(JsonValue::Object(obj))
        }
        _ => match existing {
            Some(v) => Ok(v.clone()),
            None => prompt_raw_json(path, &resolved, reader, writer),
        },
    }
}

fn prompt_builtin<R: BufRead, W: Write>(
    name: &str,
    path: &str,
    reader: &mut R,
    writer: &mut W,
) -> Result<JsonValue> {
    if name == "nil" || name == "null" {
        return Ok(JsonValue::Null);
    }

    let line = read_line(writer, &format!("{path} ({name})"), reader)?;
    match name {
        "int" => line
            .parse::<i64>()
            .map(JsonValue::from)
            .with_context(|| format!("{path}: expected an integer, got {line:?}")),
        "uint" => line
            .parse::<u64>()
            .map(JsonValue::from)
            .with_context(|| format!("{path}: expected a non-negative integer, got {line:?}")),
        "float" => line
            .parse::<f64>()
            .map(JsonValue::from)
            .with_context(|| format!("{path}: expected a number, got {line:?}")),
        "bool" => match line.trim() {
            "true" | "t" | "yes" | "y" => Ok(JsonValue::Bool(true)),
            "false" | "f" | "no" | "n" => Ok(JsonValue::Bool(false)),
            _ => bail!("{path}: expected true/false, got {line:?}"),
        },
        "text" | "bytes" => Ok(JsonValue::String(line)),
        "any" => Ok(serde_json::from_str(&line).unwrap_or(JsonValue::String(line))),
        other => bail!("{path}: unknown builtin type {other:?}"),
    }
}

/// Fallback for shapes with no fixed field set to walk (arrays, maps,
/// choices, literals, constrained types): ask for the whole value as JSON.
fn prompt_raw_json<R: BufRead, W: Write>(
    path: &str,
    resolved: &TypeExpression,
    reader: &mut R,
    writer: &mut W,
) -> Result<JsonValue> {
    let line = read_line(writer, &format!("{path} (JSON)"), reader)?;
    serde_json::from_str(&line)
        .with_context(|| format!("{path}: expected JSON matching {resolved:?}, got {line:?}"))
}

fn read_line<R: BufRead, W: Write>(writer: &mut W, prompt: &str, reader: &mut R) -> Result<String> {
    write!(writer, "{prompt}: ")?;
    writer.flush()?;
    let mut line = String::new();
    reader.read_line(&mut line)?;
    Ok(line.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn load(src: &str) -> File {
        let mut f = NamedTempFile::with_suffix(".csil").unwrap();
        f.write_all(src.as_bytes()).unwrap();
        File::load(f.path().to_str().unwrap()).unwrap()
    }

    #[test]
    fn prompts_only_required_fields() {
        let f = load("Req = { name: text, ? nickname: text, age: int }");
        let mut input = std::io::Cursor::new(b"Alice\n30\n".to_vec());
        let mut output = Vec::new();
        let value = prompt_value(
            &TypeExpression::Reference("Req".to_string()),
            &f,
            "",
            None,
            &mut input,
            &mut output,
        )
        .unwrap();
        assert_eq!(value, serde_json::json!({"name": "Alice", "age": 30}));
        let prompts = String::from_utf8(output).unwrap();
        assert!(prompts.contains("name (text)"));
        assert!(prompts.contains("age (int)"));
        assert!(!prompts.contains("nickname"));
    }

    #[test]
    fn falls_back_to_raw_json_for_map_typed_fields() {
        // csilgen-core's parser only accepts a catch-all entry (`* key =>
        // value`) as a group's sole entry, not mixed with named fields — as
        // a standalone entry it parses to `TypeExpression::Map`, which has
        // no fixed field set to walk, so prompting falls back to raw JSON.
        let f = load("Extra = {* text => int}\nReq = { name: text, extra: Extra }");
        let mut input = std::io::Cursor::new(b"Alice\n{\"a\": 1}\n".to_vec());
        let mut output = Vec::new();
        let value = prompt_value(
            &TypeExpression::Reference("Req".to_string()),
            &f,
            "",
            None,
            &mut input,
            &mut output,
        )
        .unwrap();
        assert_eq!(value, serde_json::json!({"name": "Alice", "extra": {"a": 1}}));
        assert!(String::from_utf8(output).unwrap().contains("extra (JSON)"));
    }

    #[test]
    fn prompts_nested_groups_with_dotted_path() {
        let f = load("Inner = { id: int }\nReq = { inner: Inner }");
        let mut input = std::io::Cursor::new(b"7\n".to_vec());
        let mut output = Vec::new();
        let value = prompt_value(
            &TypeExpression::Reference("Req".to_string()),
            &f,
            "",
            None,
            &mut input,
            &mut output,
        )
        .unwrap();
        assert_eq!(value, serde_json::json!({"inner": {"id": 7}}));
        assert!(String::from_utf8(output).unwrap().contains("inner.id (int)"));
    }

    #[test]
    fn rejects_malformed_numeric_input() {
        let f = load("Req = { age: int }");
        let mut input = std::io::Cursor::new(b"not-a-number\n".to_vec());
        let mut output = Vec::new();
        let err = prompt_value(
            &TypeExpression::Reference("Req".to_string()),
            &f,
            "",
            None,
            &mut input,
            &mut output,
        )
        .unwrap_err();
        assert!(err.to_string().contains("age"));
    }

    #[test]
    fn prompts_only_for_fields_missing_from_existing_data() {
        let f = load("Req = { name: text, age: int }");
        let existing = serde_json::json!({"name": "Alice"});
        let mut input = std::io::Cursor::new(b"30\n".to_vec());
        let mut output = Vec::new();
        let value = prompt_value(
            &TypeExpression::Reference("Req".to_string()),
            &f,
            "",
            Some(&existing),
            &mut input,
            &mut output,
        )
        .unwrap();
        assert_eq!(value, serde_json::json!({"name": "Alice", "age": 30}));
        let prompts = String::from_utf8(output).unwrap();
        assert!(!prompts.contains("name"));
        assert!(prompts.contains("age (int)"));
    }

    #[test]
    fn prompts_only_for_missing_nested_field() {
        let f = load("Inner = { id: int, label: text }\nReq = { inner: Inner }");
        let existing = serde_json::json!({"inner": {"label": "x"}});
        let mut input = std::io::Cursor::new(b"7\n".to_vec());
        let mut output = Vec::new();
        let value = prompt_value(
            &TypeExpression::Reference("Req".to_string()),
            &f,
            "",
            Some(&existing),
            &mut input,
            &mut output,
        )
        .unwrap();
        assert_eq!(value, serde_json::json!({"inner": {"id": 7, "label": "x"}}));
        let prompts = String::from_utf8(output).unwrap();
        assert!(prompts.contains("inner.id (int)"));
        assert!(!prompts.contains("label"));
    }

    #[test]
    fn fills_in_default_value_without_prompting() {
        let f = load(r#"Req = { name: text .default "anonymous", age: int }"#);
        let mut input = std::io::Cursor::new(b"30\n".to_vec());
        let mut output = Vec::new();
        let value = prompt_value(
            &TypeExpression::Reference("Req".to_string()),
            &f,
            "",
            None,
            &mut input,
            &mut output,
        )
        .unwrap();
        assert_eq!(value, serde_json::json!({"name": "anonymous", "age": 30}));
        let prompts = String::from_utf8(output).unwrap();
        assert!(!prompts.contains("name"));
        assert!(prompts.contains("age (int)"));
    }

    #[test]
    fn existing_value_overrides_default() {
        let f = load(r#"Req = { name: text .default "anonymous" }"#);
        let existing = serde_json::json!({"name": "Alice"});
        let mut input = std::io::Cursor::new(Vec::new());
        let mut output = Vec::new();
        let value = prompt_value(
            &TypeExpression::Reference("Req".to_string()),
            &f,
            "",
            Some(&existing),
            &mut input,
            &mut output,
        )
        .unwrap();
        assert_eq!(value, serde_json::json!({"name": "Alice"}));
    }

    #[test]
    fn fully_supplied_data_prompts_for_nothing() {
        let f = load("Req = { name: text, age: int }");
        let existing = serde_json::json!({"name": "Alice", "age": 30});
        let mut input = std::io::Cursor::new(Vec::new());
        let mut output = Vec::new();
        let value = prompt_value(
            &TypeExpression::Reference("Req".to_string()),
            &f,
            "",
            Some(&existing),
            &mut input,
            &mut output,
        )
        .unwrap();
        assert_eq!(value, existing);
        assert!(output.is_empty());
    }
}
