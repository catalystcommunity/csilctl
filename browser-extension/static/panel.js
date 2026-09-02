import initDecoder, {
  decode_cbor,
  descriptor_info,
  inspect_event_verbose,
  inspect_rpc_request,
  inspect_rpc_response,
} from "./wasm/csil_decoder.js";
import {
  bytesToHex,
  classifyCsil,
  decodeBodyCandidates,
  embeddedPayload,
  headerValue,
  textMap,
} from "./capture.js";
import {
  locatorKey,
  maximumSchemaBytes,
  normalizeRuntimeLocator,
  originPermission,
  parseSchemaLocator,
  schemaMatchesRoute,
  shouldAutomaticallyLoad,
} from "./schema.js";

const api = globalThis.browser ?? globalThis.chrome;
const captures = [];
const maximumCaptures = 250;
const locators = new Map();
const schemas = new Map();
let selectedId = null;
let paused = false;
let nextId = 1;
let lastRuntimeDiscovery = 0;

const elements = {
  clear: document.querySelector("#clear"),
  count: document.querySelector("#capture-count"),
  details: document.querySelector("#details"),
  empty: document.querySelector("#empty-state"),
  list: document.querySelector("#capture-list"),
  loadSchema: document.querySelector("#load-schema"),
  pause: document.querySelector("#pause"),
  runtimeError: document.querySelector("#runtime-error"),
  schemaFile: document.querySelector("#schema-file"),
  schemaStatus: document.querySelector("#schema-status"),
};

const decoderReady = initDecoder().catch((error) => {
  elements.runtimeError.hidden = false;
  elements.runtimeError.textContent = `The CSIL decoder could not start: ${error}`;
  throw error;
});

elements.clear.addEventListener("click", clearCaptures);
elements.loadSchema.addEventListener("click", () => elements.schemaFile.click());
elements.schemaFile.addEventListener("change", () => void loadSchemaFile());
elements.pause.addEventListener("click", () => {
  paused = !paused;
  elements.pause.textContent = paused ? "Resume" : "Pause";
  elements.pause.classList.toggle("active", paused);
});

api.devtools.network.onRequestFinished.addListener((entry) => {
  if (!paused) {
    void captureRequest(entry);
  }
});

async function captureRequest(entry) {
  try {
    await decoderReady;
    const request = entry.request;
    const requestType = headerValue(request.headers, "content-type");
    const postData = request.postData;
    if (!postData?.text) {
      return;
    }

    const requestBody = decodeBodyCandidates(
      postData.text,
      postData.encoding ?? postData._encoding,
      postData.mimeType ?? requestType,
      decode_cbor,
    );
    const classification = classifyCsil(requestBody?.decoded, request, Boolean(requestBody?.error));
    if (!classification) {
      return;
    }

    const responseContent = await getResponseContent(entry);
    const responseType = headerValue(entry.response.headers, "content-type") ||
      entry.response.content?.mimeType || "";
    const responseBody = responseContent.text === null ? null : decodeBodyCandidates(
      responseContent.text,
      responseContent.encoding,
      responseType,
      decode_cbor,
    );

    await discoverRuntimeLocators(request.url);
    const headerLocator = parseSchemaLocator(
      headerValue(entry.response.headers, "csil-schema"),
      request.url,
    );
    const locator = headerLocator ? registerLocator(headerLocator) : locatorForRoute(classification);
    if (locator) {
      await loadLocator(locator, false);
    }

    const capture = {
      id: nextId++,
      at: new Date(),
      classification,
      method: request.method,
      url: request.url,
      httpStatus: entry.response.status,
      httpStatusText: entry.response.statusText,
      requestHeaders: request.headers ?? [],
      responseHeaders: entry.response.headers ?? [],
      requestBody,
      responseBody,
      responseError: responseContent.error,
      duration: entry.time,
      locatorKey: locator ? locatorKey(locator) : null,
      schemaDigest: null,
      requestTyped: null,
      responseTyped: null,
    };
    applySchema(capture);
    captures.unshift(capture);
    if (captures.length > maximumCaptures) {
      captures.length = maximumCaptures;
    }
    selectedId ??= capture.id;
    render();
  } catch (error) {
    elements.runtimeError.hidden = false;
    elements.runtimeError.textContent = `A captured request could not be inspected: ${error}`;
  }
}

