mod common;
use code_factory_controller::{
    execution,
    model::*,
    supervisor::{self, Plan, ProcessReceipt},
};
use common::*;
use serde_json::json;
use std::{
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const BIN: &str = env!("CARGO_BIN_EXE_code-factory-controller");
fn plan(root: &Path, script: &str, seconds: u64) -> Plan {
    Plan {
        execution_id: new_id(),
        argv: vec!["/usr/bin/python3".into(), "-c".into(), script.into()],
        cwd: root.into(),
        log: root.join("log"),
        stdout: None,
        commit: "a".repeat(40),
        seconds,
        receipt: root.join("process.json"),
    }
}
fn run(plan: &Plan) -> ProcessReceipt {
    let path = plan.cwd.join("plan.json");
    supervisor::write_json(&path, plan).unwrap();
    let mut child = Command::new(BIN)
        .args(["verifier-supervisor", "--plan", path.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let control = child.stdin.take().unwrap();
    let deadline = Instant::now() + Duration::from_secs(plan.seconds + 5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "supervisor did not finish within its bound"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    drop(control);
    assert!(
        status.success(),
        "supervisor facilities must be supported; no silent lifecycle skip"
    );
    supervisor::read_json(&plan.receipt).unwrap()
}
fn wait_path(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !path.exists() {
        assert!(Instant::now() < deadline, "process fixture did not start");
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn normal_child_tree_is_reaped_and_receipt_has_actual_kernel_identity() {
    let temp = tempfile::tempdir().unwrap();
    let observed = run(&plan(
        temp.path(),
        "import subprocess; p=subprocess.Popen(['/usr/bin/true']); p.wait()",
        2,
    ));
    assert!(observed.passed());
    assert!(observed.quiescent, "{observed:?}");
    assert!(!observed.leaked_descendants);
    assert!(observed.root_pid.is_some());
    assert!(observed.supervisor.start_ticks > 0);
    assert!(!observed.supervisor.boot_id.is_empty());
}
#[test]
fn detached_descendant_invalidates_zero_root_exit_and_is_reaped() {
    let temp = tempfile::tempdir().unwrap();
    let observed = run(&plan(
        temp.path(),
        "import os,time; p=os.fork();\nif p==0:\n os.setsid(); q=os.fork();\n if q==0: time.sleep(30)\n os._exit(0)\nos._exit(0)",
        2,
    ));
    assert_eq!(observed.root_exit_status, Some(0));
    assert!(!observed.passed());
    assert!(observed.leaked_descendants);
    assert!(observed.quiescent, "{observed:?}");
    assert!(observed.descendants_reaped >= 1);
    assert_eq!(
        observed.termination_reason,
        "owned_descendants_after_root_exit"
    );
}
#[test]
fn deadline_cleans_root_and_detached_descendants_without_group_signals() {
    let temp = tempfile::tempdir().unwrap();
    let observed = run(&plan(
        temp.path(),
        "import os,time; p=os.fork();\nif p==0: os.setsid()\ntime.sleep(30)",
        1,
    ));
    assert!(!observed.passed());
    assert!(observed.quiescent, "{observed:?}");
    assert_eq!(observed.termination_reason, "timeout");
}
#[test]
fn controller_pipe_loss_causes_cleanup_while_unrelated_process_survives() {
    let temp = tempfile::tempdir().unwrap();
    let mut unrelated = Command::new("/usr/bin/sleep").arg("30").spawn().unwrap();
    let mut input = plan(
        temp.path(),
        "import pathlib,time; pathlib.Path('ready').write_text('yes'); time.sleep(30)",
        2,
    );
    input.receipt = temp.path().join("receipt.json");
    let path = temp.path().join("plan.json");
    supervisor::write_json(&path, &input).unwrap();
    let mut child = Command::new(BIN)
        .args(["verifier-supervisor", "--plan", path.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let control = child.stdin.take().unwrap();
    wait_path(&temp.path().join("ready"));
    drop(control);
    let deadline = Instant::now() + Duration::from_secs(4);
    while child.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let observed: ProcessReceipt = supervisor::read_json(&input.receipt).unwrap();
    assert_eq!(observed.termination_reason, "controller_disconnected");
    assert!(observed.quiescent, "{observed:?}");
    assert!(!observed.passed());
    assert!(unrelated.try_wait().unwrap().is_none());
    // Only this test's own unreaped Child is cleaned; no recorded numeric PID.
    unrelated.kill().unwrap();
    unrelated.wait().unwrap();
}
#[tokio::test]
async fn zero_exit_with_owned_descendant_fails_independent_receipt_and_delivery() {
    let f = Fixture::new().await;
    let mut job = f.assigned("job", "session", TOKEN).await;
    let candidate = Fixture::commit(&job, "result.txt", "valid result").await;
    job.profile.checks[0].argv = vec![
        "/usr/bin/python3".into(),
        "-c".into(),
        "import subprocess; subprocess.Popen(['/usr/bin/sleep','30'],start_new_session=True)"
            .into(),
    ];
    let receipt = execution::verify(&f.config, &job, &candidate, "owned-leak")
        .await
        .unwrap();
    assert_eq!(receipt.commands[0].exit_status, 0);
    assert_eq!(receipt.outcome, "failed");
    assert_eq!(
        receipt.environment["checks"][0]["executor"]["leaked_descendants"],
        true
    );
    assert_eq!(
        receipt.environment["checks"][0]["executor"]["quiescent"],
        true
    );
    assert_eq!(receipt.feature_map[0]["verified"], false);
    assert!(f.engine.get("job").unwrap().delivery.is_none());
}
#[tokio::test]
async fn completed_execution_cache_prevents_duplicate_effects_and_detects_log_corruption() {
    let f = Fixture::new().await;
    let mut job = f.assigned("job", "session", TOKEN).await;
    let candidate = Fixture::commit(&job, "result.txt", "valid").await;
    let counter = f.temp.path().join("count");
    job.profile.checks[0].argv = vec![
        "/usr/bin/python3".into(),
        "-c".into(),
        format!(
            "from pathlib import Path; p=Path({:?}); p.write_text(p.read_text()+'x' if p.exists() else 'x')",
            counter.to_str().unwrap()
        ),
    ];
    let first = execution::verify(&f.config, &job, &candidate, "cached")
        .await
        .unwrap();
    let second = execution::verify(&f.config, &job, &candidate, "cached")
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(counter).unwrap(), "x");
    assert_eq!(first.environment_id, second.environment_id);
    let log = f.config.data_dir.join("artifacts").join(
        first.artifacts[0]
            .reference
            .strip_prefix("artifact:")
            .unwrap(),
    );
    std::fs::write(log, "altered").unwrap();
    assert!(
        execution::verify(&f.config, &job, &candidate, "cached")
            .await
            .is_err()
    );
}
#[tokio::test]
async fn unfinished_intent_fences_all_new_verification_without_pid_signals_or_effect_replay() {
    let f = Fixture::new().await;
    let mut job = f.assigned("job", "session", TOKEN).await;
    let candidate = Fixture::commit(&job, "result.txt", "valid").await;
    let ledger = f
        .config
        .data_dir
        .join("verification-executions")
        .join("unfinished");
    std::fs::create_dir_all(&ledger).unwrap();
    // Reused/unrelated numeric identities in stale diagnostics never authorize a signal.
    supervisor::write_json(
        &ledger.join("intent.json"),
        &json!({"pid":std::process::id(),"start_ticks":0,"execution_id":"old"}),
    )
    .unwrap();
    let marker = f.temp.path().join("must-not-execute");
    job.profile.checks[0].argv = vec!["/usr/bin/touch".into(), marker.to_str().unwrap().into()];
    assert!(
        execution::verify(&f.config, &job, &candidate, "fresh-logical-id")
            .await
            .is_err()
    );
    assert!(!marker.exists());
    let status = execution::execution_status(&f.config).unwrap();
    assert_eq!(status["signals_sent"], false);
    assert_eq!(status["effects_replayed"], false);
}
#[tokio::test]
async fn interrupting_verifier_future_releases_control_pipe_and_blocks_retry() {
    let f = Fixture::new().await;
    let mut job = f.assigned("job", "session", TOKEN).await;
    let candidate = Fixture::commit(&job, "result.txt", "valid").await;
    let marker = f.temp.path().join("started");
    job.profile.checks[0].argv = vec![
        "/usr/bin/python3".into(),
        "-c".into(),
        format!(
            "from pathlib import Path; import time; Path({:?}).write_text('started'); time.sleep(30)",
            marker.to_str().unwrap()
        ),
    ];
    let config = f.config.clone();
    let j = job.clone();
    let c = candidate.clone();
    let task = tokio::spawn(async move { execution::verify(&config, &j, &c, "interrupted").await });
    tokio::time::timeout(Duration::from_secs(3), async {
        while !marker.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(
        execution::verify(&f.config, &job, &candidate, "interrupted")
            .await
            .is_err()
    );
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let mut receipts = vec![];
        for directory in
            std::fs::read_dir(f.config.data_dir.join("verification-executions")).unwrap()
        {
            for entry in std::fs::read_dir(directory.unwrap().path()).unwrap() {
                let path = entry.unwrap().path();
                if path.to_string_lossy().ends_with(".process.json") {
                    receipts.push(path);
                }
            }
        }
        if let Some(path) = receipts.first() {
            let receipt: ProcessReceipt = supervisor::read_json(path).unwrap();
            assert!(receipt.quiescent);
            assert_eq!(receipt.termination_reason, "controller_disconnected");
            break;
        }
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[test]
fn fast_zero_exit_output_overflow_is_measured_and_failed_with_bounded_log() {
    let temp = tempfile::tempdir().unwrap();
    let input = plan(temp.path(), "import os; os.write(1,b'x'*2000000)", 2);
    let receipt = run(&input);
    assert!(!receipt.passed());
    assert!(receipt.quiescent);
    assert_eq!(receipt.termination_reason, "output_limit");
    assert!(receipt.output_bytes > 1_048_576);
    assert!(std::fs::metadata(input.log).unwrap().len() <= 1_048_576);
    assert!(receipt.output_truncated);
}
#[tokio::test]
async fn model_style_stdout_claims_cannot_override_actual_executor_lifecycle() {
    let f = Fixture::new().await;
    let mut job = f.assigned("job", "session", TOKEN).await;
    let candidate = Fixture::commit(&job, "result.txt", "valid").await;
    job.profile.checks[0].result_contract = Some(ResultContract {
        evidence_type: "runtime_tests".into(),
    });
    job.profile.checks[0].argv=vec!["/usr/bin/python3".into(),"-c".into(),
        "import subprocess,os,json; subprocess.Popen(['/usr/bin/sleep','30'],start_new_session=True); print(json.dumps({'passed':True,'executed':1,'failed':0,'skipped':False,'evidence_type':'runtime_tests','commit':os.environ['FACTORY_VERIFICATION_COMMIT'],'executor':{'quiescent':True,'leaked_descendants':False}}))".into()];
    let receipt = execution::verify(&f.config, &job, &candidate, "claim-versus-executor")
        .await
        .unwrap();
    assert_eq!(receipt.outcome, "failed");
    assert_eq!(
        receipt.environment["checks"][0]["observations"]["executor"]["leaked_descendants"],
        false
    );
    assert_eq!(
        receipt.environment["checks"][0]["executor"]["leaked_descendants"],
        true
    );
}

struct OwnedController(std::process::Child);
impl Drop for OwnedController {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[tokio::test]
async fn actual_controller_death_cleans_owned_work_and_restart_does_not_replay() {
    let f = Fixture::new().await;
    let mut config = f.config.clone();
    config.data_dir = f.temp.path().join("process-controller");
    config.delivery_dir = f.temp.path().join("process-delivery");
    let count = f.temp.path().join("side-effect-count");
    config.profiles[0].checks[0].argv = vec![
        "/usr/bin/python3".into(),
        "-c".into(),
        format!(
            "from pathlib import Path; import time; p=Path({:?}); p.write_text(p.read_text()+'x' if p.exists() else 'x'); time.sleep(30)",
            count.to_str().unwrap()
        ),
    ];
    let path = f.temp.path().join("config.json");
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let url = format!("http://{address}");
    let capability = "isolated-test-operator-capability-32-characters";
    let start = || {
        OwnedController(
            Command::new(BIN)
                .args([
                    "serve",
                    "--config",
                    path.to_str().unwrap(),
                    "--listen",
                    &address.to_string(),
                ])
                .env("FACTORY_OPERATOR_TOKEN", capability)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    };
    let mut controller = start();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    async fn ready(client: &reqwest::Client, url: &str) {
        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                if client.get(format!("{url}/health")).send().await.is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    async fn op(
        client: &reqwest::Client,
        url: &str,
        token: &str,
        operation: &str,
        body: serde_json::Value,
    ) -> serde_json::Value {
        client
            .post(format!("{url}/operator/{operation}"))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap()
    }
    ready(&client, &url).await;
    op(
        &client,
        &url,
        capability,
        "admit",
        serde_json::to_value(f.input("job")).unwrap(),
    )
    .await;
    op(
        &client,
        &url,
        capability,
        "prepare",
        json!({"job_id":"job","mode":"manual"}),
    )
    .await;
    let job: Job = serde_json::from_value(
        op(
            &client,
            &url,
            capability,
            "bind",
            json!({"job_id":"job","session_id":"session","worker_token":TOKEN}),
        )
        .await,
    )
    .unwrap();
    let candidate = Fixture::commit(&job, "result.txt", "valid").await;
    client
        .post(format!("{url}/v1/factory_request_verification"))
        .bearer_auth(TOKEN)
        .json(&f.verify_request(&job, &candidate, "verify"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let request = client
        .post(format!("{url}/operator/advance"))
        .bearer_auth(capability)
        .json(&json!({"job_id":"job"}));
    let advance = tokio::spawn(async move { request.send().await });
    tokio::time::timeout(Duration::from_secs(4), async {
        while !count.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    // This is the test's own unreaped Child, never a recovered numeric PID.
    controller.0.kill().unwrap();
    controller.0.wait().unwrap();
    let _ = advance.await;
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let receipts = std::fs::read_dir(config.data_dir.join("verification-executions"))
            .unwrap()
            .flat_map(|entry| std::fs::read_dir(entry.unwrap().path()).unwrap())
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.to_string_lossy().ends_with(".process.json"))
            .collect::<Vec<_>>();
        if let Some(path) = receipts.first() {
            let receipt: ProcessReceipt = supervisor::read_json(path).unwrap();
            assert!(receipt.quiescent);
            assert_eq!(receipt.termination_reason, "controller_disconnected");
            break;
        }
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let _restarted = start();
    ready(&client, &url).await;
    let observed = op(
        &client,
        &url,
        capability,
        "advance",
        json!({"job_id":"job"}),
    )
    .await;
    assert_eq!(observed["state"], "blocked");
    assert!(observed["delivery"].is_null());
    assert_eq!(std::fs::read_to_string(count).unwrap(), "x");
    let diagnostics = op(&client, &url, capability, "executions", json!({})).await;
    assert_eq!(diagnostics["signals_sent"], false);
    assert_eq!(diagnostics["effects_replayed"], false);
}
#[tokio::test]
async fn legacy_integration_receipt_without_executor_evidence_cannot_deliver() {
    let f = Fixture::new().await;
    let mut job = f.accepted("job").await;
    let candidate = job.candidate.clone().unwrap();
    job.integrated_commit = Some(candidate);
    job.integration_base = Some(f.base.clone());
    job.outcome = Some("integration_verified".into());
    let receipt = job.receipt.as_mut().unwrap();
    receipt.environment["checks"][0]
        .as_object_mut()
        .unwrap()
        .remove("executor");
    f.engine
        .save_if_current(
            &mut job,
            "simulate historical integration without ownership evidence",
        )
        .unwrap();
    let rejected = f.engine.advance("job").await.unwrap();
    assert_eq!(rejected.state, "blocked");
    assert!(rejected.delivery.is_none());
}

#[test]
fn started_journal_failure_after_spawn_cleans_owned_child() {
    let temp = tempfile::tempdir().unwrap();
    let input = plan(temp.path(), "import time; time.sleep(2)", 2);
    // Deterministic rename failure after spawn, without permissions/host changes.
    std::fs::create_dir(input.receipt.with_extension("started.json")).unwrap();
    let observed = run(&input);
    assert!(observed.root_pid.is_some());
    assert!(observed.root_exit_status.is_some());
    assert!(observed.quiescent, "{observed:?}");
    assert!(!observed.passed());
    assert_eq!(observed.termination_reason, "observer_error");
    assert!(observed.cleanup_error.is_some());
}

#[test]
fn missing_output_metadata_after_spawn_cleans_root_and_detached_child() {
    let temp = tempfile::tempdir().unwrap();
    let input = plan(
        temp.path(),
        "import pathlib,subprocess,time; subprocess.Popen(['/usr/bin/sleep','2'],start_new_session=True); pathlib.Path('log').unlink(); time.sleep(2)",
        2,
    );
    let observed = run(&input);
    assert!(observed.root_pid.is_some());
    assert!(observed.root_exit_status.is_some());
    assert!(observed.quiescent, "{observed:?}");
    assert!(observed.descendants_reaped >= 1);
    assert!(!observed.passed());
    assert_eq!(observed.termination_reason, "observer_error");
}

async fn substituted_recovery_evidence_is_fenced(kind: &str) {
    let f = Fixture::new().await;
    let mut job = f.assigned("job", "session", TOKEN).await;
    let candidate = Fixture::commit(&job, "result.txt", "valid").await;
    execution::verify(&f.config, &job, &candidate, "run-a")
        .await
        .unwrap();
    let b = execution::verify(&f.config, &job, &candidate, "run-b")
        .await
        .unwrap();
    let ledger = f.config.data_dir.join("verification-executions");
    let dir = |id: &str| {
        std::fs::read_dir(&ledger)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| {
                let intent: serde_json::Value =
                    supervisor::read_json(&p.join("intent.json")).unwrap();
                intent["verification_id"] == id
            })
            .unwrap()
    };
    let a_dir = dir("run-a");
    let b_dir = dir("run-b");
    match kind {
        "receipt" => {
            // Simulate an unfinished B receiving A's otherwise valid terminal record.
            std::fs::remove_file(b_dir.join("receipt.json")).unwrap();
            std::fs::copy(a_dir.join("receipt.json"), b_dir.join("receipt.json")).unwrap();
        }
        "intent" => {
            std::fs::copy(a_dir.join("intent.json"), b_dir.join("intent.json")).unwrap();
        }
        "process" => {
            let name = format!("{}.process.json", digest(&job.profile.checks[0].id));
            let foreign: serde_json::Value = supervisor::read_json(&a_dir.join(&name)).unwrap();
            std::fs::copy(a_dir.join(&name), b_dir.join(&name)).unwrap();
            let mut record: serde_json::Value =
                supervisor::read_json(&b_dir.join("receipt.json")).unwrap();
            record["receipt"]["environment"]["checks"][0]["executor"] = foreign;
            // A coherent checksum cannot establish that another run's kernel facts belong to B.
            let substituted: Receipt = serde_json::from_value(record["receipt"].clone()).unwrap();
            record["receipt_sha256"] = json!(digest(serde_json::to_vec(&substituted).unwrap()));
            supervisor::write_json(&b_dir.join("receipt.json"), &record).unwrap();
        }
        "legacy" => {
            let intent: serde_json::Value =
                supervisor::read_json(&b_dir.join("intent.json")).unwrap();
            supervisor::write_json(&b_dir.join("intent.json"), &json!({
                "version":1,"execution_id":intent["execution_id"],"binding":intent["binding"],
                "commit":intent["commit"],"profile_digest":b.config_digest,"phase":"intent_before_launch"
            })).unwrap();
        }
        "artifact" => {
            let name = b.artifacts[0].reference.strip_prefix("artifact:").unwrap();
            std::fs::write(
                f.config.data_dir.join("artifacts").join(name),
                "changed bytes",
            )
            .unwrap();
        }
        _ => unreachable!(),
    }
    let status = execution::execution_status(&f.config).unwrap();
    let a_key = a_dir.file_name().unwrap().to_str().unwrap();
    assert!(
        status["executions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["execution_key"] == a_key && r["terminal_receipt"] == true),
        "valid untouched run must still be recognized: {status}"
    );
    let key = b_dir.file_name().unwrap().to_str().unwrap();
    let observed = status["executions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["execution_key"] == key)
        .unwrap();
    assert_eq!(observed["terminal_receipt"], false, "{kind}: {status}");
    assert_eq!(observed["status"], "outcome_unknown_requires_host_review");
    assert_eq!(status["signals_sent"], false);
    assert_eq!(status["effects_replayed"], false);
    let error = execution::verify(&f.config, &job, &candidate, "run-b")
        .await
        .unwrap_err();
    let reason = format!("{error:#}");
    if kind == "process" {
        assert!(
            reason.contains("cached check belongs to another execution"),
            "{reason}"
        );
    } else if kind == "artifact" {
        assert!(
            reason.contains("cached verification log integrity mismatch"),
            "{reason}"
        );
    }
    let marker = f.temp.path().join("must-not-replay");
    job.profile.checks[0].argv = vec!["/usr/bin/touch".into(), marker.to_str().unwrap().into()];
    assert!(
        execution::verify(&f.config, &job, &candidate, "run-c")
            .await
            .is_err(),
        "{kind}: global fence released"
    );
    assert!(!marker.exists(), "{kind}: new effects occurred");
}

#[tokio::test]
async fn foreign_successful_receipt_cannot_release_another_runs_fence() {
    substituted_recovery_evidence_is_fenced("receipt").await;
}
#[tokio::test]
async fn foreign_intent_cannot_release_another_runs_fence() {
    substituted_recovery_evidence_is_fenced("intent").await;
}
#[tokio::test]
async fn foreign_process_receipt_with_rehashed_wrapper_is_rejected() {
    substituted_recovery_evidence_is_fenced("process").await;
}
#[tokio::test]
async fn corrupted_artifact_blocks_global_recovery_and_diagnostics() {
    substituted_recovery_evidence_is_fenced("artifact").await;
}

#[test]
fn terminal_journal_failure_occurs_after_cleanup_and_reports_unavailable_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let input = plan(
        temp.path(),
        "import pathlib,subprocess; subprocess.Popen(['/usr/bin/python3','-c',\"import pathlib,time; time.sleep(1); pathlib.Path('late-effect').write_text('leaked')\"],start_new_session=True); pathlib.Path('ready').write_text('yes')",
        2,
    );
    std::fs::create_dir(&input.receipt).unwrap();
    let path = temp.path().join("plan.json");
    supervisor::write_json(&path, &input).unwrap();
    let mut child = Command::new(BIN)
        .args(["verifier-supervisor", "--plan", path.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let control = child.stdin.take().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("journal failure exceeded cleanup bound");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    drop(control);
    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(
        error.contains("terminal execution record unavailable"),
        "{error}"
    );
    assert!(error.contains("owned quiescence observed: true"), "{error}");
    assert!(temp.path().join("ready").exists());
    let started: ProcessReceipt =
        supervisor::read_json(&input.receipt.with_extension("started.json")).unwrap();
    assert!(started.root_pid.is_some());
    assert!(supervisor::read_json::<ProcessReceipt>(&input.receipt).is_err());
    std::thread::sleep(Duration::from_millis(1200));
    assert!(!temp.path().join("late-effect").exists());
}

#[tokio::test]
async fn legacy_intent_lacking_run_binding_fields_remains_unresolved() {
    substituted_recovery_evidence_is_fenced("legacy").await;
}
