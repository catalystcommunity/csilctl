//! Converts between user-supplied JSON (`--data`) and the CBOR payloads the
//! CSIL-RPC wire format expects, and back for displaying a response — guided
//! by the operation's request/response type as resolved from the parsed
//! `.csil` file (`crate::list::File`).

use std::cmp::Ordering;

use anyhow::{Context, Result, anyhow, bail};
use ciborium::value::Value as CborValue;
use csilgen_core::{ControlOperator, GroupKey, LiteralValue, SizeConstraint, TypeExpression};
use regex::Regex;
use serde_json::Value as JsonValue;

use crate::list::{File, entry_name, resolve};

/// Encodes a JSON value against `type_expr` (resolved through `f`) into a CBOR
/// value, using canonical (sorted) map key order so the bytes match what a
/// real CSIL-RPC server expects.
pub fn json_to_cbor(json: &JsonValue, type_expr: &TypeExpression, f: &File) -> Result<CborValue> {
    let resolved = resolve(type_expr, f);
    match &resolved {
        TypeExpression::Builtin(name) => encode_builtin(json, name),
        TypeExpression::Literal(lit) => Ok(literal_to_cbor(lit)),
        TypeExpression::Group(g) | TypeExpression::Tuple(g) => {
            let obj = json
                .as_object()
                .ok_or_else(|| anyhow!("expected a JSON object, got {json}"))?;

            let mut consumed = std::collections::HashSet::new();
            let mut entries = Vec::new();
            let mut catch_all: Option<(&TypeExpression, &TypeExpression)> = None;

            for entry in &g.entries {
                if let Some(GroupKey::Type(key_type)) = &entry.key {
                    catch_all = Some((key_type, &entry.value_type));
                    continue;
                }

                let name = entry_name(entry);
                consumed.insert(name.clone());
                let optional = matches!(
                    entry.occurrence,
                    Some(csilgen_core::Occurrence::Optional)
                );
                match obj.get(&name) {
                    Some(v) => {
                        entries.push((CborValue::Text(name), json_to_cbor(v, &entry.value_type, f)?));
                    }
                    None if optional => {}
                    None => bail!("missing required field {name:?}"),
                }
            }

            if let Some((key_type, value_type)) = catch_all {
                for (k, v) in obj {
                    if consumed.contains(k) {
                        continue;
                    }
                    let key_cbor = json_to_cbor(&JsonValue::String(k.clone()), key_type, f)?;
                    entries.push((key_cbor, json_to_cbor(v, value_type, f)?));
                }
            }

            Ok(canon_map(entries))
        }
        TypeExpression::Array {
            element_type,
            occurrence: _,
        } => {
            let arr = json
                .as_array()
                .ok_or_else(|| anyhow!("expected a JSON array, got {json}"))?;
            let items = arr
                .iter()
                .map(|v| json_to_cbor(v, element_type, f))
                .collect::<Result<Vec<_>>>()?;
            Ok(CborValue::Array(items))
        }
        TypeExpression::Map { key, value, .. } => {
            let obj = json
                .as_object()
                .ok_or_else(|| anyhow!("expected a JSON object, got {json}"))?;
            let mut entries = Vec::new();
            for (k, v) in obj {
                let key_cbor = json_to_cbor(&JsonValue::String(k.clone()), key, f)?;
                entries.push((key_cbor, json_to_cbor(v, value, f)?));
            }
            Ok(canon_map(entries))
        }
        TypeExpression::Choice(arms) => {
            // Try `{"_type": "ArmName", ...}` first; otherwise use the first
            // arm that encodes without error.
            if let Some(hint) = json.get("_type").and_then(|v| v.as_str()) {
                for arm in arms {
                    if arm_name(arm) == Some(hint) {
                        return json_to_cbor(json, arm, f);
                    }
                }
            }
            for arm in arms {
                if let Ok(v) = json_to_cbor(json, arm, f) {
                    return Ok(v);
                }
            }
            bail!("value did not match any arm of the choice type")
        }
        TypeExpression::Constrained { base_type, constraints } => {
            let value = json_to_cbor(json, base_type, f)?;
            for constraint in constraints {
                validate_constraint(&value, constraint, json, f)?;
            }
            Ok(value)
        }
        TypeExpression::Reference(name) => bail!("unknown type {name:?}"),
        other => bail!("unsupported type expression for encoding: {other:?}"),
    }
}

