//! Finite owner-side publication of accepted Factory deliveries. No scheduler/model calls.
use anyhow::{Context, Result, bail, ensure};
use regex::{Regex, RegexBuilder};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::Read,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
};

use crate::{
    delivery_audit::audit_cycle,
    goal_acceptance::audit_result,
    goal_state::{Lock, RawJson, resolve, save_raw},
    recovery::{read_json, sha, supervised_command, supervised_owner_command},
};

pub const TARGETS: [(&str, i64); 3] = [
    ("lhemerly/code-factory-controller", 1414818646),
    ("lhemerly/spark-scroll-mapper", 1418813893),
    ("lhemerly/ink-detector", 1422424015),
];

const MAX_PATCH_BYTES: usize = 2_000_000;

fn is_exact_sha40(s: &str) -> bool {
    s.len() == 40
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn is_valid_preflight_prefix(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("factory/") else {
        return false;
    };
    (1..=80).contains(&rest.len())
        && rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn target_id(repo: &str) -> Option<i64> {
    TARGETS
        .iter()
        .find(|(name, _)| *name == repo)
        .map(|(_, id)| *id)
}

fn json_int(v: &Value) -> Option<i64> {
    if v.is_boolean() {
        return None;
    }
    v.as_i64()
        .or_else(|| v.as_u64().and_then(|u| i64::try_from(u).ok()))
}

fn exact_int_eq(v: &Value, expected: i64) -> bool {
    if v.is_boolean() {
        return false;
    }
    if let Some(i) = v.as_i64() {
        return i == expected;
    }
    if let Some(u) = v.as_u64() {
        return i64::try_from(u).ok() == Some(expected);
    }
    if let Some(f) = v.as_f64() {
        return f == (expected as f64);
    }
    false
}

fn exact_values_equal(a: &Value, b: &Value) -> bool {
    if a.is_boolean() != b.is_boolean() {
        return false;
    }
    if let (Some(na), Some(nb)) = (a.as_f64(), b.as_f64())
        && !a.is_boolean()
        && !b.is_boolean()
    {
        if let (Some(ia), Some(ib)) = (json_int(a), json_int(b)) {
            return ia == ib;
        }
        return na == nb;
    }
    a == b
}

pub fn identity(b: &Value, s: &Value) -> Result<()> {
    let b_obj = b.as_object().context("binding must be an object")?;
    let s_obj = s.as_object().context("snapshot must be an object")?;

    let initial_head = b_obj
        .get("initial_head")
        .context("missing initial_head")?
        .as_str()
        .context("initial_head must be a string")?;
    ensure!(
        is_exact_sha40(initial_head),
        "Exact bound initial main head required"
    );

    let repo_val = b_obj.get("repository").context("missing repository")?;
    let repo_id_val = b_obj
        .get("repository_id")
        .context("missing repository_id")?;
    let branch_val = b_obj.get("branch").context("missing branch")?;
    let private_val = b_obj.get("private").context("missing private")?;

    let valid_target = if let Some(repo_name) = repo_val.as_str()
        && let Some(expected_id) = target_id(repo_name)
    {
        exact_int_eq(repo_id_val, expected_id)
            && branch_val.as_str() == Some("main")
            && private_val.as_bool() == Some(true)
    } else {
        false
    };
    ensure!(
        valid_target,
        "Destination is outside explicitly approved private main targets"
    );
    let repo_name = repo_val.as_str().unwrap();

    let remote_url = b_obj.get("remote_url").context("missing remote_url")?;
    let expected_remote = format!("git@github.com:{repo_name}.git");
    ensure!(
        remote_url.as_str() == Some(expected_remote.as_str()),
        "Exact canonical remote required"
    );

    let r = s_obj
        .get("repository")
        .context("missing snapshot repository")?
        .as_object()
        .context("snapshot repository must be an object")?;
    let r_id = r.get("id").context("missing snapshot repository.id")?;
    let r_full_name = r
        .get("full_name")
        .context("missing snapshot repository.full_name")?;
    let r_private = r
        .get("private")
        .context("missing snapshot repository.private")?;
    let r_default_branch = r
        .get("default_branch")
        .context("missing snapshot repository.default_branch")?;
    ensure!(
        exact_values_equal(r_id, repo_id_val)
            && r_full_name.as_str() == Some(repo_name)
            && exact_values_equal(r_private, private_val)
            && r_default_branch.as_str() == Some("main"),
        "Remote identity, visibility or default branch changed"
    );

    let head = s_obj
        .get("head")
        .context("missing snapshot head")?
        .as_str()
        .context("snapshot head must be a string")?;
    ensure!(is_exact_sha40(head), "Exact remote head missing");

    if let Some(prefix_val) = b_obj.get("preflight_branch_prefix")
        && !prefix_val.is_null()
    {
        let prefix = prefix_val
            .as_str()
            .context("preflight_branch_prefix must be a string")?;
        ensure!(
            is_valid_preflight_prefix(prefix),
            "Preflight branch must stay in the approved factory namespace"
        );
        let req = b_obj
            .get("required_checks")
            .and_then(Value::as_array)
            .filter(|arr| {
                !arr.is_empty()
                    && arr
                        .iter()
                        .all(|x| x.as_str().is_some_and(|s| !s.is_empty()))
            });
        ensure!(req.is_some(), "Preflight requires explicit CI check names");
    }

    Ok(())
}

fn validate_required_checks(required: Option<&Value>) -> Result<Vec<String>> {
    let arr = required
        .and_then(Value::as_array)
        .context("Explicit required_checks policy missing")?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let s = item
            .as_str()
            .filter(|s| !s.is_empty())
            .context("Explicit required_checks policy missing")?;
        out.push(s.to_string());
    }
    Ok(out)
}

pub fn ci_state(b: &Value, checks: &Value) -> Result<(String, Value)> {
    let b_obj = b.as_object().context("binding must be an object")?;
    let required = validate_required_checks(b_obj.get("required_checks"))?;
    let checks_arr = checks.as_array().context("checks must be an array")?;
    ensure!(checks_arr.len() <= 100, "CI inventory exceeds bound");

    let mut failed = false;
    for c in checks_arr {
        let c_obj = c.as_object().context("check row must be an object")?;
        if c_obj.get("status").and_then(Value::as_str) == Some("completed") {
            let conclusion = c_obj.get("conclusion").and_then(Value::as_str);
            if !matches!(conclusion, Some("success" | "neutral" | "skipped")) {
                failed = true;
                break;
            }
        }
    }
    if !failed {
        for c in checks_arr {
            let c_obj = c.as_object().context("check row must be an object")?;
            let is_req = c_obj
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|n| required.iter().any(|r| r == n));
            if is_req
                && c_obj.get("status").and_then(Value::as_str) == Some("completed")
                && c_obj.get("conclusion").and_then(Value::as_str) != Some("success")
            {
                failed = true;
                break;
            }
        }
    }

    let mut pending = false;
    for c in checks_arr {
        let c_obj = c.as_object().context("check row must be an object")?;
        if c_obj.get("status").and_then(Value::as_str) != Some("completed") {
            pending = true;
            break;
        }
    }
    if !pending {
        for req_name in &required {
            let mut matched_success = false;
            for c in checks_arr {
                let c_obj = c.as_object().context("check row must be an object")?;
                let name_val = c_obj.get("name").context("check missing name")?;
                if name_val.as_str() == Some(req_name.as_str())
                    && c_obj.get("conclusion").and_then(Value::as_str) == Some("success")
                {
                    matched_success = true;
                    break;
                }
            }
            if !matched_success {
                pending = true;
                break;
            }
        }
    }

    let state = if failed {
        "failed"
    } else if pending {
        "pending"
    } else if !checks_arr.is_empty() {
        "passed"
    } else {
        "not_configured"
    };

    Ok((
        state.to_string(),
        json!({
            "status": state,
            "checks": checks_arr,
        }),
    ))
}

