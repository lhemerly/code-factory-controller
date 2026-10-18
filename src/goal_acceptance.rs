//! Whole-goal acceptance and read-only audit of Python version-1 receipts.
//! New executions always use the existing Linux ownership supervisor.
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("missing string: {key}"))
}
fn array<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>> {
    value
        .get(key)
        .and_then(Value::as_array)
        .with_context(|| format!("missing array: {key}"))
}
pub fn sha(path: &Path) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(fs::read(path)?)))
}
fn read(path: impl AsRef<Path>) -> Result<Value> {
    crate::supervisor::read_json(path.as_ref())
}
// Python's legacy policy distinguishes an absent/empty suite from a malformed
// nonempty suite. Preserve that distinction in persisted failure statuses.
fn json_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64() != Some(0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}
fn exact_commit(commit: &str) -> bool {
    matches!(commit.len(), 40 | 64)
        && commit
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn validate_evidence(e: &Value, check: &Value, commit: &str) -> Result<()> {
    ensure!(e.is_object(), "acceptance evidence must be a JSON object");
    ensure!(
        e["passed"] == true
            && e["commit"] == commit
            && e["evidence_type"] == check["evidence_type"],
        "acceptance must pass without skips for the exact commit and evidence type"
    );
    ensure!(
        (e.get("skipped").is_some() || e.get("skipped_count").is_some())
            && e.get("skipped").is_none_or(|v| v == false)
            && e.get("skipped_count").is_none_or(|v| v.as_u64() == Some(0)),
        "acceptance must explicitly report no skipped checks"
    );
    if check["evidence_type"] != "vulkan_execution" {
        ensure!(
            e.get("executed").is_some() && e.get("failed").is_some(),
            "acceptance requires measured executed and failed counts"
        );
    }
    for (key, positive) in [
        ("executed", true),
        ("failed", false),
        ("skipped_count", false),
    ] {
        if let Some(value) = e.get(key) {
            ensure!(
                value
                    .as_u64()
                    .is_some_and(|n| if positive { n > 0 } else { n == 0 }),
                "acceptance observations must execute tests without failures or skips"
            );
        }
    }
    if check["evidence_type"] == "vulkan_execution" {
        let device = &e["device"];
        ensure!(
            e["gpu_dispatches"].as_u64().is_some_and(|n| n > 0)
                && e["validation_passed"] == true
                && device.is_object()
                && json_truthy(&device["name"])
                && device.get("vendor_id").is_some()
                && device.get("device_id").is_some(),
            "Vulkan acceptance requires validated device dispatches and device identity"
        );
    }
    Ok(())
}
pub fn validate_policy(m: &Value) -> Result<()> {
    let checks = array(m, "completion_checks")?;
    let required = array(m, "required_evidence")?;
    ensure!(
        !checks.is_empty() && !required.is_empty(),
        "mandatory completion checks/evidence are missing or invalid"
    );
    let mut ids = BTreeSet::new();
    let mut kinds = BTreeSet::new();
    for c in checks {
        let id = string(c, "id")?;
        let kind = string(c, "evidence_type")?;
        ensure!(
            !id.is_empty() && !kind.is_empty(),
            "invalid completion identity"
        );
        ensure!(ids.insert(id), "completion check IDs must be unique");
        kinds.insert(kind);
        let argv = array(c, "argv")?;
        ensure!(
            !argv.is_empty()
                && argv.iter().all(|v| v
                    .as_str()
                    .is_some_and(|s| !s.is_empty() && !s.contains('\0'))),
            "invalid completion command/timeout"
        );
        let seconds = c
            .get("timeout_seconds")
            .unwrap_or(&json!(300))
            .as_f64()
            .context("invalid completion command/timeout")?;
        ensure!(
            seconds.is_finite() && (1.0..=3600.0).contains(&seconds),
            "invalid completion command/timeout"
        );
    }
    ensure!(
        required.iter().all(|v| v
            .as_str()
            .is_some_and(|s| !s.is_empty() && kinds.contains(s))),
        "mandatory completion checks/evidence are missing or invalid"
    );
    ensure!(
        Path::new(string(m, "repository")?).is_absolute(),
        "absolute repository required"
    );
    if let Some(extra) = m.get("verification_environment") {
        let extra = extra
            .as_object()
            .context("unsupported verification environment key")?;
        for (key, value) in extra {
            ensure!(
                [
                    "PATH",
                    "RUSTUP_HOME",
                    "CARGO_HOME",
                    "RUSTUP_TOOLCHAIN",
                    "VK_ICD_FILENAMES",
                    "VK_DRIVER_FILES",
                    "XDG_RUNTIME_DIR"
                ]
                .contains(&key.as_str()),
                "unsupported verification environment key"
            );
            ensure!(
                value.as_str().is_some_and(|s| !s.contains('\0')),
                "verification environment values must be strings without null bytes"
            );
        }
    }
    Ok(())
}
fn environment(m: &Value, commit: &str) -> Result<BTreeMap<String, String>> {
    let mut env = BTreeMap::from([
        (
            "PATH".into(),
            std::env::var("PATH").unwrap_or("/usr/bin:/bin".into()),
        ),
        ("CI".into(), "true".into()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
    ]);
    if let Some(extra) = m.get("verification_environment") {
        for (k, v) in extra.as_object().context("invalid environment")? {
            env.insert(
                k.clone(),
                v.as_str().context("invalid environment value")?.into(),
            );
        }
    }
    env.insert("FACTORY_VERIFICATION_COMMIT".into(), commit.into());
    Ok(env)
}
fn command_argv(argv: &Value, env: &BTreeMap<String, String>) -> Result<Vec<String>> {
    let mut result = vec!["/usr/bin/env".into()];
    result.extend(
        env.iter()
            .filter(|(k, _)| !["PATH", "FACTORY_VERIFICATION_COMMIT"].contains(&k.as_str()))
            .map(|(k, v)| format!("{k}={v}")),
    );
    for value in argv.as_array().context("invalid command")? {
        result.push(value.as_str().context("invalid command argument")?.into());
    }
    Ok(result)
}
#[derive(Debug)]
struct Unresolved;
impl std::fmt::Display for Unresolved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Goal verifier ownership/quiescence unconfirmed")
    }
}
impl std::error::Error for Unresolved {}
pub fn audit_processes(state: &Path) -> Result<usize> {
    let root = PathBuf::from(format!("{}.acceptance", state.display()));
    if !root.exists() {
        return Ok(0);
    }
    let audit = || -> Result<usize> {
        let mut count = 0;
        for directory in fs::read_dir(root)? {
            let directory = directory?;
            ensure!(
                !directory.file_type()?.is_symlink(),
                "Prior goal verifier directory must not be a symlink"
            );
            if !directory.file_type()?.is_dir() {
                continue;
            }
            for file in fs::read_dir(directory.path())? {
                let path = file?.path();
                let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                let Some(stem) = name.strip_suffix(".plan.json") else {
                    continue;
                };
                let plan = read(&path)?;
                let observed = read(path.with_file_name(format!("{stem}.process.json")))?;
                ensure!(
                    observed["version"].as_u64() == Some(1)
                        && observed["execution_id"] == plan["execution_id"]
                        && plan["execution_id"].as_str().is_some_and(|s| !s.is_empty())
                        && observed["observer"] == "linux_subreaper_waitpid_pidfd"
                        && observed["quiescent"] == true,
                    "Prior goal verifier ownership/quiescence unconfirmed"
                );
                count += 1;
            }
        }
        Ok(count)
    };
    audit().map_err(|e| e.context(Unresolved))
}
pub fn audit_logs(receipt: &Value, require_stderr: bool) -> Result<()> {
    for name in ["stdout", "stderr"] {
        let key = format!("{name}_sha256");
        if name == "stderr" && receipt.get(&key).is_none_or(Value::is_null) && !require_stderr {
            continue;
        }
        ensure!(
            receipt[&key].as_str() == Some(&sha(Path::new(string(receipt, name)?))?),
            "Goal acceptance evidence changed: {name}"
        );
    }
    Ok(())
}
pub fn audit_result(m: &Value, result: &Value, commit: &str) -> Result<()> {
    validate_policy(m)?;
    ensure!(
        result["passed"] == true && result["status"] == "passed" && result["commit"] == commit,
        "Goal completion identity changed"
    );
    let checks = array(m, "completion_checks")?;
    let receipts = array(result, "receipts")?;
    let ids: BTreeSet<_> = receipts
        .iter()
        .map(|r| string(r, "check_id"))
        .collect::<Result<_>>()?;
    ensure!(
        receipts.len() == checks.len()
            && ids.len() == checks.len()
            && checks
                .iter()
                .all(|c| ids.contains(c["id"].as_str().unwrap_or(""))),
        "Goal completion check coverage changed"
    );
    for receipt in receipts {
        let check = checks
            .iter()
            .find(|c| c["id"] == receipt["check_id"])
            .context("unknown check")?;
        let supervised = m.get("verifier_executable").is_some_and(|v| !v.is_null())
            || receipt.get("executor").is_some()
            || receipt.get("process_receipt").is_some();
        ensure!(
            receipt["commit"] == commit && receipt["exit_status"].as_i64() == Some(0),
            "Goal completion receipt changed"
        );
        audit_logs(receipt, supervised)?;
        let evidence = read(string(receipt, "stdout")?)?;
        validate_evidence(&evidence, check, commit)?;
        ensure!(
            evidence == receipt["evidence"],
            "Goal structured evidence changed"
        );
        if !supervised {
            continue;
        }
        let stdout = fs::canonicalize(string(receipt, "stdout")?)?;
        let stderr = fs::canonicalize(string(receipt, "stderr")?)?;
        let process_path = stdout.with_extension("process.json");
        ensure!(
            fs::canonicalize(string(receipt, "process_receipt")?)? == process_path,
            "Goal process receipt path changed"
        );
        let observed = read(&process_path)?;
        let plan = read(stdout.with_extension("plan.json"))?;
        ensure!(
            observed == receipt["executor"]
                && observed["version"].as_u64() == Some(1)
                && observed["observer"] == "linux_subreaper_waitpid_pidfd"
                && observed["root_exit_status"].as_i64() == Some(0)
                && observed["quiescent"] == true
                && observed["leaked_descendants"] == false
                && observed["termination_reason"] == "root_exit"
                && observed["cleanup_error"].is_null()
                && observed["output_truncated"] == false,
            "Goal execution ownership unconfirmed or evidence changed"
        );
        let env = environment(m, commit)?;
        let expected = command_argv(&check["argv"], &env)?;
        let argv: Vec<&str> = array(&plan, "argv")?
            .iter()
            .map(|v| v.as_str().context("invalid plan argv"))
            .collect::<Result<_>>()?;
        let assignments_len = expected.len() - array(check, "argv")?.len() - 1;
        ensure!(
            argv.len() == expected.len() && argv.first() == Some(&"/usr/bin/env"),
            "Goal process plan binding changed"
        );
        let actual_assignments: BTreeSet<_> =
            argv[1..1 + assignments_len].iter().copied().collect();
        let expected_assignments: BTreeSet<_> = expected[1..1 + assignments_len]
            .iter()
            .map(String::as_str)
            .collect();
        ensure!(
            actual_assignments == expected_assignments
                && actual_assignments.len() == assignments_len
                && argv[1 + assignments_len..] == expected[1 + assignments_len..],
            "Goal process plan binding changed"
        );
        let seconds = check
            .get("timeout_seconds")
            .and_then(Value::as_f64)
            .unwrap_or(300.0)
            .ceil() as u64;
        ensure!(
            observed["execution_id"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
                && plan["execution_id"] == observed["execution_id"]
                && plan["commit"] == commit
                && plan["seconds"].as_u64() == Some(seconds)
                && plan["stdout"] == json!(stdout)
                && plan["log"] == json!(stderr)
                && plan["receipt"] == json!(process_path)
                && plan["cwd"] == json!(stdout.parent().context("stdout parent")?.join("checkout")),
            "Goal process plan binding changed"
        );
    }
    Ok(())
}

/// Keeping stdin open is the supervisor's live ownership contract. Drop closes it
/// on errors, unwinding and owner loss; saved PIDs never authorize signals.
pub fn supervised_check(
    executable: &Path,
    plan: &crate::supervisor::Plan,
    path: &Path,
    search_path: &str,
    receipt: &mut Value,
) -> Result<Option<String>> {
    supervised_check_mode(executable, plan, path, search_path, receipt, false)
}

pub fn supervised_owner_check(
    executable: &Path,
    plan: &crate::supervisor::Plan,
    path: &Path,
    search_path: &str,
    receipt: &mut Value,
) -> Result<Option<String>> {
    supervised_check_mode(executable, plan, path, search_path, receipt, true)
}

fn supervised_check_mode(
    executable: &Path,
    plan: &crate::supervisor::Plan,
    path: &Path,
    search_path: &str,
    receipt: &mut Value,
    owner: bool,
) -> Result<Option<String>> {
    ensure!(
        executable.is_absolute(),
        "absolute trusted verifier executable required"
    );
    crate::supervisor::write_json(path, plan)?;
    receipt["process_receipt"] = json!(plan.receipt);
    let diagnostic = File::create(path.with_extension("supervisor.stderr"))?;
    let mut command = Command::new(executable);
    command
        .args([
            if owner {
                "owner-supervisor"
            } else {
                "verifier-supervisor"
            },
            "--plan",
        ])
        .arg(path)
        .env_clear()
        .env("PATH", search_path);
    if owner {
        for key in [
            "HOME",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "XDG_STATE_HOME",
            "GH_CONFIG_DIR",
            "SSH_AUTH_SOCK",
            "GH_TOKEN",
            "GITHUB_TOKEN",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(diagnostic)
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(plan.seconds + 5);
    let outcome = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    drop(child.stdin.take());
    let status = match outcome {
        Some(status) => status,
        None => {
            let cleanup_deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < cleanup_deadline {
                if child.try_wait()?.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            return Err(anyhow::anyhow!(Unresolved));
        }
    };
    let observed = read(&plan.receipt).map_err(|e| e.context(Unresolved))?;
    receipt["executor"] = observed.clone();
    ensure!(
        status.success()
            && observed["version"].as_u64() == Some(1)
            && observed["execution_id"] == plan.execution_id
            && observed["observer"] == "linux_subreaper_waitpid_pidfd"
            && observed["quiescent"] == true,
        Unresolved
    );
    receipt["exit_status"] = observed["root_exit_status"].clone();
    if observed["root_exit_status"].as_i64() != Some(0)
        || observed["leaked_descendants"] != false
        || observed["termination_reason"] != "root_exit"
        || !observed["cleanup_error"].is_null()
        || observed["output_truncated"] != false
    {
        return Ok(Some(
            observed["termination_reason"]
                .as_str()
                .unwrap_or("supervised execution failed")
                .into(),
        ));
    }
    Ok(None)
}
fn setup_command(
    executable: &Path,
    plan: &crate::supervisor::Plan,
    stage: &str,
    env: &BTreeMap<String, String>,
    receipt: &mut Value,
) -> Result<i32> {
    let stdout = plan.stdout.as_ref().context("setup stdout required")?;
    let failure = supervised_check(
        executable,
        plan,
        &stdout.with_extension("plan.json"),
        &env["PATH"],
        receipt,
    )?;
    if let Some(reason) = failure {
        if reason == "timeout" {
            bail!("acceptance {stage} timeout");
        }
        if receipt["exit_status"].as_i64().is_none() || receipt["exit_status"].as_i64() == Some(0) {
            bail!("acceptance {stage} failed ({reason}); inspect retained setup logs");
        }
    }
    Ok(receipt["exit_status"].as_i64().unwrap_or(-1) as i32)
}
/// Always returns a durable-result-shaped value, including partial receipts.
pub fn run_acceptance(m: &Value, state: &Path, commit: &str, default_verifier: &Path) -> Value {
    let mut receipts = vec![];
    let mut setup = vec![];
    let mut phase = "policy";
    let run = || -> Result<Value> {
        audit_processes(state)?;
        let checks = m.get("completion_checks").and_then(Value::as_array);
        let required = m.get("required_evidence").and_then(Value::as_array);
        if !json_truthy(&m["completion_checks"])
            || !json_truthy(&m["required_evidence"])
            || required.is_some_and(|r| {
                r.iter().all(Value::is_string)
                    && checks.is_some_and(|c| {
                        c.iter().all(|v| v["evidence_type"].is_string())
                            && r.iter()
                                .any(|v| !c.iter().any(|c| c["evidence_type"] == *v))
                    })
            })
        {
            return Ok(
                json!({"passed":false,"status":"awaiting_acceptance","reason":"mandatory completion checks/evidence are missing"}),
            );
        }
        validate_policy(m)?;
        ensure!(
            exact_commit(commit),
            "absolute repository and exact commit required"
        );
        let env = environment(m, commit)?;
        let executable = m
            .get("verifier_executable")
            .filter(|v| !v.is_null())
            .map(|v| v.as_str().context("invalid verifier executable"))
            .transpose()?
            .map(Path::new)
            .unwrap_or(default_verifier);
        phase = "setup";
        let output = PathBuf::from(format!("{}.acceptance", state.display()))
            .join(uuid::Uuid::new_v4().simple().to_string());
        fs::create_dir_all(&output)?;
        let output = fs::canonicalize(output)?;
        let checkout = output.join("checkout");
        let checkout_str = checkout.to_str().context("non-UTF8 checkout")?;
        for (stage, argv, cwd) in [
            (
                "clone",
                vec![
                    "git",
                    "-c",
                    "core.hooksPath=/dev/null",
                    "clone",
                    "--quiet",
                    "--no-hardlinks",
                    "--no-checkout",
                    string(m, "repository")?,
                    checkout_str,
                ],
                None,
            ),
            (
                "checkout",
                vec![
                    "git",
                    "-c",
                    "core.hooksPath=/dev/null",
                    "checkout",
                    "--quiet",
                    "--detach",
                    commit,
                ],
                Some(checkout.as_path()),
            ),
        ] {
            let stdout = output.join(format!("{stage}.stdout"));
            let stderr = output.join(format!("{stage}.stderr"));
            setup.push(json!({"stage":stage,"stdout":stdout,"stderr":stderr,"exit_status":null}));
            let setup_receipt = setup.last_mut().unwrap();
            let plan = crate::supervisor::Plan {
                execution_id: uuid::Uuid::new_v4().simple().to_string(),
                argv: command_argv(&json!(argv), &env)?,
                cwd: cwd.unwrap_or(&output).to_path_buf(),
                log: stderr,
                stdout: Some(stdout.clone()),
                commit: commit.into(),
                seconds: 60,
                receipt: stdout.with_extension("process.json"),
            };
            let status = setup_command(executable, &plan, stage, &env, setup_receipt)?;
            setup_receipt["exit_status"] = json!(status);
            ensure!(
                status == 0,
                "acceptance {stage} failed; inspect retained setup logs"
            );
        }
        for (index, check) in array(m, "completion_checks")?.iter().enumerate() {
            let stdout = output.join(format!("{index}.stdout"));
            let stderr = output.join(format!("{index}.stderr"));
            receipts.push(json!({"check_id":check["id"],"commit":commit,"exit_status":null,"stdout":stdout,"stderr":stderr}));
            let receipt = receipts.last_mut().unwrap();
            let plan = crate::supervisor::Plan {
                execution_id: uuid::Uuid::new_v4().simple().to_string(),
                argv: command_argv(&check["argv"], &env)?,
                cwd: checkout.clone(),
                log: stderr.clone(),
                stdout: Some(stdout.clone()),
                commit: commit.into(),
                seconds: check
                    .get("timeout_seconds")
                    .and_then(Value::as_f64)
                    .unwrap_or(300.0)
                    .ceil() as u64,
                receipt: stdout.with_extension("process.json"),
            };
            phase = "check_launch";
            let failure = supervised_check(
                executable,
                &plan,
                &stdout.with_extension("plan.json"),
                &env["PATH"],
                receipt,
            )?;
            receipt["stdout_sha256"] = json!(sha(&stdout)?);
            receipt["stderr_sha256"] = json!(sha(&stderr)?);
            phase = "check_evidence";
            if let Some(failure) = failure {
                bail!("{failure}");
            }
            ensure!(receipt["exit_status"].as_i64() == Some(0), "command failed");
            let evidence = read(&stdout)?;
            validate_evidence(&evidence, check, commit)?;
            receipt["evidence"] = evidence;
        }
        Ok(
            json!({"passed":true,"status":"passed","commit":commit,"receipts":receipts,"setup":setup}),
        )
    };
    let mut run = run;
    match run() {
        Ok(result) => result,
        Err(error) => {
            json!({"passed":false,"status":"acceptance_failed","commit":commit,"phase":phase,
            "reason":format!("{error:#}").chars().map(|c| if c.is_control() {' '} else {c}).take(1000).collect::<String>(),
            "error_type":"AcceptanceError","execution_unresolved":error.is::<Unresolved>(),"receipts":receipts,"setup":setup,
            "retry_guidance":"Restore the approved repository and verifier prerequisites, inspect retained logs, and resume the same goal state."})
        }
    }
}

/// JSON bridge for parity tests and existing JS/TS callers; no implicit effects.
pub fn request(request: &Value, executable: &Path) -> Result<Value> {
    let m = &request["manifest"];
    Ok(match string(request, "operation")? {
        "validate_policy" => {
            validate_policy(m)?;
            Value::Null
        }
        "validate_evidence" => {
            validate_evidence(
                &request["evidence"],
                &request["check"],
                string(request, "commit")?,
            )?;
            request["evidence"].clone()
        }
        "audit_result" => {
            audit_result(m, &request["result"], string(request, "commit")?)?;
            request["result"].clone()
        }
        "audit_processes" => json!(audit_processes(Path::new(string(request, "state")?))?),
        "audit_logs" => {
            audit_logs(&request["receipt"], request["require_stderr"] == true)?;
            Value::Null
        }
        "run" => run_acceptance(
            m,
            Path::new(string(request, "state")?),
            string(request, "commit")?,
            executable,
        ),
        _ => bail!("unknown goal acceptance operation"),
    })
}
