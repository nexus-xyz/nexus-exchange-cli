// cli.nexus.xyz — installer proxy (ENG-3454)
//
// Serves the cargo-dist installer for the `nexus` CLI so that
//
//     curl https://cli.nexus.xyz | sh                 # macOS + Linux
//     irm   https://cli.nexus.xyz | iex                # Windows PowerShell
//
// install the latest release.
//
// Why a proxy and not a redirect: cargo-dist publishes the installer to each
// GitHub release under a stable "latest download" URL, but reaching it takes an
// HTTP redirect, and `curl <url> | sh` does NOT follow redirects unless `-L` is
// passed. The ticket requires the bare, redirect-free form to work, so this
// Worker fetches the installer server-side and returns the script body inline
// with a 200. The shell pipeline only ever sees a finished script (or a script
// that cleanly errors out) — never an HTML redirect/error page.
//
// Security model (this content is piped straight into a shell, so treat every
// byte as load-bearing):
//   * No SSRF / open proxy. The upstream URL is built entirely from vetted,
//     pinned constants — never from the request path, query, or headers. The
//     only request-derived decision is sh-vs-ps1, chosen from a fixed allowlist.
//   * Origin is asserted to be exactly https://github.com before any fetch.
//   * Config (owner/repo/app) is validated against a strict charset so a
//     mis-set env var can't smuggle in path traversal or a different host.
//   * Responses are rebuilt from scratch, so no upstream headers/cookies leak.
//   * On ANY failure the client receives a *valid* tiny script for the detected
//     variant that prints an error to stderr and exits non-zero — so a transient
//     upstream blip can never pipe a garbage/HTML body into the user's shell.
//   * text/plain + nosniff so a browser can't be tricked into executing it.
//
// Concurrency: a Worker invocation is request-scoped and shares no mutable
// state, so there is nothing to lock and no deadlock to reason about.

const DEFAULTS = Object.freeze({
  owner: "nexus-xyz",
  repo: "nexus-exchange-cli",
  // cargo-dist names the installer artifacts `<app>-installer.sh` / `.ps1`,
  // where <app> is the Cargo package name.
  app: "nexus-exchange-cli",
});

// GitHub org/repo names allow [A-Za-z0-9._-]; the artifact prefix follows the
// same charset. Anything else is rejected rather than reflected into a URL.
const SEGMENT_RE = /^[A-Za-z0-9._-]+$/;

// cargo-dist installers are a few KiB. Cap generously; anything larger means we
// fetched the wrong thing and must not stream it to a shell.
const MAX_BODY_BYTES = 2 * 1024 * 1024; // 2 MiB
const UPSTREAM_TIMEOUT_MS = 10_000;
const CACHE_MAX_AGE_S = 300; // 5 min: fresh enough to pick up a new release fast.

const RELEASES_PAGE = `https://github.com/${DEFAULTS.owner}/${DEFAULTS.repo}/releases`;

/**
 * Decide which installer variant to serve. Order of precedence:
 *   1. explicit path suffix (`/install.ps1`, `/install.sh`)
 *   2. explicit `?powershell` query flag
 *   3. User-Agent sniff (PowerShell's `irm`/`iwr` send "PowerShell" in the UA)
 * Default is the POSIX shell installer.
 * @returns {"sh"|"ps1"}
 */
export function pickVariant(request) {
  const url = new URL(request.url);
  const path = url.pathname.toLowerCase();
  if (path.endsWith(".ps1")) return "ps1";
  if (path.endsWith(".sh")) return "sh";
  if (url.searchParams.has("powershell") || url.searchParams.has("ps1")) return "ps1";
  const ua = (request.headers.get("user-agent") || "").toLowerCase();
  if (ua.includes("powershell")) return "ps1";
  return "sh";
}

/**
 * Build the pinned upstream "latest release" URL for a variant. Throws if the
 * configured segments are anything but a strict name, or if — against all
 * expectation — the assembled URL does not point at github.com.
 */
export function upstreamUrl(cfg, ext) {
  for (const segment of [cfg.owner, cfg.repo, cfg.app]) {
    if (typeof segment !== "string" || !SEGMENT_RE.test(segment)) {
      throw new Error("installer config segment failed validation");
    }
  }
  if (ext !== "sh" && ext !== "ps1") throw new Error("unknown installer variant");

  const file = `${cfg.app}-installer.${ext}`;
  const target = `https://github.com/${cfg.owner}/${cfg.repo}/releases/latest/download/${encodeURIComponent(file)}`;

  // Defense in depth: never fetch anything that isn't github.com over https.
  if (new URL(target).origin !== "https://github.com") {
    throw new Error("refusing non-github upstream origin");
  }
  return target;
}

function configFrom(env) {
  return {
    owner: env?.INSTALLER_OWNER || DEFAULTS.owner,
    repo: env?.INSTALLER_REPO || DEFAULTS.repo,
    app: env?.INSTALLER_APP || DEFAULTS.app,
  };
}

