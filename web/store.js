// Opening a file or a folder (an OME-Zarr image) over HTTPS. This is the only module that
// fetches from another site, and only from an address the visitor opened. Each file the
// reader asks for is fetched with a range request, so only what is shown is downloaded, and
// nothing is ever sent.

/** Metadata documents that name a Zarr hierarchy; an address ending in one means its folder. */
const ROOT_DOCS = /\/(zarr\.json|\.zattrs|\.zgroup)$/;

/** Paths that name a folder without asking the server: a Zarr hierarchy or a trailing slash. */
const FOLDER = /(\.zarr|\/)$/i;

/** Answers that mean a file is not there. S3 answers 403 for a missing key in a bucket that can't be listed. */
const ABSENT = new Set([400, 403, 404, 410]);

/** How long a download may go without receiving anything before it is tried again. */
const STALL_MS = 20_000;

/**
 * The HTTPS address `text` names. `s3://bucket/key` becomes the bucket's public address,
 * and the pages of a Zenodo file and a GitHub file become the addresses of their contents.
 * A root metadata document of a Zarr hierarchy names its folder.
 */
export function webAddress(text) {
  let t = text.trim();
  const s3 = /^s3:\/\/([^/]+)\/?(.*)$/i.exec(t);
  if (s3) t = `https://${s3[1]}.s3.amazonaws.com/${s3[2]}`;
  let u;
  try {
    u = new URL(t);
  } catch {
    throw new Error("that is not a URL");
  }
  if (u.protocol !== "https:") throw new Error("the page opens https:// and s3:// addresses only");
  const zenodo = /^\/records?\/(\d+)\/files\/([^/]+)$/.exec(u.pathname);
  if (u.host === "zenodo.org" && zenodo) return `https://zenodo.org/api/records/${zenodo[1]}/files/${zenodo[2]}/content`;
  const github = /^\/([^/]+)\/([^/]+)\/(?:blob|raw)\/(.+)$/.exec(u.pathname);
  if (u.host === "github.com" && github) return `https://raw.githubusercontent.com/${github[1]}/${github[2]}/${github[3]}`;
  if (ROOT_DOCS.test(u.pathname)) return `${u.origin}${u.pathname.replace(ROOT_DOCS, "")}`;
  return `${u.origin}${u.pathname.replace(/(.)\/+$/, "$1")}${u.search}`;
}

/** What an address is called: the last part of its path, before the `/content` of Zenodo's. */
export function addressName(url) {
  const u = new URL(url);
  const parts = u.pathname.split("/").filter(Boolean).map(decodeURIComponent);
  if (parts.length > 1 && parts.at(-1) === "content") parts.pop();
  return parts.at(-1) ?? u.host;
}

/** A file's name from its address, when the address gives one (`123` from Figshare does not). */
function fileName(url) {
  const name = addressName(url);
  return name.includes(".") ? name : null;
}

/**
 * What to open at `url` (from `webAddress`): `{ name, root, path, get }` for `openStore`.
 * A folder is opened as a store. A file is opened inside a store of its own, so a file that
 * needs companion files (an `.imzML` and its `.ibd`) finds them next to it: the address of a
 * companion is the file's address with its name replaced.
 */
export async function locate(url) {
  const u = new URL(url);
  const found = FOLDER.test(u.pathname) ? null : await fileAt(url);
  if (!found) {
    const base = `${u.origin}${u.pathname.replace(/\/+$/, "")}`;
    const name = addressName(base);
    return { name, root: name, get: httpStore((p) => `${base}/${p.split("/").map(encodeURIComponent).join("/")}`) };
  }
  // A URL that redirects (Figshare's) may name the file only where it ends up.
  const name = fileName(found) ?? fileName(url) ?? addressName(url);
  const parts = u.pathname.split("/");
  const at = parts.findLastIndex((p) => decodeURIComponent(p) === name);
  const root = u.host;
  const sibling = (p) => {
    if (p === name) return url;
    if (at < 0) return null;
    const path = [...parts.slice(0, at), ...p.split("/").map(encodeURIComponent), ...parts.slice(at + 1)];
    return `${u.origin}${path.join("/")}`;
  };
  return { name, root, path: name, get: httpStore(sibling) };
}

/**
 * The address `url` ends up at when it answers with a file, or `null` for a folder, which is
 * missing (an S3 prefix) or answers with a page listing it.
 */
async function fileAt(url) {
  const res = await fetchRetrying(url, { headers: { Range: "bytes=0-0" } }, false);
  if (ABSENT.has(res.status) || (res.headers.get("content-type") ?? "").startsWith("text/html")) return null;
  if (res.status !== 200 && res.status !== 206) throw new Error(`${new URL(url).host} answered HTTP ${res.status}, not with the file`);
  return res.url || url;
}

let downloads = 0;
let quiet = null;
let onDownloading = () => {};

