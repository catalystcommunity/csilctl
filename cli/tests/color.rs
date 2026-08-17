//! End-to-end coverage for the NO_COLOR / FORCE_COLOR / --disable-color
//! priority, run against the real binary. `color::init` latches a
//! process-global `OnceLock`, so each combination needs its own process
//! rather than being exercised as library calls within one test binary.

use std::fs;
use std::process::Command;

const FIXTURE: &str = r#"
CreateRequest = { name: text }
CreateResponse = { ok: bool }
ErrorType = { message: text }

service Widgets {
	Create: CreateRequest -> CreateResponse / ErrorType
}
"#;

fn run(args: &[&str], envs: &[(&str, &str)]) -> String {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.csil");
    fs::write(&path, FIXTURE).unwrap();

    let mut command = Command::new(env!("CARGO_BIN_EXE_csilctl"));
    command
        .arg("--client")
        .arg(&path)
        .args(args)
        .env_remove("NO_COLOR")
        .env_remove("FORCE_COLOR")
        .envs(envs.iter().copied());
    let output = command.output().expect("run csilctl");
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

fn has_ansi(s: &str) -> bool {
    s.contains('\x1b')
}

#[test]
fn colorized_by_default() {
    let out = run(&["list"], &[]);
    assert!(has_ansi(&out));
}

#[test]
fn disable_color_flag_strips_ansi() {
    let out = run(&["--disable-color", "list"], &[]);
    assert!(!has_ansi(&out));
}

#[test]
fn no_color_env_strips_ansi_without_flag() {
    let out = run(&["list"], &[("NO_COLOR", "1")]);
    assert!(!has_ansi(&out));
}

#[test]
fn force_color_env_overrides_disable_color_flag() {
    let out = run(&["--disable-color", "list"], &[("FORCE_COLOR", "1")]);
    assert!(has_ansi(&out));
}

#[test]
fn no_color_env_overrides_force_color_env() {
    let out = run(&["list"], &[("NO_COLOR", "1"), ("FORCE_COLOR", "1")]);
    assert!(!has_ansi(&out));
}

#[test]
fn no_color_env_overrides_force_color_and_disable_color_flag() {
    let out = run(
        &["--disable-color", "list"],
        &[("NO_COLOR", "1"), ("FORCE_COLOR", "1")],
    );
    assert!(!has_ansi(&out));
}