function getResponseContent(entry) {
  return new Promise((resolve) => {
    let settled = false;
    const finish = (text, encoding) => {
      if (!settled) {
        settled = true;
        resolve({ text: typeof text === "string" ? text : null, encoding, error: null });
      }
    };
    try {
      const possiblePromise = entry.getContent(finish);
      if (possiblePromise?.then) {
        possiblePromise.then((value) => {
          if (Array.isArray(value)) {
            finish(value[0], value[1]);
          } else if (typeof value === "string") {
            finish(value, undefined);
          }
        }).catch((error) => {
          if (!settled) {
            settled = true;
            resolve({ text: null, encoding: null, error: String(error) });
          }
        });
      }
    } catch (error) {
      resolve({ text: null, encoding: null, error: String(error) });
    }
  });
}

async function loadSchemaFile() {
  const file = elements.schemaFile.files?.[0];
  elements.schemaFile.value = "";
  if (!file) {
    return;
  }
  if (file.size > maximumSchemaBytes) {
    showRuntimeError(`The schema is ${file.size} bytes. The limit is ${maximumSchemaBytes} bytes.`);
    return;
  }
  try {
    await decoderReady;
    const bytes = new Uint8Array(await file.arrayBuffer());
    const info = descriptor_info(bytes);
    schemas.set(info.digestHex, { bytes, info, source: "file", url: null });
    reapplySchemas();
  } catch (error) {
    showRuntimeError(`The selected schema could not be loaded: ${error}`);
  }
}

function registerLocator(locator) {
  const key = locatorKey(locator);
  const existing = locators.get(key);
  if (existing) {
    return existing;
  }
  const record = {
    ...locator,
    key,
    state: "idle",
    message: null,
    schemaDigest: null,
    retryAt: null,
  };
  locators.set(key, record);
  updateSchemaStatus();
  return record;
}

async function loadLocator(locator, userInitiated) {
  if (!userInitiated && !shouldAutomaticallyLoad(locator)) {
    return;
  }
  if (userInitiated && (locator.state === "loaded" || locator.state === "loading")) {
    return;
  }

  let permission;
  try {
    permission = originPermission(locator.url);
  } catch {
    setLocatorFailure(locator, "invalid-url", "The schema URL is invalid.");
    return;
  }

  let permitted = await api.permissions.contains({ origins: [permission] });
  if (!permitted && userInitiated) {
    permitted = await api.permissions.request({ origins: [permission] });
  }
  if (!permitted) {
    setLocatorFailure(locator, "permission", `Permission is required to load ${locator.url}.`);
    return;
  }

  locator.state = "loading";
  locator.message = null;
  updateSchemaStatus();
  try {
    const response = await fetch(locator.url, { credentials: "include", cache: "no-cache" });
    const declaredLength = Number(response.headers.get("content-length"));
    if (Number.isFinite(declaredLength) && declaredLength > maximumSchemaBytes) {
      setLocatorFailure(locator, "too-large", `The schema exceeds the ${maximumSchemaBytes}-byte limit.`);
      return;
    }
    if (!response.ok) {
      const retryAfter = response.status === 429 ? retryAfterTime(response.headers.get("retry-after")) : null;
      locator.retryAt = retryAfter;
      const state = response.status === 401 || response.status === 403
        ? "denied"
        : response.status === 404 || response.status === 410
          ? "missing"
          : "http-error";
      if (state === "http-error" && !locator.retryAt) {
        locator.retryAt = Date.now() + 30_000;
      }
      setLocatorFailure(locator, state, `Schema request returned HTTP ${response.status}.`);
      return;
    }

    const bytes = new Uint8Array(await response.arrayBuffer());
    if (bytes.length > maximumSchemaBytes) {
      setLocatorFailure(locator, "too-large", `The schema exceeds the ${maximumSchemaBytes}-byte limit.`);
      return;
    }
    let info;
    try {
      info = descriptor_info(bytes);
    } catch (error) {
      setLocatorFailure(locator, "invalid-descriptor", String(error));
      return;
    }
    if (locator.digest && locator.digest !== info.digestHex) {
      setLocatorFailure(locator, "digest-mismatch", `Expected ${locator.digest}, but the descriptor is ${info.digestHex}.`);
      return;
    }
    if (locator.version && locator.version !== info.version && locator.version !== "1") {
      setLocatorFailure(locator, "version-mismatch", `Expected ${locator.version}, but the descriptor is ${info.version}.`);
      return;
    }

    schemas.set(info.digestHex, { bytes, info, source: locator.source, url: locator.url });
    locator.state = "loaded";
    locator.message = null;
    locator.schemaDigest = info.digestHex;
    locator.retryAt = null;
    reapplySchemas();
  } catch (error) {
    locator.retryAt = Date.now() + 30_000;
    setLocatorFailure(locator, "network-error", String(error));
  }
}

