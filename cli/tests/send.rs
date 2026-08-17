use std::fs;
use std::net::TcpListener;
use std::thread;

use csilctl::send::run_send;
use csilgen_transport::carrier::StreamCarrier;
use csilgen_transport::conventions::{decode_value, encode_value};
use csilgen_transport::rpc::{HandlerOutcome, RpcServer};
use ciborium::value::Value as CborValue;

const FIXTURE: &str = r#"
EchoRequest = { message: text }
EchoResponse = { message: text }

service Echo {
	Say: EchoRequest -> EchoResponse
}
"#;

#[test]
fn send_round_trips_a_message_over_tcp() {
    let dir = tempfile::tempdir().unwrap();
    let csil_path = dir.path().join("echo.csil");
    fs::write(&csil_path, FIXTURE).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let carrier = StreamCarrier::new(stream);
        let mut server = RpcServer::new(carrier);
        server
            .serve_one(&mut |req| {
                let value = decode_value(&req.payload).unwrap();
                let message = match &value {
                    CborValue::Map(entries) => entries
                        .iter()
                        .find_map(|(k, v)| match (k, v) {
                            (CborValue::Text(k), CborValue::Text(v)) if k == "message" => {
                                Some(v.clone())
                            }
                            _ => None,
                        })
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                let response = CborValue::Map(vec![(
                    CborValue::Text("message".to_string()),
                    CborValue::Text(format!("echo: {message}")),
                )]);
                HandlerOutcome::Reply {
                    variant: "EchoResponse".to_string(),
                    payload: encode_value(&response).unwrap(),
                }
            })
            .unwrap();
    });

    let out = run_send(
        csil_path.to_str().unwrap(),
        "Say",
        Some(r#"{"message": "hi"}"#),
        &addr.to_string(),
    )
    .unwrap();

    server.join().unwrap();

    assert!(out.contains("EchoResponse"));
    assert!(out.contains("echo: hi"));
}

#[test]
fn send_prompts_instead_of_failing_on_missing_required_field() {
    let dir = tempfile::tempdir().unwrap();
    let csil_path = dir.path().join("echo.csil");
    fs::write(&csil_path, FIXTURE).unwrap();

    // With `message` missing from `--data`, run_send now prompts for it on
    // stdin (which is empty/EOF in this test process, yielding "") rather
    // than immediately failing with "missing required field" — the request
    // gets built successfully and the run only fails later, at connect time.
    let err = run_send(csil_path.to_str().unwrap(), "Say", Some("{}"), "127.0.0.1:1").unwrap_err();
    assert!(err.to_string().contains("connecting to 127.0.0.1:1"));
    assert!(!format!("{err:?}").contains("missing required field"));
}

#[test]
fn send_reports_unknown_message() {
    let dir = tempfile::tempdir().unwrap();
    let csil_path = dir.path().join("echo.csil");
    fs::write(&csil_path, FIXTURE).unwrap();

    let err = run_send(csil_path.to_str().unwrap(), "DoesNotExist", None, "127.0.0.1:1")
        .unwrap_err();
    assert!(err.to_string().contains("no method named"));
}
