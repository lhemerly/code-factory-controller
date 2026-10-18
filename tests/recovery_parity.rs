//! Parity and structural verification tests for recovery, supervised subprocess execution,
//! ordered JSON persistence, journal rollback, trace ledger schema bounds, and Bonsai preflight.
use code_factory_controller::{
    check_bonsai,
    goal_state::{
        self, RawJson, digest, python_json, python_json_compact_ascii, python_json_compact_utf8,
        raw_json_compact_utf8, raw_json_pretty, resolve, save_raw,
    },
    recovery::{self, INTERRUPTION_NAME, RESOLUTION_NAME},
    trace_ledger::{self, Ledger},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    net::TcpListener,
    os::{fd::AsRawFd, unix::fs::PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
    thread,
};

fn controller_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_code-factory-controller"))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn extract_retained_dir(err_msg: &str) -> Option<PathBuf> {
    if let Some(idx) = err_msg.find("retained=") {
        let rest = &err_msg[idx + "retained=".len()..];
        let path_str = rest.lines().next().unwrap_or(rest).trim();
        return Some(PathBuf::from(path_str));
    }
    if let Some(idx) = err_msg.find("retained at ") {
        let rest = &err_msg[idx + "retained at ".len()..];
        let end = rest.find(')').unwrap_or(rest.len());
        return Some(PathBuf::from(rest[..end].trim()));
    }
    None
}

#[test]
fn compact_json_formatters_preserve_python_semantics() {
    let value = json!({
        "cfg": {"scope_base": "abc", "models": ["a", "b"]},
        "goal": {"id": "test", "unicode": "日本語\u{7f}"}
    });
    let utf8 = python_json_compact_utf8(&value);
    assert!(!utf8.contains(' '));
    assert!(utf8.contains("日本語"));

    let ascii = python_json_compact_ascii(&value);
    assert!(!ascii.contains(' '));
    assert!(ascii.contains("\\u65e5\\u672c\\u8a9e"));
}

#[test]
fn raw_json_exact_nested_key_order_floats_and_strict_parsing() {
    let raw_input = r#"{"z":1,"nested":{"β":2,"a":3},"a":4,"floats":[0.0000001,0.00001,0.0001,10000000000000000.0]}"#;
    let raw = RawJson::parse(raw_input.as_bytes()).unwrap();
    let compact = raw_json_compact_utf8(&raw);

    let pos_z = compact.find("\"z\":1").unwrap();
    let pos_nested = compact.find("\"nested\":").unwrap();
    let pos_beta = compact.find("\"β\":2").unwrap();
    let pos_inner_a = compact.find("\"a\":3").unwrap();
    let pos_outer_a = compact.find("\"a\":4").unwrap();
    let pos_floats = compact.find("\"floats\":").unwrap();

    assert!(pos_z < pos_nested);
    assert!(pos_nested < pos_beta);
    assert!(pos_beta < pos_inner_a);
    assert!(pos_inner_a < pos_outer_a);
    assert!(pos_outer_a < pos_floats);

    assert!(compact.contains("1e-07"));
    assert!(compact.contains("1e-05"));
    assert!(compact.contains("1e+16"));

    let hash = sha256_bytes(compact.as_bytes());
    let val: Value = serde_json::from_str(raw_input).unwrap();
    let val_compact = python_json_compact_utf8(&val);
    let val_hash = sha256_bytes(val_compact.as_bytes());
    assert_ne!(
        hash, val_hash,
        "RawJson must preserve insertion order distinct from sorted Value"
    );

    // Reject trailing input
    assert!(RawJson::parse(b"{\"a\":1} trailing").is_err());

    // Duplicate keys: last-value wins while preserving initial key position (matching Python dict)
    let dup = RawJson::parse(b"{\"z\":1,\"a\":2,\"z\":99}").unwrap();
    assert_eq!(raw_json_compact_utf8(&dup), "{\"z\":99,\"a\":2}");

    // Roundtrip through save_raw and RawJson::read preserves exact key order
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("ordered.json");
    save_raw(&p, &raw).unwrap();
    let reread = RawJson::read(&p).unwrap();
    assert_eq!(raw_json_compact_utf8(&reread), compact);
    let pretty = raw_json_pretty(&reread);
    assert!(pretty.contains("\\u03b2"));
}

