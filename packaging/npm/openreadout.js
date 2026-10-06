#!/usr/bin/env node
'use strict';

// The `openreadout` command (also what `npx openreadout` runs). It runs the native binary from
// the platform package with the same arguments, stdio and exit code.

const fs = require('fs');
const { spawnSync } = require('child_process');
const { binaryPath, versionMismatch } = require('./lib/binary');

function run(bin) {
  return spawnSync(bin, process.argv.slice(2), { stdio: 'inherit', windowsHide: true });
}

let bin;
try {
  bin = binaryPath();
} catch (err) {
  process.stderr.write(`[openreadout] ${err.message}\n`);
  process.exit(1);
}

const mismatch = versionMismatch();
if (mismatch) process.stderr.write(`[openreadout] warning: ${mismatch}; reinstall openreadout to match them\n`);

let res = run(bin);
if (res.error && res.error.code === 'EACCES' && process.platform !== 'win32') {
  // Some package managers or archive tools drop the executable bit. Restore it once and retry.
  try {
    fs.chmodSync(bin, 0o755);
    res = run(bin);
  } catch {
    // Report the original error below.
  }
}
if (res.error) {
  process.stderr.write(`[openreadout] could not run ${bin}: ${res.error.message}\n`);
  process.exit(1);
}
if (res.signal) process.kill(process.pid, res.signal);
process.exit(res.status === null ? 1 : res.status);
