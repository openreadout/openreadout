"""Run the eval set against a headless Claude Code agent: with OpenReadout, with the best open-source stack, and bare.

Each question runs in a fresh temporary directory that holds only a copy of its corpus file(s)
under a neutral name (truncated when the question says so). The agent is `claude -p` with a
turn cap, a per-question dollar cap, a wall-clock timeout, no session persistence, no user
settings, and only these tools: Bash, Read, Write, Edit, Glob, Grep.

Conditions:
  with      + the openreadout binary on PATH, the openreadout skill (as a plugin) and the
              openreadout MCP server (`openreadout mcp`). `--with-mode cli|mcp|all`.
  baseline  the same agent with the open-source reader stack preinstalled (bioio and its plugins,
            nd2, czifile, liffile, tifffile, flowio, pyabf, neo, nmrglue, pyteomics, rainbow, ...),
            Bio-Formats' showinf/bfconvert, and a skill naming the right reader per format
            (`baseline.py`). No openreadout anywhere. This is the comparison that matters.
  without   the same agent, shell and Python, with no openreadout on PATH, no skill and no MCP
            server. It may install whatever it wants within the turn cap.

Each question can run several times (`--repeats N`); every record carries its repetition and
the question's split (`dev` for optimizing against, `test` sealed; see generate.split_of;
`heldout`, questions about held-out corpus files no reader was developed on, runs only with
`--split heldout`: see evals/heldout.py).
Auth: the child uses the Claude Code login (your subscription). A run refuses to start when
ANTHROPIC_API_KEY or ANTHROPIC_AUTH_TOKEN is set (pay-as-you-go API billing) unless
--allow-api-key, records the auth source Claude Code reports, and stops cleanly (exit 5,
resumable) when the subscription's usage limit is hit. Dollar figures are Claude Code's
estimate at API prices, used as a usage meter and cap, not a bill.

Nothing is sent anywhere but through the Claude Code CLI you are logged in with. No secret or
personal value is put in a prompt; the child's environment is an allow-list (see `child_env`).

Examples:
    uv run --project evals python evals/run.py --dry-run                     # no model calls
    uv run --project evals python evals/run.py preflight                     # stage + openreadout, no model calls
    uv run --project evals python evals/run.py --sample 20 --model claude-haiku-4-5-20251001 \\
        --budget-usd 5 --max-turns 15 --run-name 2026-09-22-haiku-calibration
    uv run --project evals python evals/run.py report --run-name 2026-09-22-haiku-calibration
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
from collections import Counter, defaultdict
from concurrent.futures import FIRST_COMPLETED, Future, ThreadPoolExecutor, wait
from pathlib import Path
from typing import Any

import baseline
import score
import stats

EVALS = Path(__file__).resolve().parent
ROOT = EVALS.parent
RESULTS = EVALS / "results"
FIXTURES = EVALS / "fixtures" / "dry-run-answers.jsonl"
SKILL_DIR = ROOT / "skills" / "openreadout"

DEFAULT_MODEL = "claude-haiku-4-5-20251001"
CONDITIONS = ("with", "baseline", "without")
API_KEY_VARS = ("ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN")
BASE_TOOLS = ["Bash", "Read", "Write", "Edit", "Glob", "Grep"]
# Environment variables the child may inherit (auth and locale only). Values are never logged.
ENV_ALLOW = [
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TERM",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TMPDIR",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CONFIG_DIR",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "SSL_CERT_FILE",
    "REQUESTS_CA_BUNDLE",
]

PROMPT = """You are helping a scientist with a data file from a lab instrument.

The data is in the current working directory: {listing}.

Question: {question}

Rules:
- Use only what is in the current working directory. Do not read, list or search anywhere else
  on this computer, and do not look the answer up online.
- You may run shell commands, write scripts and install software inside this directory if you need to.
- You have a limited number of turns, so be efficient.
- End your reply with exactly one final line of the form
  ANSWER: <your answer>
  where <your answer> is {hint}. If you cannot determine it, write ANSWER: unknown
