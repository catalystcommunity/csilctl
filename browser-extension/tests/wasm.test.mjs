import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import {
  decode_cbor,
  initSync,
} from "../dist/wasm/csil_decoder.js";

const moduleBytes = await readFile(new URL("../dist/wasm/csil_decoder_bg.wasm", import.meta.url));
initSync({ module: moduleBytes });

const decoded = decode_cbor(new Uint8Array([
  0xa2,
  0x61, 0x61, 0x01,
  0x61, 0x62, 0xc1, 0x1a, 0x65, 0x53, 0xf1, 0x00,
]));
assert.equal(decoded.generic.kind, "map");
assert.equal(decoded.generic.entries.length, 2);
assert.equal(decoded.generic.entries[1].value.kind, "timestamp");

const hostileLength = decode_cbor(new Uint8Array([0x9b, 0, 0, 1, 0, 0, 0, 0, 0]));
assert.equal(hostileLength.generic, null);
assert.match(hostileLength.diagnostics[0].message, /does not fit in memory/);

console.log("WASM decoder checks passed.");
