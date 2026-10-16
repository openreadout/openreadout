//! `cargo xtask ci`: run the pull-request CI checks on your machine.
//!
//! The commands come from the `run:` steps of .github/workflows/ci.yml, and .github/ci.toml says
//! which jobs a change affects (through scripts/ci/changes.py) and how each job runs locally (its
//! `local` table). Both files are read on every run, so this command runs what CI runs.
//! `--check` validates the two files against each other without running anything; CI's `repo`
//! job runs it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use yaml_rust2::{Yaml, YamlLoader};

use crate::root;

const WORKFLOW: &str = ".github/workflows/ci.yml";
const CONFIG: &str = ".github/ci.toml";
/// The CI output that says which jobs a change runs; every gated job's `if:` reads it.
const CHANGES_OUTPUT: &str = "fromJSON(needs.changes.outputs.jobs)";
/// Jobs that are not gated on the changes output by design.
const CHANGES_JOB: &str = "changes";
const SUMMARY_JOB: &str = "ci-passed";

#[derive(clap::Args, Debug)]
pub struct CiArgs {
    /// Run every job that has local rules, whatever the change.
    #[arg(long, conflicts_with_all = ["quick", "job"])]
    full: bool,
    /// Run only the fast steps (format and clippy), then the unit tests of the crates you changed.
    #[arg(long, conflicts_with = "job")]
    quick: bool,
    /// Run only this job (repeat for several), whatever the change.
    #[arg(long = "job", value_name = "NAME")]
    job: Vec<String>,
    /// Print what would run and why, without running it.
    #[arg(long)]
    list: bool,
    /// Stop at the first failing step. By default every step runs and all failures are reported.
    #[arg(long, conflicts_with = "keep_going")]
    fail_fast: bool,
    /// Run every step and report all failures (the default).
    #[arg(long)]
    keep_going: bool,
    /// Check that ci.toml and ci.yml agree, without running anything.
    #[arg(long, conflicts_with_all = ["full", "quick", "job", "list"])]
    check: bool,
}

// --- the workflow ---------------------------------------------------------------------------------

#[derive(Debug, Default)]
struct Workflow {
    env: Vec<(String, String)>,
    jobs: Vec<Job>,
}

#[derive(Debug, Default)]
struct Job {
    id: String,
    cond: Option<String>,
    needs: Vec<String>,
    env: Vec<(String, String)>,
    /// `defaults.run.working-directory`
    working_dir: Option<String>,
    steps: Vec<Step>,
}

#[derive(Debug, Default)]
struct Step {
    /// The step's `name`, or what GitHub shows for an unnamed step.
    label: String,
    run: Option<String>,
    cond: Option<String>,
    env: Vec<(String, String)>,
    working_dir: Option<String>,
    shell: Option<String>,
}

fn scalar(y: &Yaml) -> Option<String> {
    match y {
        Yaml::String(s) | Yaml::Real(s) => Some(s.clone()),
        Yaml::Integer(i) => Some(i.to_string()),
        Yaml::Boolean(b) => Some(b.to_string()),
        _ => None,
    }
}

fn env_map(y: &Yaml, at: &str) -> Result<Vec<(String, String)>> {
    let Some(h) = y.as_hash() else {
        return Ok(Vec::new());
    };
    h.iter()
        .map(|(k, v)| {
            let k = scalar(k).with_context(|| format!("{at}: env key is not a string"))?;
            let v = scalar(v).with_context(|| format!("{at}: env {k} is not a plain value"))?;
            Ok((k, v))
        })
        .collect()
}

fn parse_workflow(text: &str) -> Result<Workflow> {
    let docs = YamlLoader::load_from_str(text).context("parse the workflow YAML")?;
    let doc = docs.first().context("the workflow file is empty")?;
    let mut wf = Workflow {
        env: env_map(&doc["env"], "workflow")?,
        jobs: Vec::new(),
    };
    let jobs = doc["jobs"]
        .as_hash()
        .context("the workflow has no `jobs`")?;
    for (id, j) in jobs {
        let id = scalar(id).context("a job id is not a string")?;
        let needs = match &j["needs"] {
            Yaml::Array(a) => a.iter().filter_map(scalar).collect(),
            other => scalar(other).into_iter().collect(),
        };
        let mut job = Job {
            cond: scalar(&j["if"]),
            needs,
            env: env_map(&j["env"], &id)?,
            working_dir: scalar(&j["defaults"]["run"]["working-directory"]),
            ..Job::default()
        };
        if let Some(steps) = j["steps"].as_vec() {
            for s in steps {
                let run = scalar(&s["run"]);
                let uses = scalar(&s["uses"]);
                let label = scalar(&s["name"])
                    .or_else(|| {
                        run.as_deref()
                            .map(|r| format!("Run {}", r.lines().next().unwrap_or("").trim()))
                    })
                    .or_else(|| uses.clone())
                    .unwrap_or_default();
                job.steps.push(Step {
                    label,
                    env: env_map(&s["env"], &format!("{id} step"))?,
                    cond: scalar(&s["if"]),
                    working_dir: scalar(&s["working-directory"]),
                    shell: scalar(&s["shell"]),
                    run,
                });
            }
        }
        job.id = id;
        wf.jobs.push(job);
    }
    Ok(wf)
}

impl Workflow {
    fn job(&self, id: &str) -> Option<&Job> {
        self.jobs.iter().find(|j| j.id == id)
    }
}