function setLocatorFailure(locator, state, message) {
  locator.state = state;
  locator.message = message;
  updateSchemaStatus();
  render();
}

async function discoverRuntimeLocators(baseUrl) {
  if (Date.now() - lastRuntimeDiscovery < 1000) {
    return;
  }
  lastRuntimeDiscovery = Date.now();
  const expression = `(() => {
    try {
      const direct = typeof globalThis.getCSILdefs === "function" ? globalThis.getCSILdefs() : null;
      const registry = globalThis.__CSIL_DEVTOOLS_V1__;
      const listed = direct ?? (registry && typeof registry.listDefinitions === "function" ? registry.listDefinitions() : []);
      const values = Array.isArray(listed) ? listed : [listed];
      return values.filter(Boolean).map((value) => ({ version: value.version, digest: value.digest, url: value.url }));
    } catch (error) {
      return { error: String(error) };
    }
  })()`;
  const result = await inspectedEval(expression);
  if (!Array.isArray(result)) {
    return;
  }
  for (const value of result) {
    const locator = normalizeRuntimeLocator(value, baseUrl);
    if (!locator) {
      continue;
    }
    const registered = registerLocator(locator);
    await loadLocator(registered, false);
  }
}

function inspectedEval(expression) {
  return new Promise((resolve) => {
    let settled = false;
    const finish = (value, exception) => {
      if (!settled) {
        settled = true;
        resolve(exception ? null : value);
      }
    };
    try {
      const possiblePromise = api.devtools.inspectedWindow.eval(expression, finish);
      if (possiblePromise?.then) {
        possiblePromise.then((value) => {
          if (Array.isArray(value) && value.length === 2) {
            finish(value[0], value[1]);
          } else {
            finish(value, null);
          }
        }).catch(() => finish(null, true));
      }
    } catch {
      finish(null, true);
    }
  });
}

function locatorForRoute(classification) {
  for (const locator of locators.values()) {
    if (!classification.service || !classification.operation) {
      return locator;
    }
    const schema = locator.schemaDigest ? schemas.get(locator.schemaDigest) : null;
    if (!schema || schemaMatchesRoute(schema, classification.service, classification.operation)) {
      return locator;
    }
  }
  return null;
}

