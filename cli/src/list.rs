use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result, bail};
use csilgen_core::{
    ControlOperator, CsilSpec, GroupEntry, GroupExpression, GroupKey, ImportResolver,
    LiteralValue, Occurrence, RuleType, ServiceDirection, ServiceOperation, SizeConstraint,
    TypeExpression, parse_csil_file,
};

use crate::color::{bold, cyan, field_line, green};

/// A parsed `.csil` file plus the indexes needed to resolve/render its types.
pub struct File {
    spec: CsilSpec,
    /// Definition names (`TypeDef`/`GroupDef` only) in source order.
    order: Vec<String>,
    /// Definition name -> index into `spec.rules`.
    by_name: HashMap<String, usize>,
}

impl File {
    pub fn load(csil_path: &str) -> Result<File> {
        let mut spec = parse_csil_file(csil_path)
            .with_context(|| format!("list: parsing {csil_path}"))?;

        let mut resolver = ImportResolver::new();
        if let Some(parent) = Path::new(csil_path).parent() {
            resolver.add_search_path(parent.to_path_buf());
        }
        resolver
            .resolve_imports(&mut spec, Path::new(csil_path))
            .with_context(|| format!("list: parsing {csil_path}"))?;

        let mut order = Vec::new();
        let mut by_name = HashMap::new();
        for (idx, rule) in spec.rules.iter().enumerate() {
            if matches!(rule.rule_type, RuleType::TypeDef(_) | RuleType::GroupDef(_)) {
                order.push(rule.name.clone());
                by_name.insert(rule.name.clone(), idx);
            }
        }

        Ok(File {
            spec,
            order,
            by_name,
        })
    }

    fn services(&self) -> impl Iterator<Item = (&str, &[ServiceOperation])> {
        self.spec.rules.iter().filter_map(|r| match &r.rule_type {
            RuleType::ServiceDef(sd) => Some((r.name.as_str(), sd.operations.as_slice())),
            _ => None,
        })
    }

    pub fn find_operation(&self, name: &str) -> Option<(&str, &ServiceOperation)> {
        for (svc_name, ops) in self.services() {
            if let Some(op) = ops.iter().find(|op| op.name == name) {
                return Some((svc_name, op));
            }
        }
        None
    }
}

/// Follows a chain of `Reference`s through the file's definitions, stopping at
/// a builtin, a structural type, an unresolvable name, or a cycle.
pub(crate) fn resolve(t: &TypeExpression, f: &File) -> TypeExpression {
    let mut seen = HashSet::new();
    let mut current = t.clone();
    loop {
        let name = match &current {
            TypeExpression::Reference(n) => n.clone(),
            _ => return current,
        };
        if !seen.insert(name.clone()) {
            return current;
        }
        let Some(&idx) = f.by_name.get(&name) else {
            return current;
        };
        current = match &f.spec.rules[idx].rule_type {
            RuleType::TypeDef(te) => te.clone(),
            RuleType::GroupDef(ge) => TypeExpression::Group(ge.clone()),
            _ => return current,
        };
    }
}

fn render_literal(lit: &LiteralValue) -> String {
    match lit {
        LiteralValue::Integer(n) => n.to_string(),
        LiteralValue::Float(n) => n.to_string(),
        LiteralValue::Text(s) => format!("{s:?}"),
        LiteralValue::Bytes(b) => format!("{b:?}"),
        LiteralValue::Bool(b) => b.to_string(),
        LiteralValue::Null => "null".to_string(),
        LiteralValue::Array(items) => {
            let rendered: Vec<String> = items.iter().map(render_literal).collect();
            format!("[{}]", rendered.join(", "))
        }
    }
}

fn render_occurrence(occ: &Option<Occurrence>) -> String {
    match occ {
        None => String::new(),
        Some(Occurrence::Optional) => "?".to_string(),
        Some(Occurrence::ZeroOrMore) => "*".to_string(),
        Some(Occurrence::OneOrMore) => "+".to_string(),
        Some(Occurrence::Exact(n)) => n.to_string(),
        Some(Occurrence::Range { min, max }) => match (min, max) {
            (Some(a), Some(b)) => format!("{a}*{b}"),
            (Some(a), None) => format!("{a}*"),
            (None, Some(b)) => format!("*{b}"),
            (None, None) => "*".to_string(),
        },
    }
}