"""


# ---------------------------------------------------------------- selection


def group_of(q: dict) -> str:
    return q["category"] if q["category"] in ("integrity", "conversion") else q["family"]


def stratified(questions: list[dict], n: int) -> list[dict]:
    """Round-robin over families (integrity and conversion count as their own groups); within a
    family, the question whose category is least represented so far (ties: file order)."""
    groups: dict[str, list[dict]] = defaultdict(list)
    for q in questions:
        groups[group_of(q)].append(q)
    order = sorted(groups)
    picked: list[dict] = []
    per_category: Counter[str] = Counter()
    i = 0
    while len(picked) < n and any(groups.values()):
        g = order[i % len(order)]
        if groups[g]:
            best = min(range(len(groups[g])), key=lambda k: (per_category[groups[g][k]["category"]], k))
            q = groups[g].pop(best)
            per_category[q["category"]] += 1
            picked.append(q)
        i += 1
    return picked


def select(args: argparse.Namespace) -> list[dict]:
    qs = list(score.load_questions().values())
    if args.ids:
        wanted = [i.strip() for i in args.ids.split(",") if i.strip()]
        missing = [i for i in wanted if i not in {q["id"] for q in qs}]
        if missing:
            raise SystemExit(f"unknown question ids: {missing}")
        qs = [q for q in qs if q["id"] in wanted]
    if args.split != "all":
        qs = [q for q in qs if q.get("split") == args.split]
    elif not args.ids:
        # `all` is dev + test: the held-out set (evals/heldout.py) runs only when asked for by name
        qs = [q for q in qs if q.get("split") != "heldout"]
    if args.family:
        qs = [q for q in qs if q["family"] in args.family.split(",")]
    if args.category:
        qs = [q for q in qs if q["category"] in args.category.split(",")]
    if args.sample:
        qs = stratified(qs, args.sample)
    if args.limit:
        qs = qs[: args.limit]
    return qs


# ---------------------------------------------------------------- staging


def corpus_dir() -> Path:
    env = os.environ.get("OPENREADOUT_CORPUS_DIR")
    if env:
        return Path(env)
    here = ROOT / "corpus" / "files"
    if here.exists():
        return here
    # A git worktree under .claude/worktrees/ shares the main checkout's downloads.
    for parent in ROOT.parents:
        cand = parent / "corpus" / "files"
        if cand.exists() and (parent / "corpus" / "manifest.toml").exists():
            return cand
    raise SystemExit("corpus files not found: run `cargo xtask corpus fetch` or set OPENREADOUT_CORPUS_DIR")


def _copy(src: Path, dst: Path) -> None:
    if src.is_dir():
        shutil.copytree(src, dst, ignore=shutil.ignore_patterns("*.ok"), symlinks=False)
    else:
        shutil.copyfile(src, dst)


def _clone(src: Path, dst: Path) -> None:
    """A copy-on-write clone where the file system has them (APFS `cp -c`, `cp --reflink`), else a
    plain copy. Either way the corpus file is never written."""
    dst.parent.mkdir(parents=True, exist_ok=True)
    if src.is_dir():
        _copy(src, dst)
        return
    flag = "-c" if sys.platform == "darwin" else "--reflink=auto"
    done = subprocess.run(["cp", flag, str(src), str(dst)], capture_output=True)
    if done.returncode != 0 or not dst.exists():
        shutil.copyfile(src, dst)


def stage_share(q: dict, workdir: Path, corpus: Path) -> list[str]:
    """Search questions: clone every file of the share under its neutral path, truncate the
    copies the question names. Returns the share directory."""
    for ref in q["share"]:
        src = corpus / ref["path"]
        if not src.exists():
            raise FileNotFoundError(f"{q['id']}: corpus file missing: {ref['path']}")
        _clone(src, workdir / ref["stage_as"])
    # generated files (a sample sheet or plate map), written as the question stores them
    for g in q.get("generated", []):
        dst = workdir / g["stage_as"]
        dst.parent.mkdir(parents=True, exist_ok=True)
        dst.write_text(g["content"])
    for rel, fraction in (q.get("prepare") or {}).get("truncate", {}).items():
        target = workdir / rel
        size = target.stat().st_size
        with target.open("r+b") as fh:
            fh.truncate(int(size * float(fraction)))
    return [q["file"]["stage_as"] + "/"]


def stage(q: dict, workdir: Path, corpus: Path) -> list[str]:
    """Copy the question's file(s) into workdir under their neutral names. Never touches the corpus."""
    if "share" in q:
        return stage_share(q, workdir, corpus)
    names = []
    for ref in [q["file"], *q.get("extra_files", [])]:
        src = corpus / ref["path"]
        if not src.exists():
            raise FileNotFoundError(f"{q['id']}: corpus file missing: {ref['path']}")
        dst = workdir / ref["stage_as"]
        # A copy-on-write clone where the file system has them: multi-GB slides stage instantly and
        # take no space; truncating the clone (integrity questions) never touches the corpus file.
        _clone(src, dst)
        names.append(ref["stage_as"] + ("/" if src.is_dir() else ""))
    prep = q.get("prepare") or {}
    for rel in prep.get("remove", []):
        # e.g. a Bruker experiment staged without its processed data (pdata): only the FID
        target = workdir / q["file"]["stage_as"] / rel
        if target.is_dir():
            shutil.rmtree(target)
        elif target.exists():
            target.unlink()
    if "truncate_fraction" in prep:
        target = workdir / q["file"]["stage_as"]
        size = target.stat().st_size
        with target.open("r+b") as fh:
            fh.truncate(int(size * float(prep["truncate_fraction"])))
    return names


def listing(names: list[str], share_files: int | None = None) -> str:
    if share_files is not None:
        return f"the directory `{names[0]}`, a lab file share with {share_files} files in sub-folders"
    parts = []
    for n in names:
        parts.append(f"the directory `{n}` (one data set)" if n.endswith("/") else f"the file `{n}`")
    return ", ".join(parts)