function schemaForCapture(capture) {
  if (capture.locatorKey) {
    const locator = locators.get(capture.locatorKey);
    const located = locator?.schemaDigest ? schemas.get(locator.schemaDigest) : null;
    if (located) {
      return located;
    }
  }
  if (!capture.classification.service || !capture.classification.operation) {
    return schemas.size === 1 ? schemas.values().next().value : null;
  }
  const matches = Array.from(schemas.values()).filter((schema) => schemaMatchesRoute(
    schema,
    capture.classification.service,
    capture.classification.operation,
  ));
  return matches.length === 1 ? matches[0] : null;
}

function applySchema(capture) {
  capture.requestTyped = null;
  capture.responseTyped = null;
  capture.schemaDigest = null;
  capture.schemaError = null;
  const schema = schemaForCapture(capture);
  const { service, operation } = capture.classification;
  if (!schema || !operation) {
    return;
  }
  if (capture.classification.kind !== "event" && !service) {
    return;
  }
  try {
    const requestPayload = embeddedPayload(capture.requestBody?.decoded?.generic);
    if (capture.classification.kind === "event") {
      if (requestPayload) {
        capture.requestTyped = inspect_event_verbose(
          schema.bytes,
          service,
          operation,
          false,
          true,
          requestPayload,
        );
      }
      capture.schemaDigest = schema.info.digestHex;
      return;
    }
    if (!service) {
      return;
    }
    if (requestPayload) {
      capture.requestTyped = inspect_rpc_request(schema.bytes, service, operation, true, requestPayload);
    }
    const responseMap = textMap(capture.responseBody?.decoded?.generic);
    const responsePayload = embeddedPayload(capture.responseBody?.decoded?.generic);
    const variantNode = responseMap?.get("variant");
    const variant = variantNode?.kind === "text" ? variantNode.value : null;
    if (responsePayload) {
      capture.responseTyped = inspect_rpc_response(
        schema.bytes,
        service,
        operation,
        variant,
        false,
        responsePayload,
      );
    }
    capture.schemaDigest = schema.info.digestHex;
  } catch (error) {
    capture.schemaError = String(error);
  }
}

function reapplySchemas() {
  for (const capture of captures) {
    applySchema(capture);
  }
  updateSchemaStatus();
  render();
}

function updateSchemaStatus() {
  if (schemas.size > 0) {
    const roots = Array.from(schemas.values(), (schema) => schema.info.root);
    elements.schemaStatus.textContent = roots.length === 1 ? `Schema: ${roots[0]}` : `${roots.length} schemas`;
    elements.schemaStatus.className = "schema-status loaded";
    return;
  }
  const failure = Array.from(locators.values()).find((locator) => !["idle", "loading"].includes(locator.state));
  if (failure) {
    elements.schemaStatus.textContent = `Schema: ${failure.state.replaceAll("-", " ")}`;
    elements.schemaStatus.className = "schema-status error";
  } else if (locators.size > 0) {
    elements.schemaStatus.textContent = "Schema configured";
    elements.schemaStatus.className = "schema-status";
  } else {
    elements.schemaStatus.textContent = "No schema";
    elements.schemaStatus.className = "schema-status";
  }
}

function retryAfterTime(value) {
  if (!value) {
    return Date.now() + 30_000;
  }
  const seconds = Number(value);
  if (Number.isFinite(seconds)) {
    return Date.now() + Math.max(0, seconds) * 1000;
  }
  const date = Date.parse(value);
  return Number.isNaN(date) ? Date.now() + 30_000 : date;
}

function showRuntimeError(message) {
  elements.runtimeError.hidden = false;
  elements.runtimeError.textContent = message;
}

function clearCaptures() {
  captures.length = 0;
  selectedId = null;
  render();
}

function render() {
  updateSchemaStatus();
  elements.count.textContent = String(captures.length);
  elements.empty.hidden = captures.length !== 0;
  elements.list.replaceChildren(...captures.map(captureListItem));
  const selected = captures.find((capture) => capture.id === selectedId);
  if (selected) {
    renderDetails(selected);
  } else {
    elements.details.replaceChildren(node("div", "details-placeholder", "Select a captured message."));
  }
}

