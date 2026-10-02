// Regenerate site/data/formats.json from the CLI: `openreadout self formats --json`, plus the
// documentation page of each format and the field it is listed under on the site.
//
//   node scripts/gen-data.mjs            # uses $OPENREADOUT, else ../target/release/openreadout,
//                                        # else ../target/debug/openreadout, else `openreadout`
//   node scripts/gen-data.mjs --check    # exit 1 if the committed file is stale
//
// The docs workflow runs it before `npm run build`, so the published list is the code's.
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const book = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const root = resolve(book, "..");
const out = join(book, "site/data/formats.json");

const bin =
  process.env.OPENREADOUT ||
  [join(root, "target/release/openreadout"), join(root, "target/debug/openreadout")].find(existsSync) ||
  "openreadout";

// Formats documented on a page named after another id.
const PAGE = {
  imzml: "mzml", mzxml: "mzml", mzmlb: "mzml",
  oib: "oif",
  rdml: "qpcr", "applied-biosystems-eds": "qpcr", "bio-rad-pcrd": "qpcr", "rotor-gene-rex": "qpcr", "roche-lightcycler-ixo": "qpcr",
  atf: "abf", nwb: "hdf5",
  "cytiva-biacore-blr": "cytiva-biacore", "cytiva-biacore-bme": "cytiva-biacore",
  "agilent-seahorse-asyr": "agilent-seahorse", "sartorius-octet-frd": "sartorius-octet",
  "malvern-zetasizer-dts": "malvern-zetasizer",
  "cytiva-unicorn-res": "cytiva-unicorn", "cytiva-unicorn-zip": "cytiva-unicorn",
  "bruker-bes3t": "bruker-epr", "bruker-esp": "bruker-epr",
  "panalytical-xrdml": "xrd", "bruker-raw": "xrd", "bruker-brml": "xrd", "rigaku-ras": "xrd", "rigaku-rasx": "xrd",
  "biologic-mpr": "biologic-eclab", "biologic-mpt": "biologic-eclab",
  "neware-nda": "neware", "neware-ndax": "neware",
  plate: "plate-readers",
};

// The fields the site groups formats under (the CLI's families are finer).
const FIELD = {
  microscopy: "Light microscopy",
  "electron-microscopy": "Electron microscopy",
  "mass-spectrometry": "Mass spectrometry",
  chromatography: "Chromatography",
  spectroscopy: "IR, Raman and UV-Vis",
  nmr: "NMR",
  "flow-cytometry": "Flow cytometry",
  electrophysiology: "Electrophysiology",
  qpcr: "Plate readers and qPCR",
  "plate-reader": "Plate readers and qPCR",
  calorimetry: "Bench biophysics",
  spr: "Bench biophysics",
  "cell-metabolism": "Bench biophysics",
  "binding-kinetics": "Bench biophysics",
  "particle-sizing": "Bench biophysics",
  microarray: "Bench biophysics",
  "gel-imaging": "Bench biophysics",
  epr: "Materials and electrochemistry",
  diffraction: "Materials and electrochemistry",
  electrochemistry: "Materials and electrochemistry",
  "thermal-analysis": "Materials and electrochemistry",
  container: "Containers",
};
export const FIELDS = [...new Set(Object.values(FIELD))];

const raw = JSON.parse(execFileSync(bin, ["self", "formats", "--json"], { encoding: "utf8", maxBuffer: 64 << 20 }));
const version = execFileSync(bin, ["--version"], { encoding: "utf8" }).trim();
const problems = [];
const formats = raw.data.formats.map((f) => {
  const page = PAGE[f.id] ?? f.id;
  if (!existsSync(join(book, "src/formats", page + ".md"))) problems.push(`${f.id}: no page book/src/formats/${page}.md (add it to PAGE)`);
  const field = FIELD[f.family];
  if (!field) problems.push(`${f.id}: family ${f.family} has no field (add it to FIELD)`);
  return { ...f, page, field };
});
if (problems.length) {
  console.error(problems.join("\n"));
  process.exit(1);
}
const text = JSON.stringify({ version, fields: FIELDS, formats }, null, 1) + "\n";
if (process.argv.includes("--check")) {
  const old = existsSync(out) ? readFileSync(out, "utf8") : "";
  if (old !== text) {
    console.error(`error: ${out} is stale; run node book/scripts/gen-data.mjs`);
    process.exit(1);
  }
} else {
  writeFileSync(out, text);
  console.log(`wrote ${out}: ${formats.length} formats (${version})`);
}