def build_prompt(q: dict, names: list[str]) -> str:
    share_files = len(q["share"]) if "share" in q else None
    text = listing(names, share_files)
    if q.get("category") in ("batch", "scenario"):
        n = share_files or 0
        sheets = ", ".join(f"`{g['stage_as']}`" for g in q.get("generated", []))
        text = f"the directory `{names[0]}`, a lab folder with {n} data files" + (f" and {sheets}" if sheets else "")
    return PROMPT.format(listing=text, question=q["question"], hint=q["answer_hint"])


# ---------------------------------------------------------------- environment


def find_binary(explicit: str | None) -> Path | None:
    cands = [explicit, os.environ.get("OPENREADOUT_BIN"), shutil.which("openreadout")]
    for parent in [ROOT, *ROOT.parents]:
        cands.append(str(parent / "target" / "release" / "openreadout"))
    for c in cands:
        if c and Path(c).is_file() and os.access(c, os.X_OK):
            return Path(c).resolve()
    return None


def repo_roots() -> list[Path]:
    """Paths the agent must not look at: this checkout, the main checkout above a worktree, the corpus."""
    roots = {ROOT}
    for parent in ROOT.parents:
        if (parent / "corpus" / "manifest.toml").exists():
            roots.add(parent)
    try:
        roots.add(corpus_dir().resolve().parent)
    except SystemExit:
        pass
    return sorted(roots)


def clean_path() -> str:
    """The user's PATH without any directory that holds openreadout, a virtualenv or this repository."""
    keep = []
    roots = [str(r) for r in repo_roots()]
    for d in os.environ.get("PATH", "").split(os.pathsep):
        if not d or any(d.startswith(r) for r in roots):
            continue
        if (Path(d) / "openreadout").exists() or "/.venv/" in d + "/" or os.environ.get("VIRTUAL_ENV", "\0") in d:
            continue
        if d not in keep:
            keep.append(d)
    for d in ("/usr/local/bin", "/opt/homebrew/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"):
        if d not in keep and Path(d).exists():
            keep.append(d)
    return os.pathsep.join(keep)


def child_env(extra_path: Path | list[Path] | None, allow_api_key: bool = True) -> dict[str, str]:
    env = {k: os.environ[k] for k in ENV_ALLOW if k in os.environ}
    if not allow_api_key:
        for k in API_KEY_VARS:
            env.pop(k, None)
    path = clean_path()
    extra = [extra_path] if isinstance(extra_path, Path) else list(extra_path or [])
    env["PATH"] = os.pathsep.join([*map(str, extra), path])
    env["DISABLE_AUTOUPDATER"] = "1"
    return env


class Harness:
    """Per-invocation scratch space: the copied binary, the skill plugin and the MCP config."""

    def __init__(self, binary: Path | None, with_mode: str, keep: bool) -> None:
        self.tmp = Path(tempfile.mkdtemp(prefix="openreadout-eval-"))
        self.keep = keep
        self.with_mode = with_mode
        self.bin_dir = self.tmp / "bin"
        self.bin_dir.mkdir()
        self.binary = None
        if binary:
            # A copy, not a symlink: the agent should not learn where the repository lives.
            self.binary = self.bin_dir / "openreadout"
            shutil.copy2(binary, self.binary)
        self.plugin = self.tmp / "plugin"
        (self.plugin / ".claude-plugin").mkdir(parents=True)
        (self.plugin / ".claude-plugin" / "plugin.json").write_text(
            json.dumps(
                {
                    "name": "openreadout",
                    "version": "0.0.0-eval",
                    "skills": ["./skills/openreadout"],
                }
            )
        )
        shutil.copytree(SKILL_DIR, self.plugin / "skills" / "openreadout")
        self.baseline_plugin = self.tmp / "baseline-plugin"
        (self.baseline_plugin / ".claude-plugin").mkdir(parents=True)
        (self.baseline_plugin / ".claude-plugin" / "plugin.json").write_text(
            json.dumps({"name": "lab-file-readers", "version": "0.0.0-eval", "skills": ["./skills/lab-file-readers"]})
        )
        shutil.copytree(baseline.SKILL_DIR, self.baseline_plugin / "skills" / "lab-file-readers")
        self.mcp_config = self.tmp / "mcp.json"
        if self.binary:
            self.mcp_config.write_text(
                json.dumps({"mcpServers": {"openreadout": {"command": str(self.binary), "args": ["mcp"]}}})
            )

    def close(self) -> None:
        if not self.keep:
            shutil.rmtree(self.tmp, ignore_errors=True)

    def version(self) -> str | None:
        if not self.binary:
            return None
        out = subprocess.run([str(self.binary), "--version"], capture_output=True, text=True, timeout=30)
        return out.stdout.strip() or None