// --- ci.toml ---------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Config {
    #[serde(default)]
    tools: BTreeMap<String, String>,
    jobs: BTreeMap<String, JobRules>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JobRules {
    #[allow(dead_code)] // read by scripts/ci/changes.py
    paths: Vec<String>,
    #[serde(default, rename = "with")]
    #[allow(dead_code)]
    with_: Vec<String>,
    local: Option<Local>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Local {
    #[serde(default)]
    requires: Vec<String>,
    #[serde(default)]
    skip: Vec<String>,
    #[serde(default)]
    needs: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    quick: Vec<String>,
    #[serde(default)]
    run_if: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    full_only: bool,
}

fn parse_config(text: &str) -> Result<Config> {
    toml::from_str(text).context("parse .github/ci.toml")
}

// --- expressions -----------------------------------------------------------------------------------

/// The `${{ … }}` expressions in `s`, trimmed.
fn expressions(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(start) = rest.find("${{") {
        let after = &rest[start + 3..];
        let Some(end) = after.find("}}") else {
            out.push(after.trim());
            break;
        };
        out.push(after[..end].trim());
        rest = &after[end + 2..];
    }
    out
}

const SUPPORTED: [&str; 2] = ["github.workspace", "runner.temp"];

fn unsupported(s: &str) -> Vec<&str> {
    expressions(s)
        .into_iter()
        .filter(|e| !SUPPORTED.contains(e))
        .collect()
}

/// Replace `${{ github.workspace }}` and `${{ runner.temp }}`; leave anything else as written.
fn substitute(s: &str, workspace: &str, temp: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find("${{") {
        let after = &rest[start + 3..];
        let Some(end) = after.find("}}") else {
            break;
        };
        out.push_str(&rest[..start]);
        match after[..end].trim() {
            "github.workspace" => out.push_str(workspace),
            "runner.temp" => out.push_str(temp),
            _ => out.push_str(&rest[start..start + 3 + end + 2]),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// Whether a job's `if:` turns it on from its own entry in the changes output.
fn gated_on(cond: &str, id: &str) -> bool {
    let bracket = format!("{CHANGES_OUTPUT}['{id}']");
    let dot = format!("{CHANGES_OUTPUT}.{id}");
    cond.contains(&bracket)
        || cond.match_indices(&dot).any(|(i, m)| {
            !cond[i + m.len()..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
}

/// Every job id an expression reads from the changes output.
fn changes_refs(cond: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, m) in cond.match_indices(CHANGES_OUTPUT) {
        let rest = &cond[i + m.len()..];
        if let Some(r) = rest.strip_prefix("['") {
            if let Some(end) = r.find("']") {
                out.push(r[..end].to_string());
            }
        } else if let Some(r) = rest.strip_prefix('.') {
            let id: String = r
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .collect();
            out.push(id);
        }
    }
    out
}

// --- the plan --------------------------------------------------------------------------------------

/// Why a step or job does not run locally.
#[derive(Debug, Clone, PartialEq)]
enum Skip {
    /// Listed under `skip` in ci.toml.
    Listed,
    /// Has an `if:` the local run does not evaluate.
    Condition(String),
    /// A tool it needs is missing.
    Tool { tool: String, probe: String },
}

impl std::fmt::Display for Skip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Skip::Listed => write!(f, "runner only (ci.toml skip)"),
            Skip::Condition(c) => write!(f, "runner only (if: {c})"),
            Skip::Tool { tool, probe } => write!(f, "{tool} not installed (probe: {probe})"),
        }
    }
}

/// A step the local run would run, with what it needs.
#[derive(Debug)]
struct LocalStep<'a> {
    step: &'a Step,
    skip: Option<Skip>,
}

/// The steps of `job` that would run locally, without probing tools: `uses:` steps are left out,
/// steps in `skip` or with an `if:` not in `run_if` are marked skipped.
fn local_steps<'a>(job: &'a Job, local: &Local) -> Vec<LocalStep<'a>> {
    job.steps
        .iter()
        .filter(|s| s.run.is_some())
        .map(|step| {
            let skip = if local.skip.contains(&step.label) {
                Some(Skip::Listed)
            } else {
                step.cond
                    .as_ref()
                    .filter(|_| !local.run_if.contains(&step.label))
                    .map(|c| Skip::Condition(c.clone()))
            };
            LocalStep { step, skip }
        })
        .collect()
}

// --- --check ---------------------------------------------------------------------------------------