pub fn checked_ci(b: &Value, s: &Value) -> Result<Value> {
    let b_obj = b.as_object().context("binding must be an object")?;
    let s_obj = s.as_object().context("snapshot must be an object")?;
    let checks_val = s_obj.get("checks").context("missing snapshot checks")?;
    let req_val = b_obj
        .get("required_checks")
        .context("missing required_checks")?;
    let required = validate_required_checks(Some(req_val))?;
    let checks_arr = checks_val.as_array().context("checks must be an array")?;
    ensure!(checks_arr.len() <= 100, "CI inventory exceeds bound");

    for c in checks_arr {
        let c_obj = c.as_object().context("check row must be an object")?;
        let status = c_obj.get("status").and_then(Value::as_str);
        let conclusion = c_obj.get("conclusion").and_then(Value::as_str);
        if status != Some("completed")
            || !matches!(conclusion, Some("success" | "neutral" | "skipped"))
        {
            bail!("Remote CI pending or failed");
        }
    }

    for name in &required {
        let mut matches = Vec::new();
        for c in checks_arr {
            let c_obj = c.as_object().context("check row must be an object")?;
            let c_name = c_obj.get("name").context("check missing name")?;
            if c_name.as_str() == Some(name.as_str()) {
                matches.push(c_obj);
            }
        }
        if matches.is_empty()
            || matches
                .iter()
                .any(|m| m.get("conclusion").and_then(Value::as_str) != Some("success"))
        {
            bail!("Required exact-head CI not passed: {name}");
        }
    }

    let status = if !checks_arr.is_empty() {
        "passed"
    } else {
        "not_configured"
    };
    Ok(json!({
        "status": status,
        "checks": checks_arr,
    }))
}

pub trait PublicationRemote {
    fn snapshot(&mut self, b: &Value, commit: Option<&str>) -> Result<Value>;
    fn push(&mut self, b: &Value, repo: &Path, commit: &str) -> Result<()>;
    fn preflight_head(&mut self, b: &Value, branch: &str) -> Result<Option<String>>;
    fn push_preflight(&mut self, b: &Value, repo: &Path, commit: &str, branch: &str) -> Result<()>;
}

type ApiReader = Box<dyn FnMut(&str) -> Result<Value>>;

pub struct GitHub {
    supervisor_executable: PathBuf,
    api_override: Option<ApiReader>,
}

impl GitHub {
    pub fn new(supervisor_executable: PathBuf) -> Self {
        Self {
            supervisor_executable,
            api_override: None,
        }
    }

    pub fn with_api<F>(supervisor_executable: PathBuf, f: F) -> Self
    where
        F: FnMut(&str) -> Result<Value> + 'static,
    {
        Self {
            supervisor_executable,
            api_override: Some(Box::new(f)),
        }
    }

    pub fn api(&mut self, path: &str) -> Result<Value> {
        if let Some(ref mut f) = self.api_override {
            return f(path);
        }
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp"));
        let out = supervised_owner_command(
            &self.supervisor_executable,
            Path::new("gh"),
            &["api", "--hostname", "github.com", path],
            &cwd,
            60,
            0,
        )?;
        serde_json::from_slice(&out.stdout).context("parsing gh api json")
    }
}

