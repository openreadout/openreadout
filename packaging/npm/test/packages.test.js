'use strict';

const test = require('node:test');
const assert = require('node:assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { execFileSync } = require('child_process');
const { PLATFORMS, platformFor, assetName } = require('../lib/platforms');
const { binaryPath, platformPackage, VERSION } = require('../lib/binary');
const { manifest, parseSums } = require('../scripts/platform-packages');

const pkg = require('../package.json');

test('optionalDependencies list every platform package at this version', () => {
  const expected = Object.fromEntries(PLATFORMS.map((p) => [p.name, pkg.version]));
  assert.deepStrictEqual(pkg.optionalDependencies, expected);
});

test('the main package has no install scripts', () => {
  for (const s of ['preinstall', 'install', 'postinstall']) assert.ok(!(s in pkg.scripts), s);
});

test('platform lookup', () => {
  assert.strictEqual(platformPackage('linux', 'x64'), '@openreadout/cli-linux-x64');
  assert.strictEqual(platformPackage('darwin', 'arm64'), '@openreadout/cli-darwin-arm64');
  assert.strictEqual(platformPackage('win32', 'arm64'), '@openreadout/cli-win32-x64');
  assert.strictEqual(platformPackage('freebsd', 'x64'), undefined);
  assert.strictEqual(assetName(platformFor('win32', 'x64').target), 'openreadout-x86_64-pc-windows-msvc.zip');
  assert.strictEqual(assetName(platformFor('linux', 'arm64').target), 'openreadout-aarch64-unknown-linux-musl.tar.gz');
});

test('platform package manifests', () => {
  for (const p of PLATFORMS) {
    const m = manifest(p);
    assert.strictEqual(m.name, p.name);
    assert.strictEqual(m.version, pkg.version);
    assert.deepStrictEqual(m.os, [p.os]);
    assert.deepStrictEqual(m.cpu, p.cpu);
    // npm provenance checks that the repository matches the one that built the package.
    assert.deepStrictEqual(m.repository, pkg.repository);
    assert.ok(!('bin' in m) && !('scripts' in m));
  }
});

test('binaryPath finds the binary in the platform package', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'openreadout-'));
  fs.mkdirSync(path.join(dir, 'bin'));
  fs.writeFileSync(path.join(dir, 'package.json'), '{}');
  fs.writeFileSync(path.join(dir, 'bin', 'openreadout'), '');
  const resolve = (id) => {
    assert.strictEqual(id, '@openreadout/cli-linux-arm64/package.json');
    return path.join(dir, 'package.json');
  };
  assert.strictEqual(binaryPath({ platform: 'linux', arch: 'arm64', resolve }), path.join(dir, 'bin', 'openreadout'));
});

test('binaryPath errors name the package to install', () => {
  const missing = () => {
    throw new Error('Cannot find module');
  };
  assert.throws(
    () => binaryPath({ platform: 'darwin', arch: 'x64', resolve: missing }),
    (err) => err.message.includes(`npm install @openreadout/cli-darwin-x64@${VERSION}`) && /install\.html/.test(err.message),
  );
  assert.throws(() => binaryPath({ platform: 'aix', arch: 'ppc64', resolve: missing }), /no prebuilt openreadout binary for aix-ppc64/);
});

test('platform-packages.js builds a package from a binary', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'openreadout-'));
  const bin = path.join(dir, 'fake');
  fs.writeFileSync(bin, 'BINARY');
  const out = path.join(dir, 'out');
  execFileSync(process.execPath, [path.join(__dirname, '..', 'scripts', 'platform-packages.js'), '--binary', `linux-x64=${bin}`, '--out', out]);
  const built = path.join(out, 'linux-x64');
  assert.strictEqual(fs.readFileSync(path.join(built, 'bin', 'openreadout'), 'utf8'), 'BINARY');
  if (process.platform !== 'win32') assert.ok(fs.statSync(path.join(built, 'bin', 'openreadout')).mode & 0o100);
  assert.strictEqual(JSON.parse(fs.readFileSync(path.join(built, 'package.json'))).name, '@openreadout/cli-linux-x64');
  for (const f of ['README.md', 'LICENSE-MIT', 'LICENSE-APACHE', 'NOTICE', 'THIRD-PARTY-NOTICES.md']) {
    assert.ok(fs.existsSync(path.join(built, f)), f);
  }
});

test('SHA256SUMS parsing', () => {
  const a = 'a'.repeat(64);
  const s = parseSums(`${a}  openreadout-x86_64-unknown-linux-musl.tar.gz\n${'B'.repeat(64)} *dist/x.zip\n`);
  assert.strictEqual(s.get('openreadout-x86_64-unknown-linux-musl.tar.gz'), a);
  assert.strictEqual(s.get('x.zip'), 'b'.repeat(64));
});
