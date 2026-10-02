#!/usr/bin/env node
'use strict';

// postinstall: fetch the native binary for this platform from the matching GitHub release and
// verify it against SHA256SUMS. A network failure only warns (the `openreadout` shim retries on
// first run, which also covers `npm install --ignore-scripts`); a checksum mismatch fails the install.

const { install, binaryPath, VERSION } = require('./lib/binary');
const fs = require('fs');

async function main() {
  if (process.env.OPENREADOUT_SKIP_DOWNLOAD || process.env.OPENREADOUT_BINARY) return;
  if (fs.existsSync(binaryPath())) return;
  const log = (m) => process.stderr.write(`[openreadout] ${m}\n`);
  try {
    const p = await install({ log });
    log(`installed ${p} (v${VERSION})`);
  } catch (err) {
    if (/checksum mismatch|not listed in/.test(err.message)) {
      log(`ERROR: ${err.message}`);
      process.exit(1);
    }
    log(`warning: ${err.message}`);
    log('the binary will be downloaded on first run instead');
  }
}

main();