impl PublicationRemote for GitHub {
    fn snapshot(&mut self, b: &Value, commit: Option<&str>) -> Result<Value> {
        let repo_name = b
            .get("repository")
            .and_then(Value::as_str)
            .context("binding repository must be a string")?;
        let r = self.api(&format!("repos/{repo_name}"))?;
        let ref_resp = self.api(&format!("repos/{repo_name}/git/ref/heads/main"))?;
        let head = ref_resp
            .get("object")
            .and_then(|o| o.get("sha"))
            .and_then(Value::as_str)
            .context("missing main ref sha")?
            .to_string();
        let target = commit.unwrap_or(&head).to_string();

        let c = self.api(&format!(
            "repos/{repo_name}/commits/{target}/check-runs?per_page=100&filter=all"
        ))?;
        let suites_resp = self.api(&format!(
            "repos/{repo_name}/commits/{target}/check-suites?per_page=100"
        ))?;
        let statuses = self.api(&format!(
            "repos/{repo_name}/commits/{target}/status?per_page=100"
        ))?;

        let c_total = json_int(c.get("total_count").context("check_runs total_count")?)
            .context("invalid check_runs total_count")?;
        let suites_total = json_int(
            suites_resp
                .get("total_count")
                .context("check_suites total_count")?,
        )
        .context("invalid check_suites total_count")?;
        let statuses_total = json_int(statuses.get("total_count").context("status total_count")?)
            .context("invalid status total_count")?;
        ensure!(
            c_total <= 100 && suites_total <= 100 && statuses_total <= 100,
            "CI inventory incomplete"
        );

        let check_runs = c
            .get("check_runs")
            .and_then(Value::as_array)
            .context("missing check_runs array")?;

        let mut latest_checks: Vec<(String, i64, &Value)> = Vec::new();
        for row in check_runs {
            let row_obj = row.as_object().context("check_run must be an object")?;
            if row_obj.get("head_sha").and_then(Value::as_str) != Some(target.as_str()) {
                continue;
            }
            let Some(id) = row_obj.get("id").and_then(json_int) else {
                bail!("CI run identity missing");
            };
            let name = row_obj
                .get("name")
                .and_then(Value::as_str)
                .context("check_run name missing")?
                .to_string();
            if let Some(pos) = latest_checks.iter().position(|(n, _, _)| *n == name) {
                if id > latest_checks[pos].1 {
                    latest_checks[pos] = (name, id, row);
                }
            } else {
                latest_checks.push((name, id, row));
            }
        }

        let mut checks = Vec::new();
        for (_, _, x) in latest_checks {
            checks.push(json!({
                "name": x["name"],
                "status": x["status"],
                "conclusion": x.get("conclusion").cloned().unwrap_or(Value::Null),
            }));
        }

        let status_rows = statuses
            .get("statuses")
            .and_then(Value::as_array)
            .context("missing statuses array")?;
        for x in status_rows {
            let state = x
                .get("state")
                .and_then(Value::as_str)
                .context("status state missing")?;
            let context = x.get("context").context("status context missing")?;
            checks.push(json!({
                "name": context,
                "status": if state != "pending" { "completed" } else { "pending" },
                "conclusion": if state == "success" { "success" } else { "failure" },
            }));
        }

        let mut suite_by_id: HashMap<i64, &Value> = HashMap::new();
        if let Some(suites) = suites_resp.get("check_suites").and_then(Value::as_array) {
            for s in suites {
                if let Some(id) = s.get("id").and_then(json_int) {
                    suite_by_id.insert(id, s);
                }
            }
        }

        let branch = b
            .get("branch")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or("main");

        let mut latest_main: Vec<(String, i64, &Value)> = Vec::new();
        for run in check_runs {
            let Some(suite_id) = run
                .get("check_suite")
                .and_then(Value::as_object)
                .and_then(|cs| cs.get("id"))
                .and_then(json_int)
            else {
                continue;
            };
            let Some(suite) = suite_by_id.get(&suite_id).and_then(|s| s.as_object()) else {
                continue;
            };
            if suite.get("head_branch").and_then(Value::as_str) != Some(branch) {
                continue;
            }
            if suite.get("head_sha").and_then(Value::as_str) != Some(target.as_str()) {
                continue;
            }
            if run.get("head_sha").and_then(Value::as_str) != Some(target.as_str()) {
                continue;
            }
            let name = run
                .get("name")
                .and_then(Value::as_str)
                .context("check_run name missing")?
                .to_string();
            let key = run
                .get("id")
                .and_then(json_int)
                .context("CI run identity missing")?;
            if let Some(pos) = latest_main.iter().position(|(n, _, _)| *n == name) {
                if key > latest_main[pos].1 {
                    latest_main[pos] = (name, key, run);
                }
            } else {
                latest_main.push((name, key, run));
            }
        }

        let main_checks: Vec<Value> = latest_main
            .into_iter()
            .map(|(_, _, run)| {
                json!({
                    "name": run["name"],
                    "status": run["status"],
                    "conclusion": run.get("conclusion").cloned().unwrap_or(Value::Null),
                })
            })
            .collect();

        Ok(json!({
            "repository": r,
            "head": head,
            "checks": checks,
            "main_checks": main_checks,
            "checked_commit": target,
        }))
    }

