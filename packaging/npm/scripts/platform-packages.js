#!/usr/bin/env node
'use strict';

// Build the platform packages (@openreadout/cli-<os>-<cpu>) that carry the native binary.
//
//   node scripts/platform-packages.js --archives DIR --out OUT [--sums FILE]
//       Take each binary from the release archive openreadout-<target>.tar.gz (.zip on Windows)
//       in DIR. With --sums, check every archive against that SHA256SUMS file first. Every
//       platform must have its archive.
//   node scripts/platform-packages.js --binary darwin-arm64=PATH [--binary ...] --out OUT
//       Package binaries you already have, e.g. a local `cargo build --release`. Only the
//       platforms you name are built.
//
// Each package lands in OUT/<os>-<cpu>/ with its package.json, README.md, bin/ and the license
// and notice files. The version is the one in this directory's package.json, which the release
// sets from Cargo.toml.

const crypto = require('crypto');
const fs = require('fs');
const path = require('path');
const { PLATFORMS, exeName, assetName } = require('../lib/platforms');
const { extractFromTarGz, extractFromZip } = require('./archive');

const PKG_DIR = path.join(__dirname, '..');
const REPO_ROOT = path.join(PKG_DIR, '..', '..');
const MAIN = require('../package.json');
const COPIED = ['LICENSE-MIT', 'LICENSE-APACHE', 'NOTICE', 'THIRD-PARTY-NOTICES.md'];

function usage(msg) {
  if (msg) process.stderr.write(`error: ${msg}\n`);
  process.stderr.write(
    'usage: platform-packages.js (--archives DIR [--sums FILE] | --binary <os>-<cpu>=PATH ...) --out DIR\n',
  );
  process.exit(2);
}

function parseArgs(argv) {
  const args = { binaries: new Map() };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const value = () => {
      if (i + 1 >= argv.length) usage(`${a} needs a value`);
      return argv[++i];
    };
    if (a === '--archives') args.archives = value();
    else if (a === '--sums') args.sums = value();
    else if (a === '--out') args.out = value();
    else if (a === '--binary') {
      const v = value();
      const eq = v.indexOf('=');
      if (eq < 1) usage(`--binary expects <os>-<cpu>=PATH, got ${v}`);
      args.binaries.set(v.slice(0, eq), v.slice(eq + 1));
    } else usage(`unknown argument ${a}`);
  }
  if (!args.out) usage('--out is required');
  if (!args.archives === !args.binaries.size) usage('give either --archives or --binary');
  return args;
}

/** The short key of a platform package, e.g. `linux-x64`. */
function key(p) {
  return p.name.replace('@openreadout/cli-', '');
}

function parseSums(text) {
  const sums = new Map();
  for (const line of text.split(/\r?\n/)) {
    const m = /^([0-9a-fA-F]{64})\s+\*?(.+?)\s*$/.exec(line);
    if (m) sums.set(path.posix.basename(m[2]), m[1].toLowerCase());
  }
  return sums;
}

function binaryFromArchive(p, dir, sums) {
  const asset = assetName(p.target);
  const file = path.join(dir, asset);
  if (!fs.existsSync(file)) throw new Error(`${asset} is not in ${dir}`);
  const data = fs.readFileSync(file);
  if (sums) {
    const expected = sums.get(asset);
    if (!expected) throw new Error(`${asset} is not listed in the checksums file`);
    const actual = crypto.createHash('sha256').update(data).digest('hex');
    if (actual !== expected) throw new Error(`checksum mismatch for ${asset}: expected ${expected}, got ${actual}`);
  }
  const exe = exeName(p.os);
  return asset.endsWith('.zip') ? extractFromZip(data, exe) : extractFromTarGz(data, exe);
}

function manifest(p) {
  return {
    name: p.name,
    version: MAIN.version,
    description: `The OpenReadout binary for ${p.label}. Install the openreadout package, which picks this one for you.`,
    license: MAIN.license,
    author: MAIN.author,
    homepage: MAIN.homepage,
    repository: MAIN.repository,
    bugs: MAIN.bugs,
    os: [p.os],
    cpu: p.cpu,
    files: ['bin/', 'README.md', ...COPIED],
    // Yarn's Plug'n'Play keeps the binary on disk instead of inside a zip archive.
    preferUnplugged: true,
    publishConfig: { access: 'public' },
  };
}

function readme(p) {
  return `# ${p.name}

The [OpenReadout](https://github.com/openreadout/openreadout) binary for ${p.label}, built for the Rust target \`${p.target}\`.

You don't need to install this package yourself. Install [\`openreadout\`](https://www.npmjs.com/package/openreadout) instead: it lists this package as an optional dependency, and your package manager installs the one that matches your machine.

\`\`\`bash
npx openreadout --version
\`\`\`

Dual-licensed MIT OR Apache-2.0. NOTICE and THIRD-PARTY-NOTICES.md list the bundled third-party code.
`;
}

function writePackage(p, binary, outDir) {
  const dir = path.join(outDir, key(p));
  fs.rmSync(dir, { recursive: true, force: true });
  fs.mkdirSync(path.join(dir, 'bin'), { recursive: true });
  fs.writeFileSync(path.join(dir, 'package.json'), `${JSON.stringify(manifest(p), null, 2)}\n`);
  fs.writeFileSync(path.join(dir, 'README.md'), readme(p));
  for (const f of COPIED) fs.copyFileSync(path.join(REPO_ROOT, f), path.join(dir, f));
  fs.writeFileSync(path.join(dir, 'bin', exeName(p.os)), binary, { mode: 0o755 });
  return dir;
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  let chosen = PLATFORMS;
  if (args.binaries.size) {
    const known = new Set(PLATFORMS.map(key));
    for (const k of args.binaries.keys()) {
      if (!known.has(k)) usage(`unknown platform ${k} (known: ${[...known].join(', ')})`);
    }
    chosen = PLATFORMS.filter((p) => args.binaries.has(key(p)));
  }
  const sums = args.sums ? parseSums(fs.readFileSync(args.sums, 'utf8')) : null;
  fs.mkdirSync(args.out, { recursive: true });
  for (const p of chosen) {
    const binary = args.archives
      ? binaryFromArchive(p, args.archives, sums)
      : fs.readFileSync(args.binaries.get(key(p)));
    const dir = writePackage(p, binary, args.out);
    process.stdout.write(`${p.name}@${MAIN.version} -> ${dir} (${binary.length} bytes)\n`);
  }
}

if (require.main === module) {
  try {
    main();
  } catch (err) {
    process.stderr.write(`error: ${err.message}\n`);
    process.exit(1);
  }
}

module.exports = { manifest, parseSums, key };