fn render_control_operator(op: &ControlOperator) -> String {
    match op {
        ControlOperator::Size(s) => match s {
            SizeConstraint::Exact(n) => format!(".size({n})"),
            SizeConstraint::Range { min, max } => format!(".size({min}..{max})"),
            SizeConstraint::Min(n) => format!(".size({n}..)"),
            SizeConstraint::Max(n) => format!(".size(..{n})"),
        },
        ControlOperator::Regex(s) => format!(".regex({s})"),
        ControlOperator::Default(v) => format!(".default({})", render_literal(v)),
        ControlOperator::GreaterEqual(v) => format!(".ge({})", render_literal(v)),
        ControlOperator::LessEqual(v) => format!(".le({})", render_literal(v)),
        ControlOperator::GreaterThan(v) => format!(".gt({})", render_literal(v)),
        ControlOperator::LessThan(v) => format!(".lt({})", render_literal(v)),
        ControlOperator::Equal(v) => format!(".eq({})", render_literal(v)),
        ControlOperator::NotEqual(v) => format!(".ne({})", render_literal(v)),
        ControlOperator::Bits(s) => format!(".bits({s})"),
        ControlOperator::And(t) => format!(".and({})", render_type(t)),
        ControlOperator::Within(t) => format!(".within({})", render_type(t)),
        ControlOperator::Json => ".json".to_string(),
        ControlOperator::Cbor => ".cbor".to_string(),
        ControlOperator::Cborseq => ".cborseq".to_string(),
    }
}

pub(crate) fn entry_name(entry: &GroupEntry) -> String {
    match &entry.key {
        Some(GroupKey::Bare(n)) => n.clone(),
        Some(GroupKey::Literal(lit)) => render_literal(lit),
        Some(GroupKey::Type(_)) | None => "*".to_string(),
    }
}

fn render_group(g: &GroupExpression) -> String {
    let names: Vec<String> = g.entries.iter().map(entry_name).collect();
    format!("{{{}}}", names.join(", "))
}

/// Renders t as a compact, CSIL-like type string, including any control
/// operator constraints.
fn render_type(t: &TypeExpression) -> String {
    let s = match t {
        TypeExpression::Builtin(s) => s.clone(),
        TypeExpression::Reference(s) => s.clone(),
        TypeExpression::Literal(lit) => render_literal(lit),
        TypeExpression::Group(g) | TypeExpression::Tuple(g) => render_group(g),
        TypeExpression::Array {
            element_type,
            occurrence,
        } => {
            let occ = render_occurrence(occurrence);
            let occ = if occ.is_empty() {
                String::new()
            } else {
                format!("{occ} ")
            };
            format!("[{occ}{}]", render_type(element_type))
        }
        TypeExpression::Map { key, value, .. } => {
            format!("{{* {} => {}}}", render_type(key), render_type(value))
        }
        TypeExpression::Choice(arms) => {
            let rendered: Vec<String> = arms.iter().map(render_type).collect();
            rendered.join(" / ")
        }
        TypeExpression::Range {
            start,
            end,
            inclusive,
        } => {
            let sep = if *inclusive { "..." } else { ".." };
            format!(
                "{}{sep}{}",
                start.map(|n| n.to_string()).unwrap_or_default(),
                end.map(|n| n.to_string()).unwrap_or_default()
            )
        }
        TypeExpression::Socket(s) => format!("${s}"),
        TypeExpression::Plug(s) => format!("~{s}"),
        TypeExpression::Constrained {
            base_type,
            constraints,
        } => {
            let rendered: Vec<String> = constraints.iter().map(render_control_operator).collect();
            return format!("{} {}", render_type(base_type), rendered.join(" "));
        }
    };
    s
}

/// Resolves t (following named references through f) and renders its fields
/// if it's a group, or a single scalar line otherwise.
fn print_fields(t: &TypeExpression, f: &File, indent: &str) -> String {
    let resolved = resolve(t, f);
    match &resolved {
        TypeExpression::Group(g) | TypeExpression::Tuple(g) => {
            let mut out = String::new();
            for entry in &g.entries {
                let mut name = entry_name(entry);
                if matches!(entry.occurrence, Some(Occurrence::Optional)) {
                    name.push('?');
                }
                out.push_str(&field_line(indent, &name, &render_type(&entry.value_type)));
            }
            out
        }
        _ => format!("{indent}{}\n", green(&render_type(&resolved))),
    }
}

fn print_type(name: &str, f: &File) -> String {
    let mut out = format!("{}\n", bold(name));
    out.push_str(&print_fields(&TypeExpression::Reference(name.to_string()), f, "  "));
    out
}

/// Separates an operation's output into its success type and any error arms;
/// `Output -> Success / Error1 / Error2` parses as a single choice type where
/// the first arm is the success case.
fn split_output(out: &TypeExpression) -> (&TypeExpression, &[TypeExpression]) {
    if let TypeExpression::Choice(arms) = out {
        if !arms.is_empty() {
            return (&arms[0], &arms[1..]);
        }
    }
    (out, &[])
}

fn is_push_only(op: &ServiceOperation) -> bool {
    matches!(&op.input_type, TypeExpression::Builtin(b) if b == "null")
        && matches!(op.direction, ServiceDirection::Unidirectional | ServiceDirection::Reverse)
}