    fn push(&mut self, b: &Value, repo: &Path, commit: &str) -> Result<()> {
        let remote_url = b
            .get("remote_url")
            .and_then(Value::as_str)
            .context("remote_url missing")?;
        let repo_str = repo.to_str().context("repo path must be UTF-8")?;
        let refspec = format!("{commit}:refs/heads/main");
        supervised_owner_command(
            &self.supervisor_executable,
            Path::new("/usr/bin/git"),
            &[
                "-c",
                "core.hooksPath=/dev/null",
                "-C",
                repo_str,
                "push",
                remote_url,
                &refspec,
            ],
            repo,
            60,
            0,
        )?;
        Ok(())
    }

    fn preflight_head(&mut self, b: &Value, branch: &str) -> Result<Option<String>> {
        let remote_url = b
            .get("remote_url")
            .and_then(Value::as_str)
            .context("remote_url missing")?;
        let ref_name = format!("refs/heads/{branch}");
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp"));
        let out = supervised_owner_command(
            &self.supervisor_executable,
            Path::new("/usr/bin/git"),
            &["ls-remote", "--heads", remote_url, &ref_name],
            &cwd,
            60,
            0,
        )?;
        let text = String::from_utf8(out.stdout).context("ls-remote output not UTF-8")?;
        let lines: Vec<&str> = text.lines().collect();
        if lines.is_empty() {
            return Ok(None);
        }
        let parts: Vec<&str> = lines[0].split_whitespace().collect();
        ensure!(
            lines.len() == 1
                && parts.len() == 2
                && parts[1] == ref_name
                && is_exact_sha40(parts[0]),
            "Unexpected preflight ref observation"
        );
        Ok(Some(parts[0].to_string()))
    }

    fn push_preflight(&mut self, b: &Value, repo: &Path, commit: &str, branch: &str) -> Result<()> {
        let remote_url = b
            .get("remote_url")
            .and_then(Value::as_str)
            .context("remote_url missing")?;
        let repo_str = repo.to_str().context("repo path must be UTF-8")?;
        let refspec = format!("{commit}:refs/heads/{branch}");
        supervised_owner_command(
            &self.supervisor_executable,
            Path::new("/usr/bin/git"),
            &[
                "-c",
                "core.hooksPath=/dev/null",
                "-C",
                repo_str,
                "push",
                remote_url,
                &refspec,
            ],
            repo,
            60,
            0,
        )?;
        Ok(())
    }
}

pub(crate) fn create_private_diff_dir() -> Result<PathBuf> {
    let base = std::env::temp_dir();
    let execution_id = uuid::Uuid::new_v4().simple().to_string();
    let dir = base.join(format!(
        "code-factory-publication-diff-{}-{execution_id}",
        std::process::id()
    ));
    fs::DirBuilder::new().mode(0o700).create(&dir)?;
    let meta = fs::symlink_metadata(&dir)?;
    let uid = unsafe { libc::getuid() };
    ensure!(
        meta.is_dir()
            && !meta.file_type().is_symlink()
            && meta.uid() == uid
            && (meta.permissions().mode() & 0o077) == 0,
        "unsafe publication diff directory: {}",
        dir.display()
    );
    resolve(&dir)
}

pub(crate) fn read_bounded_nofollow(path: &Path, max_bytes: usize) -> Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    let uid = unsafe { libc::getuid() };
    ensure!(
        meta.is_file() && meta.uid() == uid && (meta.permissions().mode() & 0o077) == 0,
        "unsafe diff output file: {}",
        path.display()
    );
    let mut bytes = Vec::new();
    file.take((max_bytes + 1) as u64).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn split_lines_bytes(bytes: &[u8]) -> Vec<&[u8]> {
    let mut lines = Vec::new();
    let mut start = 0;
    let len = bytes.len();
    while start < len {
        let mut end = start;
        while end < len && bytes[end] != b'\n' && bytes[end] != b'\r' {
            end += 1;
        }
        lines.push(&bytes[start..end]);
        if end < len {
            if bytes[end] == b'\r' && end + 1 < len && bytes[end + 1] == b'\n' {
                start = end + 2;
            } else {
                start = end + 1;
            }
        } else {
            break;
        }
    }
    lines
}

fn save_preserved_state(path: &Path, value: &Value) -> Result<()> {
    let raw = RawJson::from_value(value);
    save_raw(path, &raw)
}

fn read_preserved_state(path: &Path) -> Result<Value> {
    Ok(RawJson::read(path)?.to_value())
}

pub struct Publisher<R: PublicationRemote = GitHub> {
    pub root: PathBuf,
    pub remote: R,
    pub supervisor_executable: PathBuf,
}

impl Publisher<GitHub> {
    pub fn open(state: &Path) -> Result<Self> {
        let exe = resolve(&std::env::current_exe().context("resolving current executable")?)?;
        Self::open_with_remote(state, GitHub::new(exe.clone()), exe)
    }
}

impl<R: PublicationRemote> Publisher<R> {
    pub fn open_with_remote(
        state: &Path,
        remote: R,
        supervisor_executable: PathBuf,
    ) -> Result<Self> {
        if !state.exists() {
            if let Some(parent) = state.parent()
                && !parent.as_os_str().is_empty()
            {
                fs::create_dir_all(parent)?;
            }
            fs::DirBuilder::new().mode(0o700).create(state)?;
        }
        let root = resolve(state)?;
        let meta = fs::symlink_metadata(&root)?;
        let uid = unsafe { libc::getuid() };
        ensure!(
            meta.is_dir()
                && !meta.file_type().is_symlink()
                && meta.uid() == uid
                && (meta.permissions().mode() & 0o077) == 0,
            "Publication state must be owner-only"
        );
        Ok(Self {
            root,
            remote,
            supervisor_executable: resolve(&supervisor_executable)?,
        })
    }

