import { test } from "node:test";
import assert from "node:assert/strict";

import worker, {
  handleInstall,
  pickVariant,
  upstreamUrl,
  errorScript,
} from "./installer.mjs";

const SH_URL =
  "https://github.com/nexus-xyz/nexus-exchange-cli/releases/latest/download/nexus-exchange-cli-installer.sh";
const PS1_URL =
  "https://github.com/nexus-xyz/nexus-exchange-cli/releases/latest/download/nexus-exchange-cli-installer.ps1";

const SH_BODY = "#!/bin/sh\necho installing nexus\n";
const PS1_BODY = "# nexus installer\nWrite-Host 'installing nexus'\n";

// Build a fetch stub that records the URL/options it was called with and
// returns a canned upstream Response. Lets every test assert exactly what would
// hit the network — the core SSRF guarantee.
function stubFetch({ body = SH_BODY, status = 200, contentLength, throws = false } = {}) {
  const calls = [];
  const fetchImpl = async (url, opts) => {
    calls.push({ url, opts });
    if (throws) throw new Error("network down");
    const headers = new Headers();
    headers.set("content-length", String(contentLength ?? new TextEncoder().encode(body).length));
    return new Response(body, { status, headers });
  };
  return { fetchImpl, calls };
}

function req(url = "https://cli.nexus.xyz/", { method = "GET", ua } = {}) {
  const headers = new Headers();
  if (ua) headers.set("user-agent", ua);
  return new Request(url, { method, headers });
}

test("serves the shell installer by default (bare curl)", async () => {
  const { fetchImpl, calls } = stubFetch({ body: SH_BODY });
  const res = await handleInstall(req("https://cli.nexus.xyz/", { ua: "curl/8.4.0" }), {}, { fetch: fetchImpl });

  assert.equal(res.status, 200);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].url, SH_URL);
  assert.equal(res.headers.get("content-type"), "text/plain; charset=utf-8");
  assert.equal(res.headers.get("x-content-type-options"), "nosniff");
  assert.equal(res.headers.get("x-installer-variant"), "sh");
  assert.match(res.headers.get("strict-transport-security") || "", /max-age=/);
  assert.equal(await res.text(), SH_BODY);
});

test("serves powershell installer for PowerShell user-agent", async () => {
  const { fetchImpl, calls } = stubFetch({ body: PS1_BODY });
  const res = await handleInstall(
    req("https://cli.nexus.xyz/", { ua: "Mozilla/5.0 (Windows NT 10.0) WindowsPowerShell/5.1.19041.4291" }),
    {},
    { fetch: fetchImpl },
  );

  assert.equal(res.status, 200);
  assert.equal(calls[0].url, PS1_URL);
  assert.equal(res.headers.get("x-installer-variant"), "ps1");
  assert.equal(await res.text(), PS1_BODY);
});

test("explicit .ps1 path forces powershell variant", async () => {
  const { fetchImpl, calls } = stubFetch({ body: PS1_BODY });
  const res = await handleInstall(req("https://cli.nexus.xyz/install.ps1", { ua: "curl/8" }), {}, { fetch: fetchImpl });
  assert.equal(res.status, 200);
  assert.equal(calls[0].url, PS1_URL);
});

test("?powershell query forces powershell variant", async () => {
  const { fetchImpl, calls } = stubFetch({ body: PS1_BODY });
  const res = await handleInstall(req("https://cli.nexus.xyz/?powershell"), {}, { fetch: fetchImpl });
  assert.equal(res.status, 200);
  assert.equal(calls[0].url, PS1_URL);
});

test("explicit .sh path forces shell even for a powershell UA", async () => {
  const { fetchImpl, calls } = stubFetch({ body: SH_BODY });
  const res = await handleInstall(
    req("https://cli.nexus.xyz/install.sh", { ua: "PowerShell/7.4.0" }),
    {},
    { fetch: fetchImpl },
  );
  assert.equal(calls[0].url, SH_URL);
  assert.equal(res.status, 200);
});

