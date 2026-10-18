//! Rust implementations for recovery, block resolution, pre-admission recovery, and spark control.
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};

use crate::goal_state::{
    self, RawJson, digest, raw_json_compact_utf8, resolve, save_raw, write_bytes_atomic,
};

pub const RESOLUTION_NAME: &str = "contract-only-resolution.json";
pub const INTERRUPTION_NAME: &str = "owner-interruption.json";

#[derive(Clone, Debug)]
pub struct SupervisedCommandResult {
    pub exit_status: i64,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub receipt: Value,
    pub evidence_dir: PathBuf,
}

fn create_private_evidence_dir() -> Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    let base = std::env::temp_dir();
    let execution_id = uuid::Uuid::new_v4().simple().to_string();
    let dir = base.join(format!(
        "code-factory-recovery-{}-{execution_id}",
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
        "unsafe operational evidence directory: {}",
        dir.display()
    );
    resolve(&dir)
}

pub fn supervised_command(
    supervisor_executable: &Path,
    cmd_name: &Path,
    args: &[&str],
    cwd: &Path,
    timeout_secs: u64,
    expected_exit: i64,
) -> Result<SupervisedCommandResult> {
    supervised_command_mode(
        supervisor_executable,
        cmd_name,
        args,
        cwd,
        timeout_secs,
        expected_exit,
        false,
    )
}

pub fn supervised_owner_command(
    supervisor_executable: &Path,
    cmd_name: &Path,
    args: &[&str],
    cwd: &Path,
    timeout_secs: u64,
    expected_exit: i64,
) -> Result<SupervisedCommandResult> {
    supervised_command_mode(
        supervisor_executable,
        cmd_name,
        args,
        cwd,
        timeout_secs,
        expected_exit,
        true,
    )
}

fn supervised_command_mode(
    supervisor_executable: &Path,
    cmd_name: &Path,
    args: &[&str],
    cwd: &Path,
    timeout_secs: u64,
    expected_exit: i64,
    owner: bool,
) -> Result<SupervisedCommandResult> {
    let supervisor_executable = resolve(supervisor_executable)?;
    let cwd = resolve(cwd)?;
    let evidence_dir = create_private_evidence_dir()?;
    let execution_id = uuid::Uuid::new_v4().simple().to_string();
    let plan_path = evidence_dir.join("command.plan.json");
    let stdout_path = evidence_dir.join("command.stdout");
    let stderr_path = evidence_dir.join("command.stderr");
    let receipt_path = evidence_dir.join("command.process.json");

    let mut argv = Vec::with_capacity(1 + args.len());
    argv.push(cmd_name.to_string_lossy().into_owned());
    for a in args {
        argv.push((*a).to_string());
    }

    let plan = crate::supervisor::Plan {
        execution_id: execution_id.clone(),
        argv,
        cwd,
        log: stderr_path.clone(),
        stdout: Some(stdout_path.clone()),
        commit: "0".repeat(40),
        seconds: timeout_secs,
        receipt: receipt_path.clone(),
    };

    let search_path = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into());
    let mut check_receipt = json!({});
    let failure = if owner {
        crate::goal_acceptance::supervised_owner_check(
            &supervisor_executable,
            &plan,
            &plan_path,
            &search_path,
            &mut check_receipt,
        )
    } else {
        crate::goal_acceptance::supervised_check(
            &supervisor_executable,
            &plan,
            &plan_path,
            &search_path,
            &mut check_receipt,
        )
    }
    .with_context(|| {
        format!(
            "supervised command unresolved (retained at {})",
            evidence_dir.display()
        )
    })?;

    let observed = check_receipt
        .get("executor")
        .cloned()
        .unwrap_or(Value::Null);
    let stdout_bytes = fs::read(&stdout_path).with_context(|| {
        format!(
            "reading supervised stdout (retained at {})",
            evidence_dir.display()
        )
    })?;
    let stderr_bytes = fs::read(&stderr_path).with_context(|| {
        format!(
            "reading supervised stderr (retained at {})",
            evidence_dir.display()
        )
    })?;

    let valid_completion = observed.get("version").and_then(Value::as_u64) == Some(1)
        && observed.get("execution_id").and_then(Value::as_str) == Some(execution_id.as_str())
        && observed.get("observer").and_then(Value::as_str)
            == Some("linux_subreaper_waitpid_pidfd")
        && observed.get("termination_reason").and_then(Value::as_str) == Some("root_exit")
        && observed.get("quiescent").and_then(Value::as_bool) == Some(true)
        && observed.get("leaked_descendants").and_then(Value::as_bool) == Some(false)
        && observed.get("cleanup_error").is_none_or(Value::is_null)
        && observed.get("output_truncated").and_then(Value::as_bool) == Some(false)
        && observed.get("root_exit_status").and_then(Value::as_i64) == Some(expected_exit)
        && (expected_exit != 0 || failure.is_none());

    if !valid_completion {
        let reason = failure.unwrap_or_else(|| {
            observed
                .get("termination_reason")
                .and_then(Value::as_str)
                .unwrap_or("unexpected_exit_or_unconfirmed_cleanup")
                .to_string()
        });
        bail!(
            "supervised command failed ({reason}): status={:?}, expected={expected_exit}, stderr={}, retained={}",
            observed.get("root_exit_status"),
            String::from_utf8_lossy(&stderr_bytes).trim(),
            evidence_dir.display()
        );
    }

    let _ = fs::remove_dir_all(&evidence_dir);

    Ok(SupervisedCommandResult {
        exit_status: expected_exit,
        stdout: stdout_bytes,
        stderr: stderr_bytes,
        receipt: observed,
        evidence_dir,
    })
}

pub fn sha(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}

pub fn read_json(path: &Path) -> Result<Value> {
    let text =
        fs::read_to_string(path).with_context(|| format!("reading json {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing json {}", path.display()))
}

pub fn regular(path: &Path) -> Result<PathBuf> {
    let meta = fs::symlink_metadata(path).with_context(|| format!("lstat {}", path.display()))?;
    ensure!(meta.is_file(), "Untrusted owner file: {}", path.display());
    let uid = unsafe { libc::getuid() };
    ensure!(
        meta.uid() == uid,
        "Untrusted owner file: {}",
        path.display()
    );
    ensure!(
        meta.permissions().mode() & 0o022 == 0,
        "Untrusted owner file: {}",
        path.display()
    );
    Ok(path.to_path_buf())
}

pub fn ancestry() -> HashMap<u32, String> {
    let mut result = HashMap::new();
    let mut pid = unsafe { libc::getpid() as u32 };
    for _ in 0..64 {
        let stat_path = format!("/proc/{pid}/stat");
        let content = match fs::read_to_string(&stat_path) {
            Ok(c) => c,
            Err(_) => break,
        };
        let Some((_, rest)) = content.rsplit_once(')') else {
            break;
        };
        let fields: Vec<&str> = rest.split_whitespace().collect();
        if fields.len() < 20 {
            break;
        }
        let ticks = fields[19].to_string();
        let parent: u32 = match fields[1].parse() {
            Ok(p) => p,
            Err(_) => break,
        };
        result.insert(pid, ticks);
        if parent <= 1 || result.contains_key(&parent) {
            break;
        }
        pid = parent;
    }
    result
}

fn parse_kernel_locks(
    text: &str,
    fdinfo: bool,
) -> Vec<(String, String, String, String, String, String, String)> {
    let mut result = Vec::new();
    for line in text.lines() {
        let mut fields: Vec<&str> = line.split_whitespace().collect();
        if fdinfo {
            if fields.is_empty() || fields[0] != "lock:" {
                continue;
            }
            fields.remove(0);
        }
        if fields.len() == 8
            && fields[1..4] == ["FLOCK", "ADVISORY", "WRITE"]
            && fields[6..] == ["0", "EOF"]
        {
            result.push((
                fields[1].to_string(),
                fields[2].to_string(),
                fields[3].to_string(),
                fields[4].to_string(),
                fields[5].to_string(),
                fields[6].to_string(),
                fields[7].to_string(),
            ));
        }
    }
    result
}

pub fn owner_lock(path: &Path) -> Result<Value> {
    let target = fs::symlink_metadata(path).with_context(|| format!("lstat {}", path.display()))?;
    ensure!(target.is_file(), "Lock target is not a regular file");
    let target_dev = target.dev();
    let target_ino = target.ino();

    let before = ancestry();
    let proc_locks_text = fs::read_to_string("/proc/locks").context("reading /proc/locks")?;
    let global_locks = parse_kernel_locks(&proc_locks_text, false);

    for (pid, ticks) in &before {
        let fd_dir = format!("/proc/{pid}/fd");
        let entries = match fs::read_dir(&fd_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let fd_path = entry.path();
            let Ok(observed) = fs::metadata(&fd_path) else {
                continue;
            };
            if (observed.dev(), observed.ino()) != (target_dev, target_ino) {
                continue;
            }
            let fd_name = entry.file_name().to_string_lossy().to_string();
            let fdinfo_path = format!("/proc/{pid}/fdinfo/{fd_name}");
            let Ok(text) = fs::read_to_string(&fdinfo_path) else {
                continue;
            };
            let mut ino_val = -1i64;
            for line in text.lines() {
                if line.starts_with("ino:")
                    && let Some(val) = line.strip_prefix("ino:").map(|s| s.trim())
                {
                    ino_val = val.parse().unwrap_or(-1);
                }
            }
            if ino_val as u64 != target_ino {
                continue;
            }
            let local_locks = parse_kernel_locks(&text, true);
            for lock in local_locks {
                if lock.3 != pid.to_string() || !global_locks.contains(&lock) {
                    continue;
                }
                let stat_path = format!("/proc/{pid}/stat");
                let Ok(stat_content) = fs::read_to_string(&stat_path) else {
                    continue;
                };
                let Some((_, rest)) = stat_content.rsplit_once(')') else {
                    continue;
                };
                let fields: Vec<&str> = rest.split_whitespace().collect();
                if fields.len() < 20 || fields[19] != ticks {
                    continue;
                }
                let Ok(latest) = fs::symlink_metadata(path) else {
                    continue;
                };
                if (latest.dev(), latest.ino()) != (target_dev, target_ino) {
                    continue;
                }
                let Ok(re_read_proc_locks) = fs::read_to_string("/proc/locks") else {
                    continue;
                };
                let fresh_global = parse_kernel_locks(&re_read_proc_locks, false);
                if !fresh_global.contains(&lock) {
                    continue;
                }

                let major = libc::major(target_dev);
                let minor = libc::minor(target_dev);
                return Ok(json!({
                    "pid": pid,
                    "fd": fd_name,
                    "stat_device": format!("{major:02x}:{minor:02x}"),
                    "kernel_lock_key": lock.4,
                    "inode": target_ino,
                    "start_ticks": ticks,
                }));
            }
        }
    }
    bail!("Exclusive owner lock unavailable: {}", path.display())
}

pub fn audit_permission_block_resolution(
    root: &Path,
    directory: &Path,
    controller: &Path,
    next_sequence: u64,
) -> Result<Value> {
    let exe = std::env::current_exe()?;
    audit_permission_block_resolution_with(root, directory, controller, next_sequence, &exe)
}