function baseHeaders(ext) {
  return {
    // text/plain + nosniff: a browser must download, never render/execute.
    "content-type": "text/plain; charset=utf-8",
    "x-content-type-options": "nosniff",
    "strict-transport-security": "max-age=63072000; includeSubDomains; preload",
    "referrer-policy": "no-referrer",
    "cache-control": `public, max-age=${CACHE_MAX_AGE_S}`,
    "x-installer-variant": ext,
  };
}

// Single-quote for POSIX sh: wrap in '...' and turn embedded ' into '\''.
function shQuote(s) {
  return `'${String(s).replace(/'/g, "'\\''")}'`;
}

// Single-quote for PowerShell: wrap in '...' and double embedded '.
function psQuote(s) {
  return `'${String(s).replace(/'/g, "''")}'`;
}

/**
 * A minimal, always-valid script for the detected variant that reports an error
 * and exits non-zero. Returned instead of any upstream/error body so a shell
 * pipeline degrades safely. `reason` is always quoted (it can contain a request
 * method or status code), so it cannot break out of the string.
 */
export function errorScript(ext, reason) {
  const msg = `nexus installer unavailable: ${reason}. See ${RELEASES_PAGE}`;
  if (ext === "ps1") {
    return `Write-Error ${psQuote(msg)}\nexit 1\n`;
  }
  return `#!/bin/sh\necho ${shQuote(msg)} 1>&2\nexit 1\n`;
}

// Guard against piping a non-script (e.g. a GitHub HTML error page) to a shell.
function looksLikeInstaller(body, ext) {
  if (!body) return false;
  const head = body.slice(0, 4096).trimStart();
  // Reject obvious HTML/XML. Note PowerShell block comments start with "<#",
  // so we only reject the specific HTML/XML openers, not every "<".
  const lower = head.toLowerCase();
  if (lower.startsWith("<!") || lower.startsWith("<html") || lower.startsWith("<?xml")) {
    return false;
  }
  // The POSIX installer is a real shell script; require a shebang.
  if (ext === "sh") return head.startsWith("#!");
  // PowerShell installers have no required prefix; non-empty + non-HTML is enough.
  return head.length > 0;
}

/**
 * Core handler. Pure with respect to its inputs: pass `deps.fetch` to stub the
 * network in tests. Never throws — every path returns a Response.
 */
export async function handleInstall(request, env = {}, deps = {}) {
  const fetchImpl = deps.fetch || fetch;
  const ext = pickVariant(request);

  if (request.method !== "GET" && request.method !== "HEAD") {
    return new Response(errorScript(ext, `method ${request.method} not allowed`), {
      status: 405,
      headers: { ...baseHeaders(ext), allow: "GET, HEAD" },
    });
  }

  let target;
  try {
    target = upstreamUrl(configFrom(env), ext);
  } catch {
    // Misconfiguration is an operator bug, not a client error.
    return new Response(errorScript(ext, "installer is misconfigured"), {
      status: 500,
      headers: baseHeaders(ext),
    });
  }

  let upstream;
  try {
    upstream = await fetchImpl(target, {
      method: "GET",
      redirect: "follow", // GitHub's "latest/download" 302s to the asset.
      headers: { "user-agent": "nexus-cli-installer-proxy", accept: "*/*" },
      signal: AbortSignal.timeout(UPSTREAM_TIMEOUT_MS),
      // Cloudflare edge-cache hint (ignored off-Workers).
      cf: { cacheTtl: CACHE_MAX_AGE_S, cacheEverything: true },
    });
  } catch {
    return new Response(errorScript(ext, "could not reach the release host"), {
      status: 502,
      headers: baseHeaders(ext),
    });
  }

  if (!upstream.ok) {
    return new Response(errorScript(ext, `release host returned HTTP ${upstream.status}`), {
      status: 502,
      headers: baseHeaders(ext),
    });
  }

  const declaredLen = Number(upstream.headers.get("content-length") || "0");
  if (Number.isFinite(declaredLen) && declaredLen > MAX_BODY_BYTES) {
    return new Response(errorScript(ext, "installer is unexpectedly large"), {
      status: 502,
      headers: baseHeaders(ext),
    });
  }

  const body = await upstream.text();
  const byteLen = new TextEncoder().encode(body).length;
  if (byteLen > MAX_BODY_BYTES || !looksLikeInstaller(body, ext)) {
    return new Response(errorScript(ext, "release host did not return a valid installer"), {
      status: 502,
      headers: baseHeaders(ext),
    });
  }

  const headers = baseHeaders(ext);
  if (request.method === "HEAD") {
    headers["content-length"] = String(byteLen);
    return new Response(null, { status: 200, headers });
  }
  return new Response(body, { status: 200, headers });
}

export default {
  async fetch(request, env, _ctx) {
    try {
      return await handleInstall(request, env);
    } catch {
      // Last-resort guard: a thrown error must still yield a safe script body,
      // never a Cloudflare HTML 500 that could be piped to a shell.
      const ext = (() => {
        try {
          return pickVariant(request);
        } catch {
          return "sh";
        }
      })();
      return new Response(errorScript(ext, "unexpected internal error"), {
        status: 500,
        headers: baseHeaders(ext),
      });
    }
  },
};
