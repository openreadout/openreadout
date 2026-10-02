// The npm wrapper (index.js) over the built module (pkg/): run `scripts/wasm.sh build` first.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { InstrumentError, formats, open, openAmong, version } from "../index.js";

const fixture = (rel) => readFileSync(new URL(`../../../${rel}`, import.meta.url));
const CZI = fixture("crates/openreadout-cli/tests/fixtures/mini.czi");
const ND2 = fixture("crates/openreadout-cli/tests/fixtures/mini.nd2");
const LIF = fixture("crates/openreadout-cli/tests/fixtures/mini.lif");

test("formats and version", async () => {
  const f = await formats();
  assert.ok(f.some((d) => d.id === "czi"));
  assert.match(await version(), /^\d+\.\d+\.\d+/);
});

test("bytes, Blob and File give the same info", async () => {
  const fromBytes = await open(new Uint8Array(CZI), { name: "mini.czi" });
  const want = await fromBytes.info();
  assert.equal(want.path, "mini.czi");
  assert.equal(want.format.id, "czi");
  const blob = await open(new Blob([CZI]), { name: "mini.czi" });
  assert.deepEqual(await blob.info(), want);
  const file = await open(new File([CZI], "mini.czi"));
  assert.deepEqual(await file.info(), want);
});

test("large blobs are read lazily, block by block", async () => {
  // Force the lazy path with a tiny whole-file limit and count the bytes actually read.
  let read = 0;
  const src = new Blob([LIF]);
  const reader = {
    size: src.size,
    read: async (at, len) => {
      read += len;
      return new Uint8Array(await src.slice(at, at + len).arrayBuffer());
    },
  };
  const lazy = await open(reader, { name: "mini.lif" });
  const whole = await open(new Uint8Array(LIF), { name: "mini.lif" });
  assert.deepEqual(await lazy.info(), await whole.info());
  assert.deepEqual(await lazy.check(), await whole.check());
  const a = await lazy.preview({ max_size: 32 });
  const b = await whole.preview({ max_size: 32 });
  assert.deepEqual(a.meta, b.meta);
  assert.deepEqual(a.png, b.png);
  assert.ok(read > 0 && read <= LIF.length);
  assert.equal(lazy.bytesHeld, read);
  const blobLazy = await open(new Blob([LIF]), { name: "mini.lif", wholeFileLimit: 0 });
  assert.deepEqual(await blobLazy.info({ view: "structure" }), await whole.info({ view: "structure" }));
});

test("every command", async () => {
  const f = await open(new Uint8Array(ND2), { name: "mini.nd2" });
  assert.equal((await f.info({ view: "format" })).format, "nd2");
  const full = await f.info({ view: "full" });
  assert.ok(full.vendor);
  assert.equal((await f.info({ view: "full", vendor: false })).vendor, undefined);
  assert.ok((await f.info({ view: "explain" })).summary.length > 0);
  assert.equal(typeof (await f.check({ headersOnly: true })).ok, "boolean");
  assert.ok(Array.isArray((await f.info({ view: "structure" })).entries));
  await assert.rejects(f.info({ view: "dump" }), (e) => e.exitCode === 2);
  const { meta, png } = await f.preview({ max_size: 48 });
  assert.equal(meta.encoding, "png");
  assert.deepEqual([...png.subarray(0, 4)], [0x89, 0x50, 0x4e, 0x47]);
  f.close();
});

test("errors carry the CLI's code, exit code and hint", async () => {
  const f = await open(new Uint8Array([0, 1, 2, 3, 4, 5, 6, 7]), { name: "x.bin" });
  await assert.rejects(f.info(), (e) => {
    assert.ok(e instanceof InstrumentError);
    assert.equal(e.code, "unknown_format");
    assert.equal(e.exitCode, 3);
    assert.ok(e.hint);
    return true;
  });
  const g = await open(new Uint8Array(ND2), { name: "mini.nd2" });
  await assert.rejects(g.table(0), (e) => e.exitCode === 6 || e.exitCode === 2);
});

test("a dropped folder resolves siblings", async () => {
  const f = await openAmong(
    [
      { path: "drop/mini.czi", data: CZI },
      { path: "drop/readme.txt", data: new TextEncoder().encode("hello") },
    ],
    "drop/mini.czi",
  );
  assert.equal((await f.info()).format.id, "czi");
});
