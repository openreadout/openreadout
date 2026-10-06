'use strict';

// The platform packages that carry the native binary, one per release target. The `openreadout`
// package lists each of them as an optional dependency, and npm, pnpm, yarn and bun install only
// the one whose `os` and `cpu` match the machine. scripts/platform-packages.js builds them from
// the release archives.

const PLATFORMS = [
  {
    name: '@openreadout/cli-darwin-arm64',
    os: 'darwin',
    cpu: ['arm64'],
    target: 'aarch64-apple-darwin',
    label: 'macOS on Apple silicon',
  },
  {
    name: '@openreadout/cli-darwin-x64',
    os: 'darwin',
    cpu: ['x64'],
    target: 'x86_64-apple-darwin',
    label: 'macOS on Intel',
  },
  {
    name: '@openreadout/cli-linux-arm64',
    os: 'linux',
    cpu: ['arm64'],
    target: 'aarch64-unknown-linux-musl',
    label: 'Linux on arm64 (static build, any distribution)',
  },
  {
    name: '@openreadout/cli-linux-x64',
    os: 'linux',
    cpu: ['x64'],
    target: 'x86_64-unknown-linux-musl',
    label: 'Linux on x64 (static build, any distribution)',
  },
  {
    // Windows on Arm runs the x64 build under emulation, so this package installs there too.
    name: '@openreadout/cli-win32-x64',
    os: 'win32',
    cpu: ['x64', 'arm64'],
    target: 'x86_64-pc-windows-msvc',
    label: 'Windows on x64 (and on Arm, under emulation)',
  },
];

/** The platform package for this machine, or undefined if there is none. */
function platformFor(platform = process.platform, arch = process.arch) {
  return PLATFORMS.find((p) => p.os === platform && p.cpu.includes(arch));
}

/** The binary's file name on `platform`. */
function exeName(platform = process.platform) {
  return platform === 'win32' ? 'openreadout.exe' : 'openreadout';
}

/** The release archive that holds the binary for `target`. */
function assetName(target) {
  return `openreadout-${target}${target.includes('windows') ? '.zip' : '.tar.gz'}`;
}

module.exports = { PLATFORMS, platformFor, exeName, assetName };