def claude_command(args: argparse.Namespace, harness: Harness, condition: str, prompt: str) -> tuple[list[str], dict]:
    tools = list(BASE_TOOLS)
    allowed = list(BASE_TOOLS)
    cmd = [
        args.claude,
        "-p",
        prompt,
        "--model",
        args.model,
        "--output-format",
        "stream-json",
        "--verbose",
        "--max-turns",
        str(args.max_turns),
        "--max-budget-usd",
        f"{args.max_budget_usd:.2f}",
        "--permission-mode",
        "dontAsk",
        "--no-session-persistence",
        "--setting-sources",
        "project",
        "--strict-mcp-config",
    ]
    extra_path: list[Path] = []
    if condition == "with":
        if harness.binary is None:
            raise SystemExit("condition `with` needs the openreadout binary (--binary or OPENREADOUT_BIN)")
        if harness.with_mode in ("cli", "all"):
            extra_path = [harness.bin_dir]
            tools.append("Skill")
            allowed.append("Skill")
            cmd += ["--plugin-dir", str(harness.plugin)]
        if harness.with_mode in ("mcp", "all"):
            cmd += ["--mcp-config", str(harness.mcp_config)]
            allowed.append("mcp__openreadout")
    elif condition == "baseline":
        missing = baseline.ready()
        if missing:
            raise SystemExit(f"condition `baseline`: {missing}")
        extra_path = baseline.bin_dirs()
        tools.append("Skill")
        allowed.append("Skill")
        cmd += ["--plugin-dir", str(harness.baseline_plugin)]
    else:
        cmd.append("--disable-slash-commands")
    cmd += ["--tools", ",".join(tools), "--allowedTools", *allowed]
    deny = [f"Read(/{r}/**)" for r in repo_roots()]
    cmd += ["--disallowedTools", *deny]
    return cmd, child_env(extra_path, allow_api_key=getattr(args, "allow_api_key", False))


# ---------------------------------------------------------------- one run


def parse_stream(lines: list[str]) -> dict:
    """Pull the result, usage and tool calls out of `--output-format stream-json` lines."""
    out: dict[str, Any] = {"tool_calls": Counter(), "tool_inputs": [], "init": None, "result": None, "images_seen": 0}
    for line in lines:
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        t = msg.get("type")
        if t == "user":
            # tool results that handed the model a picture (a preview, a PNG it read, a thumbnail)
            for block in (msg.get("message") or {}).get("content") or []:
                if isinstance(block, dict) and block.get("type") == "tool_result":
                    inner = block.get("content")
                    if isinstance(inner, list):
                        out["images_seen"] += sum(1 for b in inner if isinstance(b, dict) and b.get("type") == "image")
        if t == "system" and msg.get("subtype") == "init":
            out["init"] = {
                "tools": msg.get("tools"),
                "mcp_servers": msg.get("mcp_servers"),
                "skills": msg.get("skills"),
                "plugins": msg.get("plugins"),
                "model": msg.get("model"),
                "claude_code_version": msg.get("claude_code_version"),
                "api_key_source": msg.get("apiKeySource"),
            }
        elif t == "assistant":
            for block in (msg.get("message") or {}).get("content") or []:
                if block.get("type") == "tool_use":
                    out["tool_calls"][block.get("name", "?")] += 1
                    out["tool_inputs"].append({"name": block.get("name"), "input": block.get("input")})
        elif t == "result":
            out["result"] = msg
    return out


def used_openreadout(tool_inputs: list[dict]) -> bool:
    for call in tool_inputs:
        name = call.get("name") or ""
        if name.startswith("mcp__openreadout"):
            return True
        if name == "Bash" and "openreadout" in json.dumps(call.get("input")):
            return True
        if name == "Skill" and "openreadout" in json.dumps(call.get("input")):
            return True
    return False


IMAGE_SUFFIXES = (".png", ".jpg", ".jpeg", ".gif", ".webp")
PREVIEW_CLI = re.compile(r"\bopenreadout\s+preview\b")


def look_calls(tool_inputs: list[dict]) -> dict[str, int]:
    """Tool calls that look at the data as a picture: the MCP preview tool (or preview resource),
    `openreadout preview` in a shell command, and Read of an image file (a PNG the agent wrote
    with the CLI, matplotlib or PIL)."""
    out = {"preview_mcp": 0, "preview_cli": 0, "image_read": 0}
    for call in tool_inputs:
        name = call.get("name") or ""
        blob = json.dumps(call.get("input"))
        if name.endswith("openreadout_preview") or (name.startswith("mcp__") and "openreadout://preview" in blob):
            out["preview_mcp"] += 1
        elif name == "Bash" and PREVIEW_CLI.search(str((call.get("input") or {}).get("command", ""))):
            out["preview_cli"] += 1
        elif name == "Read" and str((call.get("input") or {}).get("file_path", "")).lower().endswith(IMAGE_SUFFIXES):
            out["image_read"] += 1
    return out


def contamination(tool_inputs: list[dict], roots: list[Path]) -> list[str]:
    """Tool calls that reached for the repository, the corpus or its ground truth."""
    needles = [str(r) for r in roots] + [
        "corpus/oracle",
        "corpus/files",
        "manifest.toml",
        "openreadout/.claude",
    ]
    hits = []
    for call in tool_inputs:
        blob = json.dumps(call.get("input"))
        for n in needles:
            if n in blob:
                hits.append(f"{call.get('name')}: {n}")
    return sorted(set(hits))


