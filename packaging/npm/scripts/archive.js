'use strict';

// Minimal, dependency-free extraction of ONE regular file from the release archives:
// gzip-compressed tar (macOS, Linux) and zip (Windows; stored or deflate entries).
// Only what `tar -czf` and `7z a` produce for our releases needs to be understood.

const zlib = require('zlib');
const path = require('path');

function cString(buf, start, len) {
  const end = buf.indexOf(0, start);
  return buf.toString('utf8', start, end === -1 || end > start + len ? start + len : end);
}

function octal(buf, start, len) {
  const s = cString(buf, start, len).trim();
  return s ? parseInt(s, 8) : 0;
}

function baseName(name) {
  return path.posix.basename(name.replace(/\\/g, '/'));
}

/** Return the contents of the entry whose base name is `wanted` in a .tar.gz buffer. */
function extractFromTarGz(gz, wanted) {
  const tar = zlib.gunzipSync(gz);
  let off = 0;
  let longName = null;
  while (off + 512 <= tar.length) {
    const header = tar.subarray(off, off + 512);
    if (header.every((b) => b === 0)) break; // end-of-archive marker
    let name = cString(header, 0, 100);
    const size = octal(header, 124, 12);
    const type = String.fromCharCode(header[156] || 0x30);
    const prefix = cString(header, 345, 155);
    if (prefix && header.toString('latin1', 257, 262) === 'ustar') name = prefix + '/' + name;
    const dataStart = off + 512;
    const dataEnd = dataStart + size;
    if (dataEnd > tar.length) throw new Error('truncated tar archive');
    if (type === 'L') {
      // GNU long name: the data block is the next entry's name.
      longName = cString(tar, dataStart, size);
    } else if (type === 'x') {
      // pax header: may carry `path=`.
      const m = /\d+ path=([^\n]*)\n/.exec(tar.toString('utf8', dataStart, dataEnd));
      if (m) longName = m[1];
    } else {
      if (longName !== null) {
        name = longName;
        longName = null;
      }
      if ((type === '0' || type === '\0') && baseName(name) === wanted) {
        return Buffer.from(tar.subarray(dataStart, dataEnd));
      }
    }
    off = dataStart + Math.ceil(size / 512) * 512;
  }
  throw new Error(`${wanted} not found in archive`);
}

/** Return the contents of the entry whose base name is `wanted` in a .zip buffer. */
function extractFromZip(zip, wanted) {
  // End of central directory: signature 0x06054b50, within the last 64 KiB + 22 bytes.
  let eocd = -1;
  for (let i = zip.length - 22; i >= Math.max(0, zip.length - 22 - 0xffff); i--) {
    if (zip.readUInt32LE(i) === 0x06054b50) {
      eocd = i;
      break;
    }
  }
  if (eocd < 0) throw new Error('not a zip archive (no end-of-central-directory record)');
  const entries = zip.readUInt16LE(eocd + 10);
  let p = zip.readUInt32LE(eocd + 16);
  for (let n = 0; n < entries; n++) {
    if (zip.readUInt32LE(p) !== 0x02014b50) throw new Error('corrupt zip central directory');
    const method = zip.readUInt16LE(p + 10);
    const compSize = zip.readUInt32LE(p + 20);
    const size = zip.readUInt32LE(p + 24);
    const nameLen = zip.readUInt16LE(p + 28);
    const extraLen = zip.readUInt16LE(p + 30);
    const commentLen = zip.readUInt16LE(p + 32);
    const local = zip.readUInt32LE(p + 42);
    const name = zip.toString('utf8', p + 46, p + 46 + nameLen);
    p += 46 + nameLen + extraLen + commentLen;
    if (baseName(name) !== wanted || name.endsWith('/')) continue;
    if (zip.readUInt32LE(local) !== 0x04034b50) throw new Error('corrupt zip local header');
    const dataStart = local + 30 + zip.readUInt16LE(local + 26) + zip.readUInt16LE(local + 28);
    const data = zip.subarray(dataStart, dataStart + compSize);
    let out;
    if (method === 0) out = Buffer.from(data);
    else if (method === 8) out = zlib.inflateRawSync(data);
    else throw new Error(`unsupported zip compression method ${method}`);
    if (out.length !== size) throw new Error(`${wanted}: size mismatch after decompression`);
    return out;
  }
  throw new Error(`${wanted} not found in archive`);
}

module.exports = { extractFromTarGz, extractFromZip };