fn arm_name(t: &TypeExpression) -> Option<&str> {
    match t {
        TypeExpression::Reference(n) => Some(n.as_str()),
        _ => None,
    }
}

fn encode_builtin(json: &JsonValue, name: &str) -> Result<CborValue> {
    match name {
        "int" => {
            let n = json
                .as_i64()
                .ok_or_else(|| anyhow!("expected an integer, got {json}"))?;
            Ok(CborValue::Integer(n.into()))
        }
        "uint" => {
            let n = json
                .as_u64()
                .ok_or_else(|| anyhow!("expected a non-negative integer, got {json}"))?;
            Ok(CborValue::Integer(n.into()))
        }
        "float" => {
            let n = json
                .as_f64()
                .ok_or_else(|| anyhow!("expected a number, got {json}"))?;
            Ok(CborValue::Float(n))
        }
        "text" => {
            let s = json
                .as_str()
                .ok_or_else(|| anyhow!("expected a string, got {json}"))?;
            Ok(CborValue::Text(s.to_string()))
        }
        "bool" => {
            let b = json
                .as_bool()
                .ok_or_else(|| anyhow!("expected a boolean, got {json}"))?;
            Ok(CborValue::Bool(b))
        }
        "bytes" => encode_bytes(json),
        "nil" => Ok(CborValue::Null),
        "any" => Ok(json_to_cbor_any(json)),
        other => bail!("unknown builtin type {other:?}"),
    }
}

fn encode_bytes(json: &JsonValue) -> Result<CborValue> {
    match json {
        JsonValue::String(s) => {
            if let Some(hex) = s.strip_prefix("0x") {
                let bytes = hex_decode(hex)?;
                Ok(CborValue::Bytes(bytes))
            } else {
                Ok(CborValue::Bytes(s.as_bytes().to_vec()))
            }
        }
        JsonValue::Array(items) => {
            let bytes = items
                .iter()
                .map(|v| {
                    v.as_u64()
                        .and_then(|n| u8::try_from(n).ok())
                        .ok_or_else(|| anyhow!("expected a byte (0-255), got {v}"))
                })
                .collect::<Result<Vec<u8>>>()?;
            Ok(CborValue::Bytes(bytes))
        }
        other => bail!("expected a string or array of bytes, got {other}"),
    }
}

fn hex_decode(s: &str) -> Result<Vec<u8>> {
    if s.len() % 2 != 0 {
        bail!("hex string must have an even number of digits");
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| anyhow!(e)))
        .collect()
}

fn literal_to_cbor(lit: &LiteralValue) -> CborValue {
    match lit {
        LiteralValue::Integer(n) => CborValue::Integer((*n).into()),
        LiteralValue::Float(n) => CborValue::Float(*n),
        LiteralValue::Text(s) => CborValue::Text(s.clone()),
        LiteralValue::Bytes(b) => CborValue::Bytes(b.clone()),
        LiteralValue::Bool(b) => CborValue::Bool(*b),
        LiteralValue::Null => CborValue::Null,
        LiteralValue::Array(items) => CborValue::Array(items.iter().map(literal_to_cbor).collect()),
    }
}

