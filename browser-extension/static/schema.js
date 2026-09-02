export const maximumSchemaBytes = 10 * 1024 * 1024;

export function parseSchemaLocator(value, baseUrl, source = "header") {
  if (typeof value !== "string" || value.trim() === "") {
    return null;
  }
  const parts = value.split(";").map((part) => part.trim()).filter(Boolean);
  let urlValue = parts.shift();
  if (urlValue?.startsWith("url=")) {
    urlValue = unquote(urlValue.slice(4));
  } else {
    urlValue = urlValue?.replace(/^<|>$/g, "");
  }
  if (!urlValue) {
    return null;
  }

  const parameters = new Map();
  for (const part of parts) {
    const separator = part.indexOf("=");
    if (separator > 0) {
      parameters.set(part.slice(0, separator).trim().toLowerCase(), unquote(part.slice(separator + 1).trim()));
    }
  }

  try {
    return {
      url: new URL(urlValue, baseUrl).href,
      digest: normalizeDigest(parameters.get("digest")),
      version: parameters.get("version") ?? null,
      source,
    };
  } catch {
    return null;
  }
}

export function normalizeRuntimeLocator(value, baseUrl) {
  if (!value || typeof value !== "object" || typeof value.url !== "string") {
    return null;
  }
  try {
    return {
      url: new URL(value.url, baseUrl).href,
      digest: normalizeDigest(value.digest),
      version: typeof value.version === "string" || typeof value.version === "number"
        ? String(value.version)
        : null,
      source: "runtime",
    };
  } catch {
    return null;
  }
}

export function normalizeDigest(value) {
  if (typeof value !== "string") {
    return null;
  }
  const normalized = value.trim().toLowerCase().replace(/^sha-?256[:=-]?/, "");
  return /^[0-9a-f]{64}$/.test(normalized) ? normalized : null;
}

export function locatorKey(locator) {
  return `${locator.url}\n${locator.digest ?? ""}`;
}

export function originPermission(url) {
  const parsed = new URL(url);
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    throw new TypeError("Schema URLs must use HTTP or HTTPS.");
  }
  return `${parsed.protocol}//${parsed.host}/*`;
}

export function schemaMatchesRoute(schema, service, operation) {
  return schema.info.services.some((candidate) =>
    (!service || candidate.name === service) &&
      candidate.operations.some((item) => item.name === operation));
}

export function shouldAutomaticallyLoad(locator, now = Date.now()) {
  if (locator.state === "loaded" || locator.state === "loading") {
    return false;
  }
  if ([
    "denied",
    "digest-mismatch",
    "invalid-descriptor",
    "invalid-url",
    "missing",
    "permission",
    "too-large",
    "version-mismatch",
  ].includes(locator.state)) {
    return false;
  }
  return !locator.retryAt || now >= locator.retryAt;
}

function unquote(value) {
  if (value.length >= 2 && value.startsWith('"') && value.endsWith('"')) {
    return value.slice(1, -1).replace(/\\"/g, '"');
  }
  return value;
}