fn check(wf: &Workflow, cfg: &Config) -> Vec<String> {
    let mut problems = Vec::new();
    let all_conds: Vec<&str> = wf
        .jobs
        .iter()
        .flat_map(|j| {
            j.cond
                .iter()
                .chain(j.steps.iter().filter_map(|s| s.cond.as_ref()))
        })
        .map(String::as_str)
        .collect();
    let referenced: BTreeSet<String> = all_conds.iter().flat_map(|c| changes_refs(c)).collect();

    // ci.toml jobs exist in ci.yml, or are read only as conditions.
    for (id, rules) in &cfg.jobs {
        if wf.job(id).is_some() {
            continue;
        }
        if !referenced.contains(id) {
            problems.push(format!(
                "ci.toml job `{id}` is not a job in ci.yml and no `if:` reads it"
            ));
        } else if rules.local.is_some() {
            problems.push(format!(
                "ci.toml job `{id}` has a `local` table but is not a job in ci.yml"
            ));
        }
    }
    for id in &referenced {
        if !cfg.jobs.contains_key(id) {
            problems.push(format!(
                "ci.yml reads `{id}` from the changes output, but ci.toml has no job `{id}`"
            ));
        }
    }

    // Every job is gated on its own entry and is a dependency of ci-passed.
    let summary_needs: &[String] = wf.job(SUMMARY_JOB).map_or(&[], |j| &j.needs);
    if wf.job(SUMMARY_JOB).is_none() {
        problems.push(format!("ci.yml has no `{SUMMARY_JOB}` job"));
    }
    for job in &wf.jobs {
        if job.id == SUMMARY_JOB {
            continue;
        }
        if !summary_needs.contains(&job.id) {
            problems.push(format!(
                "ci.yml job `{}` is missing from {SUMMARY_JOB}'s `needs`",
                job.id
            ));
        }
        if job.id == CHANGES_JOB {
            continue;
        }
        match &job.cond {
            None => problems.push(format!(
                "ci.yml job `{}` has no `if:`; gate it with `if: {CHANGES_OUTPUT}['{}']`",
                job.id, job.id
            )),
            Some(c) if c.contains("needs.changes.outputs.jobs") && !gated_on(c, &job.id) => {
                problems.push(format!(
                    "ci.yml job `{}`: its `if:` reads the changes output but not its own entry `{}`",
                    job.id, job.id
                ));
            }
            Some(_) => {}
        }
    }

    // Local tables name real steps and tools, and local steps need no runner-only expressions.
    let wf_env: Vec<(&str, &str)> = wf
        .env
        .iter()
        .filter(|(k, _)| k != "RUSTFLAGS")
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    for (id, rules) in &cfg.jobs {
        let (Some(local), Some(job)) = (&rules.local, wf.job(id)) else {
            continue;
        };
        let labels: BTreeSet<&str> = job.steps.iter().map(|s| s.label.as_str()).collect();
        let named = local
            .skip
            .iter()
            .map(|n| ("skip", n))
            .chain(local.needs.keys().map(|n| ("needs", n)))
            .chain(local.quick.iter().map(|n| ("quick", n)))
            .chain(local.run_if.iter().map(|n| ("run_if", n)));
        for (key, name) in named {
            if !labels.contains(name.as_str()) {
                problems.push(format!(
                    "ci.toml jobs.{id}.local.{key}: job `{id}` has no step named \"{name}\""
                ));
            }
        }
        for tool in local.requires.iter().chain(local.needs.values().flatten()) {
            if !cfg.tools.contains_key(tool) {
                problems.push(format!(
                    "ci.toml jobs.{id}.local: tool `{tool}` is not in [tools]"
                ));
            }
        }
        let mut env_checks: Vec<(String, &str, &str)> = wf_env
            .iter()
            .map(|(k, v)| ("workflow env".to_string(), *k, *v))
            .collect();
        env_checks.extend(
            job.env
                .iter()
                .map(|(k, v)| ("job env".to_string(), k.as_str(), v.as_str())),
        );
        let mut texts: Vec<(String, &str)> = job
            .working_dir
            .iter()
            .map(|w| ("defaults.run.working-directory".to_string(), w.as_str()))
            .collect();
        for ls in local_steps(job, local) {
            if ls.skip.is_some() {
                continue;
            }
            let s = ls.step;
            let at = format!("step \"{}\"", s.label);
            if let Some(sh) = s.shell.as_deref().filter(|sh| *sh != "bash") {
                problems.push(format!(
                    "ci.yml job `{id}` {at}: shell `{sh}` (only bash runs locally)"
                ));
            }
            texts.extend(s.run.iter().map(|r| (at.clone(), r.as_str())));
            texts.extend(
                s.working_dir
                    .iter()
                    .map(|w| (format!("{at} working-directory"), w.as_str())),
            );
            env_checks.extend(
                s.env
                    .iter()
                    .map(|(k, v)| (format!("{at} env"), k.as_str(), v.as_str())),
            );
        }
        for (at, k, v) in env_checks {
            if local.env.contains_key(k) {
                continue;
            }
            for e in unsupported(v) {
                problems.push(format!(
                    "ci.yml job `{id}` {at} {k}: `${{{{ {e} }}}}` has no local value \
                     (set it in ci.toml jobs.{id}.local.env, or skip the step)"
                ));
            }
        }
        for (at, text) in texts {
            for e in unsupported(text) {
                problems.push(format!(
                    "ci.yml job `{id}` {at}: `${{{{ {e} }}}}` has no local value \
                     (pass it through `env:` and set it in ci.toml, or skip the step)"
                ));
            }
        }
    }
    problems
}

// --- running ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Changed,
    Full,
    Quick,
    Jobs,
}

#[derive(Debug)]
enum Outcome {
    Pass,
    Fail(String),
    Skip(String),
}

#[derive(Debug)]
struct Row {
    job: String,
    step: String,
    outcome: Outcome,
    time: Duration,
}

struct Probes {
    tools: BTreeMap<String, String>,
    cache: HashMap<String, bool>,
    root: PathBuf,
}

impl Probes {
    /// The first missing tool of `names`, with its probe.
    fn missing(&mut self, names: &[String]) -> Option<Skip> {
        for name in names {
            let probe = self.tools.get(name).cloned().unwrap_or_default();
            let ok = *self.cache.entry(name.clone()).or_insert_with(|| {
                Command::new("sh")
                    .arg("-c")
                    .arg(&probe)
                    .current_dir(&self.root)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|s| s.success())
            });
            if !ok {
                return Some(Skip::Tool {
                    tool: name.clone(),
                    probe,
                });
            }
        }
        None
    }
}