pub fn audit_permission_block_resolution_with(
    root: &Path,
    directory: &Path,
    controller: &Path,
    next_sequence: u64,
    supervisor_executable: &Path,
) -> Result<Value> {
    let root = resolve(root)?;
    let directory = resolve(directory)?;
    let supervisor_executable = resolve(supervisor_executable)?;
    let proof = regular(&root.join(RESOLUTION_NAME))?;
    let v = read_json(&proof)?;

    let version = v.get("version").and_then(Value::as_i64);
    let kind = v.get("kind").and_then(Value::as_str).unwrap_or("");
    ensure!(
        version == Some(1)
            && [
                "contract-only-checker-reference",
                "owner-checkout-isolation",
                "host-git-ownership"
            ]
            .contains(&kind),
        "Unsupported resolution"
    );
    let host_git = kind == "host-git-ownership";

    ensure!(
        directory.parent() == Some(&root.join("cycles"))
            && directory
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.chars().all(|c| c.is_ascii_digit())),
        "Foreign cycle"
    );

    let original = regular(&directory.join("result.json"))?;
    let out = read_json(&original)?;
    let seq_from_name: u64 = directory
        .file_name()
        .and_then(|n| n.to_str())
        .context("cycle name")?
        .parse()?;

    let v_orig_sha = v
        .get("original_sha256")
        .and_then(Value::as_str)
        .unwrap_or("");
    let v_seq = v.get("sequence").and_then(Value::as_u64);
    let out_seq = out.get("sequence").and_then(Value::as_u64);
    ensure!(
        v_orig_sha == sha(&original)?
            && v_seq == Some(seq_from_name)
            && out_seq == Some(seq_from_name)
            && next_sequence == seq_from_name + 1,
        "Stale failure or next sequence"
    );

    ensure!(
        out.get("status").and_then(Value::as_str) == Some("failed")
            && out.get("failure_kind").and_then(Value::as_str) == Some("blocked")
            && matches!(
                out.get("phase").and_then(Value::as_str),
                Some("native" | "native_shells" | "native_completion")
            )
            && out.get("cleanup_confirmed").and_then(Value::as_bool) == Some(true)
            && out.get("fence_error").is_none_or(Value::is_null),
        "Not a confirmed failed native block"
    );
    ensure!(
        out.get("candidate").is_none_or(Value::is_null)
            && out.get("acceptance").is_none_or(Value::is_null),
        "Resolution cannot approve candidate work"
    );

    let id_keys = [
        "goal_id", "job_id", "attempt", "child", "parent", "worktree",
    ];
    let mut identity_map = serde_json::Map::new();
    for k in id_keys {
        let val = out.get(k).and_then(Value::as_str).unwrap_or("");
        ensure!(!val.is_empty(), "Execution identity mismatch");
        identity_map.insert(k.to_string(), json!(val));
    }
    let expected_identity = Value::Object(identity_map);
    ensure!(
        v.get("identity") == Some(&expected_identity),
        "Execution identity mismatch"
    );

    let cfg = read_json(&root.join("supervisor.json"))?;
    let goal = read_json(&root.join("goal.json"))?;
    let cfg_controller = cfg.get("controller").and_then(Value::as_str).unwrap_or("");
    let cfg_ctrl_resolved = resolve(Path::new(cfg_controller))?;
    let target_ctrl_resolved = resolve(controller)?;
    ensure!(
        goal.get("id") == out.get("goal_id")
            && cfg_ctrl_resolved == target_ctrl_resolved
            && cfg.get("controller_sha256").and_then(Value::as_str) == Some(&sha(controller)?),
        "Goal/controller mismatch"
    );
    ensure!(
        v.get("configuration_sha256").and_then(Value::as_str)
            == Some(&sha(&root.join("supervisor.json"))?),
        "Configuration changed"
    );

    let denied = root
        .parent()
        .unwrap_or(Path::new("."))
        .join("code-factory-recovery-owner/scripts/queue-fixture-policy.py");
    let denied_str = denied.to_string_lossy().to_string();

    let history = regular(&directory.join("native-context-final.json"))?;
    ensure!(
        v.get("denial_history_sha256").and_then(Value::as_str) == Some(&sha(&history)?),
        "Denial history changed"
    );

    let history_val = read_json(&history)?;
    let history_arr = history_val.as_array().context("history not array")?;
    let mut rejected = Vec::new();
    for message in history_arr {
        if let Some(content_arr) = message.get("content").and_then(Value::as_array) {
            for content in content_arr {
                if let Some(state) = content.get("state")
                    && let Some(error) = state.get("error")
                    && error.get("type").and_then(Value::as_str) == Some("permission.rejected")
                {
                    rejected.push(content);
                }
            }
        }
    }
    ensure!(rejected.len() == 1, "Additional or absent denial");
    let call = rejected[0];

    if kind == "contract-only-checker-reference" {
        ensure!(
            call.get("name").and_then(Value::as_str) == Some("read")
                && call.get("executed").and_then(Value::as_bool) == Some(false)
                && call.get("state").and_then(|s| s.get("input"))
                    == Some(&json!({"path": denied_str}))
                && call
                    .get("state")
                    .and_then(|s| s.get("error"))
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    == Some("Permission denied: external_directory"),
            "Different denied action"
        );
    } else if host_git {
        let attempt = out.get("attempt").and_then(Value::as_str).unwrap_or("");
        ensure!(
            regex_attempt_valid(attempt),
            "Invalid cleanup attempt identity"
        );
        ensure!(
            call.get("type").and_then(Value::as_str) == Some("tool"),
            "Different denied action"
        );
        let worktree = directory.join("controller/workers").join(attempt);
        ensure!(
            out.get("worktree").and_then(Value::as_str)
                == Some(worktree.to_string_lossy().as_ref()),
            "Foreign worktree cleanup target"
        );
        let expected_cmd = format!(
            "cd {} && git worktree remove --force {} 2>&1 | tail -2; echo rc=$?; git worktree prune 2>&1 | tail -1; echo rc2=$?",
            root.join("repository").display(),
            worktree.display()
        );
        let last_row = history_arr.last().context("empty history")?;
        let last_content = last_row
            .get("content")
            .and_then(Value::as_array)
            .context("no content")?;
        ensure!(
            last_content.len() == 1
                && &last_content[0] == call
                && call
                    .get("state")
                    .and_then(|s| s.get("status"))
                    .and_then(Value::as_str)
                    == Some("error"),
            "Actions after denied cleanup or incomplete denial"
        );
        ensure!(
            v.get("denied_action_sha256").and_then(Value::as_str) == Some(&digest(call)),
            "Denied action fingerprint changed"
        );
        ensure!(
            v.get("worker_task_sha256").and_then(Value::as_str)
                == Some(&sha(&regular(&root.join("task.txt"))?)?),
            "Worker Git ownership contract changed"
        );
        ensure!(
            call.get("name").and_then(Value::as_str) == Some("shell")
                && call.get("executed").and_then(Value::as_bool) == Some(false)
                && call
                    .get("state")
                    .and_then(|s| s.get("input"))
                    .and_then(|i| i.get("command"))
                    .and_then(Value::as_str)
                    == Some(&expected_cmd)
                && call
                    .get("state")
                    .and_then(|s| s.get("error"))
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    == Some("Permission denied: external_directory"),
            "Different denied action"
        );
        let chk = out.get("checkpoint").and_then(Value::as_str).unwrap_or("");
        ensure!(
            chk.len() == 40 && v.get("checkpoint").and_then(Value::as_str) == Some(chk),
            "Unaccepted checkpoint mismatch"
        );
    } else {
        let expected_cmd = format!(
            "cd {} && python3 scripts/test-queue-cli.py 2>&1 | tail -15",
            root.join("repository").display()
        );
        ensure!(
            call.get("name").and_then(Value::as_str) == Some("shell")
                && call.get("executed").and_then(Value::as_bool) == Some(false)
                && call
                    .get("state")
                    .and_then(|s| s.get("input"))
                    .and_then(|i| i.get("command"))
                    .and_then(Value::as_str)
                    == Some(&expected_cmd)
                && call
                    .get("state")
                    .and_then(|s| s.get("error"))
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    == Some("Permission denied: external_directory"),
            "Different denied action"
        );
        let chk = out.get("checkpoint").and_then(Value::as_str).unwrap_or("");
        ensure!(
            chk.len() == 40 && v.get("checkpoint").and_then(Value::as_str) == Some(chk),
            "Unaccepted checkpoint mismatch"
        );
    }

    let manifest_path_val = v
        .get("manifest")
        .and_then(Value::as_str)
        .context("manifest path")?;
    let manifest_path = regular(Path::new(manifest_path_val))?;
    let manifest = read_json(&manifest_path)?;
    let state_path_val = manifest
        .get("state")
        .and_then(Value::as_str)
        .context("manifest state")?;
    let state = read_json(&regular(Path::new(state_path_val))?)?;

    ensure!(
        state.get("manifest_digest").and_then(Value::as_str) == Some(&digest(&manifest)),
        "Owner manifest not reconciled"
    );

    let pool_lock_val = manifest
        .get("pool_lock")
        .and_then(Value::as_str)
        .context("pool lock")?;
    let pool_ownership = owner_lock(Path::new(pool_lock_val))?;
    let supervisor_ownership = owner_lock(&root.join("supervisor.lock"))?;
    let ownership = json!([pool_ownership, supervisor_ownership]);

    let current = state.get("current");
    if let Some(curr) = current
        && !curr.is_null()
    {
        ensure!(
            curr.get("goal") == out.get("goal_id")
                && curr.get("expected_sequence").and_then(Value::as_u64) == Some(next_sequence),
            "Another active goal or sequence"
        );
    }

    let goals_arr = manifest
        .get("goals")
        .and_then(Value::as_array)
        .context("manifest goals")?;
    ensure!(
        goals_arr.iter().any(|g| {
            g.get("id") == out.get("goal_id")
                && g.get("root")
                    .and_then(Value::as_str)
                    .is_some_and(|r| resolve(Path::new(r)).is_ok_and(|rp| rp == root))
        }),
        "Manifest goal mismatch"
    );

    let pins = manifest
        .get("trusted_files")
        .and_then(Value::as_object)
        .context("trusted_files")?;
    let proof_str = proof.to_string_lossy().to_string();
    ensure!(
        pins.get(&proof_str).and_then(Value::as_str) == Some(&sha(&proof)?),
        "Resolution is not owner-pinned"
    );

    let mut without = manifest.clone();
    if let Some(tf) = without
        .get_mut("trusted_files")
        .and_then(Value::as_object_mut)
    {
        tf.remove(&proof_str);
    }
    ensure!(
        v.get("manifest_without_resolution_sha256")
            .and_then(Value::as_str)
            == Some(&digest(&without)),
        "Owner manifest changed"
    );

    let pinned = |p: &Path| -> Result<Value> {
        let p_reg = regular(p)?;
        let p_str = p_reg.to_string_lossy().to_string();
        let expected_hash = pins
            .get(&p_str)
            .and_then(Value::as_str)
            .context("unpinned file")?;
        ensure!(
            sha(&p_reg)? == expected_hash,
            "Unpinned resolution input: {}",
            p.display()
        );
        read_json(&p_reg)
    };

    let self_exe = regular(&supervisor_executable)?;
    let self_exe_str = self_exe.to_string_lossy().to_string();
    let exe_pin = pins
        .get(&self_exe_str)
        .and_then(Value::as_str)
        .context("Unpinned resolver implementation: native executable not pinned")?;
    ensure!(
        sha(&self_exe)? == exe_pin,
        "Unpinned resolver implementation"
    );

    let auth_path = v
        .get("authorization")
        .and_then(Value::as_str)
        .context("auth path")?;
    let authorization = pinned(Path::new(auth_path))?;
    ensure!(
        authorization.get("decision").and_then(Value::as_str) == Some("approve")
            && authorization.get("source").and_then(Value::as_str)
                == Some("verified-human-conversation")
            && ["thread_id", "message_id", "reply"].iter().all(|k| {
                authorization
                    .get(*k)
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.trim().is_empty())
            }),
        "Explicit human authorization missing"
    );

    let mut expected_scope = json!({
        "goal_id": out["goal_id"],
        "original_sha256": sha(&original)?,
        "denial_history_sha256": sha(&history)?,
        "packaging_sha256": v["packaging_sha256"],
        "next_sequence": next_sequence,
        "denied_file_remains_inaccessible": true,
        "missing_usage_remains_unknown": true,
        "same_goal_only": true,
    });
    if kind == "owner-checkout-isolation" || host_git {
        let obj = expected_scope.as_object_mut().unwrap();
        obj.insert("kind".into(), json!(kind));
        obj.insert("checkpoint".into(), out["checkpoint"].clone());
        obj.insert("isolation_sha256".into(), v["isolation_sha256"].clone());
        obj.insert("post_denial_history_preserved".into(), json!(true));
        obj.insert("denied_command_will_not_be_retried".into(), json!(true));
        if host_git {
            obj.insert(
                "denied_action_sha256".into(),
                v["denied_action_sha256"].clone(),
            );
            obj.insert("worker_task_sha256".into(), v["worker_task_sha256"].clone());
            obj.insert("host_owns_git_mutations".into(), json!(true));
        }
    }
    ensure!(
        authorization.get("scope") == Some(&expected_scope),
        "Authorization scope mismatch"
    );

    let pkg_path = v
        .get("packaging")
        .and_then(Value::as_str)
        .context("pkg path")?;
    let packaging = pinned(Path::new(pkg_path))?;
    ensure!(
        sha(Path::new(pkg_path))?
            == v.get("packaging_sha256")
                .and_then(Value::as_str)
                .unwrap_or("")
            && packaging.get("applied").and_then(Value::as_bool) == Some(true)
            && packaging
                .get("host_checker_bytes_unchanged")
                .and_then(Value::as_bool)
                == Some(true)
            && packaging
                .get("host_acceptance_wrapper_only_changes_checker_location")
                .and_then(Value::as_bool)
                == Some(true)
            && packaging
                .get("manifest_goals_acceptance_models_publication_and_permissions_unchanged")
                .and_then(Value::as_bool)
                == Some(true)
            && packaging
                .get("denied_helper_exposed_to_worker")
                .and_then(Value::as_bool)
                == Some(false),
        "Packaging proof mismatch"
    );

    let data = root.join("trusted/queue-publication-fixture");
    let host = root.join("trusted/queue-publication-host/check.py");
    let contract = data.join("VERIFIER-CONTRACT.md");
    let cfg_data = resolve(Path::new(
        cfg.get("data").and_then(Value::as_str).unwrap_or(""),
    ));
    ensure!(
        cfg_data.is_ok() && cfg_data.unwrap() == data && !data.is_symlink(),
        "Worker inputs are not contract-only"
    );
    let mut data_entries = Vec::new();
    for entry in fs::read_dir(&data)? {
        data_entries.push(entry?.file_name().to_string_lossy().to_string());
    }
    data_entries.sort();
    ensure!(
        data_entries == vec!["VERIFIER-CONTRACT.md"],
        "Worker inputs are not contract-only"
    );

    for (p, h) in [
        (
            &host,
            packaging
                .get("host_checker_sha256")
                .and_then(Value::as_str)
                .unwrap_or(""),
        ),
        (
            &contract,
            packaging
                .get("contract_sha256")
                .and_then(Value::as_str)
                .unwrap_or(""),
        ),
    ] {
        let p_reg = regular(p)?;
        let p_sha = sha(&p_reg)?;
        let p_str = p_reg.to_string_lossy().to_string();
        ensure!(
            p_sha == h && pins.get(&p_str).and_then(Value::as_str) == Some(h),
            "Frozen checker/contract changed"
        );
    }

    for p in [
        root.join("task.txt"),
        root.join("trusted/verify.py"),
        root.join("supervisor.json"),
    ] {
        let p_reg = regular(&p)?;
        let p_str = p_reg.to_string_lossy().to_string();
        ensure!(
            pins.get(&p_str).and_then(Value::as_str) == Some(&sha(&p_reg)?),
            "Unpinned runtime input"
        );
    }

    let task_text = fs::read_to_string(root.join("task.txt"))?;
    ensure!(
        !task_text.contains("Read data/check.py")
            && task_text.contains("data/VERIFIER-CONTRACT.md"),
        "Stale worker instruction"
    );

    let before_dir = Path::new(pkg_path)
        .parent()
        .context("pkg parent")?
        .join("before");
    let prior = pinned(&before_dir.join("manifest.json"))?;
    ensure!(
        manifest.get("goals") == prior.get("goals")
            && manifest.get("main_publication") == prior.get("main_publication"),
        "Acceptance or publication contract changed"
    );
    let before_check = before_dir.join("check.py");
    let host_sha = sha(&host)?;
    ensure!(
        pins.get(&before_check.to_string_lossy().to_string())
            .and_then(Value::as_str)
            == Some(&sha(&regular(&before_check)?)?)
            && sha(&before_check)? == host_sha,
        "Original frozen oracle changed"
    );
    let before_verify = before_dir.join("verify.py");
    regular(&before_verify)?;
    ensure!(
        pins.get(&before_verify.to_string_lossy().to_string())
            .and_then(Value::as_str)
            == Some(&sha(&before_verify)?),
        "Unpinned original verifier"
    );

    let verify_text = fs::read_to_string(root.join("trusted/verify.py"))?;
    let before_verify_text = fs::read_to_string(&before_verify)?;
    ensure!(
        verify_text.replace(
            "T/'queue-publication-host/check.py'",
            "T/'queue-publication-fixture/check.py'"
        ) == before_verify_text,
        "Host verifier changes more than checker location"
    );

    let verif_path = v
        .get("verification")
        .and_then(Value::as_str)
        .context("verification path")?;
    let checks = pinned(Path::new(verif_path))?;
    let required_kernel_checks = [
        "worker_inputs_contract_only",
        "contract_readable",
        "host_checker_hidden",
        "denied_helper_hidden",
        "contract_readonly",
        "project_writable",
        "git_control_readonly",
    ];
    let kernel_checks = checks.get("checks").and_then(|c| c.get("kernel"));
    ensure!(
        checks.get("passed").and_then(Value::as_bool) == Some(true)
            && required_kernel_checks.iter().all(|k| kernel_checks
                .and_then(|c| c.get(*k))
                .and_then(Value::as_bool)
                == Some(true))
            && checks
                .get("checks")
                .and_then(|c| c.get("same_goal_admission_gate"))
                .and_then(|s| s.get("blocked"))
                .and_then(Value::as_bool)
                == Some(true),
        "Packaging verification missing"
    );

    if kind == "owner-checkout-isolation" || host_git {
        let iso_path = v
            .get("isolation")
            .and_then(Value::as_str)
            .context("isolation path")?;
        let isolation = pinned(Path::new(iso_path))?;
        ensure!(
            sha(Path::new(iso_path))?
                == v.get("isolation_sha256")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
            "Isolation evidence changed"
        );

        let kernel_out = pinned(Path::new(
            isolation
                .get("kernel_output")
                .and_then(Value::as_str)
                .unwrap_or(""),
        ))?;
        let required_iso = [
            "assigned_cwd",
            "worker_writable",
            "owner_source_hidden",
            "owner_canary_hidden",
            "owner_absolute_execution_blocked",
            "owner_symlink_hidden",
            ".git_readonly",
            "frozen.txt_readonly",
            "git_reads_work",
            "git_status_work",
            "git_metadata_readonly",
        ];
        let kernel_checks_obj = kernel_out.get("checks");
        ensure!(
            kernel_out.get("passed").and_then(Value::as_bool) == Some(true)
                && kernel_out.get("baseline").and_then(Value::as_bool) == Some(false)
                && required_iso.iter().all(|k| kernel_checks_obj
                    .and_then(|c| c.get(*k))
                    .and_then(Value::as_bool)
                    == Some(true)),
            "Owner-source isolation not independently verified"
        );

        let execution = pinned(Path::new(
            isolation
                .get("kernel_receipt")
                .and_then(Value::as_str)
                .unwrap_or(""),
        ))?;
        let plan = pinned(Path::new(
            isolation
                .get("kernel_plan")
                .and_then(Value::as_str)
                .unwrap_or(""),
        ))?;
        ensure!(
            execution.get("observer").and_then(Value::as_str)
                == Some("linux_subreaper_waitpid_pidfd")
                && execution.get("execution_id") == plan.get("execution_id")
                && execution.get("root_exit_status").and_then(Value::as_i64) == Some(0)
                && execution.get("quiescent").and_then(Value::as_bool) == Some(true)
                && execution.get("leaked_descendants").and_then(Value::as_bool) == Some(false)
                && execution.get("cleanup_error").is_none_or(Value::is_null)
                && plan.get("stdout") == isolation.get("kernel_output"),
            "Kernel verifier execution not confirmed"
        );

        let qualified = pinned(Path::new(
            isolation
                .get("qualification")
                .and_then(Value::as_str)
                .unwrap_or(""),
        ))?;
        let qual_checks = qualified.get("checks");
        let basic_qual = [
            "candidate",
            "integration",
            "whole_goal",
            "owner_source_hidden",
            "denial_sticky",
            "terminal_denial_blocked",
        ];
        ensure!(
            qualified.get("passed").and_then(Value::as_bool) == Some(true)
                && qualified.get("source_commit") == isolation.get("source_commit")
                && basic_qual.iter().all(|k| qual_checks
                    .and_then(|c| c.get(*k))
                    .and_then(Value::as_bool)
                    == Some(true)),
            "Incomplete isolation qualification"
        );
        if host_git {
            let host_git_qual = [
                "worker_git_contract",
                "host_git_denial_exact",
                "pending_authorization_blocks",
            ];
            ensure!(
                host_git_qual.iter().all(|k| qual_checks
                    .and_then(|c| c.get(*k))
                    .and_then(Value::as_bool)
                    == Some(true)),
                "Host Git ownership recovery not independently qualified"
            );
        }

        let published = pinned(Path::new(
            isolation
                .get("publication")
                .and_then(Value::as_str)
                .unwrap_or(""),
        ))?;
        let binding = published.get("binding").context("published binding")?;
        let intent = published.get("intent").context("published intent")?;
        ensure!(
            binding.get("repository").and_then(Value::as_str)
                == Some("lhemerly/code-factory-controller")
                && binding.get("repository_id").and_then(Value::as_i64) == Some(1414818646)
                && binding.get("private").and_then(Value::as_bool) == Some(true)
                && binding.get("branch").and_then(Value::as_str) == Some("main")
                && binding.get("required_checks") == Some(&json!(["rust-and-contract"]))
                && intent.get("state").and_then(Value::as_str) == Some("confirmed")
                && intent.get("commit") == isolation.get("source_commit")
                && intent
                    .get("ci")
                    .and_then(|c| c.get("status"))
                    .and_then(Value::as_str)
                    == Some("passed"),
            "Isolation source not confirmed on private main"
        );

        let files = isolation
            .get("runtime_files")
            .and_then(Value::as_object)
            .context("runtime files")?;
        let mut expected_names = vec!["spark-sandbox.mjs", "continuous-ownership.mjs"];
        if host_git {
            expected_names.push("research-context.mjs");
        }
        ensure!(
            files.len() == expected_names.len(),
            "Incomplete isolation runtime inventory"
        );
        let mut asset_dir: Option<PathBuf> = None;
        for name in &expected_names {
            let matching: Vec<(&String, &Value)> = files
                .iter()
                .filter(|(k, _)| {
                    Path::new(k.as_str()).file_name().and_then(|n| n.to_str()) == Some(*name)
                })
                .collect();
            ensure!(
                matching.len() == 1,
                "Incomplete isolation runtime inventory"
            );
            let (p_str, file_hash_val) = matching[0];
            let p_reg = regular(Path::new(p_str))?;
            let p_resolved = resolve(&p_reg)?;
            ensure!(
                p_resolved.to_string_lossy() == p_str.as_str(),
                "Incomplete isolation runtime inventory"
            );
            let parent = p_resolved
                .parent()
                .context("runtime file parent")?
                .to_path_buf();
            if let Some(ref existing_dir) = asset_dir {
                ensure!(
                    existing_dir == &parent,
                    "Incomplete isolation runtime inventory"
                );
            } else {
                asset_dir = Some(parent);
            }
            let p_sha = sha(&p_reg)?;
            let file_hash = file_hash_val.as_str().unwrap_or("");
            let pin_hash = pins.get(p_str).and_then(Value::as_str).unwrap_or("");
            let qual_hash = qualified
                .get("runtime_sha256")
                .and_then(|r| r.get(*name))
                .and_then(Value::as_str)
                .unwrap_or("");
            ensure!(
                file_hash == p_sha && p_sha == pin_hash && pin_hash == qual_hash,
                "Isolation runtime changed"
            );
        }
    }

    let owned_receipt = pinned(Path::new(
        v.get("owned_receipt").and_then(Value::as_str).unwrap_or(""),
    ))?;
    let owned_plan = pinned(Path::new(
        v.get("owned_plan").and_then(Value::as_str).unwrap_or(""),
    ))?;
    let plan_argv = owned_plan
        .get("argv")
        .and_then(Value::as_array)
        .context("owned_plan argv")?;
    ensure!(
        owned_receipt.get("execution_id") == owned_plan.get("execution_id")
            && plan_argv
                .iter()
                .any(|a| a.as_str() == Some(manifest_path_val))
            && owned_receipt.get("quiescent").and_then(Value::as_bool) == Some(true)
            && owned_receipt
                .get("leaked_descendants")
                .and_then(Value::as_bool)
                == Some(false)
            && owned_receipt
                .get("root_exit_status")
                .is_some_and(|s| s.is_i64() || s.is_u64()),
        "Owned execution cleanup missing"
    );

    let cfg_ctrl_path = directory.join("controller.json");
    let job_id = out.get("job_id").and_then(Value::as_str).unwrap_or("");
    let output = supervised_command(
        &supervisor_executable,
        controller,
        &[
            "inspect",
            "--config",
            cfg_ctrl_path.to_str().unwrap(),
            "--job",
            job_id,
        ],
        &directory,
        40,
        0,
    )?;
    let observed: Value = serde_json::from_slice(&output.stdout)?;
    let live = observed
        .get("execution_recovery")
        .cloned()
        .unwrap_or(json!({}));
    ensure!(
        observed.get("state").and_then(Value::as_str) == Some("blocked")
            && live.get("stop_id") == v.get("stop_id")
            && live.get("attempt_id") == out.get("attempt")
            && live.get("session_id") == out.get("child")
            && live
                .get("execution_stop")
                .and_then(|e| e.get("status"))
                .and_then(Value::as_str)
                == Some("confirmed"),
        "Authoritative stop changed"
    );
    let acct_status = live
        .get("accounting")
        .and_then(|a| a.get("status"))
        .and_then(Value::as_str)
        .unwrap_or("");
    ensure!(
        acct_status == "unknown" || acct_status == "settled_zero",
        "Unknown verdict or accounting violation"
    );

    let cycles_dir = root.join("cycles");
    if cycles_dir.is_dir() {
        for entry in fs::read_dir(cycles_dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.chars().all(|c| c.is_ascii_digit()) {
                let seq: u64 = name.parse().unwrap_or(0);
                if seq >= next_sequence {
                    ensure!(
                        seq == next_sequence
                            && ![
                                "controller.json",
                                "operator-admit.json",
                                "operator-prepare.json",
                                "native-session.json"
                            ]
                            .iter()
                            .any(|n| entry.path().join(n).exists()),
                        "Resolution already consumed or superseded"
                    );
                }
            }
        }
    }

    ensure!(
        !root.join("PAUSE").exists(),
        "Explicit user pause remains authoritative"
    );

    Ok(json!({
        "kind": v["kind"],
        "owner_locks": ownership,
        "proof_sha256": sha(&proof)?,
        "authorization_sha256": sha(Path::new(auth_path))?,
        "original_sha256": sha(&original)?,
        "denial_history_sha256": sha(&history)?,
        "packaging_sha256": v["packaging_sha256"],
        "goal_id": out["goal_id"],
        "authorized_next_sequence": next_sequence,
        "prior_outcome": "failed",
        "prior_denial_preserved": true,
        "accounting": acct_status,
        "usage": out.get("usage"),
        "access_grants": [],
        "acceptance_granted": false,
    }))
}

