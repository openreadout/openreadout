#!/usr/bin/env node
'use strict';

// `openreadout` command (also what `npx openreadout` runs): execs the native binary with the
// same arguments, stdio and exit code. Downloads the binary first if postinstall did not.

const { spawnSync } = require('child_process');
const { ensureBinary } = require('./lib/binary');

async function main() {
  let bin;
  try {
    bin = await ensureBinary({ log: (m) => process.stderr.write(`[openreadout] ${m}\n`) });
  } catch (err) {
    process.stderr.write(`[openreadout] ${err.message}\n`);
    process.exit(1);
  }
  const res = spawnSync(bin, process.argv.slice(2), { stdio: 'inherit', windowsHide: true });
  if (res.error) {
    process.stderr.write(`[openreadout] failed to run ${bin}: ${res.error.message}\n`);
    process.exit(1);
  }
  if (res.signal) process.kill(process.pid, res.signal);
  process.exit(res.status === null ? 1 : res.status);
}

main();