/// Checks value against a single `.size`/`.regex`/`.ge`/`.le`/`.gt`/`.lt`/
/// `.eq`/`.ne`/`.bits`/`.and`/`.within`/`.json`/`.cbor`/`.cborseq` control
/// operator, bailing with a descriptive error if it's violated.
fn validate_constraint(
    value: &CborValue,
    constraint: &ControlOperator,
    json: &JsonValue,
    f: &File,
) -> Result<()> {
    match constraint {
        ControlOperator::Size(sc) => {
            let len = value_len(value)
                .ok_or_else(|| anyhow!(".size does not apply to a {}", cbor_kind(value)))?;
            if !size_constraint_matches(sc, len) {
                bail!(".size constraint violated: length {len} does not satisfy {sc:?}");
            }
        }
        ControlOperator::Regex(pattern) => {
            let CborValue::Text(s) = value else {
                bail!(".regex does not apply to a {}", cbor_kind(value));
            };
            let re = Regex::new(pattern)
                .map_err(|e| anyhow!("invalid .regex pattern {pattern:?}: {e}"))?;
            if !re.is_match(s) {
                bail!(".regex constraint violated: {s:?} does not match {pattern:?}");
            }
        }
        ControlOperator::GreaterEqual(lit) => {
            check_ordering(value, lit, ".ge", |o| o != Ordering::Less)?
        }
        ControlOperator::LessEqual(lit) => {
            check_ordering(value, lit, ".le", |o| o != Ordering::Greater)?
        }
        ControlOperator::GreaterThan(lit) => {
            check_ordering(value, lit, ".gt", |o| o == Ordering::Greater)?
        }
        ControlOperator::LessThan(lit) => {
            check_ordering(value, lit, ".lt", |o| o == Ordering::Less)?
        }
        ControlOperator::Equal(lit) => {
            if *value != literal_to_cbor(lit) {
                bail!(".eq constraint violated: {value:?} does not equal {lit:?}");
            }
        }
        ControlOperator::NotEqual(lit) => {
            if *value == literal_to_cbor(lit) {
                bail!(".ne constraint violated: {value:?} equals {lit:?}");
            }
        }
        ControlOperator::Bits(mask_expr) => {
            let CborValue::Integer(n) = value else {
                bail!(".bits does not apply to a {}", cbor_kind(value));
            };
            let n: i128 = (*n).into();
            let mask = parse_bits_mask(mask_expr)?;
            if n & !mask != 0 {
                bail!(".bits constraint violated: {n:#x} sets bits outside mask {mask_expr}");
            }
        }
        ControlOperator::And(and_type) => {
            json_to_cbor(json, and_type, f)
                .with_context(|| ".and constraint violated: value does not also match the intersected type")?;
        }
        ControlOperator::Within(within_type) => {
            json_to_cbor(json, within_type, f)
                .with_context(|| ".within constraint violated: value does not match the referenced type")?;
        }
        ControlOperator::Json => {
            let CborValue::Text(s) = value else {
                bail!(".json does not apply to a {}", cbor_kind(value));
            };
            serde_json::from_str::<JsonValue>(s)
                .map_err(|e| anyhow!(".json constraint violated: {s:?} is not valid JSON: {e}"))?;
        }
        ControlOperator::Cbor => {
            let CborValue::Bytes(b) = value else {
                bail!(".cbor does not apply to a {}", cbor_kind(value));
            };
            ciborium::de::from_reader::<CborValue, _>(b.as_slice())
                .map_err(|e| anyhow!(".cbor constraint violated: bytes are not valid CBOR: {e}"))?;
        }
        ControlOperator::Cborseq => {
            let CborValue::Bytes(b) = value else {
                bail!(".cborseq does not apply to a {}", cbor_kind(value));
            };
            let mut cursor = std::io::Cursor::new(b.as_slice());
            while (cursor.position() as usize) < b.len() {
                ciborium::de::from_reader::<CborValue, _>(&mut cursor).map_err(|e| {
                    anyhow!(".cborseq constraint violated: bytes are not a valid CBOR sequence: {e}")
                })?;
            }
        }
        ControlOperator::Default(_) => {} // Default is handled in crate::prompt
    }
    Ok(())
}

/// Parses a `.bits` mask expression (e.g. `"0x00FF"` or `"255"`) into the
/// integer bitmask of positions the value is allowed to set.
fn parse_bits_mask(expr: &str) -> Result<i128> {
    let trimmed = expr.trim();
    if let Some(hex) = trimmed.strip_prefix("0x").or_else(|| trimmed.strip_prefix("0X")) {
        i128::from_str_radix(hex, 16)
    } else {
        trimmed.parse::<i128>()
    }
    .map_err(|e| anyhow!("invalid .bits mask {expr:?}: {e}"))
}

fn check_ordering(
    value: &CborValue,
    lit: &LiteralValue,
    op_name: &str,
    accept: impl Fn(Ordering) -> bool,
) -> Result<()> {
    let target = literal_to_cbor(lit);
    let ordering = cbor_partial_cmp(value, &target)
        .ok_or_else(|| anyhow!("{op_name} does not apply to a {}", cbor_kind(value)))?;
    if !accept(ordering) {
        bail!("{op_name} constraint violated: {value:?} vs {lit:?}");
    }
    Ok(())
}