fn regex_attempt_valid(attempt: &str) -> bool {
    let parts: Vec<&str> = attempt.split('-').collect();
    if parts.len() != 5 {
        return false;
    }
    parts[0].len() == 8
        && parts[1].len() == 4
        && parts[2].len() == 4
        && parts[3].len() == 4
        && parts[4].len() == 12
        && parts
            .iter()
            .all(|p| p.chars().all(|c| c.is_ascii_hexdigit()))
}

pub fn audit_owner_interruption(
    root: &Path,
    directory: &Path,
    controller: &Path,
    next_sequence: u64,
) -> Result<Value> {
    let exe = std::env::current_exe()?;
    audit_owner_interruption_with(root, directory, controller, next_sequence, &exe)
}

pub fn audit_owner_interruption_with(
    root: &Path,
    directory: &Path,
    controller: &Path,
    next_sequence: u64,
    supervisor_executable: &Path,
) -> Result<Value> {
    let root = resolve(root)?;
    let directory = resolve(directory)?;
    let supervisor_executable = resolve(supervisor_executable)?;
    let proof = regular(&root.join(INTERRUPTION_NAME))?;
    let v = read_json(&proof)?;

    ensure!(
        v.get("version").and_then(Value::as_i64) == Some(1)
            && v.get("kind").and_then(Value::as_str) == Some("owner-interruption"),
        "Unsupported interruption record"
    );
    ensure!(
        directory.parent() == Some(&root.join("cycles"))
            && directory
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.chars().all(|c| c.is_ascii_digit())),
        "Foreign interruption cycle"
    );

    let original = regular(&directory.join("result.json"))?;
    let out = read_json(&original)?;
    let seq_from_name: u64 = directory
        .file_name()
        .and_then(|n| n.to_str())
        .context("cycle name")?
        .parse()?;

    ensure!(
        out.get("sequence").and_then(Value::as_u64) == Some(seq_from_name)
            && v.get("sequence").and_then(Value::as_u64) == Some(seq_from_name)
            && next_sequence == seq_from_name + 1
            && v.get("original_sha256").and_then(Value::as_str) == Some(&sha(&original)?),
        "Stale interruption identity"
    );

    ensure!(
        out.get("status").and_then(Value::as_str) == Some("failed")
            && out.get("failure_kind").and_then(Value::as_str) == Some("blocked")
            && matches!(
                out.get("phase").and_then(Value::as_str),
                Some("native" | "native_shells" | "native_completion" | "native_sealing")
            )
            && out.get("error").and_then(Value::as_str) == Some("Error: Operator stopping"),
        "Not an owner-interrupted native attempt"
    );

    ensure!(
        out.get("cleanup_confirmed").and_then(Value::as_bool) == Some(true)
            && out.get("fence_error").is_none_or(Value::is_null)
            && out.get("candidate").is_none_or(Value::is_null)
            && out.get("acceptance").is_none_or(Value::is_null),
        "Unconfirmed or accepted interruption"
    );

    let id_keys = [
        "goal_id", "job_id", "attempt", "child", "parent", "worktree",
    ];
    let mut identity_map = serde_json::Map::new();
    for k in id_keys {
        let val = out.get(k).and_then(Value::as_str).unwrap_or("");
        ensure!(!val.is_empty(), "Interrupted execution identity mismatch");
        identity_map.insert(k.to_string(), json!(val));
    }
    ensure!(
        v.get("identity") == Some(&Value::Object(identity_map)),
        "Interrupted execution identity mismatch"
    );

    let history = regular(&directory.join("native-context-final.json"))?;
    ensure!(
        v.get("history_sha256").and_then(Value::as_str) == Some(&sha(&history)?),
        "Interrupted history changed"
    );

    let context = read_json(&history)?;
    let context_arr = context.as_array().context("context not array")?;
    ensure!(!context_arr.is_empty(), "Missing final native history");

    let mut calls = Vec::new();
    for m in context_arr {
        if let Some(c_arr) = m.get("content").and_then(Value::as_array) {
            for c in c_arr {
                if c.get("type").and_then(Value::as_str) == Some("tool") {
                    calls.push(c);
                }
            }
        }
    }
    ensure!(
        !calls.is_empty()
            && calls.iter().all(|c| {
                let state = c.get("state");
                state.and_then(|s| s.get("status")).and_then(Value::as_str) == Some("completed")
                    && state
                        .and_then(|s| s.get("error"))
                        .is_none_or(Value::is_null)
            }),
        "Native denial, incomplete tool or unknown tool failure remains fenced"
    );

    let manifest_path_val = v
        .get("manifest")
        .and_then(Value::as_str)
        .context("manifest path")?;
    let manifest_path = regular(Path::new(manifest_path_val))?;
    let manifest = read_json(&manifest_path)?;
    let state_path_val = manifest
        .get("state")
        .and_then(Value::as_str)
        .context("manifest state")?;
    let state = read_json(&regular(Path::new(state_path_val))?)?;

    ensure!(
        state.get("manifest_digest").and_then(Value::as_str) == Some(&digest(&manifest)),
        "Owner manifest not reconciled"
    );

    let pool_lock_val = manifest
        .get("pool_lock")
        .and_then(Value::as_str)
        .context("pool lock")?;
    let pool_ownership = owner_lock(Path::new(pool_lock_val))?;
    let supervisor_ownership = owner_lock(&root.join("supervisor.lock"))?;
    let locks = json!([pool_ownership, supervisor_ownership]);

    let current = state.get("current");
    if let Some(curr) = current
        && !curr.is_null()
    {
        ensure!(
            curr.get("goal") == out.get("goal_id")
                && curr.get("expected_sequence").and_then(Value::as_u64) == Some(next_sequence),
            "Another active goal or sequence"
        );
    }

    let goals_arr = manifest
        .get("goals")
        .and_then(Value::as_array)
        .context("manifest goals")?;
    ensure!(
        goals_arr.iter().any(|g| {
            g.get("id") == out.get("goal_id")
                && g.get("root")
                    .and_then(Value::as_str)
                    .is_some_and(|r| resolve(Path::new(r)).is_ok_and(|rp| rp == root))
        }),
        "Manifest goal mismatch"
    );

    let pins = manifest
        .get("trusted_files")
        .and_then(Value::as_object)
        .context("trusted_files")?;
    let proof_str = proof.to_string_lossy().to_string();
    ensure!(
        pins.get(&proof_str).and_then(Value::as_str) == Some(&sha(&proof)?),
        "Interruption is not owner-pinned"
    );

    let mut without = manifest.clone();
    if let Some(tf) = without
        .get_mut("trusted_files")
        .and_then(Value::as_object_mut)
    {
        tf.remove(&proof_str);
    }
    ensure!(
        v.get("manifest_without_interruption_sha256")
            .and_then(Value::as_str)
            == Some(&digest(&without)),
        "Owner manifest changed"
    );

    let pinned = |p: &Path| -> Result<Value> {
        let p_reg = regular(p)?;
        let p_str = p_reg.to_string_lossy().to_string();
        let expected_hash = pins
            .get(&p_str)
            .and_then(Value::as_str)
            .context("unpinned file")?;
        ensure!(
            sha(&p_reg)? == expected_hash,
            "Unpinned interruption input: {}",
            p.display()
        );
        read_json(&p_reg)
    };

    let self_exe = regular(&supervisor_executable)?;
    let self_exe_str = self_exe.to_string_lossy().to_string();
    let exe_pin = pins
        .get(&self_exe_str)
        .and_then(Value::as_str)
        .context("Unpinned interruption implementation: native executable not pinned")?;
    ensure!(
        sha(&self_exe)? == exe_pin,
        "Unpinned interruption implementation"
    );

    let cfg = pinned(&root.join("supervisor.json"))?;
    let cfg_controller = cfg.get("controller").and_then(Value::as_str).unwrap_or("");
    let cfg_ctrl_resolved = resolve(Path::new(cfg_controller))?;
    let target_ctrl_resolved = resolve(controller)?;
    ensure!(
        cfg_ctrl_resolved == target_ctrl_resolved
            && cfg.get("controller_sha256").and_then(Value::as_str) == Some(&sha(controller)?),
        "Controller identity changed"
    );

    let owner = pinned(Path::new(
        v.get("owner_stop")
            .and_then(Value::as_str)
            .context("owner_stop")?,
    ))?;
    let execution = pinned(Path::new(
        v.get("owned_receipt")
            .and_then(Value::as_str)
            .context("owned_receipt")?,
    ))?;
    let plan = pinned(Path::new(
        v.get("owned_plan")
            .and_then(Value::as_str)
            .context("owned_plan")?,
    ))?;

    ensure!(
        owner.get("action").and_then(Value::as_str)
            == Some("SIGTERM exact owned foreground dispatcher")
            && owner.get("new_permission_denial").and_then(Value::as_bool) == Some(false),
        "Stop was not an explicit owner interruption"
    );
    ensure!(
        execution.get("observer").and_then(Value::as_str) == Some("linux_subreaper_waitpid_pidfd")
            && execution.get("execution_id") == plan.get("execution_id")
            && execution.get("quiescent").and_then(Value::as_bool) == Some(true)
            && execution.get("leaked_descendants").and_then(Value::as_bool) == Some(false)
            && execution.get("cleanup_error").is_none_or(Value::is_null)
            && execution
                .get("root_exit_status")
                .is_some_and(|s| s.is_i64() || s.is_u64()),
        "Owned process cleanup unconfirmed"
    );

    let stopped = owner.get("owner").context("stopped owner")?;
    let supervisor = execution
        .get("supervisor")
        .context("execution supervisor")?;
    let stopped_pid = stopped.get("pid").and_then(Value::as_i64);
    let root_pid = execution.get("root_pid").and_then(Value::as_i64);
    let stopped_ticks = stopped.get("start_ticks").and_then(Value::as_i64);
    let sup_ticks = supervisor.get("start_ticks").and_then(Value::as_i64);
    let stopped_uid = stopped.get("uid").and_then(Value::as_u64);
    let my_uid = unsafe { libc::getuid() as u64 };
    let plan_argv = plan
        .get("argv")
        .and_then(Value::as_array)
        .context("plan argv")?;

    ensure!(
        stopped_pid.is_some()
            && stopped_pid == root_pid
            && stopped_ticks.is_some()
            && sup_ticks.is_some()
            && stopped_ticks.unwrap() >= sup_ticks.unwrap()
            && stopped_uid == Some(my_uid)
            && stopped.get("boot_id") == supervisor.get("boot_id")
            && plan_argv
                .iter()
                .any(|a| a.as_str() == Some(manifest_path_val)),
        "Stopped owner/receipt identity mismatch"
    );

    let pause_sha = v.get("pause_sha256").and_then(Value::as_str).unwrap_or("");
    ensure!(pause_sha.len() == 64, "Missing exact owner pause identity");
    let pause = root.join("PAUSE");
    if pause.exists() {
        ensure!(
            sha(&regular(&pause)?)? == pause_sha,
            "Different explicit pause remains authoritative"
        );
    }

    let cfg_ctrl_path = directory.join("controller.json");
    let job_id = out.get("job_id").and_then(Value::as_str).unwrap_or("");
    let output = supervised_command(
        &supervisor_executable,
        controller,
        &[
            "inspect",
            "--config",
            cfg_ctrl_path.to_str().unwrap(),
            "--job",
            job_id,
        ],
        &directory,
        40,
        0,
    )?;
    let observed: Value = serde_json::from_slice(&output.stdout)?;
    let live = observed
        .get("execution_recovery")
        .cloned()
        .unwrap_or(json!({}));
    ensure!(
        observed.get("state").and_then(Value::as_str) == Some("blocked")
            && live.get("stop_id") == v.get("stop_id")
            && live.get("stop_id") == out.get("recovery").and_then(|r| r.get("stop_id"))
            && live.get("attempt_id") == out.get("attempt")
            && live.get("session_id") == out.get("child")
            && live
                .get("execution_stop")
                .and_then(|e| e.get("status"))
                .and_then(Value::as_str)
                == Some("confirmed"),
        "Authoritative interruption stop changed"
    );
    let acct_status = live
        .get("accounting")
        .and_then(|a| a.get("status"))
        .and_then(Value::as_str)
        .unwrap_or("");
    ensure!(
        acct_status == "unknown" || acct_status == "settled_zero",
        "Unknown accounting verdict or known policy violation"
    );

    let cycles_dir = root.join("cycles");
    if cycles_dir.is_dir() {
        for entry in fs::read_dir(cycles_dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.chars().all(|c| c.is_ascii_digit()) {
                let seq: u64 = name.parse().unwrap_or(0);
                if seq >= next_sequence {
                    ensure!(
                        seq == next_sequence
                            && ![
                                "controller.json",
                                "operator-admit.json",
                                "operator-prepare.json",
                                "native-session.json"
                            ]
                            .iter()
                            .any(|n| entry.path().join(n).exists()),
                        "Interruption record already consumed or superseded"
                    );
                }
            }
        }
    }

    Ok(json!({
        "kind": "owner-interruption",
        "owner_locks": locks,
        "proof_sha256": sha(&proof)?,
        "original_sha256": sha(&original)?,
        "history_sha256": sha(&history)?,
        "goal_id": out["goal_id"],
        "authorized_next_sequence": next_sequence,
        "prior_outcome": "failed",
        "prior_history_preserved": true,
        "accounting": acct_status,
        "usage": out.get("usage"),
        "access_grants": [],
        "acceptance_granted": false,
    }))
}

