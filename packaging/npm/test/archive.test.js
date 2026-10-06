'use strict';

const test = require('node:test');
const assert = require('node:assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const zlib = require('zlib');
const { execFileSync } = require('child_process');
const { extractFromTarGz, extractFromZip } = require('../scripts/archive');

test('tar.gz produced like the release job (tar -C dist -czf x.tar.gz .)', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'openreadout-'));
  const dist = path.join(dir, 'dist');
  fs.mkdirSync(dist);
  fs.writeFileSync(path.join(dist, 'README.md'), 'readme');
  fs.writeFileSync(path.join(dist, 'openreadout'), 'BINARY');
  const out = path.join(dir, 'a.tar.gz');
  execFileSync('tar', ['-C', dist, '-czf', out, '.']);
  assert.strictEqual(extractFromTarGz(fs.readFileSync(out), 'openreadout').toString(), 'BINARY');
  assert.throws(() => extractFromTarGz(fs.readFileSync(out), 'missing'), /not found/);
});

function zip(entries) {
  // Minimal zip writer for the test: one local header + central record per entry.
  const locals = [];
  const central = [];
  let offset = 0;
  for (const { name, data, deflate } of entries) {
    const body = deflate ? zlib.deflateRawSync(data) : data;
    const n = Buffer.from(name);
    const lh = Buffer.alloc(30);
    lh.writeUInt32LE(0x04034b50, 0);
    lh.writeUInt16LE(deflate ? 8 : 0, 8);
    lh.writeUInt32LE(body.length, 18);
    lh.writeUInt32LE(data.length, 22);
    lh.writeUInt16LE(n.length, 26);
    const ch = Buffer.alloc(46);
    ch.writeUInt32LE(0x02014b50, 0);
    ch.writeUInt16LE(deflate ? 8 : 0, 10);
    ch.writeUInt32LE(body.length, 20);
    ch.writeUInt32LE(data.length, 24);
    ch.writeUInt16LE(n.length, 28);
    ch.writeUInt32LE(offset, 42);
    locals.push(lh, n, body);
    central.push(ch, n);
    offset += 30 + n.length + body.length;
  }
  const cd = Buffer.concat(central);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(entries.length, 8);
  end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(cd.length, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, cd, end]);
}

test('zip, stored and deflated entries', () => {
  const exe = Buffer.from('MZ'.repeat(1000));
  for (const deflate of [false, true]) {
    const z = zip([
      { name: 'README.md', data: Buffer.from('r') },
      { name: 'openreadout.exe', data: exe, deflate },
    ]);
    assert.deepStrictEqual(extractFromZip(z, 'openreadout.exe'), exe);
  }
  assert.throws(() => extractFromZip(Buffer.alloc(100), 'x'), /not a zip/);
});