fn cbor_partial_cmp(a: &CborValue, b: &CborValue) -> Option<Ordering> {
    match (a, b) {
        (CborValue::Text(x), CborValue::Text(y)) => Some(x.cmp(y)),
        (CborValue::Bytes(x), CborValue::Bytes(y)) => Some(x.cmp(y)),
        (CborValue::Bool(x), CborValue::Bool(y)) => Some(x.cmp(y)),
        _ => cbor_as_f64(a).zip(cbor_as_f64(b)).and_then(|(x, y)| x.partial_cmp(&y)),
    }
}

fn cbor_as_f64(v: &CborValue) -> Option<f64> {
    match v {
        CborValue::Integer(n) => Some(i128::from(*n) as f64),
        CborValue::Float(f) => Some(*f),
        _ => None,
    }
}

fn value_len(v: &CborValue) -> Option<u64> {
    match v {
        CborValue::Text(s) => Some(s.chars().count() as u64),
        CborValue::Bytes(b) => Some(b.len() as u64),
        CborValue::Array(a) => Some(a.len() as u64),
        CborValue::Map(m) => Some(m.len() as u64),
        _ => None,
    }
}

fn size_constraint_matches(sc: &SizeConstraint, len: u64) -> bool {
    match sc {
        SizeConstraint::Exact(n) => len == *n,
        SizeConstraint::Range { min, max } => len >= *min && len <= *max,
        SizeConstraint::Min(min) => len >= *min,
        SizeConstraint::Max(max) => len <= *max,
    }
}

fn cbor_kind(v: &CborValue) -> &'static str {
    match v {
        CborValue::Integer(_) => "integer",
        CborValue::Float(_) => "float",
        CborValue::Text(_) => "text",
        CborValue::Bytes(_) => "bytes",
        CborValue::Bool(_) => "bool",
        CborValue::Null => "null",
        CborValue::Array(_) => "array",
        CborValue::Map(_) => "map",
        _ => "value",
    }
}

/// Converts arbitrary JSON into CBOR with no schema guidance (for the `any`
/// builtin type).
fn json_to_cbor_any(json: &JsonValue) -> CborValue {
    match json {
        JsonValue::Null => CborValue::Null,
        JsonValue::Bool(b) => CborValue::Bool(*b),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                CborValue::Integer(i.into())
            } else if let Some(u) = n.as_u64() {
                CborValue::Integer(u.into())
            } else {
                CborValue::Float(n.as_f64().unwrap_or_default())
            }
        }
        JsonValue::String(s) => CborValue::Text(s.clone()),
        JsonValue::Array(items) => CborValue::Array(items.iter().map(json_to_cbor_any).collect()),
        JsonValue::Object(obj) => {
            let entries = obj
                .iter()
                .map(|(k, v)| (CborValue::Text(k.clone()), json_to_cbor_any(v)))
                .collect();
            canon_map(entries)
        }
    }
}

/// Sorts map entries into RFC 8949 canonical order (by encoded key bytes) so
/// the wire bytes match what a real CSIL server/codec produces.
fn canon_map(entries: Vec<(CborValue, CborValue)>) -> CborValue {
    let mut keyed: Vec<(Vec<u8>, CborValue, CborValue)> = entries
        .into_iter()
        .map(|(k, v)| {
            let mut buf = Vec::new();
            ciborium::ser::into_writer(&k, &mut buf).expect("cbor key encodes");
            (buf, k, v)
        })
        .collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    CborValue::Map(keyed.into_iter().map(|(_, k, v)| (k, v)).collect())
}