/// The first `python3`, `python3.13`, `python3.12` or `python3.11` that is 3.11 or later.
fn python311() -> Option<&'static str> {
    ["python3", "python3.13", "python3.12", "python3.11"]
        .into_iter()
        .find(|p| {
            Command::new(p)
                .args([
                    "-c",
                    "import sys; sys.exit(0 if sys.version_info >= (3, 11) else 1)",
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        })
}

/// The jobs scripts/ci/changes.py turns on, or `None` (with the reason) when it cannot run.
fn changed_jobs(root: &Path) -> std::result::Result<BTreeMap<String, bool>, String> {
    let Some(py) = python311() else {
        return Err(
            "no Python 3.11 or later found (tried python3, python3.13, python3.12, python3.11)"
                .into(),
        );
    };
    let out = Command::new(py)
        .arg("scripts/ci/changes.py")
        .current_dir(root)
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("cannot run scripts/ci/changes.py: {e}"))?;
    if !out.status.success() {
        return Err("scripts/ci/changes.py failed".into());
    }
    serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("scripts/ci/changes.py printed unexpected output: {e}"))
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Files changed since the merge base with origin/main, committed or not, as changes.py sees them.
fn changed_files(root: &Path) -> Option<Vec<String>> {
    let base = git(root, &["merge-base", "origin/main", "HEAD"])?;
    let mut files: BTreeSet<String> = git(root, &["diff", "--name-only", base.trim()])?
        .lines()
        .map(str::to_string)
        .collect();
    files.extend(
        git(root, &["ls-files", "--others", "--exclude-standard"])?
            .lines()
            .map(str::to_string),
    );
    files.retain(|f| !f.is_empty());
    Some(files.into_iter().collect())
}

/// Workspace packages as (name, directory relative to the workspace root).
fn workspace_packages(root: &Path) -> Result<Vec<(String, String)>> {
    let out = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(root)
        .stderr(Stdio::inherit())
        .output()
        .context("run cargo metadata")?;
    if !out.status.success() {
        bail!("cargo metadata failed");
    }
    let meta: serde_json::Value =
        serde_json::from_slice(&out.stdout).context("parse cargo metadata")?;
    let ws = meta["workspace_root"].as_str().unwrap_or_default();
    let mut packages = Vec::new();
    for p in meta["packages"].as_array().into_iter().flatten() {
        let (Some(name), Some(manifest)) = (p["name"].as_str(), p["manifest_path"].as_str()) else {
            continue;
        };
        let dir = Path::new(manifest).parent().unwrap_or(Path::new(""));
        let rel = dir.strip_prefix(ws).unwrap_or(dir);
        packages.push((name.to_string(), rel.to_string_lossy().replace('\\', "/")));
    }
    Ok(packages)
}

/// The packages whose directory holds one of `files` (the innermost one when they nest).
fn changed_packages(files: &[String], packages: &[(String, String)]) -> Vec<String> {
    let mut out = BTreeSet::new();
    for f in files {
        let owner = packages
            .iter()
            .filter(|(_, dir)| !dir.is_empty() && f.starts_with(&format!("{dir}/")))
            .max_by_key(|(_, dir)| dir.len());
        if let Some((name, _)) = owner {
            out.insert(name.clone());
        }
    }
    out.into_iter().collect()
}

/// Read `KEY=value` and `KEY<<DELIM … DELIM` lines from a GITHUB_ENV-style file.
fn read_env_file(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if let Some((k, delim)) = line.split_once("<<") {
            let mut value = Vec::new();
            for l in lines.by_ref() {
                if l == delim {
                    break;
                }
                value.push(l);
            }
            out.push((k.to_string(), value.join("\n")));
        } else if let Some((k, v)) = line.split_once('=') {
            out.push((k.to_string(), v.to_string()));
        }
    }
    out
}

const RUNNER_OS: &str = if cfg!(target_os = "macos") {
    "macOS"
} else if cfg!(windows) {
    "Windows"
} else {
    "Linux"
};

