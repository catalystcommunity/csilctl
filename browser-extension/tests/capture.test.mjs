import test from "node:test";
import assert from "node:assert/strict";
import {
  bytesToHex,
  classifyCsil,
  decodeBodyCandidates,
  embeddedPayload,
  headerValue,
} from "../static/capture.js";
import {
  normalizeDigest,
  normalizeRuntimeLocator,
  originPermission,
  parseSchemaLocator,
  schemaMatchesRoute,
  shouldAutomaticallyLoad,
} from "../static/schema.js";

function text(value) {
  return { kind: "text", value };
}

function integer(value) {
  return { kind: "integer", value: BigInt(value) };
}

function map(entries) {
  return {
    kind: "map",
    entries: Object.entries(entries).map(([key, value]) => ({ key: text(key), value })),
  };
}

function payload(bytes = new Uint8Array([0xa0])) {
  return { kind: "tag", tag: 24n, value: { kind: "bytes", value: bytes } };
}

test("classifies an RPC request from its envelope", () => {
  const decoded = {
    generic: map({
      v: integer(1),
      service: text("AuthService"),
      op: text("begin-login"),
      payload: payload(),
    }),
  };
  const result = classifyCsil(decoded, { url: "https://example.test/rpc", headers: [] });
  assert.equal(result.kind, "rpc-request");
  assert.equal(result.label, "AuthService.begin-login");
  assert.equal(result.confidence, "envelope");
});

test("classifies valid CBOR on an RPC path as a candidate", () => {
  const result = classifyCsil(
    { generic: map({ value: integer(2) }) },
    { url: "https://example.test/api/rpc", headers: [] },
  );
  assert.equal(result.kind, "cbor-candidate");
  assert.equal(result.confidence, "rpc-path");
});

test("extracts embedded tag-24 bytes", () => {
  const bytes = new Uint8Array([1, 2, 3]);
  assert.deepEqual(embeddedPayload(map({ payload: payload(bytes) })), bytes);
});

test("classifies a verbose Event and keeps its optional service", () => {
  const decoded = {
    generic: map({
      service: text("WorldService"),
      event: text("move"),
      payload: payload(),
    }),
  };
  const result = classifyCsil(decoded, { url: "https://example.test/events", headers: [] });
  assert.equal(result.kind, "event");
  assert.equal(result.service, "WorldService");
  assert.equal(result.operation, "move");
});

test("tries base64 for a binary response", () => {
  const expected = new Uint8Array([0xa1, 0x61, 0x76, 0x01]);
  const result = decodeBodyCandidates("oWF2AQ==", "application/cbor", "application/cbor", (bytes) => ({
    generic: bytes[0] === 0xa1 ? { kind: "map", entries: [] } : null,
  }));
  assert.equal(result.source, "base64");
  assert.deepEqual(result.bytes, expected);
});

test("does not decode a body above the configured byte limit", () => {
  let decodeCalls = 0;
  const body = decodeBodyCandidates("AAAA", "base64", "application/cbor", () => {
    decodeCalls += 1;
    return { generic: null };
  }, 2);
  assert.equal(decodeCalls, 0);
  assert.match(body.error, /2-byte inspection limit/);

  const classification = classifyCsil(
    body.decoded,
    { url: "https://example.test/rpc", headers: [] },
    Boolean(body.error),
  );
  assert.equal(classification.kind, "cbor-candidate");
});

test("header lookup is case insensitive", () => {
  assert.equal(headerValue([{ name: "Content-Type", value: "application/cbor" }], "content-type"), "application/cbor");
});

test("hex output contains offsets and ASCII", () => {
  const result = bytesToHex(new Uint8Array([0x41, 0x42, 0x00]));
  assert.match(result, /^000000  41 42 00/);
  assert.match(result, /AB\.$/);
});

test("parses a relative schema header with a digest", () => {
  const digest = "ab".repeat(32);
  const locator = parseSchemaLocator(
    `</assets/api.csil-schema.cbor>; digest="sha256:${digest}"; version="v1alpha1"`,
    "https://example.test/v1/rpc",
  );
  assert.equal(locator.url, "https://example.test/assets/api.csil-schema.cbor");
  assert.equal(locator.digest, digest);
  assert.equal(locator.version, "v1alpha1");
});

test("validates runtime locators and normalizes their URL", () => {
  const locator = normalizeRuntimeLocator(
    { url: "schema.cbor", digest: `sha256=${"12".repeat(32)}`, version: 1 },
    "https://example.test/app/",
  );
  assert.equal(locator.url, "https://example.test/app/schema.cbor");
  assert.equal(locator.version, "1");
  assert.equal(normalizeRuntimeLocator({ url: 7 }, "https://example.test"), null);
});

test("builds one-origin optional host permissions", () => {
  assert.equal(originPermission("https://example.test:8443/schema.cbor"), "https://example.test:8443/*");
  assert.throws(() => originPermission("file:///tmp/schema.cbor"), /HTTP or HTTPS/);
});

test("matches a loaded schema to a service operation", () => {
  const schema = {
    info: {
      services: [{ name: "AuthService", operations: [{ name: "begin-login" }] }],
    },
  };
  assert.equal(schemaMatchesRoute(schema, "AuthService", "begin-login"), true);
  assert.equal(schemaMatchesRoute(schema, null, "begin-login"), true);
  assert.equal(schemaMatchesRoute(schema, "AuthService", "finish-login"), false);
});

test("does not automatically repeat terminal schema failures", () => {
  assert.equal(shouldAutomaticallyLoad({ state: "missing", retryAt: null }, 1000), false);
  assert.equal(shouldAutomaticallyLoad({ state: "denied", retryAt: null }, 1000), false);
  assert.equal(shouldAutomaticallyLoad({ state: "network-error", retryAt: 2000 }, 1000), false);
  assert.equal(shouldAutomaticallyLoad({ state: "network-error", retryAt: 2000 }, 2000), true);
});