LIMIT_RE = re.compile(r"usage limit|rate.?limit|hit your limit|limit reached|resets at|\b429\b|overloaded", re.I)


def hit_limit(res: dict, stderr: str) -> bool:
    """The subscription's usage limit (or an API rate limit) ended the run: not an answer, retry later."""
    text = f"{res.get('result') or ''}\n{stderr[-2000:] if stderr else ''}"
    return bool((res.get("is_error") or not res) and LIMIT_RE.search(text))


def run_one(args: argparse.Namespace, harness: Harness, q: dict, condition: str, corpus: Path, rep: int = 0) -> dict:
    workdir = Path(tempfile.mkdtemp(prefix=f"q-{q['id']}-", dir=harness.tmp))
    names = stage(q, workdir, corpus)
    prompt = build_prompt(q, names)
    cmd, env = claude_command(args, harness, condition, prompt)
    # `pip install --user` lands in the question's own directory, not in a user site shared by later runs.
    env["PYTHONUSERBASE"] = str(workdir / ".pyuser")
    transcript_dir = RESULTS / "transcripts" / args.run_name
    transcript_dir.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    timed_out = False
    proc = subprocess.Popen(
        cmd,
        cwd=workdir,
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,
    )
    try:
        stdout, stderr = proc.communicate(timeout=args.timeout)
    except subprocess.TimeoutExpired:
        timed_out = True
        os.killpg(proc.pid, signal.SIGKILL)
        stdout, stderr = proc.communicate()
    wall = time.monotonic() - started
    lines = stdout.splitlines()
    suffix = f".r{rep}" if rep else ""
    (transcript_dir / f"{q['id']}.{condition}{suffix}.jsonl").write_text(stdout)
    parsed = parse_stream(lines)
    res = parsed["result"] or {}
    artifact = score.check_task(q, workdir) if "task" in q else None
    rec = {
        "id": q["id"],
        "condition": condition,
        "rep": rep,
        "split": q.get("split"),
        "model": args.model,
        "final_text": res.get("result"),
        "subtype": "timeout" if timed_out else res.get("subtype"),
        "is_error": res.get("is_error"),
        "cost_usd": res.get("total_cost_usd"),
        "usage": {k: v for k, v in (res.get("usage") or {}).items() if isinstance(v, int)},
        "num_turns": res.get("num_turns"),
        "duration_ms": res.get("duration_ms"),
        "wall_s": round(wall, 1),
        "exit_code": proc.returncode,
        "tool_calls": dict(parsed["tool_calls"]),
        "used_openreadout": used_openreadout(parsed["tool_inputs"]),
        # did the agent look at the data as a picture? (score.looked reads these)
        "look_calls": look_calls(parsed["tool_inputs"]),
        "images_seen": parsed["images_seen"],
        "contamination": contamination(parsed["tool_inputs"], repo_roots()),
        "mcp_servers": (parsed["init"] or {}).get("mcp_servers"),
        "skills_loaded": [
            s for s in ((parsed["init"] or {}).get("skills") or []) if "openreadout" in str(s) or "lab-file" in str(s)
        ],
        "api_key_source": (parsed["init"] or {}).get("api_key_source"),
        "limit_hit": hit_limit(res, stderr) and not timed_out,
        "stderr_tail": stderr[-500:] if stderr else "",
        "artifact": artifact,
    }
    verdict = score.score_record(q, rec)
    rec["correct"] = bool(verdict["correct"])
    rec["verdict"] = verdict
    if not args.keep_workdirs:
        shutil.rmtree(workdir, ignore_errors=True)
    return rec


# ---------------------------------------------------------------- records


def records_path(run_name: str, condition: str) -> Path:
    return RESULTS / f"{run_name}-{condition}.records.jsonl"


def load_existing(run_name: str, condition: str) -> list[dict]:
    p = records_path(run_name, condition)
    return score.load_records(p) if p.exists() else []


def claude_version(claude: str) -> str | None:
    try:
        out = subprocess.run([claude, "--version"], capture_output=True, text=True, timeout=30)
        return out.stdout.strip() or None
    except (OSError, subprocess.TimeoutExpired):
        return None


def write_reports(run_name: str, conditions: list[str], meta: dict) -> list[Path]:
    questions = score.load_questions()
    written = []
    reports = []
    for cond in conditions:
        recs = load_existing(run_name, cond)
        if not recs:
            continue
        m = {**meta, **(recs[0].get("meta") or {}), "condition": cond}
        prefix = RESULTS / f"{run_name}-{cond}"
        reports.append(score.write_report(recs, questions, m, prefix))
        written += [prefix.with_suffix(".json"), prefix.with_suffix(".md")]
    if len(reports) > 1:
        cmp_path = RESULTS / f"{run_name}-summary.md"
        cmp_path.write_text(score.compare_markdown(reports))
        written.append(cmp_path)
    for split in stats.SPLITS:
        written += stats.write(run_name, conditions, split)
    return written