    pub fn lock(&self) -> Result<Lock> {
        Lock::acquire(&self.root.join("owner.lock"), false)
    }

    pub fn save(&self, s: &Value) -> Result<()> {
        save_preserved_state(&self.root.join("state.json"), s)
    }

    pub fn await_ci(&self, commit: &str, receipt: &Value, ci: &Value) -> Value {
        json!({
            "state": "awaiting_ci",
            "commit": commit,
            "receipt": receipt,
            "ci": ci,
        })
    }

    fn git(&self, repo: &Path, args: &[&str]) -> Result<String> {
        let repo_str = repo.to_str().context("repo path must be UTF-8")?;
        let mut full_args = Vec::with_capacity(4 + args.len());
        full_args.extend_from_slice(&["-c", "core.hooksPath=/dev/null", "-C", repo_str]);
        full_args.extend_from_slice(args);
        let out = supervised_command(
            &self.supervisor_executable,
            Path::new("/usr/bin/git"),
            &full_args,
            repo,
            60,
            0,
        )?;
        Ok(String::from_utf8(out.stdout)?.trim().to_string())
    }

    pub fn verified(&self, b: &Value, goal: &Value) -> Result<(PathBuf, String, Value)> {
        let goal_root_str = goal
            .get("root")
            .and_then(Value::as_str)
            .context("goal.root missing")?;
        let goal_root_resolved = resolve(Path::new(goal_root_str))?;
        let bound_goal_root = b
            .get("goal_root")
            .and_then(Value::as_str)
            .context("binding.goal_root missing")?;
        ensure!(
            goal.get("id") == b.get("goal_id")
                && goal_root_resolved.to_string_lossy() == bound_goal_root,
            "Exact approved goal binding required"
        );

        let root = PathBuf::from(bound_goal_root);
        let status = read_json(&root.join("status.json"))?;
        if let Some(curr) = status.get("current")
            && !curr.is_null()
            && curr != &json!(false)
            && curr.as_object().is_none_or(|m| !m.is_empty())
        {
            bail!("Cannot publish active execution");
        }

        let result_path_str = status
            .get("last_cycle")
            .and_then(|lc| lc.get("result"))
            .and_then(Value::as_str)
            .context("status.last_cycle.result missing")?;
        let path = PathBuf::from(result_path_str);
        let mut out = read_json(&path)?;
        if let Some(obj) = out.as_object_mut() {
            obj.insert(
                "_result_path".to_string(),
                Value::String(path.to_string_lossy().into_owned()),
            );
        }
        ensure!(
            out.get("status").and_then(Value::as_str) == Some("accepted"),
            "Only accepted delivery may publish"
        );

        let cfg = read_json(&root.join("supervisor.json"))?;
        let controller_path_str = cfg
            .get("controller")
            .and_then(Value::as_str)
            .context("supervisor.json controller missing")?;
        let controller_sha = cfg
            .get("controller_sha256")
            .and_then(Value::as_str)
            .context("supervisor.json controller_sha256 missing")?;
        ensure!(
            sha(Path::new(controller_path_str))? == controller_sha,
            "Controller bytes changed"
        );

        let milestone = audit_cycle(
            &self.supervisor_executable,
            Path::new(controller_path_str),
            goal,
            &mut out,
        )?
        .context("Independent verification missing")?;

        let repo_str = goal
            .get("repository")
            .and_then(Value::as_str)
            .context("goal.repository missing")?;
        let repo = PathBuf::from(repo_str);
        let commit = milestone
            .get("commit")
            .and_then(Value::as_str)
            .context("milestone commit missing")?
            .to_string();
        ensure!(is_exact_sha40(&commit), "Exact verified commit required");
        ensure!(
            resolve(&repo)? == resolve(&root.join("repository"))?,
            "Exact goal repository required"
        );

        let initial_head = b
            .get("initial_head")
            .and_then(Value::as_str)
            .context("binding initial_head missing")?;
        self.git(
            &repo,
            &["merge-base", "--is-ancestor", initial_head, &commit],
        )?;

        let mut receipt = milestone;
        if let Some(obj) = receipt.as_object_mut() {
            obj.insert("result_sha256".to_string(), Value::String(sha(&path)?));
        }
        Ok((repo, commit, receipt))
    }