function captureListItem(capture) {
  const button = node("button", `capture ${capture.id === selectedId ? "selected" : ""}`);
  button.type = "button";
  button.addEventListener("click", () => {
    selectedId = capture.id;
    render();
  });

  const primary = node("span", "capture-primary");
  primary.append(
    node("span", `kind kind-${capture.classification.confidence}`, capture.classification.kind.replaceAll("-", " ")),
    node("strong", "capture-label", capture.classification.label),
    node("span", "capture-url", capture.url),
  );
  const statusClass = capture.httpStatus >= 400 ? "http-status error" : "http-status";
  button.append(primary, node("span", statusClass, String(capture.httpStatus)));
  return button;
}

function renderDetails(capture) {
  const title = node("div", "details-title");
  const heading = document.createElement("h1");
  heading.textContent = capture.classification.label;
  const subtitle = node(
    "div",
    "details-subtitle",
    `${capture.method} ${capture.url} · ${formatDuration(capture.duration)}`,
  );
  title.append(heading, subtitle, schemaStateView(capture));

  const tabs = node("div", "tabs");
  const body = node("div", "tab-body");
  const views = [
    ["Request", () => messageBody(capture.requestBody, "request", capture.requestTyped)],
    ["Response", () => responseView(capture)],
    ["Headers", () => headersView(capture)],
    ["Raw", () => rawView(capture)],
  ];
  views.forEach(([label, factory], index) => {
    const button = node("button", index === 0 ? "active" : "", label);
    button.type = "button";
    button.addEventListener("click", () => {
      tabs.querySelectorAll("button").forEach((item) => item.classList.remove("active"));
      button.classList.add("active");
      body.replaceChildren(factory());
    });
    tabs.append(button);
  });
  body.append(messageBody(capture.requestBody, "request", capture.requestTyped));
  elements.details.replaceChildren(title, tabs, body);
}

function schemaStateView(capture) {
  const container = node("div", "schema-state");
  if (capture.schemaDigest) {
    container.append(
      node("span", "schema-chip loaded", "Typed with schema"),
      node("code", "", capture.schemaDigest.slice(0, 16)),
    );
    return container;
  }
  if (capture.schemaError) {
    container.append(node("span", "schema-chip error", "Schema error"), node("span", "", capture.schemaError));
    return container;
  }
  const locator = capture.locatorKey ? locators.get(capture.locatorKey) : null;
  if (!locator) {
    container.append(node("span", "schema-chip", "Generic CBOR"));
    return container;
  }

  container.append(
    node("span", `schema-chip ${locator.state === "loaded" ? "loaded" : "error"}`, `Schema: ${locator.state}`),
    node("span", "schema-url", locator.message ?? locator.url),
  );
  if (locator.state !== "loaded" && locator.state !== "loading") {
    const retry = node("button", "schema-retry", locator.state === "permission" ? "Allow and load" : "Retry");
    retry.type = "button";
    retry.addEventListener("click", () => void loadLocator(locator, true));
    container.append(retry);
  }
  return container;
}

function messageBody(body, label, typed = null) {
  const container = node("div", "message-view");
  if (!body) {
    container.append(node("div", "notice", `No ${label} body was available.`));
    return container;
  }
  if (body.error) {
    container.append(node("div", "notice error", body.error));
  }
  if (typed?.typed) {
    container.append(section("Typed CSIL payload", typedValueTree(typed.typed)));
  }
  if (typed?.diagnostics?.length) {
    container.append(section("Schema diagnostics", diagnosticsView(typed)));
  }
  if (body.decoded?.generic) {
    container.append(section("Decoded CBOR", valueTree(body.decoded.generic)));
    const embedded = embeddedPayload(body.decoded.generic);
    if (embedded) {
      const decoded = decode_cbor(embedded);
      container.prepend(section("CSIL payload", decoded.generic ? valueTree(decoded.generic) : diagnosticsView(decoded)));
    }
  }
  if (body.decoded?.diagnostics?.length) {
    container.append(section("Diagnostics", diagnosticsView(body.decoded)));
  }
  container.append(node("div", "encoding-note", `Body interpretation: ${body.source}`));
  return container;
}

