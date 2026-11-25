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

#[test]
fn send_matches_the_csilgen_reading_codec() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/readings.csil");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut server = RpcServer::new(StreamCarrier::new(stream));
        server
            .serve_one(&mut |req| {
                // EncodeReading from the csilgen 0.3.3 Go generator produced these bytes.
                let expected = [
                    &b"\xa5\x62at\xc0\x742024-02-29T03:04:05Z"[..],
                    b"\x63day\xd9\x03\xec\x6a2024-02-29",
                    b"\x64mode\x18\x2a",
                    b"\x64seen\xc1\xfb\xbf\xe0\x00\x00\x00\x00\x00\x00",
                    b"\x65state\x22",
                ]
                .concat();
                assert_eq!(req.payload, expected);
                HandlerOutcome::Reply {
                    variant: "Reading".into(),
                    payload: expected,
                }
            })
            .unwrap();
    });

    let json = r#"{"state":"busy","mode":42,"day":"2024-02-29","at":"2024-02-29T04:04:05+01:00","seen":-0.5}"#;
    let out = run_send(path, "Report", Some(json), &addr.to_string()).unwrap();
    server.join().unwrap();
    assert!(out.contains("2024-02-29T03:04:05Z"));
}
