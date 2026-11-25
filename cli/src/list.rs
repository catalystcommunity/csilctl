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

    fn rule(&self, name: &str) -> Option<&RuleType> {
        self.by_name
            .get(name)
            .map(|&i| &self.spec.rules[i].rule_type)
    }

    fn expand_record(
        &self,
        group: &GroupExpression,
        path: &mut Vec<String>,
    ) -> Result<GroupExpression> {
        let mut entries = Vec::new();
        for entry in &group.entries {
            let name = match (&entry.key, &entry.occurrence, &entry.value_type) {
                (
                    None,
                    None,
                    TypeExpression::Reference(name)
                    | TypeExpression::Socket(name)
                    | TypeExpression::Plug(name),
                ) => name,
                _ => {
                    entries.push(entry.clone());
                    continue;
                }
            };
            match self.rule(name) {
                Some(
                    RuleType::GroupDef(inner) | RuleType::TypeDef(TypeExpression::Group(inner)),
                ) => {
                    if path.contains(name) || path.len() >= 64 {
                        bail!("cannot expand nested group reference {name:?}");
                    }
                    path.push(name.clone());
                    entries.extend(self.expand_record(inner, path)?.entries);
                    path.pop();
                }
                None if matches!(entry.value_type, TypeExpression::Plug(_)) => {}
                _ => entries.push(entry.clone()),
            }
        }
        Ok(GroupExpression { entries })
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
pub(crate) fn resolve(t: &TypeExpression, f: &File) -> Result<TypeExpression> {
    let mut seen = HashSet::new();
    let mut current = t.clone();
    loop {
        let name = match &current {
            TypeExpression::Reference(n) => n.clone(),
            TypeExpression::Socket(n) | TypeExpression::Plug(n) => {
                if f.rule(n).is_none() {
                    return Ok(TypeExpression::Builtin("any".into()));
                }
                n.clone()
            }
            TypeExpression::Group(g) => {
                return Ok(TypeExpression::Group(f.expand_record(g, &mut Vec::new())?));
            }
            _ => return Ok(current),
        };
        if !seen.insert(name.clone()) {
            return Ok(current);
        }
        let Some(rule) = f.rule(&name) else {
            return Ok(current);
        };
        current = match rule {
            RuleType::TypeDef(te) => te.clone(),
            RuleType::GroupDef(ge) => TypeExpression::Group(ge.clone()),
            _ => return Ok(current),
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
        TypeExpression::NamedNumbers(members) => {
            let members: Vec<_> = members.iter().map(|(k, v)| format!("{k}: {v}")).collect();
            format!("&({})", members.join(", "))
        }
        TypeExpression::NamedNumberGroup(name, _) => format!("&{name}"),
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
        TypeExpression::Plug(s) => format!("$${s}"),
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
fn print_fields(t: &TypeExpression, f: &File, indent: &str) -> Result<String> {
    let resolved = resolve(t, f)?;
    Ok(match &resolved {
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
        // A union: show the arm names joined by "/" (what the type actually
        // declares), then each arm's own name and contents beneath it -- but
        // only for arms that have contents of their own to show. A literal
        // arm (e.g. `Direction = "north" / "south"`) already says everything
        // in the joined line, so re-printing it as its own name+contents
        // block would just repeat it.
        TypeExpression::Choice(arms) => {
            let mut out = format!("{indent}{}\n", green(&render_type(&resolved)));
            for arm in arms {
                if matches!(arm, TypeExpression::Literal(_)) {
                    continue;
                }
                out.push_str(&format!("{indent}{}\n", green(&render_type(arm))));
                out.push_str(&print_fields(arm, f, &format!("{indent}  "))?);
            }
            out
        }
        _ => format!("{indent}{}\n", green(&render_type(&resolved))),
    })
}

fn print_type(name: &str, f: &File) -> Result<String> {
    let mut out = format!("{}\n", bold(name));
    out.push_str(&print_fields(
        &TypeExpression::Reference(name.to_string()),
        f,
        "  ",
    )?);
    Ok(out)
}

fn is_push_only(op: &ServiceOperation) -> bool {
    matches!(&op.input_type, TypeExpression::Builtin(b) if b == "null")
        && matches!(op.direction, ServiceDirection::Unidirectional | ServiceDirection::Reverse)
}

/// Prints an operation's request/response as their declared type name plus
/// its contents -- not split into success/error, since a union's arms
/// aren't reliably distinguishable that way (e.g. a `ListErrors` type could
/// be the successful response of an operation that lists errors, not itself
/// an error case).
fn print_operation(op: &ServiceOperation, f: &File) -> Result<String> {
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
        out.push_str(&print_named_type(&op.input_type, f, "      ")?);
    }

    out.push_str(&format!("    {}\n", bold("response")));
    out.push_str(&print_named_type(&op.output_type, f, "      ")?);
    Ok(out)
}

/// Prints `t`'s own declared name (if it has one distinct from its
/// contents, i.e. it's a reference to a definition) followed by its
/// contents indented beneath it.
///
/// A named reference to a union only shows the union's declared arms (e.g.
/// `FooRequest / BarRequest`) -- not each arm's own fields, since that
/// would just re-describe what the alias already names. An anonymous union
/// written directly as the request/response type (no alias of its own) has
/// no separate name to show, so it gets the full breakdown instead: the
/// arms joined, then each arm's own name and contents.
fn print_named_type(t: &TypeExpression, f: &File, indent: &str) -> Result<String> {
    if matches!(t, TypeExpression::Reference(_)) {
        let mut out = format!("{indent}{}\n", green(&render_type(t)));
        let resolved = resolve(t, f)?;
        if let TypeExpression::Choice(_) = &resolved {
            out.push_str(&format!("{indent}  {}\n", green(&render_type(&resolved))));
        } else {
            out.push_str(&print_fields(t, f, &format!("{indent}  "))?);
        }
        Ok(out)
    } else {
        print_fields(t, f, indent)
    }
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
        out.push_str(&format!("  {}\n", green(name)));
    }
    out
}

fn verbose_listing(f: &File) -> Result<String> {
    let mut out = String::new();
    if f.services().next().is_some() {
        for (svc_name, ops) in f.services() {
            out.push_str(&format!("{}\n", bold(svc_name)));
            for op in ops {
                out.push_str(&print_operation(op, f)?);
                out.push('\n');
            }
        }
        out.push('\n');
    }
    out.push_str(&format!("{}\n", bold("Types:")));
    for name in &f.order {
        out.push_str(&print_type(name, f)?);
        out.push('\n');
    }
    Ok(out)
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
            out.push_str(&print_operation(op, &f)?);
            return Ok(out);
        }
        if f.by_name.contains_key(item) {
            return print_type(item, &f);
        }
        bail!("list: no method or type named \"{item}\" found in {csil_path}");
    }

    if verbose {
        return verbose_listing(&f);
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
    fn resolve_follows_ident_chain_to_primitive() {
        let f = load("A = B\nB = C\nC = text");
        let resolved = resolve(&TypeExpression::Reference("A".to_string()), &f).unwrap();
        assert!(matches!(resolved, TypeExpression::Builtin(b) if b == "text"));
    }

    #[test]
    fn resolve_stops_at_structural_type() {
        let f = load("A = B\nB = { id: text }");
        let resolved = resolve(&TypeExpression::Reference("A".to_string()), &f).unwrap();
        assert!(matches!(resolved, TypeExpression::Group(_)));
    }

    #[test]
    fn resolve_stops_at_unknown_ident() {
        let f = load("A = text");
        let resolved = resolve(&TypeExpression::Reference("Unknown".to_string()), &f).unwrap();
        assert!(matches!(resolved, TypeExpression::Reference(n) if n == "Unknown"));
    }

    #[test]
    fn resolve_breaks_cycles() {
        let f = load("A = B\nB = A");
        let resolved = resolve(&TypeExpression::Reference("A".to_string()), &f).unwrap();
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
        let resolved = resolve(&TypeExpression::Reference("A".to_string()), &f).unwrap();
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
        let resolved = resolve(&TypeExpression::Reference("A".to_string()), &f).unwrap();
        assert_eq!(render_type(&resolved), "{id, label}");
    }

    #[test]
    fn render_type_map() {
        let f = load("A = {* text => int}");
        let resolved = resolve(&TypeExpression::Reference("A".to_string()), &f).unwrap();
        assert_eq!(render_type(&resolved), "{* text => int}");
    }

    #[test]
    fn render_type_array_with_occurrence() {
        let f = load("A = [* text]");
        let resolved = resolve(&TypeExpression::Reference("A".to_string()), &f).unwrap();
        assert_eq!(render_type(&resolved), "[* text]");
    }

    #[test]
    fn render_type_array_without_occurrence() {
        let f = load("A = [text]");
        let resolved = resolve(&TypeExpression::Reference("A".to_string()), &f).unwrap();
        assert_eq!(render_type(&resolved), "[text]");
    }

    #[test]
    fn render_type_choice() {
        let f = load("A = text / int");
        let resolved = resolve(&TypeExpression::Reference("A".to_string()), &f).unwrap();
        assert_eq!(render_type(&resolved), "text / int");
    }

    #[test]
    fn render_type_includes_constraints() {
        let f = load("A = int .ge 0 .le 120");
        let resolved = resolve(&TypeExpression::Reference("A".to_string()), &f).unwrap();
        assert_eq!(render_type(&resolved), "int .ge(0) .le(120)");
    }

    #[test]
    fn list_expands_record_groups_and_displays_named_numbers() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/readings.csil");
        let reading = strip_ansi(&run_list(path, Some("Reading"), false).unwrap());
        assert!(reading.contains("state") && reading.contains("day"));
        assert!(reading.contains("&(off: 0, on: 42)"));
        let state = strip_ansi(&run_list(path, Some("State"), false).unwrap());
        assert!(state.contains("&codes"));
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
        // Types: lists every named type, including ones an operation
        // directly declares as its request/response -- nothing is hidden.
        assert!(basic.contains("CreateRequest"));

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