    pub fn content(&self, b: &Value, repo: &Path, previous: &str, commit: &str) -> Result<String> {
        let diff_names = self.git(repo, &["diff", "--name-only", previous, commit])?;
        let paths: Vec<&str> = diff_names.lines().filter(|l| !l.is_empty()).collect();
        ensure!(!paths.is_empty(), "No product changes to publish");

        let allowed_paths = b
            .get("allowed_paths")
            .and_then(Value::as_array)
            .filter(|a| !a.is_empty());
        let forbidden_patterns = b
            .get("forbidden_patterns")
            .and_then(Value::as_array)
            .filter(|a| !a.is_empty());
        let (Some(allowed_paths), Some(forbidden_patterns)) = (allowed_paths, forbidden_patterns)
        else {
            bail!("Explicit publication scope and content policy required");
        };
        let protected_paths = b
            .get("protected_paths")
            .and_then(Value::as_array)
            .context("protected_paths must be an array")?;

        let allowed: Vec<&str> = allowed_paths
            .iter()
            .map(|v| v.as_str().context("allowed_paths entry must be string"))
            .collect::<Result<_>>()?;
        let protected: Vec<&str> = protected_paths
            .iter()
            .map(|v| v.as_str().context("protected_paths entry must be string"))
            .collect::<Result<_>>()?;

        for p in &paths {
            let in_allowed = allowed
                .iter()
                .any(|x| *p == *x || p.starts_with(&format!("{x}/")));
            let in_protected = protected
                .iter()
                .any(|x| *p == *x || p.starts_with(&format!("{x}/")));
            if !in_allowed || in_protected {
                bail!("Publication scope violation: {p}");
            }
            let path_obj = Path::new(p);
            let bad_part = path_obj.components().any(|comp| match comp {
                Component::Normal(os) => {
                    let s = os.to_string_lossy();
                    matches!(
                        s.as_ref(),
                        ".git" | ".opencode" | ".aws" | ".ssh" | "node_modules" | "cycles"
                    )
                }
                _ => false,
            });
            let file_name = path_obj
                .file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default();
            let bad_suffix = [".sqlite", ".db", ".pem", ".key"]
                .iter()
                .any(|ext| p.ends_with(ext));
            if bad_part || file_name.starts_with(".env") || bad_suffix {
                bail!("Private runtime/credential artifact refused");
            }
        }

        let diff_dir = create_private_diff_dir()?;
        let diff_file = diff_dir.join("patch.diff");
        {
            let _f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&diff_file)?;
        }
        let output_arg = format!("--output={}", diff_file.display());
        let repo_str = repo.to_str().context("repo path must be UTF-8")?;
        let mut args = Vec::with_capacity(7 + paths.len());
        args.extend_from_slice(&[
            "-c",
            "core.hooksPath=/dev/null",
            "-C",
            repo_str,
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            &output_arg,
            previous,
            commit,
            "--",
        ]);
        args.extend_from_slice(&paths);

        let cmd_res = supervised_command(
            &self.supervisor_executable,
            Path::new("/usr/bin/git"),
            &args,
            repo,
            60,
            0,
        );
        if let Err(e) = cmd_res {
            return Err(e.context(format!(
                "publication diff command failed (artifact retained at {})",
                diff_dir.display()
            )));
        }

        let patch = match read_bounded_nofollow(&diff_file, MAX_PATCH_BYTES) {
            Ok(bytes) => {
                let _ = fs::remove_dir_all(&diff_dir);
                bytes
            }
            Err(e) => {
                return Err(e.context(format!(
                    "reading publication diff output failed (artifact retained at {})",
                    diff_dir.display()
                )));
            }
        };

        let binary_headers: [&[u8]; 2] = [b"Binary files ", b"GIT binary patch"];
        if patch.len() > MAX_PATCH_BYTES
            || split_lines_bytes(&patch)
                .into_iter()
                .any(|line| binary_headers.iter().any(|h| line.starts_with(h)))
        {
            bail!("Publication diff exceeds supported text scope");
        }

        let text =
            std::str::from_utf8(&patch).context("Publication diff is not valid UTF-8 text")?;

        let mut patterns = vec![
            r"-----BEGIN (?:RSA |OPENSSH |EC )?PRIVATE KEY-----".to_string(),
            r"gh[pousr]_[A-Za-z0-9]{30,}".to_string(),
        ];
        for fp in forbidden_patterns {
            let s = fp
                .as_str()
                .context("forbidden_patterns entry must be a string")?;
            patterns.push(s.to_string());
        }

        for pattern in &patterns {
            let re: Regex = RegexBuilder::new(pattern)
                .case_insensitive(true)
                .build()
                .with_context(|| format!("invalid forbidden pattern regex: {pattern}"))?;
            if re.is_match(text) {
                bail!("Publication content policy rejected patch");
            }
        }