test("non-GET/HEAD methods are rejected with 405 and a safe script", async () => {
  const { fetchImpl, calls } = stubFetch();
  const res = await handleInstall(req("https://cli.nexus.xyz/", { method: "POST" }), {}, { fetch: fetchImpl });
  assert.equal(res.status, 405);
  assert.equal(res.headers.get("allow"), "GET, HEAD");
  assert.equal(calls.length, 0, "must not touch the network for a rejected method");
  assert.match(await res.text(), /^#!\/bin\/sh/);
});

test("HEAD returns headers and content-length but no body", async () => {
  const { fetchImpl } = stubFetch({ body: SH_BODY });
  const res = await handleInstall(req("https://cli.nexus.xyz/", { method: "HEAD" }), {}, { fetch: fetchImpl });
  assert.equal(res.status, 200);
  assert.equal(res.headers.get("content-length"), String(new TextEncoder().encode(SH_BODY).length));
  assert.equal(await res.text(), "");
});

test("upstream non-200 yields a 502 error script, not the upstream body", async () => {
  const { fetchImpl } = stubFetch({ status: 404, body: "<!DOCTYPE html><title>Not Found</title>" });
  const res = await handleInstall(req(), {}, { fetch: fetchImpl });
  assert.equal(res.status, 502);
  const text = await res.text();
  assert.match(text, /^#!\/bin\/sh/);
  assert.match(text, /HTTP 404/);
  assert.doesNotMatch(text, /DOCTYPE/);
});

test("a 200 HTML body is treated as invalid (never piped to a shell)", async () => {
  const { fetchImpl } = stubFetch({ status: 200, body: "<!DOCTYPE html><html>oops</html>" });
  const res = await handleInstall(req(), {}, { fetch: fetchImpl });
  assert.equal(res.status, 502);
  assert.match(await res.text(), /did not return a valid installer/);
});

test("shell variant requires a shebang", async () => {
  const { fetchImpl } = stubFetch({ status: 200, body: "echo not a real script\n" });
  const res = await handleInstall(req(), {}, { fetch: fetchImpl });
  assert.equal(res.status, 502);
});

test("network failure yields a 502 error script", async () => {
  const { fetchImpl } = stubFetch({ throws: true });
  const res = await handleInstall(req(), {}, { fetch: fetchImpl });
  assert.equal(res.status, 502);
  assert.match(await res.text(), /could not reach/);
});

test("oversized content-length is refused before reading the body", async () => {
  const { fetchImpl } = stubFetch({ status: 200, body: SH_BODY, contentLength: 5 * 1024 * 1024 });
  const res = await handleInstall(req(), {}, { fetch: fetchImpl });
  assert.equal(res.status, 502);
  assert.match(await res.text(), /unexpectedly large/);
});

test("SSRF: request path/query/host never influence the upstream URL", async () => {
  const { fetchImpl, calls } = stubFetch({ body: SH_BODY });
  await handleInstall(
    req("https://cli.nexus.xyz/..%2f..%2fevil?host=evil.com&x=https://attacker"),
    {},
    { fetch: fetchImpl },
  );
  assert.equal(calls[0].url, SH_URL);
  assert.ok(calls[0].url.startsWith("https://github.com/"));
});

test("malicious env config is rejected (no fetch, 500)", async () => {
  const { fetchImpl, calls } = stubFetch();
  const res = await handleInstall(
    req(),
    { INSTALLER_OWNER: "../../evil.com", INSTALLER_REPO: "x", INSTALLER_APP: "y" },
    { fetch: fetchImpl },
  );
  assert.equal(res.status, 500);
  assert.equal(calls.length, 0);
});

test("env can legitimately override owner/repo/app", () => {
  const url = upstreamUrl({ owner: "acme", repo: "tool", app: "tool" }, "ps1");
  assert.equal(
    url,
    "https://github.com/acme/tool/releases/latest/download/tool-installer.ps1",
  );
});

test("upstreamUrl rejects unknown variants and bad segments", () => {
  assert.throws(() => upstreamUrl({ owner: "a", repo: "b", app: "c" }, "exe"));
  assert.throws(() => upstreamUrl({ owner: "a/b", repo: "b", app: "c" }, "sh"));
  assert.throws(() => upstreamUrl({ owner: "a b", repo: "b", app: "c" }, "sh"));
});

test("pickVariant precedence: path > query > UA > default", () => {
  assert.equal(pickVariant(req("https://x/install.ps1", { ua: "curl" })), "ps1");
  assert.equal(pickVariant(req("https://x/install.sh", { ua: "PowerShell" })), "sh");
  assert.equal(pickVariant(req("https://x/?ps1", { ua: "curl" })), "ps1");
  assert.equal(pickVariant(req("https://x/", { ua: "PowerShell/7" })), "ps1");
  assert.equal(pickVariant(req("https://x/", { ua: "curl/8" })), "sh");
  assert.equal(pickVariant(req("https://x/")), "sh");
});

test("errorScript is safely quoted against injection", () => {
  const sh = errorScript("sh", "method '; rm -rf / ;' not allowed");
  // The dangerous payload stays inside a single-quoted string.
  assert.match(sh, /^#!\/bin\/sh\necho '/);
  assert.match(sh, /exit 1\n$/);
  const ps = errorScript("ps1", "weird ' quote");
  assert.match(ps, /^Write-Error '/);
  assert.match(ps, /''/); // embedded quote doubled
});

test("default export fetch handler works end-to-end", async () => {
  // Uses the real global fetch path only if reachable; here we just confirm the
  // wrapper returns a Response and never throws on a benign request shape.
  const res = await worker.fetch(req("https://cli.nexus.xyz/", { method: "OPTIONS" }), {});
  assert.ok(res instanceof Response);
  assert.equal(res.status, 405);
});
