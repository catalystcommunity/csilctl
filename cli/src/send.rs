//! `send`: 1) converts the user-supplied `--data` JSON into the
//! operation's request payload, 2) sends it over CSIL-RPC and
//! prints the decoded response as JSON.


use std::net::TcpStream;

use anyhow::{Context, Result};
use csilgen_transport::carrier::StreamCarrier;
use csilgen_transport::conventions::decode_value;
use csilgen_transport::rpc::RpcClient;

use csilgen_core::TypeExpression;

use crate::color::{cyan, green};
use crate::list::File;
use crate::payload::{cbor_to_json, json_to_cbor, write_payload};
use crate::prompt::prompt_for_request;

/// Builds the request from JSON and prompted fields, sends it to the host,
/// and returns the decoded response as JSON.
pub fn run_send(csil_path: &str, message: &str, data: Option<&str>, host: &str) -> Result<String> {
    let f = File::load(csil_path)?;
    let (svc_name, op) = f
        .find_operation(message)
        .ok_or_else(|| anyhow::anyhow!("send: no method named {message:?} found in {csil_path}"))?;

    let data_json: serde_json::Value = match data {
        Some(s) => {
            let parsed: serde_json::Value =
                serde_json::from_str(s).with_context(|| "send: --data is not valid JSON")?;
            // Push-only ops (a leading direction arrow, no declared input)
            // have nothing to prompt for.
            if matches!(&op.input_type, TypeExpression::Builtin(b) if b == "null") {
                parsed
            } else {
                println!("Sending {} to {}", cyan(message), green(host));
                prompt_for_request(&op.input_type, &f, Some(&parsed))
                    .with_context(|| format!("send: prompting for {message}'s request fields"))?
            }
        }
        None if matches!(&op.input_type, TypeExpression::Builtin(b) if b == "null") => {
            serde_json::Value::Null
        }
        None => {
            println!("Sending {} to {}", cyan(message), green(host));
            prompt_for_request(&op.input_type, &f, None)
                .with_context(|| format!("send: prompting for {message}'s request fields"))?
        }
    };

    let request_cbor = json_to_cbor(&data_json, &op.input_type, &f)
        .with_context(|| format!("send: building request for {message}"))?;
    let mut payload = Vec::new();
    write_payload(&request_cbor, &mut payload)?;

    let stream = TcpStream::connect(host).with_context(|| format!("send: connecting to {host}"))?;
    let carrier = StreamCarrier::new(stream);
    let mut client = RpcClient::new(carrier, false);

    let response = client
        .call(svc_name, &op.name, payload, None)
        .map_err(|e| anyhow::anyhow!(e.to_string()))
        .with_context(|| format!("send: calling {svc_name}/{}", op.name))?;

    let response_value = decode_value(&response.payload).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let response_json = cbor_to_json(&response_value);

    let mut out = String::new();
    if let Some(variant) = &response.variant {
        out.push_str(&format!("response type: {}\n", cyan(variant)));
    }
    out.push_str(&green(&serde_json::to_string_pretty(&response_json)?));
    out.push('\n');
    Ok(out)
}
