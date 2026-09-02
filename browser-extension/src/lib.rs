use csilgen_schema::{
    ByteString, Diagnostic, DiagnosticResult, DiagnosticValue, FORMAT_NAME, FORMAT_VERSION,
    FloatWidth, MessageDirection, PayloadSide, RouteContext, SchemaBody, SchemaDescriptor,
    SpannedValue, TypedValue, unmarshal, unmarshal_descriptor,
};
use js_sys::{Array, BigInt, Object, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

/// Decode one CBOR item without a schema. The result preserves CBOR types and
/// byte offsets. Malformed input is returned as a diagnostic result.
#[wasm_bindgen]
pub fn decode_cbor(payload: &[u8]) -> Result<JsValue, JsValue> {
    let descriptor = empty_descriptor();
    let route = RouteContext::RpcRequest {
        service: "__generic__".to_string(),
        operation: "__generic__".to_string(),
        direction: MessageDirection::Received,
    };
    let mut result = unmarshal(&descriptor, &route, payload);
    result
        .diagnostics
        .retain(|diagnostic| diagnostic.schema_path != "$route");
    diagnostic_result_to_js(&result)
}

#[wasm_bindgen]
pub fn inspect_rpc_request(
    descriptor: &[u8],
    service: String,
    operation: String,
    sent: bool,
    payload: &[u8],
) -> Result<JsValue, JsValue> {
    inspect(
        descriptor,
        RouteContext::RpcRequest {
            service,
            operation,
            direction: message_direction(sent),
        },
        payload,
    )
}

#[wasm_bindgen]
pub fn inspect_rpc_response(
    descriptor: &[u8],
    service: String,
    operation: String,
    variant: Option<String>,
    sent: bool,
    payload: &[u8],
) -> Result<JsValue, JsValue> {
    inspect(
        descriptor,
        RouteContext::RpcResponse {
            service,
            operation,
            variant,
            direction: message_direction(sent),
        },
        payload,
    )
}

#[wasm_bindgen]
pub fn inspect_event_verbose(
    descriptor: &[u8],
    service: Option<String>,
    operation: String,
    output: bool,
    sent: bool,
    payload: &[u8],
) -> Result<JsValue, JsValue> {
    inspect(
        descriptor,
        RouteContext::EventVerbose {
            service,
            operation,
            payload_side: payload_side(output),
            direction: message_direction(sent),
        },
        payload,
    )
}

#[wasm_bindgen]
pub fn inspect_event_compact(
    descriptor: &[u8],
    service_wire_id: u64,
    operation_wire_id: u64,
    output: bool,
    sent: bool,
    payload: &[u8],
) -> Result<JsValue, JsValue> {
    inspect(
        descriptor,
        RouteContext::EventCompact {
            service_wire_id,
            operation_wire_id,
            payload_side: payload_side(output),
            direction: message_direction(sent),
        },
        payload,
    )
}

/// Decode and verify a descriptor. This function returns the values needed by
/// schema discovery and selection without returning the complete type graph.
#[wasm_bindgen]
pub fn descriptor_info(descriptor: &[u8]) -> Result<JsValue, JsValue> {
    let descriptor = SchemaDescriptor::decode(descriptor).map_err(js_error)?;
    let result = Object::new();
    set(&result, "format", JsValue::from_str(&descriptor.format))?;
    set(&result, "version", JsValue::from_str(&descriptor.version))?;
    set(&result, "root", JsValue::from_str(&descriptor.body.root))?;
    set(
        &result,
        "digestHex",
        JsValue::from_str(&hex(&descriptor.digest.0)),
    )?;

    let services = Array::new();
    for service in &descriptor.body.services {
        let service_value = Object::new();
        set(&service_value, "name", JsValue::from_str(&service.name))?;
        set_optional_bigint(&service_value, "wireId", service.wire_id.map(u128::from))?;
        let operations = Array::new();
        for operation in &service.operations {
            let operation_value = Object::new();
            set(&operation_value, "name", JsValue::from_str(&operation.name))?;
            set_optional_bigint(
                &operation_value,
                "wireId",
                operation.wire_id.map(u128::from),
            )?;
            set(
                &operation_value,
                "direction",
                JsValue::from_str(&format!("{:?}", operation.direction)),
            )?;
            operations.push(&operation_value);
        }
        set(&service_value, "operations", operations.into())?;
        services.push(&service_value);
    }
    set(&result, "services", services.into())?;
    Ok(result.into())
}

fn inspect(descriptor: &[u8], route: RouteContext, payload: &[u8]) -> Result<JsValue, JsValue> {
    let result = unmarshal_descriptor(descriptor, &route, payload).map_err(js_error)?;
    diagnostic_result_to_js(&result)
}

fn empty_descriptor() -> SchemaDescriptor {
    SchemaDescriptor {
        format: FORMAT_NAME.to_string(),
        version: FORMAT_VERSION.to_string(),
        digest: ByteString(Vec::new()),
        body: SchemaBody {
            root: "__generic__".to_string(),
            rules: Vec::new(),
            services: Vec::new(),
        },
    }
}

fn message_direction(sent: bool) -> MessageDirection {
    if sent {
        MessageDirection::Sent
    } else {
        MessageDirection::Received
    }
}

fn payload_side(output: bool) -> PayloadSide {
    if output {
        PayloadSide::Output
    } else {
        PayloadSide::Input
    }
}

fn diagnostic_result_to_js(result: &DiagnosticResult) -> Result<JsValue, JsValue> {
    let value = Object::new();
    set(
        &value,
        "raw",
        Uint8Array::from(result.raw_payload.as_slice()).into(),
    )?;
    set_optional(
        &value,
        "generic",
        result
            .generic_value
            .as_ref()
            .map(spanned_value_to_js)
            .transpose()?,
    )?;
    set_optional(
        &value,
        "typed",
        result
            .typed_value
            .as_ref()
            .map(typed_value_to_js)
            .transpose()?,
    )?;

    let diagnostics = Array::new();
    for diagnostic in &result.diagnostics {
        diagnostics.push(&diagnostic_to_js(diagnostic)?);
    }
    set(&value, "diagnostics", diagnostics.into())?;

    let route = result.route.as_ref().map(|route| {
        let value = Object::new();
        set(&value, "service", JsValue::from_str(&route.service))?;
        set(&value, "operation", JsValue::from_str(&route.operation))?;
        set_optional_bigint(
            &value,
            "serviceWireId",
            route.service_wire_id.map(u128::from),
        )?;
        set_optional_bigint(
            &value,
            "operationWireId",
            route.operation_wire_id.map(u128::from),
        )?;
        set(
            &value,
            "payloadSide",
            JsValue::from_str(&format!("{:?}", route.payload_side)),
        )?;
        set(
            &value,
            "direction",
            JsValue::from_str(&format!("{:?}", route.direction)),
        )?;
        if let Some(choice) = &route.choice_arm {
            set(&value, "choiceArm", JsValue::from_f64(choice.index as f64))?;
        } else {
            set(&value, "choiceArm", JsValue::NULL)?;
        }
        Ok::<JsValue, JsValue>(value.into())
    });
    set_optional(&value, "route", route.transpose()?)?;
    Ok(value.into())
}

fn diagnostic_to_js(diagnostic: &Diagnostic) -> Result<JsValue, JsValue> {
    let value = Object::new();
    set(&value, "message", JsValue::from_str(&diagnostic.message))?;
    set(
        &value,
        "schemaPath",
        JsValue::from_str(&diagnostic.schema_path),
    )?;
    set_optional_number(&value, "offset", diagnostic.offset)?;
    set_optional_string(&value, "expected", diagnostic.expected.as_deref())?;
    set_optional_string(&value, "observed", diagnostic.observed.as_deref())?;
    Ok(value.into())
}

fn spanned_value_to_js(value: &SpannedValue) -> Result<JsValue, JsValue> {
    let result = Object::new();
    set(&result, "offset", JsValue::from_f64(value.offset as f64))?;
    set(
        &result,
        "endOffset",
        JsValue::from_f64(value.end_offset as f64),
    )?;

    match &value.value {
        DiagnosticValue::Integer(integer) => {
            set(&result, "kind", JsValue::from_str("integer"))?;
            set(&result, "value", bigint_i128(*integer)?)?;
        }
        DiagnosticValue::Float(float) => {
            set(&result, "kind", JsValue::from_str("float"))?;
            set(
                &result,
                "width",
                JsValue::from_f64(match float.width {
                    FloatWidth::Sixteen => 16.0,
                    FloatWidth::ThirtyTwo => 32.0,
                    FloatWidth::SixtyFour => 64.0,
                }),
            )?;
            set(&result, "bits", bigint_u128(u128::from(float.bits))?)?;
            set(&result, "value", JsValue::from_f64(float.as_f64()))?;
        }
        DiagnosticValue::Text(text) => {
            set(&result, "kind", JsValue::from_str("text"))?;
            set(&result, "value", JsValue::from_str(text))?;
        }
        DiagnosticValue::Bytes(bytes) => {
            set(&result, "kind", JsValue::from_str("bytes"))?;
            set(&result, "value", Uint8Array::from(bytes.as_slice()).into())?;
        }
        DiagnosticValue::Bool(boolean) => {
            set(&result, "kind", JsValue::from_str("boolean"))?;
            set(&result, "value", JsValue::from_bool(*boolean))?;
        }
        DiagnosticValue::Null => set(&result, "kind", JsValue::from_str("null"))?,
        DiagnosticValue::Undefined => set(&result, "kind", JsValue::from_str("undefined"))?,
        DiagnosticValue::Simple(simple) => {
            set(&result, "kind", JsValue::from_str("simple"))?;
            set(&result, "value", JsValue::from_f64(f64::from(*simple)))?;
        }
        DiagnosticValue::Array(items) => {
            set(&result, "kind", JsValue::from_str("array"))?;
            let values = Array::new();
            for item in items {
                values.push(&spanned_value_to_js(item)?);
            }
            set(&result, "items", values.into())?;
        }
        DiagnosticValue::Map(entries) => {
            set(&result, "kind", JsValue::from_str("map"))?;
            let values = Array::new();
            for (key, entry_value) in entries {
                let entry = Object::new();
                set(&entry, "key", spanned_value_to_js(key)?)?;
                set(&entry, "value", spanned_value_to_js(entry_value)?)?;
                values.push(&entry);
            }
            set(&result, "entries", values.into())?;
        }
        DiagnosticValue::Tag { tag, value } => {
            set(&result, "kind", JsValue::from_str("tag"))?;
            set(&result, "tag", bigint_u128(u128::from(*tag))?)?;
            set(&result, "value", spanned_value_to_js(value)?)?;
        }
        DiagnosticValue::Decimal { exponent, mantissa } => {
            set(&result, "kind", JsValue::from_str("decimal"))?;
            set(&result, "exponent", bigint_i128(*exponent)?)?;
            set(&result, "mantissa", bigint_i128(*mantissa)?)?;
        }
        DiagnosticValue::Timestamp {
            original_tag,
            value,
        } => {
            set(&result, "kind", JsValue::from_str("timestamp"))?;
            set(
                &result,
                "originalTag",
                bigint_u128(u128::from(*original_tag))?,
            )?;
            set(&result, "value", spanned_value_to_js(value)?)?;
        }
    }
    Ok(result.into())
}

fn typed_value_to_js(value: &TypedValue) -> Result<JsValue, JsValue> {
    let result = Object::new();
    match value {
        TypedValue::Value(value) => {
            set(&result, "kind", JsValue::from_str("value"))?;
            set(&result, "value", spanned_value_to_js(value)?)?;
        }
        TypedValue::Array(items) | TypedValue::Tuple(items) => {
            set(
                &result,
                "kind",
                JsValue::from_str(if matches!(value, TypedValue::Array(_)) {
                    "array"
                } else {
                    "tuple"
                }),
            )?;
            let values = Array::new();
            for item in items {
                values.push(&typed_value_to_js(item)?);
            }
            set(&result, "items", values.into())?;
        }
        TypedValue::Map(entries) => {
            set(&result, "kind", JsValue::from_str("map"))?;
            let values = Array::new();
            for (key, entry_value) in entries {
                let entry = Object::new();
                set(&entry, "key", typed_value_to_js(key)?)?;
                set(&entry, "value", typed_value_to_js(entry_value)?)?;
                values.push(&entry);
            }
            set(&result, "entries", values.into())?;
        }
        TypedValue::Record {
            fields,
            unknown_fields,
        } => {
            set(&result, "kind", JsValue::from_str("record"))?;
            let field_values = Array::new();
            for field in fields {
                let field_value = Object::new();
                set_optional_string(&field_value, "name", field.name.as_deref())?;
                set(&field_value, "key", spanned_value_to_js(&field.key)?)?;
                set(&field_value, "value", typed_value_to_js(&field.value)?)?;
                field_values.push(&field_value);
            }
            set(&result, "fields", field_values.into())?;

            let unknown_values = Array::new();
            for (key, entry_value) in unknown_fields {
                let entry = Object::new();
                set(&entry, "key", spanned_value_to_js(key)?)?;
                set(&entry, "value", spanned_value_to_js(entry_value)?)?;
                unknown_values.push(&entry);
            }
            set(&result, "unknownFields", unknown_values.into())?;
        }
        TypedValue::Choice {
            arm_index,
            declared_arm,
            value,
        } => {
            set(&result, "kind", JsValue::from_str("choice"))?;
            set(&result, "armIndex", JsValue::from_f64(*arm_index as f64))?;
            set(
                &result,
                "declaredArm",
                JsValue::from_str(&format!("{declared_arm:?}")),
            )?;
            set(&result, "value", typed_value_to_js(value)?)?;
        }
    }
    Ok(result.into())
}

fn set(target: &Object, name: &str, value: JsValue) -> Result<(), JsValue> {
    Reflect::set(target, &JsValue::from_str(name), &value).map(|_| ())
}

fn set_optional(target: &Object, name: &str, value: Option<JsValue>) -> Result<(), JsValue> {
    set(target, name, value.unwrap_or(JsValue::NULL))
}

fn set_optional_string(target: &Object, name: &str, value: Option<&str>) -> Result<(), JsValue> {
    set_optional(target, name, value.map(JsValue::from_str))
}

fn set_optional_number(target: &Object, name: &str, value: Option<usize>) -> Result<(), JsValue> {
    set_optional(
        target,
        name,
        value.map(|value| JsValue::from_f64(value as f64)),
    )
}

fn set_optional_bigint(target: &Object, name: &str, value: Option<u128>) -> Result<(), JsValue> {
    set_optional(target, name, value.map(bigint_u128).transpose()?)
}

fn bigint_i128(value: i128) -> Result<JsValue, JsValue> {
    BigInt::new(&JsValue::from_str(&value.to_string()))
        .map(Into::into)
        .map_err(Into::into)
}

fn bigint_u128(value: u128) -> Result<JsValue, JsValue> {
    BigInt::new(&JsValue::from_str(&value.to_string()))
        .map(Into::into)
        .map_err(Into::into)
}

fn js_error(error: impl std::fmt::Display) -> JsValue {
    js_sys::Error::new(&error.to_string()).into()
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}