const EMPTY_TABLES: [&str; 7] = [
    "jobs", "attempts", "grants", "requests", "receipts", "events", "steps",
];
const ADMISSION_FIELDS: [&str; 9] = [
    "attempt",
    "session",
    "child",
    "parent",
    "worktree",
    "candidate",
    "checkpoint",
    "native_session",
    "acceptance",
];

pub fn pre_admission_absence(root: &Path, directory: &Path, controller: &Path) -> Result<Value> {
    let exe = resolve(&std::env::current_exe().context("resolving current executable")?)?;
    pre_admission_absence_with(root, directory, controller, &exe)
}

pub fn pre_admission_absence_with(
    root: &Path,
    directory: &Path,
    controller: &Path,
    supervisor_executable: &Path,
) -> Result<Value> {
    let root = resolve(root)?;
    let directory = resolve(directory)?;
    ensure!(
        directory.parent() == Some(&root.join("cycles")),
        "Exact cycle directory required"
    );

    let result_file = directory.join("result.json");
    let out = read_json(&result_file)?;
    let goal = read_json(&root.join("goal.json"))?;
    let cfg = read_json(&directory.join("controller.json"))?;
    let supervisor = read_json(&root.join("supervisor.json"))?;

    let seq_num = out.get("sequence").and_then(Value::as_u64).context("seq")?;
    let seq_str = format!("{seq_num:06}");
    ensure!(
        out.get("goal_id") == goal.get("id")
            && directory.file_name().and_then(|n| n.to_str()) == Some(&seq_str)
            && out.get("status").and_then(Value::as_str) == Some("failed")
            && out.get("phase").and_then(Value::as_str) == Some("admission")
            && out.get("cleanup_confirmed").and_then(Value::as_bool) == Some(true)
            && ADMISSION_FIELDS
                .iter()
                .all(|k| out.get(*k).is_none_or(Value::is_null)),
        "Not an owned failure before worker preparation"
    );

    let err_str = out.get("error").and_then(Value::as_str).unwrap_or("");
    ensure!(
        err_str
            == "Error: Project instructions exceed admission budget; no instructions were truncated"
            || err_str
                == "Error: Required goal and outcome context exceeds admission budget; no context was truncated",
        "Failure is outside the approved pre-admission recovery class"
    );

    let fence_err = out.get("fence_error");
    ensure!(
        fence_err.is_none_or(|fe| fe.is_null()
            || fe.as_str() == Some("Error: Controller cancel: {\"error\":\"job not found\"}")),
        "Unrecognized fence remains unresolved"
    );

    ensure!(
        out.get("job_id").is_some_and(|j| !j.is_null())
            && out.get("ended_utc").is_some_and(|e| !e.is_null()),
        "Reserved identifier/terminal record missing"
    );

    let cfg_data_dir = resolve(Path::new(
        cfg.get("data_dir").and_then(Value::as_str).unwrap_or(""),
    ));
    let cfg_repo = resolve(Path::new(
        cfg.get("repository").and_then(Value::as_str).unwrap_or(""),
    ));
    let sup_ctrl = resolve(Path::new(
        supervisor
            .get("controller")
            .and_then(Value::as_str)
            .unwrap_or(""),
    ));
    let target_ctrl = resolve(controller)?;

    ensure!(
        cfg.get("project_id") == goal.get("id")
            && cfg_data_dir.is_ok()
            && cfg_data_dir.unwrap() == directory.join("controller")
            && cfg_repo.is_ok()
            && cfg_repo.unwrap() == root.join("repository")
            && sup_ctrl.is_ok()
            && sup_ctrl.unwrap() == target_ctrl
            && sha(controller)?
                == supervisor
                    .get("controller_sha256")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
        "Controller/goal identity changed"
    );

    for marker in [
        "operator-admit.json",
        "operator-prepare.json",
        "operator-bind-v2.json",
        "host-private.json",
        "native-session.json",
    ] {
        ensure!(
            !directory.join(marker).exists(),
            "Admission/session evidence exists; use native execution recovery"
        );
    }

    let lock_path = directory.join("controller/controller.lock");
    let db_path = directory.join("controller/controller.sqlite");
    ensure!(
        !lock_path.is_symlink()
            && !db_path.is_symlink()
            && lock_path.is_file()
            && db_path.is_file(),
        "Original controller ledger/lock missing or redirected"
    );

    {
        use std::os::unix::fs::OpenOptionsExt;
        let lock_file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&lock_path)?;
        let flock_res =
            unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        ensure!(flock_res == 0, "Controller lock currently held");
    }

    let cfg_ctrl_path = directory.join("controller.json");
    let job_id = out.get("job_id").and_then(Value::as_str).unwrap_or("");
    let inspect_out = supervised_command(
        supervisor_executable,
        controller,
        &[
            "inspect",
            "--config",
            cfg_ctrl_path.to_str().unwrap(),
            "--job",
            job_id,
        ],
        &directory,
        30,
        1,
    )
    .map_err(|e| anyhow::anyhow!("Controller did not authoritatively confirm absent job: {e:#}"))?;
    let stderr_str = String::from_utf8_lossy(&inspect_out.stderr);
    let stdout_str = String::from_utf8_lossy(&inspect_out.stdout);
    ensure!(
        inspect_out.exit_status == 1
            && stdout_str.trim().is_empty()
            && stderr_str.trim()
                == "Error: job not found\n\nCaused by:\n    Query returned no rows",
        "Controller did not authoritatively confirm absent job"
    );

    let mut counts = serde_json::Map::new();
    {
        use std::os::unix::fs::OpenOptionsExt;
        ensure!(
            !lock_path.is_symlink()
                && !db_path.is_symlink()
                && lock_path.is_file()
                && db_path.is_file(),
            "Original controller ledger/lock missing or redirected"
        );
        let _lock_file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&lock_path)?;
        let flock_res =
            unsafe { libc::flock(_lock_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        ensure!(flock_res == 0, "Controller lock currently held");

        let sqlite_conn = rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        sqlite_conn.execute_batch("BEGIN")?;
        for table in EMPTY_TABLES {
            let count: i64 =
                sqlite_conn
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?;
            ensure!(
                count == 0,
                "Controller records a job, attempt, grant, request or execution effect"
            );
            counts.insert(table.to_string(), json!(count));
        }
    }

    Ok(json!({
        "kind": "not_admitted",
        "goal_id": goal["id"],
        "sequence": out["sequence"],
        "job_id": out["job_id"],
        "original_result_sha256": sha(&result_file)?,
        "controller_config_sha256": sha(&directory.join("controller.json"))?,
        "controller_sha256": sha(controller)?,
        "ledger_counts": Value::Object(counts),
        "worker_session_created": false,
    }))
}

