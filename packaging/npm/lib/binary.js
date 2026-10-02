'use strict';

// Locate, download and verify the native `openreadout` binary for this platform.
//
// The binary comes from the GitHub release whose version equals this package's version. The
// archive's SHA-256 is checked against the release's SHA256SUMS before anything is extracted;
// a missing or mismatching checksum is a hard error.
//
// Environment:
//   OPENREADOUT_BINARY         use this binary instead of downloading one
//   OPENREADOUT_DOWNLOAD_BASE  base URL (or file:// directory) holding the release assets and
//                                SHA256SUMS; default https://github.com/<repo>/releases/download/v<version>
//   OPENREADOUT_SKIP_DOWNLOAD  if set, postinstall does nothing (the first run downloads instead)

const crypto = require('crypto');
const fs = require('fs');
const path = require('path');
const { fileURLToPath } = require('url');
const { extractFromTarGz, extractFromZip } = require('./archive');

const REPO = 'openreadout/openreadout';
const VERSION = require('../package.json').version;

// Node's process.platform/arch -> Rust target triple of the release asset.
const TARGETS = {
  'darwin-arm64': 'aarch64-apple-darwin',
  'darwin-x64': 'x86_64-apple-darwin',
  'linux-arm64': 'aarch64-unknown-linux-musl',
  'linux-x64': 'x86_64-unknown-linux-musl',
  'win32-x64': 'x86_64-pc-windows-msvc',
  // Windows on Arm runs the x64 build under emulation.
  'win32-arm64': 'x86_64-pc-windows-msvc',
};

function target(platform = process.platform, arch = process.arch) {
  const t = TARGETS[`${platform}-${arch}`];
  if (!t) {
    throw new Error(
      `no prebuilt openreadout for ${platform}-${arch}; build from source with ` +
        '`cargo install openreadout` and set OPENREADOUT_BINARY to its path',
    );
  }
  return t;
}

function exeName(platform = process.platform) {
  return platform === 'win32' ? 'openreadout.exe' : 'openreadout';
}

function assetName(t) {
  return `openreadout-${t}${t.includes('windows') ? '.zip' : '.tar.gz'}`;
}

/** Where the downloaded binary lives inside this package. */
function binaryPath() {
  if (process.env.OPENREADOUT_BINARY) return process.env.OPENREADOUT_BINARY;
  return path.join(__dirname, '..', 'vendor', exeName());
}

function downloadBase() {
  const base = process.env.OPENREADOUT_DOWNLOAD_BASE ||
    `https://github.com/${REPO}/releases/download/v${VERSION}`;
  return base.replace(/\/+$/, '');
}

async function get(url) {
  if (url.startsWith('file://')) return fs.readFileSync(fileURLToPath(url));
  const res = await fetch(url, {
    redirect: 'follow',
    headers: { 'user-agent': `openreadout-npm/${VERSION}` },
    signal: AbortSignal.timeout(300_000),
  });
  if (!res.ok) throw new Error(`GET ${url}: HTTP ${res.status}`);
  return Buffer.from(await res.arrayBuffer());
}

/** Parse `sha256sum` output into a Map of file name -> lowercase hex digest. */
function parseSums(text) {
  const sums = new Map();
  for (const line of text.split(/\r?\n/)) {
    const m = /^([0-9a-fA-F]{64})\s+\*?(.+?)\s*$/.exec(line);
    if (m) sums.set(path.posix.basename(m[2]), m[1].toLowerCase());
  }
  return sums;
}

function sha256(buf) {
  return crypto.createHash('sha256').update(buf).digest('hex');
}

/** Download, verify and extract the binary; returns its path. */
async function install({ log = () => {} } = {}) {
  const t = target();
  const asset = assetName(t);
  const base = downloadBase();
  log(`downloading ${base}/${asset}`);
  const [archive, sumsText] = await Promise.all([get(`${base}/${asset}`), get(`${base}/SHA256SUMS`)]);
  const expected = parseSums(sumsText.toString('utf8')).get(asset);
  if (!expected) throw new Error(`${asset} is not listed in ${base}/SHA256SUMS`);
  const actual = sha256(archive);
  if (actual !== expected) {
    throw new Error(`checksum mismatch for ${asset}: expected ${expected}, got ${actual}`);
  }
  log(`sha256 ok (${actual})`);
  const exe = exeName();
  const bin = asset.endsWith('.zip') ? extractFromZip(archive, exe) : extractFromTarGz(archive, exe);
  const dest = path.join(__dirname, '..', 'vendor', exe);
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  const tmp = `${dest}.${process.pid}.tmp`;
  fs.writeFileSync(tmp, bin, { mode: 0o755 });
  fs.renameSync(tmp, dest);
  return dest;
}

/** Return a runnable binary, downloading it first if needed (e.g. postinstall was skipped). */
async function ensureBinary(opts) {
  const p = binaryPath();
  if (fs.existsSync(p)) return p;
  if (process.env.OPENREADOUT_BINARY) {
    throw new Error(`OPENREADOUT_BINARY=${p} does not exist`);
  }
  return install(opts);
}

module.exports = { VERSION, REPO, target, assetName, binaryPath, ensureBinary, install, parseSums };
