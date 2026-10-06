'use strict';

// Find the native `openreadout` binary. It ships in a platform package (for example
// @openreadout/cli-linux-x64) that the package manager installs as an optional dependency of
// `openreadout`. Nothing is downloaded at install or run time.
//
// OPENREADOUT_BINARY, if set, names a binary to use instead.

const fs = require('fs');
const path = require('path');
const { platformFor, exeName } = require('./platforms');

const VERSION = require('../package.json').version;
const INSTALL_GUIDE = 'https://openreadout.github.io/openreadout/getting-started/install.html';

function otherWays() {
  return (
    'Other ways to install OpenReadout: Homebrew (brew install openreadout/tap/openreadout), ' +
    'the install script, Scoop, cargo, Docker or the release archives. ' +
    `See ${INSTALL_GUIDE}`
  );
}

/** The platform package this machine needs, e.g. `@openreadout/cli-linux-x64`, or undefined. */
function platformPackage(platform = process.platform, arch = process.arch) {
  const p = platformFor(platform, arch);
  return p && p.name;
}

/**
 * The path of the native binary. Throws an Error with a message for the user when the platform
 * has no prebuilt binary or its package is not installed.
 */
function binaryPath({ platform = process.platform, arch = process.arch, resolve = require.resolve } = {}) {
  const override = process.env.OPENREADOUT_BINARY;
  if (override) {
    if (!fs.existsSync(override)) throw new Error(`OPENREADOUT_BINARY=${override} does not exist.`);
    return override;
  }
  const p = platformFor(platform, arch);
  if (!p) {
    throw new Error(
      `There is no prebuilt openreadout binary for ${platform}-${arch}. ` +
        'Build one with `cargo install openreadout --locked` (Rust 1.91 or newer) and set ' +
        `OPENREADOUT_BINARY to its path.\n${otherWays()}`,
    );
  }
  let manifest;
  try {
    manifest = resolve(`${p.name}/package.json`);
  } catch {
    throw new Error(
      `The openreadout binary for ${platform}-${arch} is missing. It comes in the package ` +
        `${p.name}, which the package manager should have installed as an optional dependency ` +
        'of openreadout. That does not happen when optional dependencies are turned off ' +
        '(--omit=optional, --no-optional) or when a lockfile made on another platform is reused.\n' +
        `To fix it, reinstall with optional dependencies, or install the package directly:\n` +
        `  npm install ${p.name}@${VERSION}\n${otherWays()}`,
    );
  }
  const bin = path.join(path.dirname(manifest), 'bin', exeName(platform));
  if (!fs.existsSync(bin)) {
    throw new Error(`${p.name} is installed but has no ${path.basename(bin)}. Reinstall it.\n${otherWays()}`);
  }
  return bin;
}

/** The version of the installed platform package, if it differs from this package's version. */
function versionMismatch(resolve = require.resolve) {
  if (process.env.OPENREADOUT_BINARY) return null;
  const p = platformFor();
  if (!p) return null;
  try {
    const v = JSON.parse(fs.readFileSync(resolve(`${p.name}/package.json`), 'utf8')).version;
    return v === VERSION ? null : `${p.name} is version ${v}, but openreadout is ${VERSION}`;
  } catch {
    return null;
  }
}

module.exports = { VERSION, INSTALL_GUIDE, platformPackage, binaryPath, versionMismatch };
