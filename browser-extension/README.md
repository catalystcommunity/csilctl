# Build and load the CSIL Developer Tools extension

The extension adds a CSIL panel to browser Developer Tools. It captures
completed HTTP requests. It detects CSIL-RPC and verbose CSIL-Events envelopes,
decodes nested tag-24 payloads, and keeps raw bytes when decoding fails.

The extension uses Manifest V3. It bundles the `csilgen-schema` diagnostic
decoder as WebAssembly. It does not download executable code.

## Requirements

Install these tools:

- Rust with the `wasm32-unknown-unknown` target;
- `wasm-bindgen-cli`; and
- Node.js for the JavaScript tests; and
- `bsdtar` to make the optional ZIP archive.

Use the wasm-bindgen CLI version that Cargo selects for the Rust crate when a
version mismatch occurs. This checkout currently pins version `0.2.125`.

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli
```

On Arch Linux, install the packaged WebAssembly target instead:

```sh
sudo pacman -S rust-wasm
```

If `wasm-bindgen` is not on `PATH`, give the build its absolute path:

```sh
WASM_BINDGEN="$HOME/.cargo/bin/wasm-bindgen" ./browser-extension/build.sh
```

## Build

Clone or download the GitHub repository. Then run this command from the
repository root:

```sh
./browser-extension/build.sh
```

The command creates `browser-extension/dist/`. This directory contains the
unpacked extension. The build pins `csilgen-schema` to the `csilgen/v0.2.6`
Git tag.

Run the JavaScript tests separately:

```sh
cd browser-extension
npm test
npm run test:wasm
```

Run `npm run test:wasm` after the extension build. This test uses the generated
WebAssembly files in `dist/`.

Create an archive for a GitHub release asset with:

```sh
./browser-extension/package.sh
```

This command creates `browser-extension/csil-devtools.zip`. Chrome developers
can extract this archive and load the directory. Firefox developers can select
the archive when they load a temporary add-on.

## Load in Chrome

1. Open `chrome://extensions`.
2. Enable Developer mode.
3. Select **Load unpacked**.
4. Select `browser-extension/dist/`.
5. Open Developer Tools on the application page.
6. Select the **CSIL** panel.

Reload the extension from `chrome://extensions` after each build.

See the [Chrome unpacked extension instructions](https://developer.chrome.com/docs/extensions/get-started/tutorial/hello-world#load-unpacked)
for more information.

## Load in Firefox

Use Firefox 142 or later. This version supports the manifest data collection
declaration on Firefox desktop and Android.

1. Open `about:debugging`.
2. Select **This Firefox**.
3. Select **Load Temporary Add-on**.
4. Select `browser-extension/dist/manifest.json`.
5. Open Developer Tools on the application page.
6. Open the Network panel once.
7. Select the **CSIL** panel.

Firefox removes a temporary extension when the browser exits. Reload it from
`about:debugging` after each build.

See the [Firefox temporary installation instructions](https://extensionworkshop.com/documentation/develop/temporary-installation-in-firefox/)
for more information.

## Current capture behavior

Keep the CSIL panel open before the request finishes. The panel keeps at most
250 captures for the current Developer Tools session. It does not copy a body
larger than 16 MiB into the WebAssembly decoder.

The extension identifies these messages:

- an RPC request with `v`, `service`, `op`, and a tag-24 `payload`;
- an RPC response with `status` and a tag-24 `payload`;
- a verbose Event with `event` and a tag-24 `payload`; and
- valid CBOR on an `/rpc` path or with a CSIL or CBOR content type.

The extension can apply a schema to an HTTP CSIL-RPC request or a verbose
CSIL-Events envelope. The Events payload is treated as the input side because
the browser sent the captured HTTP request.

The last case is shown as a candidate because a path-routed RPC payload does not
contain its service and operation names.

The request-body bytes in a HAR entry do not have one portable binary marker.
The extension tries an explicit base64 marker, a binary string, UTF-8, and a
valid base64 form. It accepts the first complete CBOR value. The Raw tab shows
which interpretation it selected.

## Load a schema

Select **Load schema** in the CSIL panel to load a generated
`*.csil-schema.cbor` file. The extension verifies the descriptor version and
digest before it uses the schema. It matches a loaded schema to the service and
operation in each captured RPC request.

An application can also advertise a schema with this response header:

```text
CSIL-Schema: </assets/api.csil-schema.cbor>; digest="sha256:<hex>"; version="v1alpha1"
```

The URL can be absolute or relative to the RPC request URL.
The URL must use HTTP or HTTPS. Use the file picker for a local descriptor.

A page can provide the same locator through a synchronous function:

```js
globalThis.getCSILdefs = () => [{
  version: "v1alpha1",
  digest: "sha256:<hex>",
  url: "/assets/api.csil-schema.cbor",
}];
```

The extension also checks this registry form:

```js
globalThis.__CSIL_DEVTOOLS_V1__ = {
  listDefinitions: globalThis.getCSILdefs,
};
```

The extension requests access only to the origin that hosts an advertised
schema. It does this when the user selects **Allow and load**. After the user
grants access, later descriptors from that origin load automatically.

The extension does not repeatedly request a descriptor after a 401, 403, 404,
410, invalid descriptor, or digest mismatch. The message details show the
failure and provide a Retry action. Network errors, HTTP 5xx responses, and 429
responses use a bounded retry delay. Generic CBOR and raw byte views stay
available for every schema failure.

## Current limits

- The panel does not capture WebSocket frames or other live CSIL-Events carriers.
- The public Chrome and Firefox Developer Tools APIs do not add a custom tab to
  the native Network request details view. This extension uses a top-level CSIL
  panel.
- Firefox does not report completed requests to a Developer Tools extension
  until the Network panel has been opened once.

The WASM bridge exposes typed RPC and Events inspection functions. Live Events
capture still needs a transport trace bridge because the public Developer Tools
extension APIs do not expose a portable WebSocket frame stream.