# ---------------------------------------------------------------- modes


def dry_run(args: argparse.Namespace, questions: list[dict], conditions: list[str]) -> int:
    """Print what would run and score the recorded fixture answers. No model is called."""
    harness = Harness(find_binary(args.binary), args.with_mode, keep=False)
    fixtures: dict[tuple[str, str], dict] = {}
    if FIXTURES.exists():
        for rec in score.load_records(FIXTURES):
            fixtures[(rec["id"], rec["condition"])] = rec
    corpus = None
    try:
        corpus = corpus_dir()
    except SystemExit:
        pass
    records: dict[str, list[dict]] = defaultdict(list)
    try:
        for q in questions:
            names = (
                [q["file"]["stage_as"] + "/"]
                if "share" in q
                else [
                    r["stage_as"] + ("/" if (corpus and (corpus / r["path"]).is_dir()) else "")
                    for r in [q["file"], *q.get("extra_files", [])]
                ]
            )
            prompt = build_prompt(q, names)
            for cond in conditions:
                if args.verbose or q is questions[0]:
                    if harness.binary is None and cond == "with":
                        cmd_s = "(no openreadout binary found; `with` would refuse to run)"
                    else:
                        cmd, env = claude_command(args, harness, cond, "<PROMPT>")
                        cmd_s = shlex.join(cmd) + f"\n  PATH={env['PATH']}"
                    print(f"=== {q['id']} [{cond}]\n{cmd_s}\n--- prompt ---\n{prompt}")
                fx = fixtures.get((q["id"], cond))
                if fx is None:
                    continue
                rec = {**fx, "model": "fixture"}
                records[cond].append(rec)
                verdict = score.score_record(q, rec)
                mark = "ok  " if verdict["correct"] else "FAIL"
                print(f"{mark} {q['id']:32} [{cond:7}] {verdict.get('answer')!r} -> {verdict}")
    finally:
        harness.close()
    qmap = score.load_questions()
    for cond, recs in records.items():
        summary = score.summarize(recs, qmap)
        print(f"fixtures [{cond}]: {summary['overall']['correct']}/{summary['overall']['n']} correct")
        if args.out:
            prefix = Path(args.out) / f"dry-run-{cond}"
            score.write_report(recs, qmap, {"model": "fixture", "condition": cond, "date": "dry-run"}, prefix)
            print(f"wrote {prefix}.json / .md")
    print(f"{len(questions)} questions x {len(conditions)} conditions would run; no model was called")
    return 0


def preflight(args: argparse.Namespace, questions: list[dict]) -> int:
    """Stage every question and run `openreadout check` and `info --view format` on it. No model is called."""
    binary = find_binary(args.binary)
    if binary is None:
        raise SystemExit("preflight needs the openreadout binary (--binary or OPENREADOUT_BIN)")
    corpus = corpus_dir()
    env = child_env(None)
    probe = subprocess.run(
        ["python3", "-c", "import openreadout"],
        env=env,
        capture_output=True,
        text=True,
        cwd=tempfile.gettempdir(),
    )
    print(f"`without` PATH has openreadout: {shutil.which('openreadout', path=env['PATH']) is not None}")
    print(f"`without` python3 can import openreadout: {probe.returncode == 0}")
    base_env = child_env(baseline.bin_dirs())
    print(f"`baseline` environment: {baseline.ready() or 'ready'}")
    print(f"`baseline` PATH has openreadout: {shutil.which('openreadout', path=base_env['PATH']) is not None}")
    bad = gaps = 0
    with tempfile.TemporaryDirectory(prefix="openreadout-preflight-") as tmp:
        for q in questions:
            wd = Path(tmp) / q["id"]
            wd.mkdir()
            try:
                stage(q, wd, corpus)
            except FileNotFoundError as exc:
                print(f"MISSING {q['id']}: {exc}")
                bad += 1
                continue
            target = wd / q["file"]["stage_as"]
            if "share" in q:
                # Every staged file must be recognised (truncated copies included).
                det = subprocess.run(
                    [str(binary), "info", "--view", "format", "-r", "--jsonl", str(target)],
                    capture_output=True,
                    text=True,
                )
                # generated sample sheets and gating workspaces are not instrument files
                skip = {g["stage_as"] for g in q.get("generated", [])}
                skip |= {r["stage_as"] for r in q["share"] if r["stage_as"].endswith(".wsp")}
                unknown = sum(
                    1
                    for line in det.stdout.splitlines()
                    if '"ok":false' in line and not any(line.count(s) for s in skip)
                )
                status = "ok" if unknown == 0 else "CHECK"
                bad += status != "ok"
                print(f"{status:5} {q['id']:32} share of {len(q['share'])} files, {unknown} not recognised")
                continue
            det = subprocess.run(
                [str(binary), "info", str(target), "--view", "format", "--json"], capture_output=True, text=True
            )
            chk = subprocess.run([str(binary), "check", str(target), "--json"], capture_output=True, text=True)
            fmt = None
            try:
                fmt = json.loads(det.stdout)["data"]["format"]
            except (json.JSONDecodeError, KeyError, TypeError):
                pass
            status = preflight_status(q, fmt, chk.returncode)
            bad += status == "CHECK"
            gaps += status == "GAP"
            partial = q["file"].get("partial") and (q["category"] != "integrity" or q["answer"]["type"] != "boolean")
            note = " (partial copy)" if partial else ""
            if status == "GAP":
                note += " (OpenReadout refuses this variant: a reader gap, not a staging problem)"
            print(f"{status:5} {q['id']:32} detect={fmt} check_exit={chk.returncode}{note}")
    gap_note = f", {gaps} unsupported by OpenReadout (exit 6)" if gaps else ""
    print(f"{len(questions)} staged, {bad} to look at{gap_note}")
    return 1 if bad else 0


