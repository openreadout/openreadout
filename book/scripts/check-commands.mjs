// Check that every flag a command accepts is documented on its reference page: for each
// book/src/reference/commands/<cmd>.md, every `--flag` in `openreadout <cmd> --help` (and in the
// help of its subcommands) must appear on the page. Global options are documented once, in
// reference/commands/index.md, and the shared input and table flags in index.md and batch.md. The binary is found as in gen-data.mjs.
//
//   node scripts/check-commands.mjs
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const book = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const root = resolve(book, "..");
const bin =
  process.env.OPENREADOUT ||
  [join(root, "target/release/openreadout"), join(root, "target/debug/openreadout")].find(existsSync) ||
  "openreadout";
const dir = join(book, "src/reference/commands");
const help = (args) => execFileSync(bin, [...args, "--help"], { encoding: "utf8", env: { ...process.env, COLUMNS: "200" } });

// "Options:" flags of one help text, without the "Global options:" section.
function flags(text) {
  const own = text.split(/\n(?:Global options|Global Options):/)[0];
  return new Set([...own.matchAll(/^\s+(?:-\w, )?(--[a-z0-9][a-z0-9-]*)/gm)].map((m) => m[1]).filter((f) => f !== "--help" && f !== "--version"));
}
function subcommands(text) {
  const m = text.match(/\nCommands:\n([\s\S]*?)\n\n/);
  return m ? [...m[1].matchAll(/^\s{2}([a-z][a-z0-9-]*)\s/gm)].map((x) => x[1]).filter((c) => c !== "help") : [];
}

// Flags shared by many commands are documented once: "Several inputs" (index.md) and "Batch table
// flags" (batch.md).
const index = readFileSync(join(dir, "index.md"), "utf8") + readFileSync(join(dir, "batch.md"), "utf8");
let missing = 0;
for (const f of readdirSync(dir).filter((f) => f.endsWith(".md") && f !== "index.md")) {
  const cmd = f === "index-cmd.md" ? "index" : f.replace(/\.md$/, "");
  const page = readFileSync(join(dir, f), "utf8") + index;
  const top = help([cmd]);
  const want = new Map([...flags(top)].map((x) => [x, cmd]));
  for (const sub of subcommands(top)) for (const x of flags(help([cmd, sub]))) if (!want.has(x)) want.set(x, `${cmd} ${sub}`);
  const gone = [...want].filter(([x]) => !new RegExp("`" + x + "(?![a-z0-9-])").test(page) && !page.includes(x + " ") && !page.includes(x + "="));
  for (const [x, where] of gone) console.log(`${f}: ${where} ${x} is not documented`);
  missing += gone.length;
}
if (missing) {
  console.error(`${missing} undocumented flags`);
  process.exit(1);
}
console.log("every command flag is documented");
