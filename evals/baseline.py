"""The `baseline` condition's environment: the open-source reader stack, preinstalled.

A skeptic's first objection to `with` vs `without` is that the `without` agent starts from a bare
shell. `baseline` answers it: the same agent with the Python readers a well-equipped computational
scientist would install (bioio and its plugins, nd2, czifile, liffile, tifffile, flowio, pyabf,
neo, nmrglue, pyteomics, rainbow, ...; `baseline/pyproject.toml`), Bio-Formats' command-line tools
(`showinf`, `bfconvert`) and a skill that says which reader fits which format
(`baseline/skill/lab-file-readers/SKILL.md`). Nothing in it imports or calls OpenReadout.

It lives outside the repository, under a neutral name, so tool calls that mention its paths
neither point at the repository nor contain the word `openreadout`:

    ~/.cache/lab-eval-baseline/env/      uv environment (`python3` first on the agent's PATH)
    ~/.cache/lab-eval-baseline/bftools/  copy of oracle/bftools/bftools (Bio-Formats, GPL, run only)

    uv run python baseline.py setup      # build or update it (downloads the Python packages)
    uv run python baseline.py check      # imports every reader and runs `showinf -version`
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path

EVALS = Path(__file__).resolve().parent
ROOT = EVALS.parent
PROJECT = EVALS / "baseline"
SKILL_DIR = PROJECT / "skill" / "lab-file-readers"
BFTOOLS_SRC = ROOT / "oracle" / "bftools" / "bftools"
PYTHON = "3.12"

IMPORTS = [
    "bioio",
    "bioio_czi",
    "bioio_nd2",
    "bioio_lif",
    "bioio_ome_tiff",
    "nd2",
    "czifile",
    "liffile",
    "readlif",
    "tifffile",
    "oirfile",
    "oiffile",
    "dcimg",
    "mrcfile",
    "ncempy",
    "flowio",
    "fcsparser",
    "flowkit",
    "pyabf",
    "neo",
    "pynwb",
    "nmrglue",
    "jcamp",
    "pyteomics",
    "pyimzml",
    "rainbow",
    "h5py",
    "pandas",
    "scipy",
    "pyopenms",
    "brukeropus",
    "renishawWiRE",
    "spectrochempy",
    "rdmlpython",
    "matplotlib",
    "PIL",
]


def home() -> Path:
    base = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
    return Path(base) / "lab-eval-baseline"


def env_dir() -> Path:
    return home() / "env"


def bftools_dir() -> Path:
    return home() / "bftools"


def bin_dirs() -> list[Path]:
    """Directories the `baseline` agent gets in front of its PATH."""
    return [env_dir() / "bin", bftools_dir()]


def ready(require_read_only: bool = True) -> str | None:
    """None when the environment is usable, else what is missing."""
    if not (env_dir() / "bin" / "python3").exists():
        return f"no baseline environment at {env_dir()} (run `uv run python baseline.py setup`)"
    if not (bftools_dir() / "showinf").exists():
        return f"no Bio-Formats tools at {bftools_dir()} (run `uv run python baseline.py setup`)"
    if require_read_only and os.access(env_dir() / "lib", os.W_OK):
        return f"{env_dir()} is writable, so runs could change it (rerun `uv run python baseline.py setup`)"
    return None


def set_writable(path: Path, writable: bool) -> None:
    """The environment is read-only between setups, so an agent's `pip install` cannot leak into
    later runs (the skill tells it to make a `--system-site-packages` venv in its directory)."""
    if not path.exists():
        return
    mode = "u+w" if writable else "a-w"
    subprocess.run(["chmod", "-R", mode, str(path)], check=True)


def setup() -> int:
    env = {**os.environ, "UV_PROJECT_ENVIRONMENT": str(env_dir())}
    env.pop("VIRTUAL_ENV", None)
    set_writable(home(), True)
    subprocess.run(["uv", "sync", "--project", str(PROJECT), "--python", PYTHON], env=env, check=True)
    if not (BFTOOLS_SRC / "showinf").exists():
        print(f"missing {BFTOOLS_SRC}: unpack oracle/bftools/bftools.zip there first (see oracle/)", file=sys.stderr)
        return 1
    if bftools_dir().exists():
        shutil.rmtree(bftools_dir())
    shutil.copytree(BFTOOLS_SRC, bftools_dir(), symlinks=True)
    set_writable(env_dir(), False)
    return check()


def check() -> int:
    missing = ready()
    if missing:
        print(missing, file=sys.stderr)
        return 1
    py = env_dir() / "bin" / "python3"
    code = "\n".join(
        [
            "import importlib, sys",
            "bad = []",
            f"for m in {IMPORTS!r}:",
            "    try: importlib.import_module(m)",
            "    except Exception as e: bad.append(f'{m}: {e}')",
            "print('\\n'.join(bad) or 'all readers import'); sys.exit(1 if bad else 0)",
        ]
    )
    imp = subprocess.run([str(py), "-W", "ignore", "-c", code], capture_output=True, text=True, cwd=home())
    print(imp.stdout.strip() or imp.stderr.strip())
    bf = subprocess.run([str(bftools_dir() / "showinf"), "-version"], capture_output=True, text=True, cwd=home())
    first = (bf.stdout or bf.stderr).strip().splitlines()
    print(f"showinf: {first[0] if first else '(no output)'}")
    return imp.returncode or bf.returncode


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("mode", choices=["setup", "check"])
    args = ap.parse_args(argv)
    return setup() if args.mode == "setup" else check()


if __name__ == "__main__":
    sys.exit(main())