function responseView(capture) {
  const container = messageBody(capture.responseBody, "response", capture.responseTyped);
  if (capture.responseError) {
    container.prepend(node("div", "notice error", `Response body unavailable: ${capture.responseError}`));
  }
  container.prepend(node(
    "div",
    capture.httpStatus >= 400 ? "response-status error" : "response-status",
    `${capture.httpStatus} ${capture.httpStatusText}`,
  ));
  return container;
}

function headersView(capture) {
  const container = node("div", "headers-view");
  container.append(
    section("Request headers", headerTable(capture.requestHeaders)),
    section("Response headers", headerTable(capture.responseHeaders)),
  );
  return container;
}

function headerTable(headers) {
  const table = node("dl", "header-table");
  for (const header of headers) {
    table.append(node("dt", "", header.name), node("dd", "", header.value));
  }
  return table;
}

function rawView(capture) {
  const container = node("div", "raw-view");
  container.append(
    section("Request bytes", rawBytes(capture.requestBody?.bytes)),
    section("Response bytes", rawBytes(capture.responseBody?.bytes)),
  );
  return container;
}

function rawBytes(bytes) {
  if (!(bytes instanceof Uint8Array)) {
    return node("div", "notice", "Raw bytes are not available.");
  }
  const pre = node("pre", "hex");
  pre.textContent = bytesToHex(bytes);
  return pre;
}

function valueTree(value, label = null) {
  if (!value) {
    return node("span", "scalar null", "null");
  }
  if (value.kind === "map") {
    const details = treeDetails(label ?? `map · ${value.entries.length} entries`);
    for (const entry of value.entries) {
      const row = node("div", "tree-row");
      row.append(node("span", "tree-key", compactValue(entry.key)), valueTree(entry.value));
      details.body.append(row);
    }
    return details.root;
  }
  if (value.kind === "array") {
    const details = treeDetails(label ?? `array · ${value.items.length} items`);
    value.items.forEach((item, index) => {
      const row = node("div", "tree-row");
      row.append(node("span", "tree-key", String(index)), valueTree(item));
      details.body.append(row);
    });
    return details.root;
  }
  if (value.kind === "tag") {
    const details = treeDetails(label ?? `tag ${value.tag.toString()}`);
    details.body.append(valueTree(value.value));
    if (value.tag === 24n && value.value?.kind === "bytes") {
      const embedded = decode_cbor(value.value.value);
      if (embedded.generic) {
        details.body.append(node("div", "embedded-label", "Embedded CBOR"), valueTree(embedded.generic));
      }
    }
    return details.root;
  }
  if (value.kind === "timestamp") {
    return scalar("timestamp", `${compactValue(value.value)} · tag ${value.originalTag}`);
  }
  if (value.kind === "decimal") {
    return scalar("decimal", decimalText(value.mantissa, value.exponent));
  }
  return scalar(value.kind, compactValue(value));
}