        Ok(format!("{:x}", Sha256::digest(&patch)))
    }

    pub fn preflight(
        &mut self,
        b: &Value,
        _goal: &Value,
        repo: &Path,
        commit: &str,
        receipt: &Value,
    ) -> Result<Value> {
        let observed = self.remote.snapshot(b, None)?;
        identity(b, &observed)?;
        let observed_head = observed
            .get("head")
            .and_then(Value::as_str)
            .context("observed head missing")?;
        self.git(
            repo,
            &["merge-base", "--is-ancestor", observed_head, commit],
        )?;
        let diff = self.content(b, repo, observed_head, commit)?;

        let path = self.root.join("preflight.json");
        let old = if path.exists() {
            Some(read_preserved_state(&path)?)
        } else {
            None
        };
        let prefix = b
            .get("preflight_branch_prefix")
            .and_then(Value::as_str)
            .context("preflight_branch_prefix missing")?;
        let branch = format!("{prefix}-{commit}");

        if let Some(ref old_val) = old {
            ensure!(
                old_val.get("binding") == Some(b),
                "Immutable preflight binding changed"
            );
            let old_commit = old_val.get("commit").and_then(Value::as_str);
            let old_state = old_val.get("state").and_then(Value::as_str);
            if old_commit != Some(commit) && !matches!(old_state, Some("ready" | "ci_failed")) {
                bail!("Unresolved preflight owns this goal");
            }
        }

        let current = self.remote.preflight_head(b, &branch)?;
        if let Some(ref curr) = current
            && curr != commit
        {
            bail!("Preflight branch changed; no overwrite");
        }

        let mut record = if let Some(ref old_val) = old
            && old_val.get("commit").and_then(Value::as_str) == Some(commit)
        {
            old_val.clone()
        } else {
            json!({
                "binding": b,
                "commit": commit,
                "branch": branch,
                "receipt": receipt,
                "diff_sha256": diff,
                "state": "push_intent",
            })
        };

        ensure!(
            record.get("receipt") == Some(receipt)
                && record.get("diff_sha256").and_then(Value::as_str) == Some(diff.as_str()),
            "Preflight receipt/content changed"
        );

        if current.is_none() {
            if let Some(ref old_val) = old
                && old_val.get("commit").and_then(Value::as_str) == Some(commit)
            {
                bail!("Uncertain preflight not observed; no blind retry");
            }
            save_preserved_state(&path, &record)?;
            if let Err(e) = self.remote.push_preflight(b, repo, commit, &branch) {
                if let Some(obj) = record.as_object_mut() {
                    obj.insert("state".into(), json!("uncertain"));
                }
                save_preserved_state(&path, &record)?;
                return Err(e);
            }
            let confirmed = self.remote.preflight_head(b, &branch)?;
            if confirmed.as_deref() != Some(commit) {
                if let Some(obj) = record.as_object_mut() {
                    obj.insert("state".into(), json!("uncertain"));
                }
                save_preserved_state(&path, &record)?;
                bail!("Preflight push not confirmed");
            }
        } else if old
            .as_ref()
            .is_none_or(|o| o.get("commit").and_then(Value::as_str) != Some(commit))
        {
            save_preserved_state(&path, &record)?;
        }

        let candidate = self.remote.snapshot(b, Some(commit))?;
        identity(b, &candidate)?;
        ensure!(
            candidate.get("checked_commit").and_then(Value::as_str) == Some(commit),
            "Preflight CI belongs to another commit"
        );
        let checks = candidate
            .get("checks")
            .context("candidate checks missing")?;
        let (state, _) = ci_state(b, checks)?;
        if state == "failed" {
            if let Some(obj) = record.as_object_mut() {
                obj.insert("state".into(), json!("ci_failed"));
            }
            save_preserved_state(&path, &record)?;
            bail!("Preflight CI failed");
        }
        if let Some(obj) = record.as_object_mut() {
            obj.insert(
                "state".into(),
                json!(if state == "pending" {
                    "awaiting_ci"
                } else {
                    "ready"
                }),
            );
            obj.insert(
                "ci".into(),
                json!({
                    "commit": commit,
                    "checks": checks,
                }),
            );
        }
        save_preserved_state(&path, &record)?;
        Ok(record)
    }

    pub fn publish(&mut self, b: &Value, goal: &Value) -> Result<Value> {
        let (repo, commit, receipt) = self.verified(b, goal)?;
        if b.get("require_goal_completion")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let goal_root = Path::new(
                b.get("goal_root")
                    .and_then(Value::as_str)
                    .context("binding goal_root missing")?,
            );
            let evaluation = read_json(&goal_root.join("goal-evaluation.json"))?;
            let result = evaluation.get("result").cloned().unwrap_or(json!({}));
            let receipts_nonempty = result
                .get("receipts")
                .and_then(Value::as_array)
                .is_some_and(|r| !r.is_empty());
            ensure!(
                evaluation.get("goal_id") == goal.get("id")
                    && evaluation.get("commit").and_then(Value::as_str) == Some(commit.as_str())
                    && result.get("commit").and_then(Value::as_str) == Some(commit.as_str())
                    && result.get("passed").and_then(Value::as_bool) == Some(true)
                    && result.get("status").and_then(Value::as_str) == Some("passed")
                    && receipts_nonempty,
                "Independent whole-goal completion missing"
            );
            let sup_cfg = read_json(&goal_root.join("supervisor.json"))?;
            let mut manifest = goal
                .get("acceptance")
                .and_then(Value::as_object)
                .cloned()
                .context("goal.acceptance must be an object")?;
            manifest.insert(
                "repository".into(),
                goal.get("repository").cloned().unwrap_or(Value::Null),
            );
            manifest.insert(
                "verifier_executable".into(),
                sup_cfg.get("controller").cloned().unwrap_or(Value::Null),
            );
            audit_result(&Value::Object(manifest), &result, &commit)?;
        }

        let state_path = self.root.join("state.json");
        let existing = if state_path.exists() {
            Some(read_preserved_state(&state_path)?)
        } else {
            None
        };
        if let Some(ref ex) = existing {
            ensure!(
                ex.get("binding") == Some(b),
                "Immutable publication binding changed"
            );
            let ex_intent = ex.get("intent").and_then(Value::as_object);
            let ex_state = ex_intent
                .and_then(|i| i.get("state"))
                .and_then(Value::as_str);
            let ex_commit = ex_intent
                .and_then(|i| i.get("commit"))
                .and_then(Value::as_str);
            if ex_state != Some("confirmed") && ex_commit != Some(commit.as_str()) {
                bail!("Unresolved main intent owns this goal");
            }
        }

        let has_preflight = b
            .get("preflight_branch_prefix")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
            || b.get("preflight_branch_prefix")
                .is_some_and(|v| !v.is_null() && v != &json!(false));
        let existing_commit_matches = existing
            .as_ref()
            .and_then(|e| e.get("intent"))
            .and_then(|i| i.get("commit"))
            .and_then(Value::as_str)
            == Some(commit.as_str());

        if has_preflight && !existing_commit_matches {
            let preflight = self.preflight(b, goal, &repo, &commit, &receipt)?;
            if preflight.get("state").and_then(Value::as_str) != Some("ready") {
                return Ok(json!({
                    "state": "awaiting_ci",
                    "commit": commit,
                    "branch": preflight["branch"],
                    "receipt": receipt,
                    "ci": preflight["ci"],
                }));
            }
        }

        let observed = self.remote.snapshot(b, Some(&commit))?;
        identity(b, &observed)?;
        ensure!(
            observed.get("checked_commit").and_then(Value::as_str) == Some(commit.as_str()),
            "CI observation belongs to another commit"
        );

        let old = if state_path.exists() {
            Some(read_preserved_state(&state_path)?)
        } else {
            None
        };
        if let Some(ref old_val) = old {
            ensure!(
                old_val.get("binding") == Some(b),
                "Immutable publication binding changed"
            );
        }
        let intent_opt = old.as_ref().and_then(|o| o.get("intent")).cloned();
        let observed_head = observed
            .get("head")
            .and_then(Value::as_str)
            .context("observed head missing")?
            .to_string();

        let previous = if let Some(ref intent) = intent_opt
            && !intent.is_null()
            && intent.get("state").and_then(Value::as_str) != Some("confirmed")
        {
            ensure!(
                intent.get("commit").and_then(Value::as_str) == Some(commit.as_str())
                    && intent.get("receipt") == Some(&receipt),
                "Unresolved publication owns this goal; preserve prior receipt"
            );
            let prev = intent
                .get("previous_head")
                .and_then(Value::as_str)
                .context("intent previous_head missing")?
                .to_string();
            ensure!(
                observed_head == commit,
                "Uncertain push not confirmed; no blind retry"
            );
            prev
        } else {
            if let Some(ref intent) = intent_opt
                && !intent.is_null()
                && intent.get("commit").and_then(Value::as_str) == Some(commit.as_str())
            {
                ensure!(
                    observed_head == commit,
                    "Published main changed; reconcile before new effects"
                );
                let post_checks = if has_preflight {
                    observed
                        .get("main_checks")
                        .cloned()
                        .unwrap_or_else(|| json!([]))
                } else {
                    observed.get("checks").cloned().unwrap_or_else(|| json!([]))
                };
                let (state, ci) = ci_state(b, &post_checks)?;
                ensure!(state != "failed", "Remote CI failed");
                if state == "pending" {
                    return Ok(self.await_ci(&commit, &receipt, &ci));
                }
                let mut out = intent.clone();
                if let Some(obj) = out.as_object_mut() {
                    obj.insert("ci".into(), ci);
                }
                return Ok(out);
            }
            self.git(
                &repo,
                &["merge-base", "--is-ancestor", &observed_head, &commit],
            )?;
            observed_head.clone()
        };

        let diff = self.content(b, &repo, &previous, &commit)?;

        let (mut s, mut intent) = if observed_head != commit {
            let ci = checked_ci(b, &observed)?;
            let new_intent = json!({
                "state": "push_intent",
                "commit": commit,
                "previous_head": previous,
                "receipt": receipt,
                "diff_sha256": diff,
                "ci_before": ci,
            });
            let history = old
                .as_ref()
                .and_then(|o| o.get("history"))
                .cloned()
                .unwrap_or_else(|| json!([]));
            let mut new_state = json!({
                "binding": b,
                "history": history,
                "intent": new_intent.clone(),
            });
            self.save(&new_state)?;
            if let Err(e) = self.remote.push(b, &repo, &commit) {
                if let Some(obj) = new_state.get_mut("intent").and_then(Value::as_object_mut) {
                    obj.insert("state".into(), json!("uncertain"));
                }
                self.save(&new_state)?;
                return Err(e);
            }
            (new_state, new_intent)
        } else {
            let Some(old_state) = old else {
                bail!("Remote commit lacks retained publication intent");
            };
            let Some(old_intent) = intent_opt.filter(|i| !i.is_null()) else {
                bail!("Remote commit lacks retained publication intent");
            };
            (old_state, old_intent)
        };

        let after = self.remote.snapshot(b, Some(&commit))?;
        identity(b, &after)?;
        ensure!(
            after.get("head").and_then(Value::as_str) == Some(commit.as_str())
                && after.get("checked_commit").and_then(Value::as_str) == Some(commit.as_str()),
            "Main update not observed; intent retained"
        );
        let after_checks = if has_preflight {
            after
                .get("main_checks")
                .cloned()
                .unwrap_or_else(|| json!([]))
        } else {
            after.get("checks").cloned().unwrap_or_else(|| json!([]))
        };
        if let Some(obj) = intent.as_object_mut() {
            obj.insert("state".into(), json!("remote_observed"));
            obj.insert(
                "ci_observation".into(),
                json!({
                    "commit": commit,
                    "checks": after_checks,
                }),
            );
        }
        if let Some(s_obj) = s.as_object_mut() {
            s_obj.insert("intent".into(), intent.clone());
        }
        self.save(&s)?;

        let (state, ci) = ci_state(b, &after_checks)?;
        ensure!(state != "failed", "Remote CI failed");
        if state == "pending" {
            return Ok(self.await_ci(&commit, &receipt, &ci));
        }

        if let Some(obj) = intent.as_object_mut() {
            obj.insert("state".into(), json!("confirmed"));
            obj.insert("ci".into(), ci);
        }
        if let Some(s_obj) = s.as_object_mut() {
            s_obj.insert("intent".into(), intent.clone());
            if let Some(hist) = s_obj.get_mut("history").and_then(Value::as_array_mut) {
                hist.push(intent.clone());
            }
        }
        self.save(&s)?;
        Ok(intent)
    }
}

pub fn run_cli(state: &Path, binding: &Path, goal: &Path) -> Result<Value> {
    let mut publisher = Publisher::open(state)?;
    let _lock = publisher.lock()?;
    let b = read_preserved_state(binding)?;
    let g = read_preserved_state(goal)?;
    publisher.publish(&b, &g)
}