/// Converts a decoded CBOR response payload into JSON for display, with no
/// schema guidance needed — the CBOR shape alone is enough to render it.
pub fn cbor_to_json(v: &CborValue) -> JsonValue {
    match v {
        CborValue::Integer(n) => {
            let n: i128 = (*n).into();
            serde_json::Number::from_i128(n)
                .map(JsonValue::Number)
                .unwrap_or(JsonValue::Null)
        }
        CborValue::Bytes(b) => JsonValue::String(format!("0x{}", hex_encode(b))),
        CborValue::Float(f) => serde_json::Number::from_f64(*f)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
        CborValue::Text(s) => JsonValue::String(s.clone()),
        CborValue::Bool(b) => JsonValue::Bool(*b),
        CborValue::Null => JsonValue::Null,
        CborValue::Array(items) => JsonValue::Array(items.iter().map(cbor_to_json).collect()),
        CborValue::Map(entries) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in entries {
                let key = match k {
                    CborValue::Text(s) => s.clone(),
                    other => cbor_to_json(other).to_string(),
                };
                obj.insert(key, cbor_to_json(v));
            }
            JsonValue::Object(obj)
        }
        CborValue::Tag(_, inner) => cbor_to_json(inner),
        _ => JsonValue::Null,
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn load(src: &str) -> File {
        let mut f = NamedTempFile::with_suffix(".csil").unwrap();
        f.write_all(src.as_bytes()).unwrap();
        File::load(f.path().to_str().unwrap()).unwrap()
    }

    fn encode(src: &str, json: JsonValue) -> Result<CborValue> {
        let f = load(src);
        json_to_cbor(&json, &TypeExpression::Reference("TestCase".to_string()), &f)
    }

    #[test]
    fn size_range_accepts_and_rejects() {
        let src = "TestCase = text .size (3..5)";
        assert!(encode(src, JsonValue::String("abc".into())).is_ok());
        assert!(encode(src, JsonValue::String("abcde".into())).is_ok());
        let err = encode(src, JsonValue::String("no".into())).unwrap_err();
        assert!(err.to_string().contains(".size constraint violated"));
        let err = encode(src, JsonValue::String("toolong".into())).unwrap_err();
        assert!(err.to_string().contains(".size constraint violated"));
    }

    #[test]
    fn size_applies_to_arrays_and_bytes() {
        let src = "TestCase = [text] .size (1..2)";
        assert!(encode(src, serde_json::json!(["a"])).is_ok());
        assert!(encode(src, serde_json::json!([])).is_err());

        let src = "TestCase = bytes .size 3";
        assert!(encode(src, JsonValue::String("abc".into())).is_ok());
        assert!(encode(src, JsonValue::String("ab".into())).is_err());
    }

    #[test]
    fn regex_accepts_and_rejects() {
        let src = r#"TestCase = text .regex "^[a-z]+$""#;
        assert!(encode(src, JsonValue::String("widget".into())).is_ok());
        let err = encode(src, JsonValue::String("Widget1".into())).unwrap_err();
        assert!(err.to_string().contains(".regex constraint violated"));
    }

    #[test]
    fn regex_only_applies_to_text() {
        let src = r#"TestCase = int .regex "^[0-9]+$""#;
        let err = encode(src, serde_json::json!(5)).unwrap_err();
        assert!(err.to_string().contains(".regex does not apply"));
    }

    #[test]
    fn numeric_comparisons() {
        let src = "TestCase = int .ge 0 .le 120";
        assert!(encode(src, serde_json::json!(0)).is_ok());
        assert!(encode(src, serde_json::json!(120)).is_ok());
        assert!(encode(src, serde_json::json!(-1)).is_err());
        assert!(encode(src, serde_json::json!(121)).is_err());

        let src = "TestCase = int .gt 0";
        assert!(encode(src, serde_json::json!(1)).is_ok());
        assert!(encode(src, serde_json::json!(0)).is_err());

        let src = "TestCase = int .lt 10";
        assert!(encode(src, serde_json::json!(9)).is_ok());
        assert!(encode(src, serde_json::json!(10)).is_err());
    }

    #[test]
    fn eq_and_ne() {
        let src = r#"TestCase = text .eq "widget""#;
        assert!(encode(src, JsonValue::String("widget".into())).is_ok());
        assert!(encode(src, JsonValue::String("gadget".into())).is_err());

        let src = r#"TestCase = text .ne "widget""#;
        assert!(encode(src, JsonValue::String("gadget".into())).is_ok());
        assert!(encode(src, JsonValue::String("widget".into())).is_err());
    }

    #[test]
    fn bits_accepts_and_rejects_hex_mask() {
        // mask 0x00FF = 0b0000_0000_1111_1111
        let src = r#"TestCase = int .bits "0x00FF""#;
        assert!(encode(src, JsonValue::from(0x00AA)).is_ok()); // 0b0000_0000_1010_1010: within mask
        assert!(encode(src, JsonValue::from(0x0100)).is_err()); // 0b0000_0001_0000_0000: bit 8 outside mask
        assert!(encode(src, JsonValue::from(0x01AA)).is_err()); // 0b0000_0001_1010_1010: bit 8 outside mask
    }

    #[test]
    fn bits_accepts_decimal_mask() {
        // mask 7 = 0b111
        let src = r#"TestCase = int .bits "7""#;
        assert!(encode(src, JsonValue::from(5)).is_ok()); // 0b101: within mask
        assert!(encode(src, JsonValue::from(8)).is_err()); // 0b1000: bit 3 outside mask
    }

    #[test]
    fn bits_rejects_non_integer_value() {
        let src = r#"TestCase = text .bits "0x0F""#;
        assert!(encode(src, JsonValue::String("x".into())).is_err());
    }

    #[test]
    fn and_requires_value_to_also_match_intersected_type() {
        let src = "Positive = int .ge 0\nTestCase = int .and Positive";
        assert!(encode(src, JsonValue::from(5)).is_ok());
        assert!(encode(src, JsonValue::from(-5)).is_err());
    }

    #[test]
    fn and_rejects_shape_mismatch_against_intersected_type() {
        let src = "TestCase = int .and text";
        assert!(encode(src, JsonValue::from(5)).is_err());
    }

    #[test]
    fn within_requires_value_to_match_referenced_type() {
        let src = "Positive = int .ge 0\nTestCase = int .within Positive";
        assert!(encode(src, JsonValue::from(5)).is_ok());
        assert!(encode(src, JsonValue::from(-5)).is_err());
    }

    #[test]
    fn within_rejects_shape_mismatch_against_referenced_type() {
        let src = "TestCase = int .within text";
        assert!(encode(src, JsonValue::from(5)).is_err());
    }

    #[test]
    fn json_accepts_valid_json_text_and_rejects_malformed() {
        let src = "TestCase = text .json";
        assert!(encode(src, JsonValue::String(r#"{"a":1}"#.into())).is_ok());
        assert!(encode(src, JsonValue::String("not json".into())).is_err());
    }

    #[test]
    fn json_rejects_non_text_value() {
        let src = "TestCase = int .json";
        assert!(encode(src, JsonValue::from(5)).is_err());
    }

    #[test]
    fn cbor_accepts_valid_cbor_bytes_and_rejects_malformed() {
        let src = "TestCase = bytes .cbor";
        // 0x01 is the canonical CBOR encoding of the integer 1.
        assert!(encode(src, JsonValue::String("0x01".into())).is_ok());
        // 0xa1 opens a 1-entry map but supplies no key/value bytes.
        assert!(encode(src, JsonValue::String("0xa1".into())).is_err());
    }

    #[test]
    fn cbor_rejects_non_bytes_value() {
        let src = "TestCase = text .cbor";
        assert!(encode(src, JsonValue::from("0x01")).is_err());
    }

    #[test]
    fn cborseq_accepts_concatenated_cbor_items_and_rejects_malformed() {
        let src = "TestCase = bytes .cborseq";
        // 0x01 (int 1) followed by 0x02 (int 2): two concatenated items.
        assert!(encode(src, JsonValue::String("0x0102".into())).is_ok());
        // A single-item sequence is fine too.
        assert!(encode(src, JsonValue::String("0x01".into())).is_ok());
        // Zero items (empty byte string) is also a valid sequence.
        assert!(encode(src, JsonValue::String("0x".into())).is_ok());
        // 0xa1 opens a 1-entry map but supplies no key/value bytes.
        assert!(encode(src, JsonValue::String("0xa1".into())).is_err());
        // A well-formed item (0x01) followed by a dangling, incomplete one.
        assert!(encode(src, JsonValue::String("0x01a1".into())).is_err());
    }

    #[test]
    fn cborseq_rejects_non_bytes_value() {
        let src = "TestCase = int .cborseq";
        assert!(encode(src, JsonValue::from(5)).is_err());
    }

    #[test]
    fn default_constraint_is_not_enforced_as_a_value_check() {
        // `.default` only supplies a fallback value elsewhere; by itself it
        // shouldn't reject an explicitly provided value that differs from it.
        let src = r#"TestCase = text .default "anonymous""#;
        assert!(encode(src, JsonValue::String("someone-else".into())).is_ok());
    }
}