fn print_operation(op: &ServiceOperation, f: &File) -> String {
    let mut name = op.name.clone();
    let dir = match op.direction {
        ServiceDirection::Unidirectional => "->",
        ServiceDirection::Bidirectional => "<->",
        ServiceDirection::Reverse => "<-",
    };
    if dir != "->" {
        name = format!("{name} ({dir})");
    }

    let mut out = format!("  {}\n", cyan(&name));

    if !is_push_only(op) {
        out.push_str(&format!("    {}\n", bold("request")));
        out.push_str(&print_fields(&op.input_type, f, "      "));
    }

    let (success, errs) = split_output(&op.output_type);
    out.push_str(&format!("    {}\n", bold("response")));
    out.push_str(&print_fields(success, f, "      "));
    for e in errs {
        out.push_str(&format!("    {}\n", bold("error")));
        out.push_str(&print_fields(e, f, "      "));
    }
    out
}

fn is_request_or_response(name: &str) -> bool {
    name.ends_with("Request") || name.ends_with("Response")
}

fn basic_listing(f: &File) -> String {
    let mut out = String::new();
    if f.services().next().is_some() {
        out.push_str(&format!("{}\n", bold("Services:")));
        for (svc_name, ops) in f.services() {
            out.push_str(&format!("  {}\n", bold(svc_name)));
            for op in ops {
                out.push_str(&format!("    {}\n", cyan(&op.name)));
            }
        }
        out.push('\n');
    }

    out.push_str(&format!("{}\n", bold("Types:")));
    for name in &f.order {
        if is_request_or_response(name) {
            continue;
        }
        out.push_str(&format!("  {}\n", green(name)));
    }
    out
}

fn verbose_listing(f: &File) -> String {
    let mut out = String::new();
    if f.services().next().is_some() {
        for (svc_name, ops) in f.services() {
            out.push_str(&format!("{}\n", bold(svc_name)));
            for op in ops {
                out.push_str(&print_operation(op, f));
                out.push('\n');
            }
        }
        out.push('\n');
    }
    out.push_str(&format!("{}\n", bold("Types:")));
    for name in &f.order {
        if is_request_or_response(name) {
            continue;
        }
        out.push_str(&print_type(name, f));
        out.push('\n');
    }
    out
}

