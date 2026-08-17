use std::fs;
use std::path::PathBuf;

use csilctl::list::run_list;
use tempfile::TempDir;

const FIXTURE: &str = r#"
Task = {
	id: text,
	? label: text
}
StringInt64Map = {* text => int}
CreateRequest = { name: text }
CreateResponse = { task: Task }
ErrorType = { message: text }

service Widgets {
	Create: CreateRequest -> CreateResponse / ErrorType
}
"#;

fn write_fixture(dir: &TempDir, src: &str) -> PathBuf {
    let path = dir.path().join("fixture.csil");
    fs::write(&path, src).expect("write fixture");
    path
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut in_escape = false;
    for c in s.chars() {
        if in_escape {
            if c == 'm' {
                in_escape = false;
            }
            continue;
        }
        if c == '\x1b' {
            in_escape = true;
            continue;
        }
        out.push(c);
    }
    out
}

#[test]
fn basic_listing_groups_by_service_and_lists_other_types() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_fixture(&dir, FIXTURE);
    let out = strip_ansi(&run_list(path.to_str().unwrap(), None, false).unwrap());

    assert!(out.contains("Services:"));
    assert!(out.contains("Widgets"));
    assert!(out.contains("Create"));
    assert!(out.contains("Types:"));
    assert!(out.contains("Task"));
    assert!(out.contains("StringInt64Map"));
    // *Request/*Response types are implied by the method names, not listed
    // separately.
    assert!(!out.contains("CreateRequest"));
    assert!(!out.contains("CreateResponse"));
    // Types that don't end in Request/Response (even error types) still show.
    assert!(out.contains("ErrorType"));
}

#[test]
fn verbose_listing_expands_every_message_and_type() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_fixture(&dir, FIXTURE);
    let out = strip_ansi(&run_list(path.to_str().unwrap(), None, true).unwrap());

    assert!(out.contains("request"));
    assert!(out.contains("response"));
    assert!(out.contains("error"));
    assert!(out.contains("name"));
    assert!(out.contains("message"));
    // Map alias regression: fields/items typed as a map alias must show
    // the expanded `{* key => value}` form, not just the alias name.
    assert!(out.contains("{* text => int}"));
    // Optional fields are marked with a trailing `?`.
    assert!(out.contains("label?"));
}

#[test]
fn single_method_item_prints_under_its_service() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_fixture(&dir, FIXTURE);
    let out = strip_ansi(&run_list(path.to_str().unwrap(), Some("Create"), false).unwrap());

    assert!(out.contains("Widgets"));
    assert!(out.contains("request"));
    assert!(out.contains("name"));
    assert!(out.contains("response"));
    assert!(out.contains("task"));
    assert!(out.contains("error"));
    assert!(out.contains("message"));
}

#[test]
fn single_type_item_expands_map_alias() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_fixture(&dir, FIXTURE);
    let out = strip_ansi(&run_list(path.to_str().unwrap(), Some("StringInt64Map"), false).unwrap());

    assert!(out.contains("StringInt64Map"));
    assert!(out.contains("{* text => int}"));
}

#[test]
fn verbose_is_ignored_when_an_item_is_given() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_fixture(&dir, FIXTURE);
    let with_verbose = run_list(path.to_str().unwrap(), Some("Task"), true).unwrap();
    let without_verbose = run_list(path.to_str().unwrap(), Some("Task"), false).unwrap();
    assert_eq!(with_verbose, without_verbose);
}

#[test]
fn no_service_blocks_lists_types_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_fixture(&dir, "Task = { id: text }");
    let out = strip_ansi(&run_list(path.to_str().unwrap(), None, false).unwrap());
    assert!(!out.contains("Services:"));
    assert!(out.contains("Types:"));
    assert!(out.contains("Task"));
}

#[test]
fn error_unknown_item() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_fixture(&dir, FIXTURE);
    let err = run_list(path.to_str().unwrap(), Some("DoesNotExist"), false).unwrap_err();
    assert!(err.to_string().contains("no method or type named"));
}

#[test]
fn error_missing_file() {
    let err = run_list("/nonexistent/path/does-not-exist.csil", None, false).unwrap_err();
    assert!(format!("{err:?}").to_lowercase().contains("no such file"));
}

#[test]
fn error_malformed_csil_reports_parse_failure() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_fixture(&dir, "Task = {");
    let err = run_list(path.to_str().unwrap(), None, false).unwrap_err();
    assert!(err.to_string().contains("list: parsing"));
}