#[test]
fn supervised_command_large_stdout_and_descendant_reaping() {
    let bin = controller_bin();
    let tmp = tempfile::tempdir().unwrap();

    // 1. >128KiB stdout (200,000 bytes) must not falsely time out due to pipe backpressure
    let large_script = tmp.path().join("large_stdout.py");
    fs::write(
        &large_script,
        "#!/usr/bin/env python3\nimport sys\nsys.stdout.write('A' * 200000)\nsys.stdout.flush()\n",
    )
    .unwrap();
    fs::set_permissions(&large_script, fs::Permissions::from_mode(0o700)).unwrap();

    let res = recovery::supervised_command(&bin, &large_script, &[], tmp.path(), 10, 0).unwrap();
    assert_eq!(res.exit_status, 0);
    assert_eq!(res.stdout.len(), 200_000);
    assert!(res.stdout.iter().all(|&b| b == b'A'));
    assert_eq!(res.receipt["quiescent"], true);
    assert_eq!(res.receipt["leaked_descendants"], false);
    assert!(!res.evidence_dir.exists(), "cleaned up after clean success");

    // 2. Timed-out child with a background sleeping descendant is reaped and evidence is retained
    let timeout_script = tmp.path().join("timeout_tree.py");
    fs::write(
        &timeout_script,
        "#!/usr/bin/env python3\nimport subprocess, time\nsubprocess.Popen(['sleep', '60'])\ntime.sleep(60)\n",
    )
    .unwrap();
    fs::set_permissions(&timeout_script, fs::Permissions::from_mode(0o700)).unwrap();

    let err = recovery::supervised_command(&bin, &timeout_script, &[], tmp.path(), 1, 0)
        .unwrap_err()
        .to_string();
    let retained = extract_retained_dir(&err).expect("retained evidence dir in error");
    assert!(retained.exists(), "failed command must retain evidence dir");
    let receipt: Value =
        serde_json::from_str(&fs::read_to_string(retained.join("command.process.json")).unwrap())
            .unwrap();
    assert_eq!(receipt["observer"], "linux_subreaper_waitpid_pidfd");
    assert_eq!(receipt["termination_reason"], "timeout");
    assert_eq!(receipt["quiescent"], true);
    assert_eq!(receipt["leaked_descendants"], false);
    assert!(receipt["descendants_reaped"].as_u64().unwrap_or(0) >= 1);
    let _ = fs::remove_dir_all(&retained);

    // 3. Root exits 0 immediately while leaving a background sleeping descendant:
    // supervisor must detect leaked_descendants=true, reap the descendant to quiescence, and reject
    let leak_script = tmp.path().join("leak_descendant.py");
    fs::write(
        &leak_script,
        "#!/usr/bin/env python3\nimport subprocess, sys\nsubprocess.Popen(['sleep', '60'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)\nsys.exit(0)\n",
    )
    .unwrap();
    fs::set_permissions(&leak_script, fs::Permissions::from_mode(0o700)).unwrap();

    let leak_err = recovery::supervised_command(&bin, &leak_script, &[], tmp.path(), 5, 0)
        .unwrap_err()
        .to_string();
    let leak_retained = extract_retained_dir(&leak_err).expect("retained dir on leaked descendant");
    assert!(leak_retained.exists());
    let leak_receipt: Value = serde_json::from_str(
        &fs::read_to_string(leak_retained.join("command.process.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        leak_receipt["termination_reason"],
        "owned_descendants_after_root_exit"
    );
    assert_eq!(leak_receipt["leaked_descendants"], true);
    assert_eq!(leak_receipt["quiescent"], true);
    assert!(leak_receipt["descendants_reaped"].as_u64().unwrap_or(0) >= 1);
    let _ = fs::remove_dir_all(&leak_retained);

    // 4. Expected exit 1 succeeds only when clean and quiescent
    let exit1_script = tmp.path().join("exit1.py");
    fs::write(
        &exit1_script,
        "#!/usr/bin/env python3\nimport sys\nsys.stderr.write('Error: job not found\\n\\nCaused by:\\n    Query returned no rows\\n')\nsys.exit(1)\n",
    )
    .unwrap();
    fs::set_permissions(&exit1_script, fs::Permissions::from_mode(0o700)).unwrap();

    let res1 = recovery::supervised_command(&bin, &exit1_script, &[], tmp.path(), 10, 1).unwrap();
    assert_eq!(res1.exit_status, 1);
    assert!(String::from_utf8_lossy(&res1.stderr).contains("Error: job not found"));
    assert!(!res1.evidence_dir.exists());

    // Expected exit 1 rejects exit 2
    let exit2_err = recovery::supervised_command(&bin, &exit1_script, &[], tmp.path(), 10, 2)
        .unwrap_err()
        .to_string();
    if let Some(dir) = extract_retained_dir(&exit2_err) {
        let _ = fs::remove_dir_all(dir);
    }
}

#[test]
fn owner_lock_corroboration_rejects_nonexistent_or_unlocked() {
    let tmp = tempfile::tempdir().unwrap();
    let file_path = tmp.path().join("test.lock");
    fs::write(&file_path, "test").unwrap();
    fs::set_permissions(&file_path, fs::Permissions::from_mode(0o600)).unwrap();

    assert!(recovery::owner_lock(&file_path).is_err());

    let file = File::open(&file_path).unwrap();
    let res = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(res, 0);

    let lock_info = recovery::owner_lock(&file_path).unwrap();
    assert_eq!(lock_info["pid"], std::process::id());
    assert!(lock_info["inode"].as_u64().unwrap() > 0);
}

#[test]
fn permission_block_resolution_and_external_isolation_pins() {
    let bin = resolve(&controller_bin()).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let base = resolve(tmp.path()).unwrap();

    // Place goal root in a subdirectory distinct from any checkout scripts/ directory,
    // and place runtime assets in a separate external runtime directory.
    let root = base.join("external_goal_state/goal");
    let cycle = root.join("cycles/000003");
    let data = root.join("trusted/queue-publication-fixture");
    let host = root.join("trusted/queue-publication-host");
    let runtime_dir = base.join("pinned_runtime_assets");
    fs::create_dir_all(&cycle).unwrap();
    fs::create_dir_all(&data).unwrap();
    fs::create_dir_all(&host).unwrap();
    fs::create_dir_all(&runtime_dir).unwrap();

    let put = |p: &Path, val: &Value| {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, python_json(val)).unwrap();
        fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
    };

    let controller = base.join("fixture_controller");
    let live_path = base.join("live.json");
    let checkpoint = "a".repeat(40);
    let attempt = "11111111-2222-4333-8444-555555555555";
    let worktree = cycle.join("controller/workers").join(attempt);

    let live = json!({
        "state": "blocked",
        "execution_recovery": {
            "stop_id": "synthetic-stop",
            "attempt_id": attempt,
            "session_id": "synthetic-session",
            "execution_stop": {"status": "confirmed"},
            "accounting": {"status": "unknown"}
        }
    });
    put(&live_path, &live);
    fs::write(
        &controller,
        format!(
            "#!/usr/bin/env python3\nimport pathlib\nprint(pathlib.Path({:?}).read_text())\n",
            live_path.to_string_lossy()
        ),
    )
    .unwrap();
    fs::set_permissions(&controller, fs::Permissions::from_mode(0o700)).unwrap();

    let cfg = json!({
        "controller": controller.to_string_lossy(),
        "controller_sha256": recovery::sha(&controller).unwrap(),
        "data": data.to_string_lossy()
    });
    put(&root.join("supervisor.json"), &cfg);
    put(&root.join("goal.json"), &json!({"id": "synthetic-goal"}));
    put(&cycle.join("controller.json"), &json!({}));

    let out = json!({
        "status": "failed",
        "failure_kind": "blocked",
        "phase": "native",
        "cleanup_confirmed": true,
        "sequence": 3,
        "goal_id": "synthetic-goal",
        "job_id": "synthetic-job",
        "attempt": attempt,
        "child": "synthetic-session",
        "parent": "synthetic-parent",
        "worktree": worktree.to_string_lossy(),
        "checkpoint": checkpoint,
        "error": "Permission denied in native tool history",
        "usage": {"accounting_status": "unknown", "tokens": {"input": 5}, "missing": null}
    });
    put(&cycle.join("result.json"), &out);

    let cmd = format!(
        "cd {} && git worktree remove --force {} 2>&1 | tail -2; echo rc=$?; git worktree prune 2>&1 | tail -1; echo rc2=$?",
        root.join("repository").display(),
        worktree.display()
    );
    let call = json!({
        "type": "tool",
        "name": "shell",
        "executed": false,
        "state": {
            "status": "error",
            "input": {"command": cmd},
            "error": {"type": "permission.rejected", "message": "Permission denied: external_directory"}
        }
    });
    put(
        &cycle.join("native-context-final.json"),
        &json!([{"content": [call]}]),
    );

    fs::write(
        data.join("VERIFIER-CONTRACT.md"),
        "Frozen output requirements",
    )
    .unwrap();
    fs::set_permissions(
        data.join("VERIFIER-CONTRACT.md"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    fs::write(host.join("check.py"), "# Frozen independent oracle").unwrap();
    fs::set_permissions(host.join("check.py"), fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(
        root.join("trusted/verify.py"),
        "# Host invocation T/'queue-publication-host/check.py'",
    )
    .unwrap();
    fs::set_permissions(
        root.join("trusted/verify.py"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    fs::write(
        root.join("task.txt"),
        "Read data/VERIFIER-CONTRACT.md. Leave edits uncommitted; host owns Git mutations.",
    )
    .unwrap();
    fs::set_permissions(root.join("task.txt"), fs::Permissions::from_mode(0o600)).unwrap();

    // Create pinned runtime assets in external runtime_dir (not root.parent()/scripts)
    let mut runtime_files = serde_json::Map::new();
    let mut runtime_sha256 = serde_json::Map::new();
    for name in [
        "spark-sandbox.mjs",
        "continuous-ownership.mjs",
        "research-context.mjs",
    ] {
        let p = runtime_dir.join(name);
        fs::write(&p, format!("// pinned runtime asset {name}\n")).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        let h = recovery::sha(&p).unwrap();
        runtime_files.insert(p.to_string_lossy().into_owned(), json!(h));
        runtime_sha256.insert(name.to_string(), json!(h));
    }

    let packaging = base.join("packaging.json");
    put(
        &packaging,
        &json!({
            "applied": true,
            "host_checker_bytes_unchanged": true,
            "host_acceptance_wrapper_only_changes_checker_location": true,
            "manifest_goals_acceptance_models_publication_and_permissions_unchanged": true,
            "denied_helper_exposed_to_worker": false,
            "host_checker_sha256": recovery::sha(&host.join("check.py")).unwrap(),
            "contract_sha256": recovery::sha(&data.join("VERIFIER-CONTRACT.md")).unwrap()
        }),
    );

    let verification = base.join("verification.json");
    put(
        &verification,
        &json!({
            "passed": true,
            "checks": {
                "kernel": {
                    "worker_inputs_contract_only": true,
                    "contract_readable": true,
                    "host_checker_hidden": true,
                    "denied_helper_hidden": true,
                    "contract_readonly": true,
                    "project_writable": true,
                    "git_control_readonly": true
                },
                "same_goal_admission_gate": {"blocked": true}
            }
        }),
    );

    let kernel = base.join("kernel.json");
    put(
        &kernel,
        &json!({
            "passed": true,
            "baseline": false,
            "checks": {
                "assigned_cwd": true,
                "worker_writable": true,
                "owner_source_hidden": true,
                "owner_canary_hidden": true,
                "owner_absolute_execution_blocked": true,
                "owner_symlink_hidden": true,
                ".git_readonly": true,
                "frozen.txt_readonly": true,
                "git_reads_work": true,
                "git_status_work": true,
                "git_metadata_readonly": true
            }
        }),
    );
    let kr = base.join("kernel-receipt.json");
    let kp = base.join("kernel-plan.json");
    put(
        &kr,
        &json!({
            "observer": "linux_subreaper_waitpid_pidfd",
            "execution_id": "kernel",
            "root_exit_status": 0,
            "quiescent": true,
            "leaked_descendants": false,
            "cleanup_error": null
        }),
    );
    put(
        &kp,
        &json!({
            "execution_id": "kernel",
            "stdout": kernel.to_string_lossy()
        }),
    );

    let qualification = base.join("qualification.json");
    put(
        &qualification,
        &json!({
            "passed": true,
            "source_commit": "b".repeat(40),
            "checks": {
                "candidate": true,
                "integration": true,
                "whole_goal": true,
                "owner_source_hidden": true,
                "denial_sticky": true,
                "terminal_denial_blocked": true,
                "worker_git_contract": true,
                "host_git_denial_exact": true,
                "pending_authorization_blocks": true
            },
            "runtime_sha256": runtime_sha256
        }),
    );

    let publication = base.join("publication.json");
    put(
        &publication,
        &json!({
            "binding": {
                "repository": "lhemerly/code-factory-controller",
                "repository_id": 1414818646,
                "private": true,
                "branch": "main",
                "required_checks": ["rust-and-contract"]
            },
            "intent": {
                "state": "confirmed",
                "commit": "b".repeat(40),
                "ci": {"status": "passed"}
            }
        }),
    );

    let isolation = base.join("isolation.json");
    put(
        &isolation,
        &json!({
            "source_commit": "b".repeat(40),
            "kernel_output": kernel.to_string_lossy(),
            "kernel_receipt": kr.to_string_lossy(),
            "kernel_plan": kp.to_string_lossy(),
            "qualification": qualification.to_string_lossy(),
            "publication": publication.to_string_lossy(),
            "runtime_files": runtime_files
        }),
    );

    let pool_lock = base.join("pool.lock");
    let goal_lock = root.join("supervisor.lock");
    let _l1 = goal_state::Lock::acquire(&pool_lock, false).unwrap();
    let _l2 = goal_state::Lock::acquire(&goal_lock, false).unwrap();

    let manifest_path = base.join("launch.json");
    let owner_state = base.join("dispatcher-state.json");
    let receipt = base.join("owned.json");
    let plan = base.join("owned-plan.json");
    put(
        &receipt,
        &json!({
            "execution_id": "synthetic-owned",
            "quiescent": true,
            "leaked_descendants": false,
            "root_exit_status": 0
        }),
    );
    put(
        &plan,
        &json!({
            "execution_id": "synthetic-owned",
            "argv": ["dispatcher", "run", "--manifest", manifest_path.to_string_lossy()]
        }),
    );

    let authorization = base.join("authorization.json");
    let proof_path = root.join(RESOLUTION_NAME);
    let scope = json!({
        "goal_id": "synthetic-goal",
        "original_sha256": recovery::sha(&cycle.join("result.json")).unwrap(),
        "denial_history_sha256": recovery::sha(&cycle.join("native-context-final.json")).unwrap(),
        "packaging_sha256": recovery::sha(&packaging).unwrap(),
        "next_sequence": 4,
        "denied_file_remains_inaccessible": true,
        "missing_usage_remains_unknown": true,
        "same_goal_only": true,
        "kind": "host-git-ownership",
        "checkpoint": checkpoint,
        "isolation_sha256": recovery::sha(&isolation).unwrap(),
        "post_denial_history_preserved": true,
        "denied_command_will_not_be_retried": true,
        "denied_action_sha256": digest(&call),
        "worker_task_sha256": recovery::sha(&root.join("task.txt")).unwrap(),
        "host_owns_git_mutations": true
    });
    put(
        &authorization,
        &json!({
            "decision": "approve",
            "source": "verified-human-conversation",
            "thread_id": "synthetic-thread",
            "message_id": "synthetic-user-message",
            "reply": "yes",
            "scope": scope
        }),
    );

    let before_dir = base.join("before");
    fs::create_dir_all(&before_dir).unwrap();
    fs::write(
        before_dir.join("check.py"),
        fs::read(host.join("check.py")).unwrap(),
    )
    .unwrap();
    fs::set_permissions(
        before_dir.join("check.py"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    fs::write(
        before_dir.join("verify.py"),
        "# Host invocation T/'queue-publication-fixture/check.py'",
    )
    .unwrap();
    fs::set_permissions(
        before_dir.join("verify.py"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();

    let base_manifest = json!({
        "controller": controller.to_string_lossy(),
        "pool_lock": pool_lock.to_string_lossy(),
        "goals": [{"id": "synthetic-goal", "root": root.to_string_lossy()}],
        "state": owner_state.to_string_lossy(),
        "trusted_files": {}
    });
    put(&before_dir.join("manifest.json"), &base_manifest);

    let seal = |include_bin_pin: bool, corrupt_bin_pin: bool| {
        let mut tf = serde_json::Map::new();
        for p in [
            &authorization,
            &packaging,
            &verification,
            &receipt,
            &plan,
            &root.join("task.txt"),
            &root.join("trusted/verify.py"),
            &root.join("supervisor.json"),
            &host.join("check.py"),
            &data.join("VERIFIER-CONTRACT.md"),
            &before_dir.join("check.py"),
            &before_dir.join("verify.py"),
            &before_dir.join("manifest.json"),
            &kernel,
            &kr,
            &kp,
            &qualification,
            &publication,
            &isolation,
            &runtime_dir.join("spark-sandbox.mjs"),
            &runtime_dir.join("continuous-ownership.mjs"),
            &runtime_dir.join("research-context.mjs"),
        ] {
            tf.insert(
                p.to_string_lossy().into_owned(),
                json!(recovery::sha(p).unwrap()),
            );
        }
        if include_bin_pin {
            let h = if corrupt_bin_pin {
                "0".repeat(64)
            } else {
                recovery::sha(&bin).unwrap()
            };
            tf.insert(bin.to_string_lossy().into_owned(), json!(h));
        }
        let mut m = base_manifest.clone();
        m.as_object_mut()
            .unwrap()
            .insert("trusted_files".into(), Value::Object(tf));
        let without_digest = digest(&m);

        let proof_val = json!({
            "version": 1,
            "kind": "host-git-ownership",
            "sequence": 3,
            "original_sha256": recovery::sha(&cycle.join("result.json")).unwrap(),
            "identity": {
                "goal_id": "synthetic-goal",
                "job_id": "synthetic-job",
                "attempt": attempt,
                "child": "synthetic-session",
                "parent": "synthetic-parent",
                "worktree": worktree.to_string_lossy()
            },
            "configuration_sha256": recovery::sha(&root.join("supervisor.json")).unwrap(),
            "denial_history_sha256": recovery::sha(&cycle.join("native-context-final.json")).unwrap(),
            "manifest": manifest_path.to_string_lossy(),
            "authorization": authorization.to_string_lossy(),
            "packaging": packaging.to_string_lossy(),
            "packaging_sha256": recovery::sha(&packaging).unwrap(),
            "verification": verification.to_string_lossy(),
            "owned_receipt": receipt.to_string_lossy(),
            "owned_plan": plan.to_string_lossy(),
            "stop_id": "synthetic-stop",
            "checkpoint": checkpoint,
            "isolation": isolation.to_string_lossy(),
            "isolation_sha256": recovery::sha(&isolation).unwrap(),
            "denied_action_sha256": digest(&call),
            "worker_task_sha256": recovery::sha(&root.join("task.txt")).unwrap(),
            "manifest_without_resolution_sha256": without_digest
        });
        put(&proof_path, &proof_val);
        m.get_mut("trusted_files")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(
                proof_path.to_string_lossy().into_owned(),
                json!(recovery::sha(&proof_path).unwrap()),
            );
        put(&manifest_path, &m);
        put(&owner_state, &json!({"manifest_digest": digest(&m)}));
    };

    // 1. Missing native executable pin in trusted_files must fail closed
    seal(false, false);
    let err_missing =
        recovery::audit_permission_block_resolution_with(&root, &cycle, &controller, 4, &bin)
            .unwrap_err()
            .to_string();
    assert!(
        err_missing.contains("Unpinned resolver implementation"),
        "unexpected error: {err_missing}"
    );

    // 2. Wrong native executable pin in trusted_files must fail closed
    seal(true, true);
    let err_corrupt =
        recovery::audit_permission_block_resolution_with(&root, &cycle, &controller, 4, &bin)
            .unwrap_err()
            .to_string();
    assert!(
        err_corrupt.contains("Unpinned resolver implementation"),
        "unexpected error: {err_corrupt}"
    );

    // 3. Valid native executable pin + external runtime assets succeeds without touching cycle bytes
    seal(true, false);
    let before_result = fs::read(cycle.join("result.json")).unwrap();
    let res = recovery::audit_permission_block_resolution_with(&root, &cycle, &controller, 4, &bin)
        .unwrap();
    assert_eq!(res["kind"], "host-git-ownership");
    assert_eq!(res["acceptance_granted"], false);
    assert_eq!(res["accounting"], "unknown");
    assert_eq!(fs::read(cycle.join("result.json")).unwrap(), before_result);
}

#[test]
fn owner_interruption_pin_validation_and_supervised_inspect() {
    let bin = resolve(&controller_bin()).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let base = resolve(tmp.path()).unwrap();
    let root = base.join("goal");
    let cycle = root.join("cycles/000008");
    fs::create_dir_all(&cycle).unwrap();

    let put = |p: &Path, val: &Value| {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, python_json(val)).unwrap();
        fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
    };

    let controller = base.join("controller");
    let live_path = base.join("live.json");
    let recovery_val = json!({
        "stop_id": "stop8",
        "attempt_id": "attempt8",
        "session_id": "child8",
        "execution_stop": {"status": "confirmed"},
        "accounting": {"status": "unknown"}
    });
    put(
        &live_path,
        &json!({"state": "blocked", "execution_recovery": recovery_val}),
    );
    fs::write(
        &controller,
        format!(
            "#!/usr/bin/env python3\nimport pathlib\nprint(pathlib.Path({:?}).read_text())\n",
            live_path.to_string_lossy()
        ),
    )
    .unwrap();
    fs::set_permissions(&controller, fs::Permissions::from_mode(0o700)).unwrap();

    let out = json!({
        "sequence": 8,
        "status": "failed",
        "failure_kind": "blocked",
        "phase": "native",
        "error": "Error: Operator stopping",
        "cleanup_confirmed": true,
        "goal_id": "goal",
        "job_id": "job8",
        "attempt": "attempt8",
        "child": "child8",
        "parent": "parent8",
        "worktree": base.join("worker").to_string_lossy(),
        "recovery": recovery_val,
        "usage": {"cost": 0, "accounting_status": "unknown"}
    });
    put(&cycle.join("result.json"), &out);
    put(&cycle.join("controller.json"), &json!({}));
    put(
        &cycle.join("native-context-final.json"),
        &json!([{
            "type": "assistant",
            "content": [{
                "type": "tool",
                "name": "shell",
                "state": {
                    "status": "completed",
                    "metadata": {"exit": 1},
                    "input": {"command": "synthetic nonzero test"}
                }
            }]
        }]),
    );

    let cfg_path = root.join("supervisor.json");
    put(
        &cfg_path,
        &json!({
            "controller": controller.to_string_lossy(),
            "controller_sha256": recovery::sha(&controller).unwrap()
        }),
    );
    let pause = root.join("PAUSE");
    fs::write(&pause, "Exact synthetic owner pause\n").unwrap();
    fs::set_permissions(&pause, fs::Permissions::from_mode(0o600)).unwrap();

    let uid = unsafe { libc::getuid() };
    let owner_stop = base.join("stop.json");
    put(
        &owner_stop,
        &json!({
            "action": "SIGTERM exact owned foreground dispatcher",
            "new_permission_denial": false,
            "owner": {"pid": 91, "start_ticks": 101, "boot_id": "synthetic-boot", "uid": uid}
        }),
    );
    let receipt = base.join("receipt.json");
    put(
        &receipt,
        &json!({
            "observer": "linux_subreaper_waitpid_pidfd",
            "execution_id": "exec8",
            "root_pid": 91,
            "supervisor": {"start_ticks": 100, "boot_id": "synthetic-boot"},
            "quiescent": true,
            "leaked_descendants": false,
            "cleanup_error": null,
            "root_exit_status": 0
        }),
    );
    let manifest_path = base.join("manifest.json");
    let state_path = base.join("state.json");
    let plan = base.join("plan.json");
    put(
        &plan,
        &json!({
            "execution_id": "exec8",
            "argv": ["run", "--manifest", manifest_path.to_string_lossy()]
        }),
    );

    let pool_lock = base.join("pool.lock");
    let sup_lock = root.join("supervisor.lock");
    let _l1 = goal_state::Lock::acquire(&pool_lock, false).unwrap();
    let _l2 = goal_state::Lock::acquire(&sup_lock, false).unwrap();

    let proof_path = root.join(INTERRUPTION_NAME);
    let seal = |include_bin_pin: bool| {
        let mut tf = serde_json::Map::new();
        for p in [&cfg_path, &owner_stop, &receipt, &plan] {
            tf.insert(
                p.to_string_lossy().into_owned(),
                json!(recovery::sha(p).unwrap()),
            );
        }
        if include_bin_pin {
            tf.insert(
                bin.to_string_lossy().into_owned(),
                json!(recovery::sha(&bin).unwrap()),
            );
        }
        let mut m = json!({
            "controller": controller.to_string_lossy(),
            "pool_lock": pool_lock.to_string_lossy(),
            "state": state_path.to_string_lossy(),
            "goals": [{"id": "goal", "root": root.to_string_lossy()}],
            "trusted_files": tf
        });
        let without_digest = digest(&m);
        let proof_val = json!({
            "version": 1,
            "kind": "owner-interruption",
            "sequence": 8,
            "original_sha256": recovery::sha(&cycle.join("result.json")).unwrap(),
            "history_sha256": recovery::sha(&cycle.join("native-context-final.json")).unwrap(),
            "identity": {
                "goal_id": "goal",
                "job_id": "job8",
                "attempt": "attempt8",
                "child": "child8",
                "parent": "parent8",
                "worktree": base.join("worker").to_string_lossy()
            },
            "manifest": manifest_path.to_string_lossy(),
            "owner_stop": owner_stop.to_string_lossy(),
            "owned_receipt": receipt.to_string_lossy(),
            "owned_plan": plan.to_string_lossy(),
            "stop_id": "stop8",
            "pause_sha256": recovery::sha(&pause).unwrap(),
            "manifest_without_interruption_sha256": without_digest
        });
        put(&proof_path, &proof_val);
        m.get_mut("trusted_files")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(
                proof_path.to_string_lossy().into_owned(),
                json!(recovery::sha(&proof_path).unwrap()),
            );
        put(&manifest_path, &m);
        put(&state_path, &json!({"manifest_digest": digest(&m)}));
    };

    seal(false);
    assert!(
        recovery::audit_owner_interruption_with(&root, &cycle, &controller, 9, &bin)
            .unwrap_err()
            .to_string()
            .contains("Unpinned interruption implementation")
    );

    seal(true);
    let res = recovery::audit_owner_interruption_with(&root, &cycle, &controller, 9, &bin).unwrap();
    assert_eq!(res["kind"], "owner-interruption");
    assert_eq!(res["acceptance_granted"], false);
}

#[test]
fn pre_admission_absence_lock_protocol_and_reconcile() {
    let bin = resolve(&controller_bin()).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let root = resolve(tmp.path()).unwrap();
    let cycle = root.join("cycles/000002");
    let ctrl_dir = cycle.join("controller");
    fs::create_dir_all(&ctrl_dir).unwrap();
    fs::create_dir_all(root.join("repository")).unwrap();

    let lock_path = ctrl_dir.join("controller.lock");
    File::create(&lock_path).unwrap();

    // Controller script that verifies controller.lock is NOT held during `inspect`!
    // If pre_admission_absence held controller.lock across inspect, this flock would fail.
    let controller = root.join("controller_fixture");
    fs::write(
        &controller,
        format!(
            "#!/usr/bin/env python3\nimport fcntl, sys\nwith open({:?}, 'r') as f:\n    fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nsys.stderr.write('Error: job not found\\n\\nCaused by:\\n    Query returned no rows\\n')\nsys.exit(1)\n",
            lock_path.to_string_lossy()
        ),
    )
    .unwrap();
    fs::set_permissions(&controller, fs::Permissions::from_mode(0o700)).unwrap();

    let db_path = ctrl_dir.join("controller.sqlite");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        for table in [
            "jobs", "attempts", "grants", "requests", "receipts", "events", "steps",
        ] {
            conn.execute(&format!("CREATE TABLE {table} (id TEXT)"), [])
                .unwrap();
        }
    }

    fs::write(root.join("goal.json"), r#"{"id":"goal"}"#).unwrap();
    fs::write(
        root.join("supervisor.json"),
        serde_json::to_string(&json!({
            "controller": controller.to_string_lossy(),
            "controller_sha256": recovery::sha(&controller).unwrap()
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        cycle.join("controller.json"),
        serde_json::to_string(&json!({
            "project_id": "goal",
            "repository": root.join("repository").to_string_lossy(),
            "data_dir": ctrl_dir.to_string_lossy()
        }))
        .unwrap(),
    )
    .unwrap();
    let out = json!({
        "goal_id": "goal",
        "sequence": 2,
        "status": "failed",
        "phase": "admission",
        "job_id": "reserved-2",
        "cleanup_confirmed": true,
        "ended_utc": "now",
        "error": "Error: Project instructions exceed admission budget; no instructions were truncated",
        "fence_error": "Error: Controller cancel: {\"error\":\"job not found\"}"
    });
    fs::write(
        cycle.join("result.json"),
        serde_json::to_string(&out).unwrap(),
    )
    .unwrap();

    let proof = recovery::pre_admission_absence_with(&root, &cycle, &controller, &bin).unwrap();
    assert_eq!(proof["kind"], "not_admitted");
    assert_eq!(proof["worker_session_created"], false);

    // Reconciliation and audit roundtrip
    let mp = root.join("manifest.json");
    let state_path = root.join("dispatch.json");
    let process_path = root.join("owner.process.json");
    let plan_path = root.join("owner.plan.json");
    fs::write(
        &process_path,
        serde_json::to_string(&json!({
            "observer": "linux_subreaper_waitpid_pidfd",
            "quiescent": true,
            "leaked_descendants": false,
            "cleanup_error": null,
            "execution_id": "owned",
            "root_exit_status": 1
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        &plan_path,
        serde_json::to_string(&json!({
            "execution_id": "owned",
            "receipt": process_path.to_string_lossy(),
            "argv": ["run", "--manifest", mp.to_string_lossy()]
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        &mp,
        serde_json::to_string(&json!({
            "goals": [{"id": "goal", "root": root.to_string_lossy(), "repository": root.join("repository").to_string_lossy()}],
            "controller": controller.to_string_lossy(),
            "pool_lock": root.join("pool.lock").to_string_lossy(),
            "state": state_path.to_string_lossy(),
            "trusted_files": {
                controller.to_string_lossy().into_owned(): recovery::sha(&controller).unwrap()
            }
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        &state_path,
        r#"{"z_order":1,"status":"needs_attention","current":{"goal":"goal","expected_sequence":2},"goals":{"goal":{"state":"paused","usage":{"tokens":{"input":999}},"model":"previous-model"}}}"#,
    )
    .unwrap();
    fs::write(
        root.join("status.json"),
        serde_json::to_string(&json!({
            "sequence": 2,
            "status": "paused",
            "current": {"sequence": 2},
            "pause_reason": format!("Execution/recovery evidence incomplete: {}", out["error"].as_str().unwrap())
        }))
        .unwrap(),
    )
    .unwrap();

    let rec = recovery::reconcile_pre_admission_with(&mp, "goal", &process_path, &bin).unwrap();
    assert_eq!(rec, proof);
    let audited = recovery::audit_pre_admission_with(&root, &cycle, &controller, &bin).unwrap();
    assert_eq!(audited, proof);
}

#[test]
fn advance_scope_base_two_consecutive_advances_and_exact_byte_rollback() {
    let bin = resolve(&controller_bin()).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let root = resolve(tmp.path()).unwrap();
    let repo = root.join("repository");
    fs::create_dir_all(&repo).unwrap();

    let git = |args: &[&str]| -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@localhost",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };

    git(&["init", "-q"]);
    fs::write(repo.join("product.py"), "old\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "initial"]);
    let base = git(&["rev-parse", "HEAD"]);

    fs::write(repo.join("result.txt"), "unaccepted repair\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "repair input"]);
    let repair = git(&["rev-parse", "HEAD"]);

    git(&["checkout", "--detach", &base]);
    fs::write(repo.join("product.py"), "verified upgrade 1\n").unwrap();
    git(&["commit", "-qam", "published 1"]);
    let published1 = git(&["rev-parse", "HEAD"]);

    git(&["checkout", "--detach", &repair]);
    git(&[
        "merge",
        "--no-ff",
        &published1,
        "-m",
        "Merge published 1 into repair",
    ]);
    let merged1 = git(&["rev-parse", "HEAD"]);

    git(&["checkout", "--detach", &published1]);
    fs::write(repo.join("product.py"), "verified upgrade 2\n").unwrap();
    git(&["commit", "-qam", "published 2"]);
    let published2 = git(&["rev-parse", "HEAD"]);

    git(&["checkout", "--detach", &merged1]);
    git(&[
        "merge",
        "--no-ff",
        &published2,
        "-m",
        "Merge published 2 into repair",
    ]);
    let merged2 = git(&["rev-parse", "HEAD"]);

    // Write supervisor.json and goal.json with deliberate non-alphabetical key order and floats
    let cfg_initial_str = "{\n  \"z_first\": 1,\n  \"nested\": {\n    \"\\u03b2\": 0.0000001,\n    \"a\": 3\n  },\n  \"a_last\": 4\n}\n";
    let goal_str = "{\n  \"z_goal\": \"keep\",\n  \"id\": \"goal\"\n}\n";
    fs::write(root.join("supervisor.json"), cfg_initial_str).unwrap();
    fs::write(root.join("goal.json"), goal_str).unwrap();

    let cfg_raw = RawJson::read(&root.join("supervisor.json")).unwrap();
    let goal_raw = RawJson::read(&root.join("goal.json")).unwrap();
    let fp_obj = RawJson::Object(vec![
        ("cfg".into(), cfg_raw.clone()),
        ("goal".into(), goal_raw.clone()),
    ]);
    let initial_policy_hash = sha256_bytes(raw_json_compact_utf8(&fp_obj).as_bytes());

    let state_path = root.join("dispatch.json");
    let mp = root.join("manifest.json");
    fs::write(
        root.join("status.json"),
        format!(
            "{{\n  \"z_status\": true,\n  \"status\": \"waiting\",\n  \"current\": null,\n  \"base\": \"{repair}\",\n  \"policy_hash\": \"{initial_policy_hash}\"\n}}\n"
        ),
    )
    .unwrap();
    fs::write(
        &state_path,
        "{\n  \"z_state\": 1,\n  \"current\": null,\n  \"goals\": {\n    \"goal\": {\n      \"state\": \"ready\"\n    }\n  }\n}\n",
    )
    .unwrap();
    fs::write(
        root.join("job-template.json"),
        r#"{"allowed_paths":["result.txt"],"protected_paths":["product.py"]}"#,
    )
    .unwrap();
    fs::write(
        &mp,
        serde_json::to_string_pretty(&json!({
            "goals": [{"id": "goal", "root": root.to_string_lossy(), "repository": repo.to_string_lossy()}],
            "pool_lock": root.join("pool.lock").to_string_lossy(),
            "state": state_path.to_string_lossy(),
            "trusted_files": {
                root.join("supervisor.json").to_string_lossy().into_owned(): recovery::sha(&root.join("supervisor.json")).unwrap()
            }
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();

    let pub_path = root.join("published.json");
    let write_pub = |commit: &str| {
        fs::write(
            &pub_path,
            serde_json::to_string_pretty(&json!({
                "binding": {
                    "repository": "lhemerly/code-factory-controller",
                    "repository_id": 1414818646,
                    "private": true,
                    "branch": "main",
                    "required_checks": ["rust-and-contract"]
                },
                "intent": {
                    "state": "confirmed",
                    "commit": commit,
                    "ci": {"status": "passed"}
                }
            }))
            .unwrap()
                + "\n",
        )
        .unwrap();
    };

    // First advance_scope_base
    write_pub(&published1);
    let res1 = recovery::advance_scope_base_with(&mp, "goal", &pub_path, &merged1, &bin).unwrap();
    assert_eq!(res1["scope_base"], published1);

    // Check that supervisor.json preserved "z_first" before "a_last" and "1e-07"
    let sup_after_1 = fs::read_to_string(root.join("supervisor.json")).unwrap();
    assert!(sup_after_1.find("\"z_first\"").unwrap() < sup_after_1.find("\"a_last\"").unwrap());
    assert!(sup_after_1.contains("1e-07"));

    // Snapshot exact bytes after 1st advance to test journal rollback later
    let snap_sup_1 = fs::read(root.join("supervisor.json")).unwrap();
    let snap_status_1 = fs::read(root.join("status.json")).unwrap();
    let snap_state_1 = fs::read(&state_path).unwrap();
    let snap_mp_1 = fs::read(&mp).unwrap();
    let snap_pub_1 = fs::read(root.join("verified-scope-publication.json")).unwrap();
    let snap_mig_1 = fs::read(root.join("approved-policy-migration.json")).unwrap();

    // Second consecutive pending advance_scope_base!
    // Must succeed because prior_configuration and cfg preserved exact key order & float formatting.
    write_pub(&published2);
    let res2 = recovery::advance_scope_base_with(&mp, "goal", &pub_path, &merged2, &bin).unwrap();
    assert_eq!(res2["scope_base"], published2);
    let mig_after_2 = RawJson::read(&root.join("approved-policy-migration.json")).unwrap();
    let prior_compact = raw_json_compact_utf8(mig_after_2.get("prior_configuration").unwrap());
    assert_eq!(prior_compact, raw_json_compact_utf8(&cfg_raw));

    // Test Journal Rollback: simulate interrupted transaction from state 1
    let journal_path = root.join("scope-base-journal.json");
    let journal_raw = RawJson::Object(vec![
        ("manifest".into(), RawJson::parse(&snap_mp_1).unwrap()),
        ("state".into(), RawJson::parse(&snap_state_1).unwrap()),
        ("status".into(), RawJson::parse(&snap_status_1).unwrap()),
        ("configuration".into(), RawJson::parse(&snap_sup_1).unwrap()),
        (
            "files".into(),
            RawJson::Object(vec![
                (
                    "verified-scope-publication.json".into(),
                    RawJson::String(String::from_utf8(snap_pub_1.clone()).unwrap()),
                ),
                (
                    "approved-policy-migration.json".into(),
                    RawJson::String(String::from_utf8(snap_mig_1.clone()).unwrap()),
                ),
            ]),
        ),
        (
            "raw_files".into(),
            RawJson::Object(vec![
                (
                    "manifest".into(),
                    RawJson::String(String::from_utf8(snap_mp_1.clone()).unwrap()),
                ),
                (
                    "state".into(),
                    RawJson::String(String::from_utf8(snap_state_1.clone()).unwrap()),
                ),
                (
                    "status".into(),
                    RawJson::String(String::from_utf8(snap_status_1.clone()).unwrap()),
                ),
                (
                    "configuration".into(),
                    RawJson::String(String::from_utf8(snap_sup_1.clone()).unwrap()),
                ),
            ]),
        ),
    ]);
    save_raw(&journal_path, &journal_raw).unwrap();

    let rollback_err = recovery::advance_scope_base_with(&mp, "goal", &pub_path, &merged2, &bin)
        .unwrap_err()
        .to_string();
    assert!(rollback_err.contains("Interrupted baseline transaction restored"));
    assert!(!journal_path.exists());
    assert_eq!(fs::read(root.join("supervisor.json")).unwrap(), snap_sup_1);
    assert_eq!(fs::read(root.join("status.json")).unwrap(), snap_status_1);
    assert_eq!(fs::read(&state_path).unwrap(), snap_state_1);
    assert_eq!(fs::read(&mp).unwrap(), snap_mp_1);
    assert_eq!(
        fs::read(root.join("verified-scope-publication.json")).unwrap(),
        snap_pub_1
    );
    assert_eq!(
        fs::read(root.join("approved-policy-migration.json")).unwrap(),
        snap_mig_1
    );

    // If an old pin in the journal cannot be satisfied, rollback must fail closed and retain journal
    let mut bad_mp: Value = serde_json::from_slice(&snap_mp_1).unwrap();
    bad_mp["trusted_files"][root.join("supervisor.json").to_string_lossy().as_ref()] =
        json!("0".repeat(64));
    let bad_mp_str = serde_json::to_string_pretty(&bad_mp).unwrap() + "\n";
    let mut bad_journal = journal_raw.clone();
    bad_journal
        .set("manifest", RawJson::parse(bad_mp_str.as_bytes()).unwrap())
        .unwrap();
    bad_journal
        .get_mut("raw_files")
        .unwrap()
        .set("manifest", RawJson::String(bad_mp_str))
        .unwrap();
    save_raw(&journal_path, &bad_journal).unwrap();

    let bad_err = recovery::advance_scope_base_with(&mp, "goal", &pub_path, &merged2, &bin)
        .unwrap_err()
        .to_string();
    assert!(bad_err.contains("Restored baseline pin mismatch"));
    assert!(
        journal_path.exists(),
        "journal must be retained on pin mismatch"
    );
}

#[test]
fn trace_ledger_strict_schema_version_and_json_validation() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("trace.db");
    let mut ledger = Ledger::open(&db_path, false).unwrap();

    let sample_raw = json!({
        "format": "code-factory-trace-summary/v1",
        "runs": [{
            "run_id": "run-1",
            "harness": "code_factory",
            "route": "opencode",
            "outcome": "blocked",
            "failure_class": "outcome_unknown",
            "lifecycle": {
                "owned_processes_running": 1,
                "owned_processes_unknown": 0,
                "cleanup": "pending"
            }
        }]
    });
    let raw_bytes = serde_json::to_vec(&sample_raw).unwrap();
    let normalized = trace_ledger::normalize(&raw_bytes, "sanitized-v1", "mock").unwrap();
    let imported = ledger.import_summary("source-1", &normalized).unwrap();
    assert_eq!(imported["status"], "imported");
    assert_eq!(
        ledger.replay("source-1", &normalized).unwrap()["passed"],
        true
    );
    drop(ledger);

    // Trailing bytes and duplicate keys in trace_ledger::load must be rejected
    assert!(trace_ledger::load(b"{\"a\":1} trailing").is_err());
    assert!(trace_ledger::load(b"{\"a\":1,\"a\":2}").is_err());

    // Corrupt trace_meta: multiple rows ([1, 1]) must be rejected by Ledger::open
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("INSERT INTO trace_meta VALUES (1)", [])
            .unwrap();
    }
    assert!(
        Ledger::open(&db_path, true)
            .err()
            .unwrap()
            .to_string()
            .contains("unsupported ledger version")
    );

    // Empty trace_meta (0 rows on existing DB with tables) must be rejected
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("DELETE FROM trace_meta", []).unwrap();
    }
    assert!(
        Ledger::open(&db_path, true)
            .err()
            .unwrap()
            .to_string()
            .contains("unsupported ledger version")
    );

    // Wrong version ([99]) must be rejected
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("INSERT INTO trace_meta VALUES (99)", [])
            .unwrap();
    }
    assert!(
        Ledger::open(&db_path, true)
            .err()
            .unwrap()
            .to_string()
            .contains("unsupported ledger version")
    );
}

#[test]
fn check_bonsai_loopback_http_bounds_and_cli_error_json() {
    // 1. Strict loopback validation (rejects shorthand/decimal IPv4, non-loopback, credentials, query, fragment)
    for bad in [
        "http://localhost:8080/v1",
        "http://127.1:8080/v1",
        "http://127.0.1:8080/v1",
        "http://2130706433:8080/v1",
        "http://192.168.1.1:8080/v1",
        "https://127.0.0.1:8080/v1",
        "http://user:pass@127.0.0.1:8080/v1",
        "http://127.0.0.1:8080/v1?x=1",
        "http://127.0.0.1:8080/v1#frag",
        "http://127.0.0.1:8080/other",
    ] {
        assert!(
            check_bonsai::loopback(bad, "/v1").is_err(),
            "expected {bad} to be rejected"
        );
    }
    assert_eq!(
        check_bonsai::loopback("http://127.0.0.1:8080/v1/", "/v1").unwrap(),
        "http://127.0.0.1:8080/v1"
    );
    assert_eq!(
        check_bonsai::loopback("http://[::1]:8080/v1", "/v1").unwrap(),
        "http://[::1]:8080/v1"
    );

    // 2. Mock local HTTP server for non-2xx, redirects, >4MiB streaming limit, and valid full preflight
    let prism_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let prism_port = prism_listener.local_addr().unwrap().port();
    let prism_handle = thread::spawn(move || {
        if let Ok((mut stream, _)) = prism_listener.accept() {
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let body = serde_json::to_string(&json!({
                "data": [{"id": check_bonsai::MODEL_ID}]
            }))
            .unwrap();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });

    let oc_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let oc_port = oc_listener.local_addr().unwrap().port();
    let oc_handle = thread::spawn(move || {
        // First 3 requests: valid preflight (/global/health, /config, /provider)
        // 4th request: 500 error
        // 5th request: 302 redirect
        // 6th request: >4MiB response
        for i in 0..6 {
            let Ok((mut stream, _)) = oc_listener.accept() else {
                break;
            };
            let mut buf = [0u8; 2048];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let resp = match i {
                0 => {
                    assert!(req.starts_with("GET /global/health "));
                    let b = r#"{"version":"1.19.0","healthy":true}"#;
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        b.len(),
                        b
                    )
                }
                1 => {
                    assert!(req.starts_with("GET /config "));
                    let b = serde_json::to_string(&json!({
                        "model": check_bonsai::MODEL,
                        "small_model": check_bonsai::MODEL,
                        "enabled_providers": [check_bonsai::PROVIDER_ID],
                        "provider": {
                            check_bonsai::PROVIDER_ID: {
                                "npm": "@ai-sdk/openai-compatible",
                                "options": {
                                    "baseURL": format!("http://127.0.0.1:{prism_port}/v1")
                                },
                                "models": {
                                    check_bonsai::MODEL_ID: {
                                        "id": check_bonsai::MODEL_ID,
                                        "cost": {"input": 0, "output": 0, "cache_read": 0, "cache_write": 0},
                                        "limit": {"context": 8192, "output": 1024}
                                    }
                                }
                            }
                        }
                    }))
                    .unwrap();
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        b.len(),
                        b
                    )
                }
                2 => {
                    assert!(req.starts_with("GET /provider "));
                    let b = serde_json::to_string(&json!({
                        "connected": [check_bonsai::PROVIDER_ID],
                        "all": [{
                            "id": check_bonsai::PROVIDER_ID,
                            "models": {
                                check_bonsai::MODEL_ID: {
                                    "status": "active",
                                    "cost": {"input": 0, "output": 0, "cache": {"read": 0, "write": 0}}
                                }
                            }
                        }]
                    }))
                    .unwrap();
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        b.len(),
                        b
                    )
                }
                3 => {
                    let b = r#"{"version":"1.19.0","healthy":true}"#;
                    format!(
                        "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        b.len(),
                        b
                    )
                }
                4 => {
                    "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
                }
                _ => {
                    let header = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n";
                    let _ = stream.write_all(header.as_bytes());
                    let chunk = vec![b' '; 65536];
                    for _ in 0..66 {
                        if stream.write_all(&chunk).is_err() {
                            break;
                        }
                    }
                    continue;
                }
            };
            let _ = stream.write_all(resp.as_bytes());
        }
    });

    let rt = tokio::runtime::Runtime::new().unwrap();
    let oc_url = format!("http://127.0.0.1:{oc_port}");

    // Valid check
    let ok_val = rt.block_on(check_bonsai::check(&oc_url)).unwrap();
    assert_eq!(ok_val["passed"], true);
    assert_eq!(ok_val["model"], check_bonsai::MODEL);

    // HTTP 500 must be rejected even though body is valid JSON
    let err_500 = rt
        .block_on(check_bonsai::check(&oc_url))
        .unwrap_err()
        .to_string();
    assert!(err_500.contains("HTTP status"), "got: {err_500}");

    // HTTP 302 redirect must be rejected
    let err_302 = rt
        .block_on(check_bonsai::check(&oc_url))
        .unwrap_err()
        .to_string();
    assert!(
        err_302.contains("HTTP status") && err_302.contains("302"),
        "redirect was not rejected as an HTTP 302 error: {err_302}"
    );

    // >4MiB response must be rejected with "response exceeds limit"
    let err_large = rt
        .block_on(check_bonsai::check(&oc_url))
        .unwrap_err()
        .to_string();
    assert!(
        err_large.contains("response exceeds limit"),
        "got: {err_large}"
    );

    let _ = prism_handle.join();
    let _ = oc_handle.join();

    // CLI error path: must print {"passed":false,"error":...} to stdout and exit 1
    let cli_out = Command::new(controller_bin())
        .args(["check-bonsai", "--opencode-url", "http://localhost:4096"])
        .output()
        .unwrap();
    assert_eq!(cli_out.status.code(), Some(1));
    let cli_json: Value = serde_json::from_slice(&cli_out.stdout).unwrap();
    assert_eq!(cli_json["passed"], false);
    assert!(
        cli_json["error"]
            .as_str()
            .unwrap()
            .contains("endpoint must be an explicit HTTP loopback URL")
    );
}

#[test]
fn spark_control_pause_resume_and_symlink_lock_protection() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let status_path = root.join("status.json");
    let initial_status = "{\n  \"z_preserve\": 1,\n  \"status\": \"paused\",\n  \"current\": null,\n  \"circuits\": {\n    \"opencode/x-free\": {\n      \"kind\": \"denied\",\n      \"until\": 99999999\n    }\n  }\n}\n";
    fs::write(&status_path, initial_status).unwrap();

    let pause_msg = recovery::spark_pause(root, Some("Operator pause requested")).unwrap();
    assert!(pause_msg.contains("Pause requested"));
    assert!(root.join("PAUSE").exists());

    // Symlink supervisor.lock must be rejected by O_NOFOLLOW in spark_resume
    let target = root.join("symlink_target");
    fs::write(&target, "").unwrap();
    std::os::unix::fs::symlink(&target, root.join("supervisor.lock")).unwrap();
    assert!(recovery::spark_resume(root, "Inspected stop").is_err());
    fs::remove_file(root.join("supervisor.lock")).unwrap();

    // Normal resume succeeds and preserves key ordering
    let resume_msg = recovery::spark_resume(root, "Inspected confirmed stop").unwrap();
    assert!(resume_msg.contains("Resume prepared"));
    assert!(!root.join("PAUSE").exists());

    let updated_text = fs::read_to_string(&status_path).unwrap();
    assert!(
        updated_text.find("\"z_preserve\"").unwrap() < updated_text.find("\"status\"").unwrap()
    );
    let updated_status: Value = serde_json::from_str(&updated_text).unwrap();
    assert_eq!(updated_status["status"], "waiting");
    assert_eq!(
        updated_status["operator_resume"]["reason"],
        "Inspected confirmed stop"
    );

    let events = fs::read_to_string(root.join("events.ndjson")).unwrap();
    assert!(events.contains("operator_resume"));
}

#[test]
fn trace_cost_numeric_representation_keeps_python_observation_hashes() {
    for (cost, expected) in [(json!(0), "0"), (json!(0.0), "0.0"), (Value::Null, "null")] {
        let input = json!({
            "format": "code-factory-trace-summary/v1",
            "runs": [{"run_id": "numeric-cost", "harness": "code_factory", "route": "opencode",
                      "outcome": "blocked", "failure_class": "outcome_unknown", "reported_cost": cost}]
        });
        let normalized =
            trace_ledger::normalize(&serde_json::to_vec(&input).unwrap(), "sanitized-v1", "mock")
                .unwrap();
        assert_eq!(
            trace_ledger::canonical(&normalized["runs"][0]["reported_cost"]),
            expected
        );
    }
}