/// Parses the `.csil` source at csil_path and returns its listing.
///
/// With no item given: basic mode returns service names with their method
/// names, and a separate list of the file's other types; verbose mode
/// returns full request/response/error field detail for every message and
/// the resolved fields of every type.
///
/// With an item given, it looks it up first as a method, then as a type, and
/// returns the verbose detail for just that one (a method is printed under
/// its owning service's name), regardless of verbose.
pub fn run_list(csil_path: &str, item: Option<&str>, verbose: bool) -> Result<String> {
    let f = File::load(csil_path)?;

    if let Some(item) = item {
        if let Some((svc_name, op)) = f.find_operation(item) {
            let mut out = format!("{}\n", bold(svc_name));
            out.push_str(&print_operation(op, &f));
            return Ok(out);
        }
        if f.by_name.contains_key(item) {
            return Ok(print_type(item, &f));
        }
        bail!("list: no method or type named \"{item}\" found in {csil_path}");
    }

    if verbose {
        return Ok(verbose_listing(&f));
    }

    Ok(basic_listing(&f))
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
    fn should_fail() {
        assert_eq!(true, false);
    }

    #[test]
    fn resolve_follows_ident_chain_to_primitive() {
        let f = load("A = B\nB = C\nC = text");
        let resolved = resolve(&TypeExpression::Reference("A".to_string()), &f);
        assert!(matches!(resolved, TypeExpression::Builtin(b) if b == "text"));
    }

    #[test]
    fn resolve_stops_at_structural_type() {
        let f = load("A = B\nB = { id: text }");
        let resolved = resolve(&TypeExpression::Reference("A".to_string()), &f);
        assert!(matches!(resolved, TypeExpression::Group(_)));
    }

    #[test]
    fn resolve_stops_at_unknown_ident() {
        let f = load("A = text");
        let resolved = resolve(&TypeExpression::Reference("Unknown".to_string()), &f);
        assert!(matches!(resolved, TypeExpression::Reference(n) if n == "Unknown"));
    }

    #[test]
    fn resolve_breaks_cycles() {
        let f = load("A = B\nB = A");
        let resolved = resolve(&TypeExpression::Reference("A".to_string()), &f);
        assert!(matches!(resolved, TypeExpression::Reference(_)));
    }

    #[test]
    fn render_type_ident_stays_unresolved_name() {
        // A group field's rendered type prints the field's *declared* type
        // (e.g. a reference to another definition), not its resolution.
        assert_eq!(render_type(&TypeExpression::Reference("A".to_string())), "A");
    }

    #[test]
    fn render_type_builtin() {
        let f = load("A = text");
        let resolved = resolve(&TypeExpression::Reference("A".to_string()), &f);
        assert_eq!(render_type(&resolved), "text");
    }

    #[test]
    fn render_type_literal_string_and_number() {
        assert_eq!(
            render_type(&TypeExpression::Literal(LiteralValue::Text("ok".to_string()))),
            "\"ok\""
        );
        assert_eq!(
            render_type(&TypeExpression::Literal(LiteralValue::Integer(42))),
            "42"
        );
    }

    #[test]
    fn render_type_group() {
        let f = load("A = { id: text, ? label: text }");
        let resolved = resolve(&TypeExpression::Reference("A".to_string()), &f);
        assert_eq!(render_type(&resolved), "{id, label}");
    }

    #[test]
    fn render_type_map() {
        let f = load("A = {* text => int}");
        let resolved = resolve(&TypeExpression::Reference("A".to_string()), &f);
        assert_eq!(render_type(&resolved), "{* text => int}");
    }

    #[test]
    fn render_type_array_with_occurrence() {
        let f = load("A = [* text]");
        let resolved = resolve(&TypeExpression::Reference("A".to_string()), &f);
        assert_eq!(render_type(&resolved), "[* text]");
    }

    #[test]
    fn render_type_array_without_occurrence() {
        let f = load("A = [text]");
        let resolved = resolve(&TypeExpression::Reference("A".to_string()), &f);
        assert_eq!(render_type(&resolved), "[text]");
    }

    #[test]
    fn render_type_choice() {
        let f = load("A = text / int");
        let resolved = resolve(&TypeExpression::Reference("A".to_string()), &f);
        assert_eq!(render_type(&resolved), "text / int");
    }

    #[test]
    fn render_type_includes_constraints() {
        let f = load("A = int .ge 0 .le 120");
        let resolved = resolve(&TypeExpression::Reference("A".to_string()), &f);
        assert_eq!(render_type(&resolved), "int .ge(0) .le(120)");
    }

    #[test]
    fn find_operation_locates_by_name() {
        let f = load(
            "Req = { x: text }\nResp = { y: text }\nservice S {\n  Op: Req -> Resp\n}",
        );
        let (svc, op) = f.find_operation("Op").unwrap();
        assert_eq!(svc, "S");
        assert_eq!(op.name, "Op");
        assert!(f.find_operation("Missing").is_none());
    }

    #[test]
    fn split_output_separates_success_and_errors() {
        let success = TypeExpression::Builtin("text".to_string());
        let err1 = TypeExpression::Builtin("int".to_string());
        let choice = TypeExpression::Choice(vec![success.clone(), err1.clone()]);
        let (s, errs) = split_output(&choice);
        assert!(matches!(s, TypeExpression::Builtin(b) if b == "text"));
        assert_eq!(errs.len(), 1);

        let (s, errs) = split_output(&success);
        assert!(matches!(s, TypeExpression::Builtin(b) if b == "text"));
        assert!(errs.is_empty());
    }

    #[test]
    fn run_list_basic_and_verbose_and_errors() {
        let mut f = NamedTempFile::with_suffix(".csil").unwrap();
        f.write_all(
            b"Task = { id: text, ? label: text }\nStringInt64Map = {* text => int}\nCreateRequest = { name: text }\nCreateResponse = { task: Task }\nErrorType = { message: text }\n\nservice Widgets {\n  Create: CreateRequest -> CreateResponse / ErrorType\n}\n",
        )
        .unwrap();
        let path = f.path().to_str().unwrap();

        let basic = strip_ansi(&run_list(path, None, false).unwrap());
        assert!(basic.contains("Services:"));
        assert!(basic.contains("Widgets"));
        assert!(basic.contains("Create"));
        assert!(!basic.contains("CreateRequest"));

        let verbose = strip_ansi(&run_list(path, None, true).unwrap());
        assert!(verbose.contains("{* text => int}"));
        assert!(verbose.contains("label?"));

        let single = strip_ansi(&run_list(path, Some("StringInt64Map"), false).unwrap());
        assert!(single.contains("{* text => int}"));

        let mut empty = NamedTempFile::with_suffix(".csil").unwrap();
        empty.write_all(b"Task = { id: text }").unwrap();
        let no_services = strip_ansi(&run_list(empty.path().to_str().unwrap(), None, false).unwrap());
        assert!(!no_services.contains("Services:"));
        assert!(no_services.contains("Types:"));
        assert!(no_services.contains("Task"));

        let err = run_list(path, Some("DoesNotExist"), false).unwrap_err();
        assert!(err.to_string().contains("no method or type named"));

        let err = run_list("/nonexistent/does-not-exist.csil", None, false).unwrap_err();
        assert!(format!("{err:?}").to_lowercase().contains("no such file"));
    }
}
