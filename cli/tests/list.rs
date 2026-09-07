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
    // Types: lists every named type -- nothing is hidden just because an
    // operation also declares it as a request/response/error, since that
    // type may be meaningful on its own (e.g. reused elsewhere).
    assert!(out.contains("CreateRequest"));
    assert!(out.contains("CreateResponse"));
    assert!(out.contains("ErrorType"));
}

#[test]
fn verbose_listing_expands_every_message_and_type() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_fixture(&dir, FIXTURE);
    let out = strip_ansi(&run_list(path.to_str().unwrap(), None, true).unwrap());

    assert!(out.contains("request"));
    // request/response show the declared type's own name plus its contents
    // -- there's no separate "error" section anymore, since a union's arms
    // aren't reliably distinguishable as success vs. error.
    assert!(out.contains("CreateRequest"));
    assert!(out.contains("response"));
    assert!(out.contains("CreateResponse / ErrorType"));
    assert!(!out.contains("error"));
    // "name" (CreateRequest's field), "task" (CreateResponse's), and
    // "message" (ErrorType's) all show as contents nested under the
    // relevant section.
    assert!(out.contains("name"));
    assert!(out.contains("task"));
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
    assert!(out.contains("CreateRequest"));
    assert!(out.contains("response"));
    assert!(out.contains("CreateResponse / ErrorType"));
    assert!(!out.contains("error"));
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

const UNION_REQUEST_FIXTURE: &str = r#"
FooRequest = { foo: text }
BarRequest = { bar: text }
ItemRequest = FooRequest / BarRequest
ItemResponse = { items: [text] }

service Items {
	Item: ItemRequest -> ItemResponse
}
"#;

#[test]
fn basic_listing_shows_the_request_alias_and_its_union_members() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_fixture(&dir, UNION_REQUEST_FIXTURE);
    let out = strip_ansi(&run_list(path.to_str().unwrap(), None, false).unwrap());

    // Types: lists every named type -- the alias itself, and its union
    // members, even though the members' names also end in "Request".
    assert!(out.contains("ItemRequest"));
    assert!(out.contains("ItemResponse"));
    assert!(out.contains("FooRequest"));
    assert!(out.contains("BarRequest"));
}

#[test]
fn verbose_listing_shows_request_name_and_declared_union_arms() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_fixture(&dir, UNION_REQUEST_FIXTURE);
    let out = strip_ansi(&run_list(path.to_str().unwrap(), None, true).unwrap());

    // The operation's request line shows the alias's own top-level name,
    // then just the arm names it declares joined by "/" -- not each arm's
    // own fields, since that would just re-describe what the alias already
    // names (its members' full contents are still visible in Types:).
    assert!(out.contains("request"));
    assert!(out.contains("ItemRequest\n        FooRequest / BarRequest\n"));
}

#[test]
fn single_type_item_shows_union_branch_names_and_contents() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_fixture(&dir, UNION_REQUEST_FIXTURE);
    let out =
        strip_ansi(&run_list(path.to_str().unwrap(), Some("ItemRequest"), false).unwrap());

    // The alias's own name, then each branch's name and contents nested
    // beneath it.
    assert!(out.contains("ItemRequest"));
    assert!(out.contains("FooRequest"));
    assert!(out.contains("foo"));
    assert!(out.contains("BarRequest"));
    assert!(out.contains("bar"));
}

#[test]
fn single_method_item_shows_request_name_and_declared_union_arms() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_fixture(&dir, UNION_REQUEST_FIXTURE);
    let out = strip_ansi(&run_list(path.to_str().unwrap(), Some("Item"), false).unwrap());

    assert!(out.contains("request"));
    assert!(out.contains("ItemRequest\n        FooRequest / BarRequest\n"));
    // Not each arm's own fields -- those belong to the alias's own listing.
    assert!(!out.contains("foo"));
    assert!(!out.contains("bar"));
}

#[test]
fn single_method_item_expands_an_anonymous_union_request_fully() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = r#"
FooRequest = { id: text, key: text }
BarRequest = { key: text }
ItemResponse = { items: [text] }

service Items {
	Item: FooRequest / BarRequest -> ItemResponse
}
"#;
    let path = write_fixture(&dir, fixture);
    let out = strip_ansi(&run_list(path.to_str().unwrap(), Some("Item"), false).unwrap());

    // An anonymous union (no alias of its own) gets the full breakdown:
    // arms joined, then each arm's own name and contents.
    let joined = "FooRequest / BarRequest";
    let joined_idx = out.find(joined).unwrap();
    let foo_header_idx = out[joined_idx + joined.len()..].find("FooRequest").unwrap();
    assert!(out.contains("id"));
    assert!(out.contains("key"));
    assert!(foo_header_idx > 0);
}

#[test]
fn nested_union_alias_field_shows_bare_name_and_branches_appear_once_in_types() {
    // A field typed as a union alias (e.g. `[SomeUnionAlias]`) keeps
    // printing the bare alias name -- no special-casing for arrays/nested
    // occurrences -- and its branch types simply appear in the Types:
    // listing like any other named type, exactly once.
    let dir = tempfile::tempdir().unwrap();
    let fixture = r#"
FooRequest = { foo: text }
BarRequest = { bar: text }
ItemRequest = FooRequest / BarRequest
Wrapper = { items: [ItemRequest] }
"#;
    let path = write_fixture(&dir, fixture);
    let out = strip_ansi(&run_list(path.to_str().unwrap(), None, true).unwrap());

    assert!(out.contains("[ItemRequest]"));
    // Each branch gets its own Types: entry (a header line, unindented)
    // exactly once -- it also appears inline (indented) in ItemRequest's
    // own "A / B" line, so a header-line search (not a raw substring
    // count) confirms there's no duplicate full block.
    assert_eq!(out.matches("\nFooRequest\n").count(), 1);
    assert_eq!(out.matches("\nBarRequest\n").count(), 1);
    assert!(out.contains("foo"));
    assert!(out.contains("bar"));
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
fn literal_union_does_not_repeat_arms_as_their_own_blocks() {
    // Regression test: a union of literals (e.g. `Direction = "north" /
    // "south"`) already says everything on the joined "A / B" line -- it
    // shouldn't also print each literal again as its own bogus
    // name+contents block.
    let dir = tempfile::tempdir().unwrap();
    let fixture = r#"Direction = "north" / "south""#;
    let path = write_fixture(&dir, fixture);
    let out = strip_ansi(&run_list(path.to_str().unwrap(), Some("Direction"), false).unwrap());

    assert_eq!(out, "Direction\n  \"north\" / \"south\"\n");
}

#[test]
fn mixed_union_still_expands_the_non_literal_arm() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = r#"
FooRequest = { foo: text }
MixedUnion = FooRequest / "literal"
"#;
    let path = write_fixture(&dir, fixture);
    let out = strip_ansi(&run_list(path.to_str().unwrap(), Some("MixedUnion"), false).unwrap());

    assert_eq!(
        out,
        "MixedUnion\n  FooRequest / \"literal\"\n  FooRequest\n    foo text\n"
    );
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