/// One job's local environment: its temp directory, the GITHUB_* files and what steps add.
struct JobEnv {
    temp: PathBuf,
    files: [(&'static str, PathBuf); 4],
    added_env: Vec<(String, String)>,
    added_path: Vec<String>,
}

impl JobEnv {
    fn new(run_temp: &Path, job: &str) -> Result<Self> {
        let base = run_temp.join(job);
        let temp = base.join("temp");
        let github = base.join("github");
        fs::create_dir_all(&temp)?;
        fs::create_dir_all(&github)?;
        let files = [
            ("GITHUB_STEP_SUMMARY", github.join("step_summary.md")),
            ("GITHUB_OUTPUT", github.join("output")),
            ("GITHUB_ENV", github.join("env")),
            ("GITHUB_PATH", github.join("path")),
        ];
        for (_, f) in &files {
            fs::write(f, "")?;
        }
        Ok(Self {
            temp,
            files,
            added_env: Vec::new(),
            added_path: Vec::new(),
        })
    }

    /// Take what the last step wrote to GITHUB_ENV and GITHUB_PATH, as the runner does.
    fn collect(&mut self) -> Result<()> {
        let env = &self.files[2].1;
        self.added_env
            .extend(read_env_file(&fs::read_to_string(env)?));
        fs::write(env, "")?;
        let path = &self.files[3].1;
        let mut new: Vec<String> = fs::read_to_string(path)?
            .lines()
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect();
        new.reverse();
        self.added_path.splice(0..0, new);
        fs::write(path, "")?;
        Ok(())
    }
}

struct Runner {
    root: PathBuf,
    run_temp: PathBuf,
    fail_fast: bool,
    rows: Vec<Row>,
    failed: bool,
}

impl Runner {
    fn step_env(
        &self,
        wf: &Workflow,
        job: &Job,
        local: &Local,
        step: &Step,
        je: &JobEnv,
    ) -> Vec<(String, String)> {
        let ws = self.root.to_string_lossy().into_owned();
        let temp = je.temp.to_string_lossy().into_owned();
        let mut env: Vec<(String, String)> = wf
            .env
            .iter()
            .filter(|(k, _)| k != "RUSTFLAGS")
            .chain(&job.env)
            .chain(&je.added_env)
            .chain(&step.env)
            .map(|(k, v)| (k.clone(), substitute(v, &ws, &temp)))
            .collect();
        env.extend(local.env.iter().map(|(k, v)| (k.clone(), v.clone())));
        env.push(("GITHUB_WORKSPACE".into(), ws));
        env.push(("RUNNER_TEMP".into(), temp));
        env.push(("RUNNER_OS".into(), RUNNER_OS.into()));
        for (k, f) in &je.files {
            env.push(((*k).to_string(), f.to_string_lossy().into_owned()));
        }
        if !je.added_path.is_empty() {
            let mut paths: Vec<PathBuf> = je.added_path.iter().map(PathBuf::from).collect();
            if let Some(p) = std::env::var_os("PATH") {
                paths.extend(std::env::split_paths(&p));
            }
            if let Ok(joined) = std::env::join_paths(paths) {
                env.push(("PATH".into(), joined.to_string_lossy().into_owned()));
            }
        }
        env
    }

    /// Run one script; return whether to go on.
    fn run_script(
        &mut self,
        job: &str,
        label: &str,
        script: &str,
        cwd: &Path,
        env: &[(String, String)],
    ) -> bool {
        println!("\n== {job} / {label}");
        let _ = std::io::stdout().flush();
        let start = Instant::now();
        let status = Command::new("bash")
            .args(["--noprofile", "--norc", "-eo", "pipefail", "-c", script])
            .current_dir(cwd)
            .env_remove("CI")
            .envs(env.iter().map(|(k, v)| (k, v)))
            .status();
        let outcome = match status {
            Ok(s) if s.success() => Outcome::Pass,
            Ok(s) => Outcome::Fail(
                s.code()
                    .map_or_else(|| "killed".into(), |c| format!("exit {c}")),
            ),
            Err(e) => Outcome::Fail(format!("cannot start bash: {e}")),
        };
        let failed = matches!(outcome, Outcome::Fail(_));
        self.rows.push(Row {
            job: job.into(),
            step: label.into(),
            outcome,
            time: start.elapsed(),
        });
        self.failed |= failed;
        !(failed && self.fail_fast)
    }

    fn skip(&mut self, job: &str, step: &str, why: String) {
        self.rows.push(Row {
            job: job.into(),
            step: step.into(),
            outcome: Outcome::Skip(why),
            time: Duration::ZERO,
        });
    }

    /// Run the selected steps of one job; return whether to go on.
    fn run_job(
        &mut self,
        wf: &Workflow,
        job: &Job,
        local: &Local,
        probes: &mut Probes,
        quick: bool,
    ) -> Result<bool> {
        if let Some(why) = probes.missing(&local.requires) {
            println!("\n== {} skipped: {why}", job.id);
            self.skip(&job.id, "", why.to_string());
            return Ok(true);
        }
        let mut je = JobEnv::new(&self.run_temp, &job.id)?;
        for ls in local_steps(job, local) {
            let s = ls.step;
            if quick && !local.quick.contains(&s.label) {
                continue;
            }
            let skip = ls.skip.or_else(|| {
                local
                    .needs
                    .get(&s.label)
                    .and_then(|tools| probes.missing(tools))
            });
            if let Some(why) = skip {
                self.skip(&job.id, &s.label, why.to_string());
                continue;
            }
            let ws = self.root.to_string_lossy().into_owned();
            let temp = je.temp.to_string_lossy().into_owned();
            let cwd = s
                .working_dir
                .as_ref()
                .or(job.working_dir.as_ref())
                .map_or_else(
                    || self.root.clone(),
                    |w| self.root.join(substitute(w, &ws, &temp)),
                );
            let env = self.step_env(wf, job, local, s, &je);
            let script = substitute(s.run.as_deref().unwrap_or_default(), &ws, &temp);
            let go_on = self.run_script(&job.id, &s.label, &script, &cwd, &env);
            je.collect()?;
            if !go_on {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn report(&self) {
        let jw = self
            .rows
            .iter()
            .map(|r| r.job.len())
            .max()
            .unwrap_or(0)
            .max(3);
        let sw = self
            .rows
            .iter()
            .map(|r| r.step.chars().count())
            .max()
            .unwrap_or(0)
            .clamp(4, 72);
        println!("\n{:<jw$}  {:<sw$}  {:>8}  result", "job", "step", "time");
        for r in &self.rows {
            let step: String = if r.step.chars().count() > sw {
                r.step.chars().take(sw - 1).chain(['…']).collect()
            } else {
                r.step.clone()
            };
            let result = match &r.outcome {
                Outcome::Pass => "pass".to_string(),
                Outcome::Fail(why) => format!("FAIL ({why})"),
                Outcome::Skip(why) => format!("skip: {why}"),
            };
            let time = if matches!(r.outcome, Outcome::Skip(_)) {
                String::new()
            } else {
                format_duration(r.time)
            };
            println!("{:<jw$}  {step:<sw$}  {time:>8}  {result}", r.job);
        }
    }
}

fn format_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 60 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{:.1}s", d.as_secs_f64())
    }
}

/// Which jobs to run and, for the others, why not.
fn select<'a>(
    wf: &'a Workflow,
    cfg: &'a Config,
    mode: Mode,
    chosen: &[String],
    changed: Option<&BTreeMap<String, bool>>,
) -> Vec<(&'a Job, std::result::Result<&'a Local, String>)> {
    wf.jobs
        .iter()
        .filter(|j| j.id != CHANGES_JOB && j.id != SUMMARY_JOB)
        .filter(|j| mode != Mode::Jobs || chosen.contains(&j.id))
        .map(|job| {
            let local = cfg.jobs.get(&job.id).and_then(|r| r.local.as_ref());
            let pick = match local {
                None => Err("no local rules in ci.toml (runs only in CI)".to_string()),
                Some(l) => match mode {
                    Mode::Jobs => Ok(l),
                    Mode::Quick if l.quick.is_empty() => Err("no quick steps".into()),
                    Mode::Quick => Ok(l),
                    Mode::Full => Ok(l),
                    Mode::Changed if l.full_only => Err("runs only with --full".into()),
                    Mode::Changed => match changed {
                        Some(c) if !c.get(&job.id).copied().unwrap_or(false) => {
                            Err("not affected by this change".into())
                        }
                        _ => Ok(l),
                    },
                },
            };
            (job, pick)
        })
        .collect()
}

pub fn run(args: &CiArgs) -> Result<()> {
    let root = root();
    let wf_text =
        fs::read_to_string(root.join(WORKFLOW)).with_context(|| format!("read {WORKFLOW}"))?;
    let cfg_text =
        fs::read_to_string(root.join(CONFIG)).with_context(|| format!("read {CONFIG}"))?;
    let wf = parse_workflow(&wf_text).with_context(|| WORKFLOW.to_string())?;
    let cfg = match parse_config(&cfg_text) {
        Ok(c) => c,
        Err(e) if args.check => bail!("{CONFIG} does not parse: {e:#}"),
        Err(e) => return Err(e),
    };

    if args.check {
        let problems = check(&wf, &cfg);
        if problems.is_empty() {
            let local = cfg.jobs.values().filter(|r| r.local.is_some()).count();
            println!(
                "ci --check: OK ({} jobs in ci.yml, {} path rules and {local} local tables in ci.toml)",
                wf.jobs.len(),
                cfg.jobs.len()
            );
            return Ok(());
        }
        for p in &problems {
            eprintln!("{p}");
        }
        bail!("{} problem(s) in {CONFIG} or {WORKFLOW}", problems.len());
    }
    // A broken configuration would make the local run differ from CI: refuse to run.
    let problems = check(&wf, &cfg);
    if !problems.is_empty() {
        for p in &problems {
            eprintln!("{p}");
        }
        bail!("{CONFIG} and {WORKFLOW} disagree; `cargo xtask ci --check` lists the same problems");
    }

    let mode = if args.quick {
        Mode::Quick
    } else if args.full {
        Mode::Full
    } else if !args.job.is_empty() {
        Mode::Jobs
    } else {
        Mode::Changed
    };
    for name in &args.job {
        match wf.job(name) {
            None => bail!("ci.yml has no job `{name}`"),
            Some(_) if cfg.jobs.get(name).and_then(|r| r.local.as_ref()).is_none() => {
                bail!("job `{name}` has no local rules in {CONFIG}, so it runs only in CI")
            }
            Some(_) => {}
        }
    }

    let changed = if mode == Mode::Changed {
        match changed_jobs(&root) {
            Ok(c) => Some(c),
            Err(why) => {
                println!("{why}; running every job that has local rules");
                None
            }
        }
    } else {
        None
    };
    if wf.env.iter().any(|(k, _)| k == "RUSTFLAGS") {
        println!(
            "RUSTFLAGS from ci.yml is not applied: it would rebuild everything you build without it. \
             Clippy's `-D warnings` gives the same verdict."
        );
    }

    let plan = select(&wf, &cfg, mode, &args.job, changed.as_ref());
    let mut probes = Probes {
        tools: cfg.tools.clone(),
        cache: HashMap::new(),
        root: root.clone(),
    };

    // The unit tests of the crates you changed, for --quick.
    let quick_tests = if mode == Mode::Quick {
        Some(quick_test_command(&root, &mut probes)?)
    } else {
        None
    };

    if args.list {
        let why_run = match (mode, &changed) {
            (Mode::Changed, Some(_)) => "this change affects it",
            (Mode::Changed, None) => "the change could not be computed",
            (Mode::Full, _) => "--full",
            (Mode::Quick, _) => "--quick",
            (Mode::Jobs, _) => "--job",
        };
        list(&plan, &mut probes, mode, why_run, quick_tests.as_ref());
        return Ok(());
    }

    let run_temp = std::env::temp_dir().join(format!("xtask-ci-{}", std::process::id()));
    fs::create_dir_all(&run_temp)?;
    let mut runner = Runner {
        root: root.clone(),
        run_temp: run_temp.clone(),
        fail_fast: args.fail_fast,
        rows: Vec::new(),
        failed: false,
    };
    let result = (|| -> Result<()> {
        for (job, pick) in &plan {
            let Ok(local) = pick else { continue };
            if !runner.run_job(&wf, job, local, &mut probes, mode == Mode::Quick)? {
                return Ok(());
            }
        }
        match &quick_tests {
            Some(Ok(cmd)) => {
                let env = [("CARGO_TERM_COLOR".to_string(), "always".to_string())];
                runner.run_script("quick", "tests of the changed crates", cmd, &root, &env);
            }
            Some(Err(why)) => runner.skip("quick", "tests of the changed crates", why.clone()),
            None => {}
        }
        Ok(())
    })();
    let _ = fs::remove_dir_all(&run_temp);
    result?;

    if runner.rows.is_empty() {
        println!("nothing to run for this change");
        return Ok(());
    }
    runner.report();
    if runner.failed {
        bail!("some CI steps failed");
    }
    Ok(())
}

/// `cargo nextest run -p …` for the changed crates, or why no tests run.
fn quick_test_command(
    root: &Path,
    probes: &mut Probes,
) -> Result<std::result::Result<String, String>> {
    let Some(files) = changed_files(root) else {
        return Ok(Err(
            "cannot tell what changed (no merge base with origin/main)".into(),
        ));
    };
    let crates = changed_packages(&files, &workspace_packages(root)?);
    if crates.is_empty() {
        return Ok(Err("no workspace crate changed".into()));
    }
    let flags: String = crates.iter().fold(String::new(), |mut acc, c| {
        let _ = write!(acc, " -p {c}");
        acc
    });
    let nextest = probes.missing(&["cargo-nextest".to_string()]).is_none();
    Ok(Ok(if nextest {
        format!("cargo nextest run --no-fail-fast{flags}")
    } else {
        format!("cargo test{flags}")
    }))
}

fn list(
    plan: &[(&Job, std::result::Result<&Local, String>)],
    probes: &mut Probes,
    mode: Mode,
    why_run: &str,
    quick_tests: Option<&std::result::Result<String, String>>,
) {
    let mut out = String::new();
    for (job, pick) in plan {
        let local = match pick {
            Err(why) => {
                let _ = writeln!(out, "{:<20} skip: {why}", job.id);
                continue;
            }
            Ok(l) => l,
        };
        if let Some(why) = probes.missing(&local.requires) {
            let _ = writeln!(out, "{:<20} skip: {why}", job.id);
            continue;
        }
        let _ = writeln!(out, "{:<20} run ({why_run})", job.id);
        for ls in local_steps(job, local) {
            let s = ls.step;
            if mode == Mode::Quick && !local.quick.contains(&s.label) {
                continue;
            }
            let skip = ls
                .skip
                .or_else(|| local.needs.get(&s.label).and_then(|t| probes.missing(t)));
            let _ = match skip {
                None => writeln!(out, "    run   {}", s.label),
                Some(w) => writeln!(out, "    skip  {}: {w}", s.label),
            };
        }
    }
    match quick_tests {
        Some(Ok(cmd)) => {
            let _ = writeln!(
                out,
                "{:<20} run (the tests of the crates you changed)\n    run   {cmd}",
                "quick"
            );
        }
        Some(Err(why)) => {
            let _ = writeln!(out, "{:<20} skip: {why}", "quick");
        }
        None => {}
    }
    print!("{out}");
}

#[cfg(test)]
mod tests {
    use super::*;

    const WF: &str = r"
env:
  CARGO_TERM_COLOR: always
  RUSTFLAGS: -D warnings
jobs:
  changes:
    runs-on: ubuntu-latest
    steps:
      - run: python3 scripts/ci/changes.py --github-output
  lint:
    needs: changes
    if: fromJSON(needs.changes.outputs.jobs).lint
    defaults:
      run:
        working-directory: sub
    env:
      N: 4
    steps:
      - uses: actions/checkout@v9
      - name: Format
        run: cargo fmt --check
      - name: Tests
        env:
          REPORT: ${{ github.workspace }}/report.md
        run: |
          cargo test
          echo done
      - name: Upload
        if: always()
        run: cat report.md
  build-os:
    needs: changes
    if: fromJSON(needs.changes.outputs.jobs)['build-os']
    steps:
      - name: Build
        env:
          OS: ${{ matrix.os }}
        run: cargo build
  ci-passed:
    if: always()
    needs: [changes, lint, build-os]
    steps:
      - run: echo ok
";

    const CFG: &str = r#"
[jobs.lint]
paths = ["**"]
[jobs.lint.local]
requires = ["cargo"]
quick = ["Format"]
[jobs.build-os]
paths = ["**"]
[jobs.build-os.local]
env = { OS = "here" }
[tools]
cargo = "cargo --version"
"#;

    #[test]
    fn steps_come_from_the_workflow() {
        let wf = parse_workflow(WF).unwrap();
        assert_eq!(wf.env.len(), 2);
        let lint = wf.job("lint").unwrap();
        assert_eq!(lint.working_dir.as_deref(), Some("sub"));
        assert_eq!(lint.env, vec![("N".to_string(), "4".to_string())]);
        let labels: Vec<_> = lint.steps.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["actions/checkout@v9", "Format", "Tests", "Upload"]);
        assert_eq!(
            lint.steps[2].run.as_deref(),
            Some("cargo test\necho done\n")
        );
        assert_eq!(lint.steps[3].cond.as_deref(), Some("always()"));
        let changes = wf.job("changes").unwrap();
        assert_eq!(
            changes.steps[0].label,
            "Run python3 scripts/ci/changes.py --github-output"
        );
        assert_eq!(
            wf.job("ci-passed").unwrap().needs,
            ["changes", "lint", "build-os"]
        );

        let local = Local::default();
        let steps = local_steps(lint, &local);
        assert_eq!(steps.len(), 3, "uses: steps are not run");
        assert_eq!(steps[2].skip, Some(Skip::Condition("always()".into())));
    }

    #[test]
    fn expressions_are_found_and_substituted() {
        assert_eq!(expressions("a ${{ x.y }} b ${{z}}"), ["x.y", "z"]);
        assert!(unsupported("${{ github.workspace }}/x ${{runner.temp}}").is_empty());
        assert_eq!(
            unsupported("${{ matrix.os }} ${{ github.workspace }}"),
            ["matrix.os"]
        );
        assert_eq!(
            substitute(
                "${{ github.workspace }}/a ${{ runner.temp }} ${{ matrix.os }}",
                "/r",
                "/t"
            ),
            "/r/a /t ${{ matrix.os }}"
        );
        assert!(gated_on(
            "fromJSON(needs.changes.outputs.jobs).lint",
            "lint"
        ));
        assert!(!gated_on(
            "fromJSON(needs.changes.outputs.jobs).lint-x",
            "lint"
        ));
        assert!(gated_on(
            "x && fromJSON(needs.changes.outputs.jobs)['a-b']",
            "a-b"
        ));
        assert_eq!(
            changes_refs(
                "fromJSON(needs.changes.outputs.jobs).a && fromJSON(needs.changes.outputs.jobs)['b-c']"
            ),
            ["a", "b-c"]
        );
    }

    #[test]
    fn check_accepts_a_consistent_pair() {
        let problems = check(&parse_workflow(WF).unwrap(), &parse_config(CFG).unwrap());
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn check_catches_a_renamed_step() {
        let wf = parse_workflow(&WF.replace("name: Format", "name: Formatting")).unwrap();
        let problems = check(&wf, &parse_config(CFG).unwrap());
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].contains("no step named \"Format\""),
            "{problems:?}"
        );
    }