pub fn pre_admission_context_rejection(root: &Path, directory: &Path) -> Result<Value> {
    let root = resolve(root)?;
    let directory = resolve(directory)?;
    let out = read_json(&directory.join("result.json"))?;
    let goal = read_json(&root.join("goal.json"))?;

    ensure!(
        directory.parent() == Some(&root.join("cycles"))
            && out.get("goal_id") == goal.get("id")
            && out.get("phase").and_then(Value::as_str) == Some("context_preflight")
            && out.get("status").and_then(Value::as_str) == Some("failed")
            && out.get("cleanup_confirmed").and_then(Value::as_bool) == Some(true)
            && out.get("admission_requested").and_then(Value::as_bool) == Some(false)
            && out.get("ended_utc").is_some_and(|e| !e.is_null())
            && out.get("job_id").is_none_or(Value::is_null)
            && out.get("fence_error").is_none_or(Value::is_null)
            && ADMISSION_FIELDS
                .iter()
                .all(|k| out.get(*k).is_none_or(Value::is_null)),
        "Not a completed context rejection before admission"
    );

    for name in [
        "controller.json",
        "host-private.json",
        "operator-admit.json",
        "operator-prepare.json",
        "native-session.json",
    ] {
        ensure!(
            !directory.join(name).exists(),
            "Controller/session evidence exists"
        );
    }

    Ok(json!({
        "kind": "not_admitted",
        "sequence": out["sequence"],
        "goal_id": out["goal_id"],
        "worker_session_created": false,
        "original_result_sha256": sha(&directory.join("result.json"))?,
    }))
}

