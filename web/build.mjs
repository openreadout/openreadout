// Assemble the demo page into a directory: node web/build.mjs <out-dir>
//
// Copies the page (index.html, app.js, style.css), the npm wrapper (packaging/wasm/index.js)
// and the wasm-bindgen glue into <out>/openreadout/, and inlines the WebAssembly module as
// base64 (<out>/openreadout/wasm-inline.js): the page's policy forbids fetching anything, so
// the module travels as a script. Run `scripts/wasm.sh build` first.
import { copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "..");
const out = resolve(process.argv[2] ?? join(root, "book/book/demo"));
const pkg = join(root, "packaging/wasm/pkg");
const wasm = join(pkg, "openreadout_bg.wasm");

if (!existsSync(wasm)) {
  console.error(`error: ${wasm} is missing; run scripts/wasm.sh build`);
  process.exit(1);
}

mkdirSync(join(out, "openreadout/pkg"), { recursive: true });
for (const f of ["index.html", "app.js", "style.css"]) copyFileSync(join(here, f), join(out, f));
copyFileSync(join(root, "packaging/wasm/index.js"), join(out, "openreadout/index.js"));
copyFileSync(join(pkg, "openreadout.js"), join(out, "openreadout/pkg/openreadout.js"));
const b64 = readFileSync(wasm).toString("base64");
writeFileSync(
  join(out, "openreadout/wasm-inline.js"),
  `// The OpenReadout WebAssembly module (openreadout_bg.wasm), base64.\nexport default "${b64}";\n`,
);
console.log(`demo: ${out} (module ${readFileSync(wasm).length} bytes, inlined ${b64.length})`);