    #[test]
    fn check_catches_a_job_missing_from_ci_passed() {
        let wf =
            parse_workflow(&WF.replace("[changes, lint, build-os]", "[changes, lint]")).unwrap();
        let problems = check(&wf, &parse_config(CFG).unwrap());
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].contains("`build-os` is missing from ci-passed"),
            "{problems:?}"
        );
    }

    #[test]
    fn check_catches_runner_only_expressions_and_unknown_names() {
        // Without the local value, the matrix expression has none.
        let cfg = parse_config(&CFG.replace("env = { OS = \"here\" }", "")).unwrap();
        let problems = check(&parse_workflow(WF).unwrap(), &cfg);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("matrix.os"), "{problems:?}");

        let cfg = parse_config(&format!("{CFG}\n[jobs.ghost]\npaths = [\"x\"]\n")).unwrap();
        let problems = check(&parse_workflow(WF).unwrap(), &cfg);
        assert!(
            problems.iter().any(|p| p.contains("`ghost`")),
            "{problems:?}"
        );

        let wf = parse_workflow(&WF.replace("jobs)['build-os']", "jobs).lint")).unwrap();
        let problems = check(&wf, &parse_config(CFG).unwrap());
        assert!(
            problems.iter().any(|p| p.contains("not its own entry")),
            "{problems:?}"
        );

        let cfg =
            parse_config(&CFG.replace("requires = [\"cargo\"]", "requires = [\"rake\"]")).unwrap();
        let problems = check(&parse_workflow(WF).unwrap(), &cfg);
        assert!(
            problems
                .iter()
                .any(|p| p.contains("`rake` is not in [tools]")),
            "{problems:?}"
        );
    }

    #[test]
    fn changed_files_map_to_their_crates() {
        let packages = vec![
            (
                "openreadout-czi".to_string(),
                "crates/openreadout-czi".to_string(),
            ),
            (
                "openreadout-core".to_string(),
                "crates/openreadout-core".to_string(),
            ),
            ("xtask".to_string(), "xtask".to_string()),
            (
                "nested".to_string(),
                "crates/openreadout-core/nested".to_string(),
            ),
        ];
        let files: Vec<String> = [
            "crates/openreadout-czi/src/lib.rs",
            "crates/openreadout-czi/tests/a.rs",
            "crates/openreadout-core/nested/src/lib.rs",
            "crates/openreadout-cz/x.rs",
            "docs/formats/czi.md",
            "xtask/src/ci.rs",
        ]
        .map(String::from)
        .into();
        assert_eq!(
            changed_packages(&files, &packages),
            ["nested", "openreadout-czi", "xtask"]
        );
    }

    #[test]
    fn env_files_are_read_like_the_runner() {
        let text = "A=1\nB<<EOF\nx\ny\nEOF\nC=a=b\n";
        assert_eq!(
            read_env_file(text),
            [
                ("A".to_string(), "1".to_string()),
                ("B".to_string(), "x\ny".to_string()),
                ("C".to_string(), "a=b".to_string())
            ]
        );
    }
}