# Exit code 6 = unsupported feature, a clean refusal of a recognised file's variant
# (book/src/reference/commands/index.md#exit-codes).
EXIT_UNSUPPORTED = 6


def preflight_status(q: dict, fmt: str | None, check_exit: int) -> str:
    """`ok`, `GAP` (a recognised, intact file whose variant OpenReadout refuses cleanly: the copy
    staged fine, the question stands, and the run measures the gap) or `CHECK` (look at it:
    unrecognised, or `check` disagrees with the question about integrity)."""
    if fmt is None:
        return "CHECK"
    expect_intact = q["answer"]["value"] if q["category"] == "integrity" else True
    # A partial copy's `check` exit only answers yes/no integrity questions.
    partial = q["file"].get("partial") and (q["category"] != "integrity" or q["answer"]["type"] != "boolean")
    if partial:
        return "ok"
    if expect_intact and check_exit == EXIT_UNSUPPORTED and q["category"] != "integrity":
        return "GAP"
    if (expect_intact and check_exit != 0) or (not expect_intact and check_exit == 0):
        return "CHECK"
    return "ok"


def run(args: argparse.Namespace, questions: list[dict], conditions: list[str]) -> int:
    corpus = corpus_dir()
    binary = find_binary(args.binary)
    harness = Harness(binary, args.with_mode, keep=args.keep_workdirs)
    RESULTS.mkdir(parents=True, exist_ok=True)
    api_keys = [k for k in API_KEY_VARS if os.environ.get(k)]
    if api_keys and not args.allow_api_key:
        raise SystemExit(
            f"{', '.join(api_keys)} is set: the agent would bill the API instead of your Claude subscription. "
            "Unset it, or pass --allow-api-key to pay per token."
        )
    if "baseline" in conditions and baseline.ready():
        raise SystemExit(f"condition `baseline`: {baseline.ready()}")
    existing = {c: load_existing(args.run_name, c) for c in conditions}
    spent = sum(charged(r, args) for recs in existing.values() for r in recs)
    done = {(r["id"], c, r.get("rep", 0)) for c, recs in existing.items() for r in recs}
    meta = {
        "run_name": args.run_name,
        "model": args.model,
        "date": dt.date.today().isoformat(),
        "claude_version": claude_version(args.claude),
        "openreadout_version": harness.version(),
        "with_mode": args.with_mode,
        "max_turns": args.max_turns,
        "max_budget_usd_per_question": args.max_budget_usd,
        "timeout_s": args.timeout,
        "budget_usd_total": args.budget_usd,
        "repeats": args.repeats,
        "selection": {k: getattr(args, k) for k in ("ids", "family", "category", "split", "sample", "limit")},
    }
    print(
        f"run {args.run_name}: {len(questions)} questions x {conditions} x {args.repeats} repeats; "
        f"already estimated ${spent:.2f}"
    )
    started = time.monotonic()
    status = 0
    # Repetition is the outer loop, so a run stopped early still covers every question evenly.
    jobs = [
        (rep, q, cond)
        for rep in range(args.repeats)
        for q in questions
        for cond in conditions
        if (q["id"], cond, rep) not in done
    ]
    lock = threading.Lock()

    def record(rep: int, q: dict, cond: str, rec: dict) -> None:
        nonlocal spent
        rec["meta"] = meta
        with lock:
            spent += charged(rec, args)
            with records_path(args.run_name, cond).open("a") as fh:
                fh.write(json.dumps(rec, ensure_ascii=False) + "\n")
        mark = "ok  " if rec["correct"] else "FAIL"
        leak = f" LEAK {rec['contamination']}" if rec["contamination"] else ""
        print(
            f"{mark} {q['id']:32} [{cond:8}] r{rep} {rec['verdict'].get('answer')!r} "
            f"turns={rec['num_turns']} ${rec['cost_usd'] or 0:.3f} {rec['wall_s']}s {rec['subtype']} "
            f"openreadout={rec['used_openreadout']}{leak}",
            flush=True,
        )

    # Up to --jobs agents at once. Each in-flight run reserves its per-question cap against the
    # total budget, so the budget holds however many run in parallel.
    pending: dict[Future, tuple[int, dict, str]] = {}
    try:
        with ThreadPoolExecutor(max_workers=max(1, args.jobs)) as pool:
            queue = iter(jobs)
            while True:
                while not status and len(pending) < max(1, args.jobs):
                    job = next(queue, None)
                    if job is None:
                        break
                    if spent + args.max_budget_usd * (len(pending) + 1) > args.budget_usd:
                        print(f"budget stop: ${spent:.2f} used, in-flight runs reserve the rest of the cap")
                        status = 3
                        break
                    if args.deadline_s and time.monotonic() - started + args.timeout > args.deadline_s:
                        print("deadline stop: rerun the same command to resume")
                        status = 4
                        break
                    rep, q, cond = job
                    pending[pool.submit(run_one, args, harness, q, cond, corpus, rep)] = job
                if not pending:
                    break
                finished, _ = wait(pending, return_when=FIRST_COMPLETED)
                for fut in finished:
                    rep, q, cond = pending.pop(fut)
                    rec = fut.result()
                    if rec["limit_hit"]:
                        if not status:
                            print(
                                "usage limit reached "
                                f"({(rec['final_text'] or rec['stderr_tail'] or '').strip()[:120]!r}); "
                                "nothing recorded for it. Rerun the same command after the limit resets."
                            )
                        status = 5
                        continue
                    record(rep, q, cond, rec)
    finally:
        harness.close()
    for p in write_reports(args.run_name, conditions, meta):
        print(f"wrote {p.relative_to(ROOT)}")
    print(f"estimated usage in this run name (API-price equivalent): ${spent:.2f}")
    return status