pub fn pre_admission_owner_receipt(path: &Path, manifest_path: &Path) -> Result<Value> {
    let path = resolve(path)?;
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .context("receipt name")?;
    let plan_name = file_name.replace(".process.json", ".plan.json");
    let plan_path = path.with_file_name(plan_name);

    let receipt = read_json(&path)?;
    let plan = read_json(&plan_path)?;

    ensure!(
        receipt.get("observer").and_then(Value::as_str) == Some("linux_subreaper_waitpid_pidfd")
            && receipt.get("quiescent").and_then(Value::as_bool) == Some(true)
            && receipt
                .get("root_exit_status")
                .is_some_and(|s| s.is_i64() || s.is_u64())
            && receipt.get("leaked_descendants").and_then(Value::as_bool) == Some(false)
            && receipt.get("cleanup_error").is_none_or(Value::is_null)
            && receipt.get("execution_id") == plan.get("execution_id")
            && resolve(Path::new(
                plan.get("receipt").and_then(Value::as_str).unwrap_or("")
            ))? == path,
        "Positive owned-process quiescence required"
    );

    let argv = plan
        .get("argv")
        .and_then(Value::as_array)
        .context("argv not array")?;
    let manifest_idx = argv
        .iter()
        .position(|a| a.as_str() == Some("--manifest"))
        .context("--manifest missing")?;
    let argv_manifest = argv
        .get(manifest_idx + 1)
        .and_then(Value::as_str)
        .context("manifest arg missing")?;
    ensure!(
        resolve(Path::new(argv_manifest))? == resolve(manifest_path)?,
        "Owned execution belongs to another manifest"
    );

    Ok(json!({
        "path": path.to_string_lossy(),
        "sha256": sha(&path)?,
        "plan": plan_path.to_string_lossy(),
        "plan_sha256": sha(&plan_path)?,
        "execution_id": receipt["execution_id"],
    }))
}

pub fn audit_pre_admission(root: &Path, directory: &Path, controller: &Path) -> Result<Value> {
    let exe = resolve(&std::env::current_exe().context("resolving current executable")?)?;
    audit_pre_admission_with(root, directory, controller, &exe)
}

pub fn audit_pre_admission_with(
    root: &Path,
    directory: &Path,
    controller: &Path,
    supervisor_executable: &Path,
) -> Result<Value> {
    let directory = Path::new(directory);
    let res = read_json(&directory.join("result.json"))?;
    if res.get("phase").and_then(Value::as_str) == Some("context_preflight") {
        return pre_admission_context_rejection(root, directory);
    }
    let att = read_json(&directory.join("pre-admission.json"))?;
    let fresh = pre_admission_absence_with(root, directory, controller, supervisor_executable)?;
    ensure!(
        att.get("absence") == Some(&fresh),
        "Pre-admission evidence changed"
    );
    let owned = att.get("owned").context("missing owned in att")?;
    let owned_path = owned
        .get("path")
        .and_then(Value::as_str)
        .context("owned path")?;
    let att_manifest = att
        .get("manifest")
        .and_then(Value::as_str)
        .context("att manifest")?;
    let observed = pre_admission_owner_receipt(Path::new(owned_path), Path::new(att_manifest))?;
    ensure!(observed == *owned, "Pre-admission owned receipt changed");
    Ok(fresh)
}

pub fn reconcile_pre_admission(
    manifest_path: &Path,
    goal_id: &str,
    owned_path: &Path,
) -> Result<Value> {
    let exe = resolve(&std::env::current_exe().context("resolving current executable")?)?;
    reconcile_pre_admission_with(manifest_path, goal_id, owned_path, &exe)
}

pub fn reconcile_pre_admission_with(
    manifest_path: &Path,
    goal_id: &str,
    owned_path: &Path,
    supervisor_executable: &Path,
) -> Result<Value> {
    let manifest_path = resolve(manifest_path)?;
    let m = read_json(&manifest_path)?;
    let pins = m
        .get("trusted_files")
        .and_then(Value::as_object)
        .context("trusted_files")?;
    for (p, h) in pins {
        ensure!(
            sha(Path::new(p))? == h.as_str().unwrap_or(""),
            "Trusted runtime/configuration pin changed"
        );
    }

    let goals_arr = m.get("goals").and_then(Value::as_array).context("goals")?;
    let goal = goals_arr
        .iter()
        .find(|g| g.get("id").and_then(Value::as_str) == Some(goal_id))
        .context("goal not found in manifest")?;
    let root = resolve(Path::new(
        goal.get("root").and_then(Value::as_str).unwrap_or(""),
    ))?;
    let state_path = resolve(Path::new(
        m.get("state").and_then(Value::as_str).unwrap_or(""),
    ))?;
    let owned = pre_admission_owner_receipt(owned_path, &manifest_path)?;

    let pool_lock = Path::new(
        m.get("pool_lock")
            .and_then(Value::as_str)
            .context("pool_lock")?,
    );
    let _l1 = goal_state::Lock::acquire(pool_lock, false)?;
    let _l2 = goal_state::Lock::acquire(&root.join("supervisor.lock"), false)?;

    let journal = root.join("pre-admission-journal.json");
    if journal.exists() {
        let old_raw = RawJson::read(&journal)?;
        if let Some(raw_files) = old_raw.get("raw_files") {
            if let Some(s) = raw_files.get("status").and_then(RawJson::as_str) {
                write_bytes_atomic(&root.join("status.json"), s.as_bytes())?;
            } else if let Some(s) = old_raw.get("status") {
                save_raw(&root.join("status.json"), s)?;
            }
            if let Some(st) = raw_files.get("state").and_then(RawJson::as_str) {
                write_bytes_atomic(&state_path, st.as_bytes())?;
            } else if let Some(st) = old_raw.get("state") {
                save_raw(&state_path, st)?;
            }
        } else {
            let status_old = old_raw.get("status").context("journal status")?;
            let state_old = old_raw.get("state").context("journal state")?;
            save_raw(&root.join("status.json"), status_old)?;
            save_raw(&state_path, state_old)?;
        }
        for (p, h) in pins {
            ensure!(
                sha(Path::new(p))? == h.as_str().unwrap_or(""),
                "Restored reconciliation pin mismatch"
            );
        }
        fs::remove_file(&journal)?;
        bail!("Interrupted reconciliation restored; audit before retry");
    }

    let status_text = fs::read_to_string(root.join("status.json"))?;
    let state_text = fs::read_to_string(&state_path)?;
    let mut status_raw = RawJson::parse(status_text.as_bytes())?;
    let mut state_raw = RawJson::parse(state_text.as_bytes())?;
    let status = status_raw.to_value();
    let state = state_raw.to_value();
    let current = state.get("current").cloned().unwrap_or(json!({}));

    ensure!(
        !root.join("PAUSE").exists()
            && current.get("goal").and_then(Value::as_str) == Some(goal_id)
            && current.get("expected_sequence") == status.get("sequence"),
        "Exact stopped cycle required; user pause remains authoritative"
    );

    let status_seq = status
        .get("sequence")
        .and_then(Value::as_u64)
        .context("status sequence")?;
    let directory = root.join("cycles").join(format!("{status_seq:06}"));
    let controller = Path::new(
        m.get("controller")
            .and_then(Value::as_str)
            .context("controller")?,
    );
    let proof = pre_admission_absence_with(&root, &directory, controller, supervisor_executable)?;
    let out = read_json(&directory.join("result.json"))?;
    let expected_err = format!(
        "Execution/recovery evidence incomplete: {}",
        out.get("error").and_then(Value::as_str).unwrap_or("")
    );

    ensure!(
        status.get("status").and_then(Value::as_str) == Some("paused")
            && status.get("pause_reason").and_then(Value::as_str) == Some(&expected_err),
        "Unrelated/native permission pause cannot be cleared"
    );

    let att = RawJson::Object(vec![
        ("version".into(), RawJson::Number(1.into())),
        ("absence".into(), RawJson::from_value(&proof)),
        ("owned".into(), RawJson::from_value(&owned)),
        (
            "manifest".into(),
            RawJson::String(manifest_path.to_string_lossy().into_owned()),
        ),
    ]);
    save_raw(&directory.join("pre-admission.json"), &att)?;

    let before = RawJson::Object(vec![
        ("status".into(), status_raw.clone()),
        ("state".into(), state_raw.clone()),
        (
            "raw_files".into(),
            RawJson::Object(vec![
                ("status".into(), RawJson::String(status_text)),
                ("state".into(), RawJson::String(state_text)),
            ]),
        ),
    ]);
    save_raw(&journal, &before)?;

    let out_raw = RawJson::read(&directory.join("result.json"))?;
    let seq_raw = out_raw.get("sequence").cloned().unwrap_or(RawJson::Null);
    status_raw.set("status", RawJson::String("waiting".into()))?;
    status_raw.set("current", RawJson::Null)?;
    status_raw.set(
        "last_cycle",
        RawJson::Object(vec![
            ("sequence".into(), seq_raw.clone()),
            ("status".into(), RawJson::String("failed".into())),
            (
                "result".into(),
                RawJson::String(directory.join("result.json").to_string_lossy().into_owned()),
            ),
        ]),
    )?;
    status_raw.remove("pause_reason");
    let rec_entry = RawJson::Object(vec![
        ("sequence".into(), seq_raw),
        (
            "result_sha256".into(),
            RawJson::from_value(&proof["original_result_sha256"]),
        ),
        ("kind".into(), RawJson::String("not_admitted".into())),
    ]);
    if let Some(RawJson::Array(arr)) = status_raw.get_mut("pre_admission_reconciliations") {
        arr.push(rec_entry);
    } else {
        status_raw.set(
            "pre_admission_reconciliations",
            RawJson::Array(vec![rec_entry]),
        )?;
    }

    state_raw.set("current", RawJson::Null)?;
    state_raw.set("status", RawJson::String("waiting".into()))?;
    state_raw.remove("reason");
    let gs = state_raw
        .get_mut("goals")
        .and_then(|g| g.get_mut(goal_id))
        .context("goal state in state.json")?;
    gs.set("state", RawJson::String("ready".into()))?;
    gs.set(
        "last_result",
        RawJson::String(directory.join("result.json").to_string_lossy().into_owned()),
    )?;
    gs.set("native_status", RawJson::String("not_admitted".into()))?;
    gs.set(
        "usage",
        out_raw.get("usage").cloned().unwrap_or(RawJson::Null),
    )?;
    gs.set(
        "model",
        out_raw.get("model").cloned().unwrap_or(RawJson::Null),
    )?;
    gs.remove("reason");

    save_raw(&root.join("status.json"), &status_raw)?;
    save_raw(&state_path, &state_raw)?;
    fs::remove_file(&journal)?;

    Ok(proof)
}