/**
 * Call `f(true)` when the page starts downloading parts of a file and `f(false)` when it has
 * stopped for a moment. Reading takes several rounds of downloads with a little decoding in
 * between, and the short pause keeps a busy indicator from flickering between them.
 */
export function watchDownloads(f) {
  onDownloading = f;
}

function downloadStarted() {
  clearTimeout(quiet);
  if (downloads++ === 0 && quiet === null) onDownloading(true);
  quiet = null;
}

function downloadEnded() {
  if (--downloads > 0) return;
  quiet = setTimeout(() => {
    quiet = null;
    onDownloading(false);
  }, 200);
}

/**
 * The `get(path, offset, length)` of `openStore`, given `urlOf(path)`, the address of the file
 * at `path` (or `null` for a file that can't be there): up to `length` bytes of the file from
 * `offset`, and the file's size, or `null` when the server has no such file.
 */
export function httpStore(urlOf) {
  return async (path, offset, length) => {
    const at = urlOf(path);
    if (!at) return null;
    const res = await fetchRetrying(at, { headers: { Range: `bytes=${offset}-${offset + length - 1}` } });
    if (ABSENT.has(res.status)) return null;
    if (res.status !== 200 && res.status !== 206) throw new Error(`${path}: the server answered HTTP ${res.status}`);
    const data = res.data;
    // A server that ignores the range sends the whole file.
    if (res.status === 200) return { data: data.subarray(offset, offset + length), size: data.length };
    // A short answer ends at the end of the file.
    if (data.length < length) return { data, size: offset + data.length };
    const total = Number(/\/(\d+)$/.exec(res.headers.get("content-range") ?? "")?.[1]);
    if (Number.isSafeInteger(total)) return { data, size: total };
    // Browsers hide Content-Range from pages unless the server exposes it, so ask for the size:
    // with HEAD, or where only GET is allowed (Figshare's signed addresses), with a GET that
    // is dropped once the headers arrive.
    const size = (await sizeOf(at, { method: "HEAD" }, 1)) ?? (await sizeOf(at, {}));
    if (size === null) throw new Error(`${path}: the server did not give the file's size`);
    return { data, size };
  };
}

/** The Content-Length of the answer to `options`, or `null`. */
async function sizeOf(at, options, tries) {
  try {
    const res = await fetchRetrying(at, options, false, tries);
    const size = Number(res.headers.get("content-length"));
    return res.ok && res.headers.get("content-length") !== null && Number.isSafeInteger(size) ? size : null;
  } catch {
    return null;
  }
}

/**
 * `fetch`, with the body read into `data` (unless `body` is false), tried up to `tries` times: a
 * dropped connection is common when many chunks load at once, and a download that stops
 * sending is given up after `STALL_MS`.
 */
async function fetchRetrying(at, options, body = true, tries = 3) {
  downloadStarted();
  try {
    for (let i = 0; ; i++) {
      try {
        return await fetcher(at, options, body);
      } catch (e) {
        if (i < tries - 1) {
          await new Promise((ok) => setTimeout(ok, 250 * (i + 1)));
          continue;
        }
        if (e?.name === "AbortError") throw new Error(`${new URL(at).host} stopped sending data. Try again later.`);
        throw new Error(`${new URL(at).host} could not be reached. The server may not allow other sites to read it (CORS).`);
      }
    }
  } finally {
    downloadEnded();
  }
}

let fetcher = fetchOnce;

/**
 * Make each request with `f(at, options, body)` instead, which resolves to what `fetchOnce`
 * does. The demo's worker asks the page to make its requests: some servers (Figshare's)
 * challenge requests from a worker as if they came from a bot.
 */
export function fetchWith(f) {
  fetcher = f;
}

/**
 * One request: the response, with its body read into `data` unless `body` is false (then the
 * body is dropped). It is given up when nothing arrives for `STALL_MS`.
 */
export async function fetchOnce(at, options, body) {
  const stop = new AbortController();
  let timer = setTimeout(() => stop.abort(), STALL_MS);
  const alive = () => {
    clearTimeout(timer);
    timer = setTimeout(() => stop.abort(), STALL_MS);
  };
  try {
    // Nothing is cached: the readers keep what they were sent, and a cached redirect to a
    // signed address (Figshare's) stops working after a few seconds.
    const res = await fetch(at, { ...options, cache: "no-store", signal: stop.signal });
    if (!body || !res.body) {
      // A server that ignores a range would send the whole file.
      res.body?.cancel().catch(() => {});
      return res;
    }
    alive();
    const parts = [];
    let n = 0;
    const reader = res.body.getReader();
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      alive();
      parts.push(value);
      n += value.length;
    }
    const data = new Uint8Array(n);
    let o = 0;
    for (const p of parts) {
      data.set(p, o);
      o += p.length;
    }
    return Object.assign(res, { data });
  } finally {
    clearTimeout(timer);
  }
}