function typedValueTree(value, label = null) {
  if (!value) {
    return node("span", "scalar null", "null");
  }
  if (value.kind === "value") {
    return valueTree(value.value, label);
  }
  if (value.kind === "record") {
    const details = treeDetails(label ?? `record · ${value.fields.length} fields`);
    for (const field of value.fields) {
      const row = node("div", "tree-row");
      row.append(
        node("span", "tree-key", field.name ?? compactValue(field.key)),
        typedValueTree(field.value),
      );
      details.body.append(row);
    }
    for (const field of value.unknownFields) {
      const row = node("div", "tree-row unknown-field");
      row.append(node("span", "tree-key", `${compactValue(field.key)} · unknown`), valueTree(field.value));
      details.body.append(row);
    }
    return details.root;
  }
  if (value.kind === "array" || value.kind === "tuple") {
    const details = treeDetails(label ?? `${value.kind} · ${value.items.length} items`);
    value.items.forEach((item, index) => {
      const row = node("div", "tree-row");
      row.append(node("span", "tree-key", String(index)), typedValueTree(item));
      details.body.append(row);
    });
    return details.root;
  }
  if (value.kind === "map") {
    const details = treeDetails(label ?? `map · ${value.entries.length} entries`);
    for (const entry of value.entries) {
      const row = node("div", "tree-row");
      row.append(typedValueTree(entry.key), typedValueTree(entry.value));
      details.body.append(row);
    }
    return details.root;
  }
  if (value.kind === "choice") {
    const details = treeDetails(label ?? `choice arm ${value.armIndex}`);
    details.body.append(node("div", "declared-arm", value.declaredArm), typedValueTree(value.value));
    return details.root;
  }
  return node("span", "scalar", value.kind);
}

function compactValue(value) {
  if (!value) return "null";
  switch (value.kind) {
    case "text": return JSON.stringify(value.value);
    case "integer": return value.value.toString();
    case "float": return `${String(value.value)} (f${value.width})`;
    case "bytes": return `bytes[${value.value.length}] ${shortHex(value.value)}`;
    case "boolean": return String(value.value);
    case "null": return "null";
    case "undefined": return "undefined";
    case "simple": return `simple(${value.value})`;
    case "decimal": return decimalText(value.mantissa, value.exponent);
    case "timestamp": return compactValue(value.value);
    default: return value.kind;
  }
}

function scalar(kind, text) {
  const value = node("span", `scalar ${kind}`);
  value.append(node("span", "scalar-value", text), node("span", "scalar-kind", kind));
  return value;
}

function treeDetails(summary) {
  const root = node("details", "tree-group");
  root.open = true;
  root.append(node("summary", "", summary));
  const body = node("div", "tree-body");
  root.append(body);
  return { root, body };
}

function diagnosticsView(result) {
  const list = node("div", "diagnostics");
  for (const diagnostic of result.diagnostics ?? []) {
    const item = node("div", "diagnostic");
    item.append(
      node("strong", "", diagnostic.message),
      node("span", "", `${diagnostic.schemaPath}${diagnostic.offset === null ? "" : ` · byte ${diagnostic.offset}`}`),
    );
    list.append(item);
  }
  return list;
}

function section(title, content) {
  const root = node("section", "detail-section");
  root.append(node("h2", "", title), content);
  return root;
}

function node(tag, className = "", text = null) {
  const element = document.createElement(tag);
  if (className) element.className = className;
  if (text !== null) element.textContent = text;
  return element;
}

function shortHex(bytes) {
  return Array.from(bytes.subarray(0, 16), (byte) => byte.toString(16).padStart(2, "0")).join(" ") +
    (bytes.length > 16 ? " …" : "");
}

function decimalText(mantissa, exponent) {
  const negative = mantissa < 0n;
  let digits = (negative ? -mantissa : mantissa).toString();
  const exp = Number(exponent);
  if (!Number.isSafeInteger(exp)) {
    return `${mantissa} × 10^${exponent}`;
  }
  if (exp >= 0) {
    digits += "0".repeat(exp);
  } else {
    const position = digits.length + exp;
    digits = position > 0 ? `${digits.slice(0, position)}.${digits.slice(position)}` : `0.${"0".repeat(-position)}${digits}`;
  }
  return negative ? `-${digits}` : digits;
}

function formatDuration(milliseconds) {
  if (!Number.isFinite(milliseconds)) return "unknown duration";
  return milliseconds < 1000 ? `${milliseconds.toFixed(1)} ms` : `${(milliseconds / 1000).toFixed(2)} s`;
}

render();
