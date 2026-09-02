const encoder = new TextEncoder();
export const maximumBodyBytes = 16 * 1024 * 1024;

export function headerValue(headers, name) {
  const wanted = name.toLowerCase();
  return headers?.find((header) => header.name.toLowerCase() === wanted)?.value ?? "";
}

export function decodeBodyCandidates(
  text,
  encodingHint,
  contentType,
  decode,
  maximumBytes = maximumBodyBytes,
) {
  if (typeof text !== "string" || text.length === 0) {
    return null;
  }

  const candidates = [];
  const hint = String(encodingHint ?? "").toLowerCase();
  const binaryType = !/^text\//i.test(contentType ?? "") &&
    !/(json|javascript|xml|form-urlencoded)/i.test(contentType ?? "");

  if (hint === "base64" || (binaryType && looksLikeBase64(text))) {
    addCandidate(candidates, "base64", fromBase64(text, maximumBytes), maximumBytes);
  }
  if (text.length <= maximumBytes) {
    addCandidate(candidates, "binary string", fromBinaryString(text), maximumBytes);
    addCandidate(candidates, "UTF-8 text", encoder.encode(text), maximumBytes);
  }
  if (hint !== "base64" && looksLikeBase64(text)) {
    addCandidate(candidates, "base64", fromBase64(text, maximumBytes), maximumBytes);
  }
  if (candidates.length === 0) {
    return {
      source: "not decoded",
      bytes: null,
      decoded: null,
      error: `The body exceeds the ${maximumBytes}-byte inspection limit.`,
    };
  }

  let firstFailure = null;
  for (const candidate of candidates) {
    try {
      const decoded = decode(candidate.bytes);
      const result = { ...candidate, decoded };
      if (decoded?.generic) {
        return result;
      }
      firstFailure ??= result;
    } catch (error) {
      firstFailure ??= { ...candidate, error: String(error) };
    }
  }
  return firstFailure;
}

export function classifyCsil(decoded, request, bodyError = false) {
  const generic = decoded?.generic;
  const values = textMap(generic);
  const payload = values?.get("payload");
  const hasEmbeddedPayload = payload?.kind === "tag" && payload.tag === 24n &&
    payload.value?.kind === "bytes";

  if (values && isText(values.get("service")) && isText(values.get("op")) && hasEmbeddedPayload) {
    return {
      kind: "rpc-request",
      confidence: "envelope",
      service: values.get("service").value,
      operation: values.get("op").value,
      label: `${values.get("service").value}.${values.get("op").value}`,
    };
  }

  if (values && values.has("status") && hasEmbeddedPayload) {
    const status = scalarText(values.get("status"));
    return {
      kind: "rpc-response",
      confidence: "envelope",
      status,
      label: `RPC response · status ${status}`,
    };
  }

  if (values && isText(values.get("event")) && hasEmbeddedPayload) {
    const service = isText(values.get("service")) ? values.get("service").value : null;
    return {
      kind: "event",
      confidence: "envelope",
      service,
      operation: values.get("event").value,
      label: `${service ? `${service}.` : ""}${values.get("event").value}`,
    };
  }

  const contentType = headerValue(request.headers, "content-type");
  const urlSignal = safePathname(request.url).endsWith("/rpc");
  const headerSignal = /(?:application\/(?:[^;]+\+)?cbor|csil)/i.test(contentType) ||
    request.headers?.some((header) => /^x-csil-|^csil-/i.test(header.name));
  if ((generic || bodyError) && (urlSignal || headerSignal)) {
    return {
      kind: "cbor-candidate",
      confidence: urlSignal ? "rpc-path" : "header",
      label: "CSIL CBOR candidate",
    };
  }
  return null;
}

export function textMap(value) {
  if (value?.kind !== "map") {
    return null;
  }
  const result = new Map();
  for (const entry of value.entries) {
    if (entry.key?.kind === "text") {
      result.set(entry.key.value, entry.value);
    }
  }
  return result;
}

export function embeddedPayload(value) {
  const map = textMap(value);
  const payload = map?.get("payload");
  if (payload?.kind === "tag" && payload.tag === 24n && payload.value?.kind === "bytes") {
    return payload.value.value;
  }
  return null;
}

export function bytesToHex(bytes, maximum = 512) {
  if (!(bytes instanceof Uint8Array)) {
    return "";
  }
  const shown = bytes.subarray(0, maximum);
  const parts = [];
  for (let offset = 0; offset < shown.length; offset += 16) {
    const line = shown.subarray(offset, offset + 16);
    const address = offset.toString(16).padStart(6, "0");
    const hex = Array.from(line, (byte) => byte.toString(16).padStart(2, "0")).join(" ");
    const text = Array.from(line, (byte) => byte >= 32 && byte <= 126 ? String.fromCharCode(byte) : ".").join("");
    parts.push(`${address}  ${hex.padEnd(47)}  ${text}`);
  }
  if (bytes.length > maximum) {
    parts.push(`… ${bytes.length - maximum} more bytes`);
  }
  return parts.join("\n");
}

function addCandidate(candidates, source, bytes, maximumBytes) {
  if (!bytes || bytes.length > maximumBytes ||
      candidates.some((candidate) => sameBytes(candidate.bytes, bytes))) {
    return;
  }
  candidates.push({ source, bytes });
}

function sameBytes(left, right) {
  return left.length === right.length && left.every((byte, index) => byte === right[index]);
}

function fromBinaryString(text) {
  const bytes = new Uint8Array(text.length);
  for (let index = 0; index < text.length; index += 1) {
    const code = text.charCodeAt(index);
    if (code > 255) {
      return null;
    }
    bytes[index] = code;
  }
  return bytes;
}

function fromBase64(text, maximumBytes) {
  try {
    const normalized = text.replace(/\s+/g, "");
    if (normalized.length > Math.ceil(maximumBytes / 3) * 4) {
      return null;
    }
    const binary = atob(normalized);
    return fromBinaryString(binary);
  } catch {
    return null;
  }
}

function looksLikeBase64(text) {
  const normalized = text.replace(/\s+/g, "");
  return normalized.length >= 4 && normalized.length % 4 === 0 &&
    /^[A-Za-z0-9+/]*={0,2}$/.test(normalized);
}

function isText(value) {
  return value?.kind === "text";
}

function scalarText(value) {
  if (!value) {
    return "unknown";
  }
  if (typeof value.value === "bigint") {
    return value.value.toString();
  }
  return String(value.value ?? value.kind);
}

function safePathname(url) {
  try {
    return new URL(url).pathname.replace(/\/+$/, "");
  } catch {
    return "";
  }
}
