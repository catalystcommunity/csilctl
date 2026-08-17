# TODO

## Scaffolding
- [x] `go mod init` the project (module `csilctl`, go 1.25.1, in `cli/`)
- [x] Create `cli/` directory for the CLI code
- [x] Wire up `list` / `send` subcommands using `github.com/urfave/cli/v2` in `cli/main.go`, with flag definitions kept in `main.go` and logic in `cli/list.go` / `cli/send.go`
- [ ] Set up Reactorcide CI/CD pipeline (need to add webhook and verify)
- [ ] Set up semver-tags for release tagging (need to verify)
- [ ] Enforce conventional commits via commits OR pull request squash message.

## Feature 1: List messages
- [x] ~~Load the generated csilgen Go client package via static source analysis (`golang.org/x/tools/go/packages`)~~ — superseded, see below
- [x] Switched `list` to parse `.csil` source directly instead of the generated Go client: hand-written lexer/parser (`cli/csil_lexer.go`, `cli/csil_parser.go`, `cli/csil_ast.go`) covering definitions (aliases, groups, arrays, maps, choices), control-operator constraints, and `service { }` blocks, per the [CSIL spec](https://github.com/catalystcommunity/csilgen/blob/main/docs/csil-spec.md)
- [x] `--client` now takes a path to the `.csil` file for `list` (still the Go client folder for `send`, for now)
- [x] Find `service Name { ... }` blocks and list each operation as a message, resolving named types through `Definitions` to print request/response (and error, from choice arms) fields
- [x] Print a human-readable listing (`csilctl list --client <path.csil>`), grouped by service
- [x] Blank line between each message/command in the `list` output, with basic ANSI syntax highlighting (`cli/color.go`: bold service name, cyan operation names, yellow field names, green types)
- [x] Print response fields and, separately, error-arm fields (from `Output -> Success / Error1 / Error2`)
- [x] Mark optional fields (CSIL's `? field:`) with a trailing `?` on the field name — verified against `/home/juniboco/repos/corndogs/csil/corndogs.csil`
- [x] Handle/report cleanly when the `.csil` file has no `service` blocks (`list: no service definitions found in <path>`) or fails to parse (lexer/parser errors now include the source line number, e.g. `csil:3: unterminated options block`)
- [ ] Actually handle `include`/`from`/`options` blocks instead of just skipping them (`skipIncludeStatement`/`skipFromStatement`/`skipOptionsBlock` in `cli/csil_parser.go`) — `include`/`from` currently don't pull in the referenced file's definitions, and `options` content is discarded outright, so `list` silently produces incomplete/wrong output for any `.csil` file that relies on cross-file includes
- [x] `go mod tidy` dropped the now-unused `golang.org/x/tools`/`go/packages` dependency
- [x] Default `list` output is now basic: service names + their method names, plus a separate `Types:` list (all definitions except `*Request`/`*Response`)
- [x] Added `--verbose` flag to print the full request/response/error field detail for every message
- [x] `list [item]` (positional arg) prints the verbose detail for just that one item — checks methods first, then falls back to type names (e.g. `list Task`, `list StringInt64Map`) — prefixed by its owning service name for methods
- [x] `--verbose` (with no item) now also prints every type's resolved fields, not just services/methods
- [x] Fixed a bug where a scalar/map type alias (e.g. `StringInt64Map = {* text => int}`) printed its own name instead of its resolved form

## Feature 2: Build message data
- [x] Parse `--data` as a JSON-like payload
- [x] Diff parsed data against the target message's required fields
- [x] Prompt interactively for any missing required fields
- [x] Validate provided/prompted values against field constraints — `validate_constraint` in `cli/src/payload.rs` now enforces `.size`/`.regex`/`.ge`/`.le`/`.gt`/`.lt`/`.eq`/`.ne` when encoding a `TypeExpression::Constrained` value (adds the `regex` crate, already in the dependency tree transitively via `csilgen-core`).
- [x] Enforce `.default` — `prompt_for_request` (`cli/src/prompt.rs`) now fills in a field's `.default(v)` literal when it's absent from `--data`/existing input, instead of prompting or erroring on it (an explicitly provided value still isn't checked against the default — that's not what `.default` means)
- [x] Enforce `.bits` — `validate_constraint` (`cli/src/payload.rs`) now parses the `.bits` mask expression (hex `"0x.."` or decimal string) and rejects an integer value that sets any bit outside that mask; non-integer values are rejected outright
- [x] Enforce `.and` — `validate_constraint` (`cli/src/payload.rs`) now re-encodes the same JSON against the `.and`-referenced type and rejects the value if that fails
- [x] Enforce `.within` — `validate_constraint` (`cli/src/payload.rs`) now re-encodes the same JSON against the `.within`-referenced type and rejects the value if that fails (implemented the same way as `.and`, since csilgen-core doesn't distinguish intersection from subtyping at this layer)
- [x] Enforce `.json` — `validate_constraint` (`cli/src/payload.rs`) rejects a non-text value outright, and for text values requires the string parse as valid JSON (csilgen-core's `.json` carries no referenced type to validate the parsed JSON's shape against, so only well-formedness is checked)
- [x] Enforce `.cbor` — `validate_constraint` (`cli/src/payload.rs`) rejects a non-bytes value outright, and for byte strings requires them decode as a single well-formed CBOR item via `ciborium::de::from_reader` (like `.json`, csilgen-core's `.cbor` carries no referenced type to check the decoded item's shape against)
- [x] Enforce `.cborseq` — `validate_constraint` (`cli/src/payload.rs`) rejects a non-bytes value outright, and for byte strings decodes items back-to-back off a `Cursor` until every byte is consumed, requiring at least a well-formed sequence (zero or more concatenated items, no dangling/malformed trailer)

## Feature 3: Send message
- [x] Parse `--host` as `host:port`
- [x] Open a connection/`Transport` to the parsed address
- [x] Open a CSIL-RPC client (`RpcClient::new` over a `StreamCarrier`, `cli/src/send.rs`) on the connected transport — no generated struct type is constructed at runtime; see below
- [x] Marshal the completed data into the method's request payload — `payload::json_to_cbor` (`cli/src/payload.rs`) encodes the JSON against the operation's resolved `TypeExpression` directly into canonical CBOR bytes, rather than through a generated request struct
- [x] Call the matching method on the `*Client` (via reflection) and report the result/response

## CLI UX
- [x] `--help`/`help` command explaining the CLI overall (subcommands, flags, what `--client` expects) — `cli/main.go` `printUsage`
- [x] Per-subcommand help (`csilctl list --help`, `csilctl send --help`) — free from stdlib `flag.ExitOnError`
- [ ] Improve per-subcommand help text/examples if `clap`'s default derive output (`cli/src/main.rs`) feels too sparse

## Docs
- [ ] Add build/install instructions to README (`cargo build`/`cargo install`) — currently blocked on the `csilgen-core`/`csilgen`/`csilgen-transport` path dependencies in `cli/Cargo.toml`, which point at a local `../../csilgen` checkout rather than a published crate
- [x] Add usage examples once flags are implemented for real

## Testing
Run via `cargo test` (in `cli/`). Lexing/parsing of `.csil` source is no longer
hand-rolled here — it's delegated to the external `csilgen-core` crate
(`parse_csil_file`, used from `cli/src/list.rs`), so there's nothing to test
at that layer in this repo. Coverage in this repo is: unit tests in
`#[cfg(test)]` modules alongside the code (`cli/src/list.rs`,
`cli/src/prompt.rs`), plus black-box integration tests in `cli/tests/`
(`list.rs`, `send.rs`, `color.rs`) that exercise `run_list`/`run_send`/the
built binary end-to-end.

**List rendering** — `cli/src/list.rs` (unit) + `cli/tests/list.rs` (integration)
- [x] Test `resolve` follows identifier chains through `Definitions` and stops at primitives, structural types, unknown identifiers, and cycles
- [x] Test `render_type` output for each `TypeExpression` variant (reference, literal string/number, group, group with catch-all, array with/without occurrence, map, choice) plus constraint suffixes
- [x] Test map-type-alias fields/items render their expanded `{* key => value}` form, not just the alias name (regression test for the earlier map-display bug; covered at both the `render_type`/`resolve` level and end-to-end via `run_list`)
- [x] Test `find_operation` and `split_output` (success vs. error arms from a choice output, plus the non-choice case)
- [x] Test `run_list` end-to-end against a small inline `.csil` fixture: basic listing, `--verbose` listing, and single-item lookup by method name and by type name
- [x] Test `run_list` error paths: unknown item name, unreadable/nonexistent file path, malformed `.csil` — and the no-service-blocks case, which now lists just the file's types instead of erroring (see Feature 1)

**Send** — `cli/tests/send.rs`
- [x] Test a full round trip over a real TCP connection (spun up in-test) with a provided `--data` payload
- [x] Test that a missing required field falls through to the interactive prompt instead of failing outright
- [x] Test that sending an unknown message name reports an error
- [x] `cli/src/payload.rs` now has its own `#[cfg(test)]` module covering `.size`/`.regex`/`.ge`/`.le`/`.gt`/`.lt`/`.eq`/`.ne` constraint validation (see Feature 2 above)
- [ ] Still missing: unit coverage for the rest of `json_to_cbor`/`cbor_to_json` (array/map/choice encode, canonical map-key ordering, the `bytes`/`any` builtins, and decode) — only exercised indirectly through the `send.rs` integration tests

**Color / CLI flags** — `cli/tests/color.rs`
- [x] Test colorized-by-default output, `--disable-color`, and `NO_COLOR`/`FORCE_COLOR` env var precedence (`NO_COLOR` > `FORCE_COLOR` > `--disable-color`), invoking the built binary directly