pub fn advance_scope_base(
    manifest_path: &Path,
    goal_id: &str,
    published_path: &Path,
    new_base: &str,
) -> Result<Value> {
    let exe = resolve(&std::env::current_exe().context("resolving current executable")?)?;
    advance_scope_base_with(manifest_path, goal_id, published_path, new_base, &exe)
}

pub fn advance_scope_base_with(
    manifest_path: &Path,
    goal_id: &str,
    published_path: &Path,
    new_base: &str,
    supervisor_executable: &Path,
) -> Result<Value> {
    let mp = resolve(manifest_path)?;
    let manifest_text = fs::read_to_string(&mp)?;
    let m_raw = RawJson::parse(manifest_text.as_bytes())?;
    let m = m_raw.to_value();
    let goals_arr = m
        .get("goals")
        .and_then(Value::as_array)
        .context("manifest goals")?;
    let goal = goals_arr
        .iter()
        .find(|g| g.get("id").and_then(Value::as_str) == Some(goal_id))
        .context("goal not found")?;
    let root = resolve(Path::new(
        goal.get("root").and_then(Value::as_str).unwrap_or(""),
    ))?;
    let repo = root.join("repository");
    let published_raw = RawJson::read(published_path)?;
    let published = published_raw.to_value();
    let base = published
        .get("intent")
        .and_then(|i| i.get("commit"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let binding = published.get("binding").context("binding missing")?;

    ensure!(
        binding.get("repository").and_then(Value::as_str)
            == Some("lhemerly/code-factory-controller")
            && binding.get("repository_id").and_then(Value::as_i64) == Some(1414818646)
            && binding.get("private").and_then(Value::as_bool) == Some(true)
            && binding.get("branch").and_then(Value::as_str) == Some("main")
            && binding.get("required_checks") == Some(&json!(["rust-and-contract"]))
            && published
                .get("intent")
                .and_then(|i| i.get("state"))
                .and_then(Value::as_str)
                == Some("confirmed")
            && published
                .get("intent")
                .and_then(|i| i.get("ci"))
                .and_then(|c| c.get("status"))
                .and_then(Value::as_str)
                == Some("passed"),
        "Exact verified private-main publication required"
    );

    for oid in [base, new_base] {
        ensure!(
            oid.len() == 40
                && oid
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "Exact source commits required"
        );
    }

    let pool_lock = Path::new(
        m.get("pool_lock")
            .and_then(Value::as_str)
            .context("pool_lock")?,
    );
    let _l1 = goal_state::Lock::acquire(pool_lock, false)?;
    let _l2 = goal_state::Lock::acquire(&root.join("supervisor.lock"), false)?;

    let state_path = resolve(Path::new(
        m.get("state").and_then(Value::as_str).unwrap_or(""),
    ))?;
    let goal_raw = RawJson::read(&root.join("goal.json"))?;
    let fingerprint = |c: &RawJson| -> String {
        let obj = RawJson::Object(vec![
            ("cfg".to_string(), c.clone()),
            ("goal".to_string(), goal_raw.clone()),
        ]);
        let s = raw_json_compact_utf8(&obj);
        format!("{:x}", Sha256::digest(s.as_bytes()))
    };

    let journal = root.join("scope-base-journal.json");
    if journal.exists() {
        let old_raw = RawJson::read(&journal)?;
        let old = old_raw.to_value();
        if resolve(Path::new(
            old.get("manifest")
                .and_then(|m| m.get("state"))
                .and_then(Value::as_str)
                .unwrap_or(""),
        ))? != state_path
        {
            bail!("Baseline journal belongs to another owner");
        }
        if let Some(files) = old_raw.get("files") {
            for name in [
                "verified-scope-publication.json",
                "approved-policy-migration.json",
            ] {
                let target = root.join(name);
                if let Some(val) = files.get(name) {
                    if val.is_null() {
                        if target.exists() {
                            fs::remove_file(&target)?;
                            File::open(&root)?.sync_all()?;
                        }
                    } else if let Some(txt) = val.as_str() {
                        write_bytes_atomic(&target, txt.as_bytes())?;
                    }
                }
            }
        }
        let raw_files = old_raw.get("raw_files");
        if let Some(txt) = raw_files
            .and_then(|rf| rf.get("configuration"))
            .and_then(RawJson::as_str)
        {
            write_bytes_atomic(&root.join("supervisor.json"), txt.as_bytes())?;
        } else {
            let cfg_old = old_raw
                .get("configuration")
                .context("journal configuration")?;
            save_raw(&root.join("supervisor.json"), cfg_old)?;
        }
        if let Some(txt) = raw_files
            .and_then(|rf| rf.get("status"))
            .and_then(RawJson::as_str)
        {
            write_bytes_atomic(&root.join("status.json"), txt.as_bytes())?;
        } else {
            let status_old = old_raw.get("status").context("journal status")?;
            save_raw(&root.join("status.json"), status_old)?;
        }
        if let Some(txt) = raw_files
            .and_then(|rf| rf.get("state"))
            .and_then(RawJson::as_str)
        {
            write_bytes_atomic(&state_path, txt.as_bytes())?;
        } else {
            let state_old = old_raw.get("state").context("journal state")?;
            save_raw(&state_path, state_old)?;
        }
        if let Some(txt) = raw_files
            .and_then(|rf| rf.get("manifest"))
            .and_then(RawJson::as_str)
        {
            write_bytes_atomic(&mp, txt.as_bytes())?;
        } else {
            let manifest_old = old_raw.get("manifest").context("journal manifest")?;
            save_raw(&mp, manifest_old)?;
        }

        // Verify all old trusted_files pins and policy fingerprint match before removing journal.
        // Fail closed and retain journal on any unresolvable mismatch.
        let restored_manifest = read_json(&mp)?;
        let old_pins = restored_manifest
            .get("trusted_files")
            .and_then(Value::as_object)
            .context("restored trusted_files")?;
        for (p, h) in old_pins {
            ensure!(
                sha(Path::new(p))? == h.as_str().unwrap_or(""),
                "Restored baseline pin mismatch for {p}; journal retained"
            );
        }
        let restored_cfg = RawJson::read(&root.join("supervisor.json"))?;
        let restored_status = read_json(&root.join("status.json"))?;
        let restored_policy_hash = restored_status
            .get("policy_hash")
            .and_then(Value::as_str)
            .unwrap_or("");
        let restored_fp = fingerprint(&restored_cfg);
        if restored_policy_hash != restored_fp {
            let migration_path = root.join("approved-policy-migration.json");
            ensure!(
                migration_path.exists(),
                "Restored configuration policy fingerprint mismatch; journal retained"
            );
            let pending_raw = RawJson::read(&migration_path)?;
            let prior_cfg = pending_raw
                .get("prior_configuration")
                .context("restored migration prior_configuration")?;
            ensure!(
                pending_raw.get("from").and_then(RawJson::as_str) == Some(restored_policy_hash)
                    && fingerprint(prior_cfg) == restored_policy_hash
                    && pending_raw.get("to").and_then(RawJson::as_str)
                        == Some(restored_fp.as_str()),
                "Restored configuration policy fingerprint mismatch; journal retained"
            );
        }

        fs::remove_file(&journal)?;
        File::open(&root)?.sync_all()?;
        bail!("Interrupted baseline transaction restored; audit before retry");
    }

    let pins = m
        .get("trusted_files")
        .and_then(Value::as_object)
        .context("trusted_files")?;
    for (p, h) in pins {
        ensure!(
            sha(Path::new(p))? == h.as_str().unwrap_or(""),
            "Existing trusted pin changed"
        );
    }

    let status_text = fs::read_to_string(root.join("status.json"))?;
    let state_text = fs::read_to_string(&state_path)?;
    let cfg_text = fs::read_to_string(root.join("supervisor.json"))?;
    let mut status_raw = RawJson::parse(status_text.as_bytes())?;
    let mut state_raw = RawJson::parse(state_text.as_bytes())?;
    let cfg_raw = RawJson::parse(cfg_text.as_bytes())?;
    let status = status_raw.to_value();
    let state = state_raw.to_value();
    let cfg = cfg_raw.to_value();

    ensure!(
        status.get("current").is_none_or(Value::is_null)
            && state.get("current").is_none_or(Value::is_null)
            && !root.join("PAUSE").exists(),
        "Quiescent reconciled owner required"
    );

    let git_bin = if Path::new("/usr/bin/git").exists() {
        Path::new("/usr/bin/git")
    } else {
        Path::new("git")
    };
    let git_ancestor = |anc: &str, desc: &str| -> Result<()> {
        supervised_command(
            supervisor_executable,
            git_bin,
            &["merge-base", "--is-ancestor", anc, desc],
            &repo,
            30,
            0,
        )?;
        Ok(())
    };
    git_ancestor(base, new_base)?;
    let status_base = status
        .get("base")
        .and_then(Value::as_str)
        .context("status base")?;
    git_ancestor(status_base, new_base)?;

    let template = read_json(&root.join("job-template.json"))?;
    let allowed_paths: Vec<&str> = template
        .get("allowed_paths")
        .and_then(Value::as_array)
        .map_or(Vec::new(), |a| a.iter().filter_map(Value::as_str).collect());
    let protected_paths: Vec<&str> = template
        .get("protected_paths")
        .and_then(Value::as_array)
        .map_or(Vec::new(), |a| a.iter().filter_map(Value::as_str).collect());

    let diff_out = supervised_command(
        supervisor_executable,
        git_bin,
        &["diff", "--name-only", base, new_base],
        &repo,
        30,
        0,
    )?;
    let diff_paths = String::from_utf8_lossy(&diff_out.stdout);
    for p in diff_paths.lines() {
        if p.trim().is_empty() {
            continue;
        }
        let in_allowed = allowed_paths
            .iter()
            .any(|a| p == *a || p.starts_with(&format!("{a}/")));
        let in_protected = protected_paths
            .iter()
            .any(|a| p == *a || p.starts_with(&format!("{a}/")));
        ensure!(
            in_allowed && !in_protected,
            "Verified source advance would widen repair scope"
        );
    }

    let mut prior_raw = cfg_raw.clone();
    let mut superseded_raw: Option<RawJson> = None;
    let status_policy_hash = status
        .get("policy_hash")
        .and_then(Value::as_str)
        .context("policy_hash")?;
    if status_policy_hash != fingerprint(&cfg_raw) {
        let pending_raw = RawJson::read(&root.join("approved-policy-migration.json"))?;
        let previous_raw = RawJson::read(&root.join("verified-scope-publication.json"))?;
        let pending = pending_raw.to_value();
        let previous = previous_raw.to_value();
        prior_raw = pending_raw
            .get("prior_configuration")
            .cloned()
            .context("prior_configuration")?;
        let prior_val = prior_raw.to_value();
        let prev_binding = previous.get("binding").context("prev binding")?;

        let mut prior_diff = prior_val.as_object().cloned().unwrap_or_default();
        prior_diff.remove("scope_base");
        let mut cfg_diff = cfg.as_object().cloned().unwrap_or_default();
        cfg_diff.remove("scope_base");

        ensure!(
            pending.get("kind").and_then(Value::as_str) == Some("verified-scope-base-advance")
                && prior_raw.is_object()
                && pending.get("from").and_then(Value::as_str) == Some(status_policy_hash)
                && fingerprint(&prior_raw) == status_policy_hash
                && pending.get("to").and_then(Value::as_str) == Some(&fingerprint(&cfg_raw))
                && prior_diff == cfg_diff
                && pending.get("publication_sha256").and_then(Value::as_str)
                    == Some(&sha(&root.join("verified-scope-publication.json"))?)
                && prev_binding.get("repository").and_then(Value::as_str)
                    == Some("lhemerly/code-factory-controller")
                && prev_binding.get("repository_id").and_then(Value::as_i64) == Some(1414818646)
                && prev_binding.get("private").and_then(Value::as_bool) == Some(true)
                && prev_binding.get("branch").and_then(Value::as_str) == Some("main")
                && prev_binding.get("required_checks") == Some(&json!(["rust-and-contract"]))
                && previous
                    .get("intent")
                    .and_then(|i| i.get("state"))
                    .and_then(Value::as_str)
                    == Some("confirmed")
                && previous
                    .get("intent")
                    .and_then(|i| i.get("commit"))
                    .and_then(Value::as_str)
                    == cfg.get("scope_base").and_then(Value::as_str)
                && previous
                    .get("intent")
                    .and_then(|i| i.get("ci"))
                    .and_then(|c| c.get("status"))
                    .and_then(Value::as_str)
                    == Some("passed"),
            "Unreconciled native policy change"
        );
        let cfg_scope_base = cfg
            .get("scope_base")
            .and_then(Value::as_str)
            .context("cfg scope base")?;
        git_ancestor(cfg_scope_base, new_base)?;
        superseded_raw = Some(RawJson::Object(vec![
            ("migration".into(), pending_raw),
            ("publication".into(), previous_raw),
        ]));
    }

    let mut after_raw = cfg_raw.clone();
    after_raw.set("scope_base", RawJson::String(base.to_string()))?;
    let snapshot = root.join("verified-scope-publication.json");
    let migration = root.join("approved-policy-migration.json");

    let mut approval_entries = vec![
        (
            "kind".to_string(),
            RawJson::String("verified-scope-base-advance".into()),
        ),
        (
            "from".to_string(),
            RawJson::String(status_policy_hash.to_string()),
        ),
        ("to".to_string(), RawJson::String(fingerprint(&after_raw))),
        ("prior_configuration".to_string(), prior_raw),
        (
            "publication_sha256".to_string(),
            RawJson::String(sha(published_path)?),
        ),
    ];
    if let Some(sup) = superseded_raw {
        approval_entries.push(("superseded_unapplied".to_string(), sup));
    }
    let mut approval_raw = RawJson::Object(approval_entries);

    let mut file_backups = Vec::new();
    for name in [
        "verified-scope-publication.json",
        "approved-policy-migration.json",
    ] {
        let p = root.join(name);
        file_backups.push((
            name.to_string(),
            if p.exists() {
                RawJson::String(fs::read_to_string(&p)?)
            } else {
                RawJson::Null
            },
        ));
    }
    let journal_raw = RawJson::Object(vec![
        ("manifest".into(), m_raw.clone()),
        ("state".into(), state_raw.clone()),
        ("status".into(), status_raw.clone()),
        ("configuration".into(), cfg_raw.clone()),
        ("files".into(), RawJson::Object(file_backups)),
        (
            "raw_files".into(),
            RawJson::Object(vec![
                ("manifest".into(), RawJson::String(manifest_text)),
                ("state".into(), RawJson::String(state_text)),
                ("status".into(), RawJson::String(status_text)),
                ("configuration".into(), RawJson::String(cfg_text)),
            ]),
        ),
    ]);
    save_raw(&journal, &journal_raw)?;

    save_raw(&snapshot, &published_raw)?;
    save_raw(&migration, &approval_raw)?;
    save_raw(&root.join("supervisor.json"), &after_raw)?;

    approval_raw.set("publication_sha256", RawJson::String(sha(&snapshot)?))?;
    save_raw(&migration, &approval_raw)?;

    let mut updated_raw = m_raw.clone();
    let updated_pins = updated_raw
        .get_mut("trusted_files")
        .context("trusted_files")?;
    for p in [&snapshot, &migration, &root.join("supervisor.json")] {
        updated_pins.set(p.to_string_lossy().into_owned(), RawJson::String(sha(p)?))?;
    }
    let updated_val = updated_raw.to_value();

    state_raw.set("manifest_digest", RawJson::String(digest(&updated_val)))?;
    let mig_entry = RawJson::Object(vec![
        (
            "kind".into(),
            RawJson::String("verified-scope-base-advance".into()),
        ),
        ("from".into(), RawJson::String(digest(&m))),
        ("to".into(), RawJson::String(digest(&updated_val))),
        ("published".into(), RawJson::String(base.to_string())),
        ("repair_input".into(), RawJson::String(new_base.to_string())),
    ]);
    if let Some(RawJson::Array(arr)) = state_raw.get_mut("implementation_migrations") {
        arr.push(mig_entry);
    } else {
        state_raw.set("implementation_migrations", RawJson::Array(vec![mig_entry]))?;
    }

    status_raw.set("base", RawJson::String(new_base.to_string()))?;
    save_raw(&root.join("status.json"), &status_raw)?;
    save_raw(&state_path, &state_raw)?;
    save_raw(&mp, &updated_raw)?;
    fs::remove_file(&journal)?;
    File::open(&root)?.sync_all()?;

    Ok(json!({
        "scope_base": base,
        "unaccepted_repair_input": new_base,
        "allowed_paths_unchanged": true,
        "goal_and_publication_binding_unchanged": true,
    }))
}

pub fn spark_status(root: &Path) -> Result<String> {
    let r = resolve(root)?;
    fs::read_to_string(r.join("status.json"))
        .with_context(|| format!("reading status in {}", r.display()))
}

pub fn spark_pause(root: &Path, reason: Option<&str>) -> Result<String> {
    let r = resolve(root)?;
    let msg = reason.unwrap_or("Operator pause");
    fs::write(r.join("PAUSE"), format!("{msg}\n"))?;
    Ok("Pause requested at next cycle boundary. For immediate owned-worker fencing use systemctl --user stop spark-code-factory.service.".into())
}

pub fn spark_resume(root: &Path, reason: &str) -> Result<String> {
    spark_resume_mode(root, reason, true)
}
pub(crate) fn spark_resume_preserving_pause(root: &Path, reason: &str) -> Result<String> {
    spark_resume_mode(root, reason, false)
}
fn spark_resume_mode(root: &Path, reason: &str, clear_pause: bool) -> Result<String> {
    ensure!(
        !reason.trim().is_empty(),
        "resume requires an explicit --reason after inspecting recovery evidence"
    );
    let r = resolve(root)?;
    let _lock = goal_state::Lock::acquire(&r.join("supervisor.lock"), false).map_err(|_| {
        anyhow::anyhow!("Stop the owned service first; resume cannot race a live supervisor")
    })?;

    let status_path = r.join("status.json");
    let mut s_raw = RawJson::read(&status_path)?;
    let s = s_raw.to_value();
    ensure!(
        s.get("current").is_none_or(Value::is_null),
        "Unresolved active cycle; recover ownership/stop before resume. No automatic override."
    );

    if clear_pause {
        let _ = fs::remove_file(r.join("PAUSE"));
    } else {
        ensure!(
            !r.join("PAUSE").exists(),
            "Different explicit pause remains authoritative"
        );
    }

    let now_iso = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
    let operator_resume_raw = RawJson::Object(vec![
        ("at".into(), RawJson::String(now_iso.clone())),
        ("reason".into(), RawJson::String(reason.to_string())),
    ]);

    s_raw.set("status", RawJson::String("waiting".into()))?;
    s_raw.set("pause_reason", RawJson::Null)?;
    s_raw.set("operator_resume", operator_resume_raw)?;

    save_raw(&status_path, &s_raw)?;

    let events_path = r.join("events.ndjson");
    let event_raw = RawJson::Object(vec![
        ("kind".into(), RawJson::String("operator_resume".into())),
        ("at".into(), RawJson::String(now_iso)),
        ("reason".into(), RawJson::String(reason.to_string())),
    ]);
    let mut event_str = raw_json_compact_utf8(&event_raw);
    event_str.push('\n');

    use std::os::unix::fs::OpenOptionsExt;
    let mut event_file = OpenOptions::new()
        .create(true)
        .append(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&events_path)?;
    use std::io::Write;
    event_file.write_all(event_str.as_bytes())?;
    event_file.sync_all()?;

    Ok("Resume prepared. Provider-denial circuits and old receipts remain unchanged. Start the user service.".into())
}
