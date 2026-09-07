use std::process::Command;

fn assert_version_flag(flag: &str) {
    let output = Command::new(env!("CARGO_BIN_EXE_csilctl"))
        .arg(flag)
        .output()
        .expect("run csilctl");

    assert!(output.status.success(), "{output:?}");
    let expected = format!("csilctl {}\n", env!("CSILCTL_VERSION"));
    assert_eq!(output.stdout.as_slice(), expected.as_bytes(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
}

#[test]
fn long_version_flag_prints_the_build_version() {
    assert_version_flag("--version");
}

#[test]
fn short_version_flag_prints_the_build_version() {
    assert_version_flag("-V");
}