def charged(rec: dict, args: argparse.Namespace) -> float:
    """Cost counted against the budget: the reported cost, or the per-question cap when none was reported."""
    cost = rec.get("cost_usd")
    return float(cost) if cost is not None else float(args.max_budget_usd)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("mode", nargs="?", default="run", choices=["run", "preflight", "report"])
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="print commands and prompts, score fixtures; no model",
    )
    parser.add_argument("--conditions", default="with,baseline,without", help="any of with,baseline,without")
    parser.add_argument("--repeats", type=int, default=1, help="runs per question and condition")
    parser.add_argument("--jobs", type=int, default=1, help="agents running at once (mind the usage limit)")
    parser.add_argument(
        "--split",
        choices=["dev", "test", "all", "heldout"],
        default="all",
        help="question split to run (`all` = dev + test; `heldout`: held-out files and questions, evals/heldout.py)",
    )
    parser.add_argument(
        "--allow-api-key",
        action="store_true",
        help="let the agent use ANTHROPIC_API_KEY/AUTH_TOKEN (billed per token) instead of your Claude login",
    )
    parser.add_argument("--with-mode", choices=["all", "cli", "mcp"], default="all")
    parser.add_argument("--ids", help="comma-separated question ids")
    parser.add_argument("--family")
    parser.add_argument("--category")
    parser.add_argument("--sample", type=int, help="stratified sample of N questions (round-robin over families)")
    parser.add_argument("--limit", type=int)
    parser.add_argument("--model", default=DEFAULT_MODEL)
    # Caps are safety nets, set high enough that they rarely bind; stats.py reports how often they
    # did and accuracy within 10/20/40 turns from the same runs.
    parser.add_argument("--max-turns", type=int, default=60)
    parser.add_argument("--max-budget-usd", type=float, default=1.0, help="per question and condition")
    parser.add_argument("--budget-usd", type=float, default=2.0, help="hard cap for the whole run name")
    parser.add_argument("--timeout", type=int, default=900, help="seconds per question and condition")
    parser.add_argument("--run-name", default=None, help="results/<run-name>-<condition>.*; resumes if present")
    parser.add_argument("--binary", help="openreadout binary for `with` (default: PATH, then target/release)")
    parser.add_argument("--claude", default=shutil.which("claude") or "claude")
    parser.add_argument(
        "--deadline-s",
        type=int,
        default=0,
        help="do not start a run that could end after this many seconds (resume later with the same command)",
    )
    parser.add_argument("--keep-workdirs", action="store_true")
    parser.add_argument("--out", help="dry run: directory for fixture reports")
    parser.add_argument("-v", "--verbose", action="store_true")
    args = parser.parse_args(argv)
    conditions = [c for c in args.conditions.split(",") if c]
    for c in conditions:
        if c not in CONDITIONS:
            raise SystemExit(f"unknown condition {c}")
    if args.run_name is None:
        short = args.model.replace("claude-", "").split("-2")[0]
        args.run_name = f"{dt.date.today().isoformat()}-{short}"
    questions = select(args)
    if args.mode == "report":
        for p in write_reports(args.run_name, conditions, {"run_name": args.run_name, "model": args.model}):
            print(f"wrote {p}")
        return 0
    if args.mode == "preflight":
        return preflight(args, questions)
    if args.dry_run:
        return dry_run(args, questions, conditions)
    return run(args, questions, conditions)


if __name__ == "__main__":
    sys.exit(main())
