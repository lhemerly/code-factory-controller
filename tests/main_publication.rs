mod common;
use anyhow::{Result, bail};
use code_factory_controller::{
    delivery_audit::{audit_cycle, inspect_job},
    goal_acceptance::run_acceptance,
    goal_state::save,
    main_publication::{GitHub, PublicationRemote, Publisher, checked_ci, ci_state, identity},
    recovery::{read_json, sha, supervised_command, supervised_owner_command},
    supervisor::{self, Plan, ProcessReceipt},
};
use common::*;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_code-factory-controller");
const POLICY_ORACLE_JSON: &str = include_str!("fixtures/native-main-publication-policy.json");

#[test]
fn frozen_326_pure_publication_policy_oracle_cases() {
    let doc: Value = serde_json::from_str(POLICY_ORACLE_JSON).expect("valid policy oracle json");
    let cases = doc["cases"].as_array().expect("cases array");
    assert_eq!(cases.len(), 326, "expected 326 frozen oracle cases");

    let mut accepted_count = 0usize;
    for case in cases {
        let id = case["id"].as_str().expect("case id");
        let op = case["operation"].as_str().expect("case operation");
        let input = &case["input"];
        let expected_ok = case["expected"]["ok"].as_bool().expect("expected.ok");
        if expected_ok {
            accepted_count += 1;
        }

        let actual: Result<Value> = match op {
            "identity" => identity(&input["binding"], &input["snapshot"]).map(|()| Value::Null),
            "ci_state" => ci_state(&input["binding"], &input["checks"]).map(|(s, v)| json!([s, v])),
            "checked_ci" => checked_ci(&input["binding"], &input["snapshot"]),
            other => panic!("unexpected operation {other} in case {id}"),
        };

        match (expected_ok, actual) {
            (true, Ok(val)) => {
                assert_eq!(
                    val, case["expected"]["value"],
                    "oracle value mismatch on case {id}"
                );
            }
            (true, Err(e)) => {
                panic!("oracle case {id} expected success but failed: {e:#}");
            }
            (false, Ok(val)) => {
                panic!("oracle case {id} expected rejection but succeeded with {val}");
            }
            (false, Err(_)) => {}
        }
    }
    assert_eq!(accepted_count, 67, "expected 67 accepted oracle cases");
}

#[derive(Clone, Default)]
struct FakeRemote {
    head: String,
    pushes: Vec<String>,
    fail: Option<&'static str>,
    checks: Vec<Value>,
    main_checks: Option<Vec<Value>>,
    refs: HashMap<String, String>,
    preflight_pushes: Vec<(String, String)>,
    preflight_fail: Option<&'static str>,
}

impl FakeRemote {
    fn new(head: &str) -> Self {
        Self {
            head: head.to_string(),
            ..Default::default()
        }
    }
}

impl PublicationRemote for FakeRemote {
    fn snapshot(&mut self, b: &Value, commit: Option<&str>) -> Result<Value> {
        let checked = commit.unwrap_or(&self.head);
        let main_checks = self
            .main_checks
            .clone()
            .unwrap_or_else(|| self.checks.clone());
        Ok(json!({
            "repository": {
                "id": b["repository_id"],
                "full_name": b["repository"],
                "private": true,
                "default_branch": "main",
            },
            "head": self.head,
            "checked_commit": checked,
            "checks": self.checks,
            "main_checks": main_checks,
        }))
    }

    fn push(&mut self, _b: &Value, _repo: &Path, commit: &str) -> Result<()> {
        self.pushes.push(commit.to_string());
        if self.fail == Some("before") {
            bail!("lost before push");
        }
        self.head = commit.to_string();
        if self.fail == Some("after") {
            bail!("lost response");
        }
        Ok(())
    }

    fn preflight_head(&mut self, _b: &Value, branch: &str) -> Result<Option<String>> {
        Ok(self.refs.get(branch).cloned())
    }

    fn push_preflight(
        &mut self,
        _b: &Value,
        _repo: &Path,
        commit: &str,
        branch: &str,
    ) -> Result<()> {
        self.preflight_pushes
            .push((branch.to_string(), commit.to_string()));
        if self.preflight_fail == Some("before") {
            bail!("unobserved preflight failure");
        }
        self.refs.insert(branch.to_string(), commit.to_string());
        self.checks = vec![json!({
            "name": "controller",
            "status": "in_progress",
            "conclusion": null,
        })];
        self.main_checks = None;
        if self.preflight_fail == Some("after") {
            bail!("lost preflight response");
        }
        Ok(())
    }
}

struct RealDeliveryEnv {
    fixture: ReleasedDeliveryFixture,
    goal_root: PathBuf,
    goal: Value,
    binding: Value,
    commit: String,
    result_path: PathBuf,
}

struct ReleasedDeliveryFixture {
    _temp: TempDir,
    base: String,
}

impl RealDeliveryEnv {
    async fn setup() -> Self {
        let fixture = Fixture::new().await;
        let accepted = fixture.accepted("job-pub-1").await;
        let integrated = fixture.engine.advance("job-pub-1").await.unwrap();
        assert_eq!(integrated.state, "done");
        let saved = fixture.engine.reconcile_delivery("job-pub-1").unwrap();
        let done_status = fixture.engine.status(&saved);
        let commit = done_status["integrated_commit"]
            .as_str()
            .unwrap()
            .to_string();

        let goal_root = fixture.temp.path().to_path_buf();
        let cycle_dir = goal_root.join("cycles/000001");
        fs::create_dir_all(&cycle_dir).unwrap();

        let mut cfg_in_cycle = fixture.config.clone();
        cfg_in_cycle.verifier_executable = Some(PathBuf::from(BIN));
        let cycle_ctrl_path = cycle_dir.join("controller.json");
        fs::write(
            &cycle_ctrl_path,
            serde_json::to_vec_pretty(&cfg_in_cycle).unwrap(),
        )
        .unwrap();

        let result_path = cycle_dir.join("result.json");
        let out = json!({
            "goal_id": "fixture",
            "sequence": 1,
            "ended_utc": "2026-10-18T00:00:00Z",
            "status": "accepted",
            "cleanup_confirmed": true,
            "job_id": accepted.input.job_id,
            "acceptance": done_status,
            "model": "opencode/big-pickle",
        });
        save(&result_path, &out).unwrap();

        let status_json = json!({
            "sequence": 1,
            "current": null,
            "status": "waiting",
            "last_cycle": {
                "sequence": 1,
                "result": result_path.to_string_lossy(),
            }
        });
        save(&goal_root.join("status.json"), &status_json).unwrap();

        let sup_json = json!({
            "controller": BIN,
            "controller_sha256": sha(Path::new(BIN)).unwrap(),
        });
        save(&goal_root.join("supervisor.json"), &sup_json).unwrap();

        let goal = json!({
            "id": "fixture",
            "root": goal_root.to_string_lossy(),
            "repository": fixture.config.repository.to_string_lossy(),
            "objective": "Fixture delivery",
        });
        save(&goal_root.join("goal.json"), &goal).unwrap();

        let binding = json!({
            "repository": "lhemerly/ink-detector",
            "repository_id": 1422424015,
            "private": true,
            "branch": "main",
            "remote_url": "git@github.com:lhemerly/ink-detector.git",
            "initial_head": fixture.base,
            "goal_id": "fixture",
            "goal_root": fs::canonicalize(&goal_root).unwrap().to_string_lossy(),
            "allowed_paths": ["result.txt", "src"],
            "protected_paths": ["README.md", "src/frozen.py"],
            "forbidden_patterns": ["private-runtime-value"],
            "required_checks": [],
        });

        // Operational inspect must acquire the real controller ownership lock.
        // Keep the artifacts alive while releasing the in-process fixture owner.
        let Fixture {
            temp, base, engine, ..
        } = fixture;
        drop(engine);
        Self {
            fixture: ReleasedDeliveryFixture { _temp: temp, base },
            goal_root,
            goal,
            binding,
            commit,
            result_path,
        }
    }

    fn publisher(&self, state_name: &str, remote: FakeRemote) -> Publisher<FakeRemote> {
        Publisher::open_with_remote(&self.goal_root.join(state_name), remote, PathBuf::from(BIN))
            .unwrap()
    }
}

#[tokio::test]
async fn publisher_state_machine_and_tamper_controls() {
    let env = RealDeliveryEnv::setup().await;

    // 1. Direct success and idempotent restart replay one push
    {
        let mut pub1 = env.publisher("state-direct", FakeRemote::new(&env.fixture.base));
        let res1 = pub1.publish(&env.binding, &env.goal).unwrap();
        assert_eq!(res1["state"], "confirmed");
        let res2 = pub1.publish(&env.binding, &env.goal).unwrap();
        assert_eq!(res2["state"], "confirmed");
        assert_eq!(pub1.remote.pushes, vec![env.commit.clone()]);
        let saved = read_json(&pub1.root.join("state.json")).unwrap();
        assert_eq!(saved["history"].as_array().unwrap().len(), 1);
    }

    // 2. Lost response after push reconciles same commit without duplicate push
    {
        let mut remote = FakeRemote::new(&env.fixture.base);
        remote.fail = Some("after");
        let mut pub2 = env.publisher("state-lost-after", remote);
        assert!(pub2.publish(&env.binding, &env.goal).is_err());
        let saved = read_json(&pub2.root.join("state.json")).unwrap();
        assert_eq!(saved["intent"]["state"], "uncertain");

        pub2.remote.fail = None;
        let res = pub2.publish(&env.binding, &env.goal).unwrap();
        assert_eq!(res["state"], "confirmed");
        assert_eq!(pub2.remote.pushes.len(), 1);
    }

    // 3. Unknown failed push before effect is never blindly retried
    {
        let mut remote = FakeRemote::new(&env.fixture.base);
        remote.fail = Some("before");
        let mut pub3 = env.publisher("state-lost-before", remote);
        assert!(pub3.publish(&env.binding, &env.goal).is_err());
        pub3.remote.fail = None;
        let err = pub3.publish(&env.binding, &env.goal).unwrap_err();
        assert!(
            format!("{err:#}").contains("no blind retry"),
            "unexpected error: {err:#}"
        );
        assert_eq!(pub3.remote.pushes.len(), 1);
    }

    // 4. Pre-main required CI fences missing, pending, failed, skipped, neutral
    {
        let mut b = env.binding.clone();
        b["required_checks"] = json!(["controller"]);
        let mut pub4 = env.publisher("state-pre-ci", FakeRemote::new(&env.fixture.base));
        for bad_checks in [
            vec![],
            vec![json!({"name": "controller", "status": "pending", "conclusion": null})],
            vec![json!({"name": "controller", "status": "in_progress", "conclusion": null})],
            vec![json!({"name": "controller", "status": "completed", "conclusion": "failure"})],
            vec![json!({"name": "controller", "status": "completed", "conclusion": "skipped"})],
            vec![json!({"name": "controller", "status": "completed", "conclusion": "neutral"})],
            vec![json!({"name": "other", "status": "completed", "conclusion": "failure"})],
        ] {
            pub4.remote.checks = bad_checks;
            assert!(pub4.publish(&b, &env.goal).is_err());
        }
        assert!(pub4.remote.pushes.is_empty());
        pub4.remote.checks =
            vec![json!({"name": "controller", "status": "completed", "conclusion": "success"})];
        let res = pub4.publish(&b, &env.goal).unwrap();
        assert_eq!(res["state"], "confirmed");
        assert_eq!(pub4.remote.pushes, vec![env.commit.clone()]);
    }

    // 5. Preflight workflow: awaiting_ci, lost response reconciliation, unobserved failure fence, delayed main CI
    {
        let mut b = env.binding.clone();
        b["preflight_branch_prefix"] = json!("factory/verified");
        b["required_checks"] = json!(["controller"]);

        let mut remote = FakeRemote::new(&env.fixture.base);
        remote.preflight_fail = Some("after");
        let mut pub5 = env.publisher("state-preflight", remote);
        assert!(pub5.publish(&b, &env.goal).is_err());
        pub5.remote.preflight_fail = None;

        // Reconciles preflight without second push_preflight
        let res_wait = pub5.publish(&b, &env.goal).unwrap();
        assert_eq!(res_wait["state"], "awaiting_ci");
        assert_eq!(pub5.remote.preflight_pushes.len(), 1);
        assert!(pub5.remote.pushes.is_empty());

        // Preflight green, but main_checks initially empty -> pushes main once and returns awaiting_ci
        pub5.remote.checks =
            vec![json!({"name": "controller", "status": "completed", "conclusion": "success"})];
        pub5.remote.main_checks = Some(vec![]);
        let res_main_wait = pub5.publish(&b, &env.goal).unwrap();
        assert_eq!(res_main_wait["state"], "awaiting_ci");
        assert_eq!(pub5.remote.pushes, vec![env.commit.clone()]);

        // Main CI fails first -> fences without second push, state remains remote_observed
        pub5.remote.main_checks = Some(vec![
            json!({"name": "controller", "status": "completed", "conclusion": "failure"}),
        ]);
        assert!(pub5.publish(&b, &env.goal).is_err());
        assert_eq!(pub5.remote.pushes.len(), 1);

        // Main CI rerun passes -> confirms once
        pub5.remote.main_checks = Some(vec![
            json!({"name": "controller", "status": "completed", "conclusion": "success"}),
        ]);
        let res_ok = pub5.publish(&b, &env.goal).unwrap();
        assert_eq!(res_ok["state"], "confirmed");
        pub5.publish(&b, &env.goal).unwrap();
        assert_eq!(pub5.remote.pushes.len(), 1);
        let saved = read_json(&pub5.root.join("state.json")).unwrap();
        assert_eq!(saved["history"].as_array().unwrap().len(), 1);
    }

    // 6. Unobserved preflight push is never blindly retried
    {
        let mut b = env.binding.clone();
        b["preflight_branch_prefix"] = json!("factory/verified");
        b["required_checks"] = json!(["controller"]);
        let mut remote = FakeRemote::new(&env.fixture.base);
        remote.preflight_fail = Some("before");
        let mut pub6 = env.publisher("state-preflight-unobserved", remote);
        assert!(pub6.publish(&b, &env.goal).is_err());
        pub6.remote.preflight_fail = None;
        let err = pub6.publish(&b, &env.goal).unwrap_err();
        assert!(format!("{err:#}").contains("no blind retry"));
        assert_eq!(pub6.remote.preflight_pushes.len(), 1);
        assert!(pub6.remote.pushes.is_empty());
    }

    // 7. Whole-goal completion gate and tamper check
    {
        let mut b = env.binding.clone();
        b["require_goal_completion"] = json!(true);
        let mut pub7 = env.publisher("state-whole-goal", FakeRemote::new(&env.fixture.base));
        // Missing goal-evaluation.json fails
        assert!(pub7.publish(&b, &env.goal).is_err());

        let code = "import json,os,pathlib;assert pathlib.Path('result.txt').read_text()=='verified\\n';print(json.dumps({'passed':True,'skipped':False,'executed':1,'failed':0,'commit':os.environ['FACTORY_VERIFICATION_COMMIT'],'evidence_type':'runtime_tests'}))";
        let mut goal_with_acc = env.goal.clone();
        goal_with_acc["acceptance"] = json!({
            "required_evidence": ["runtime_tests"],
            "completion_checks": [{
                "id": "whole-goal",
                "evidence_type": "runtime_tests",
                "argv": ["/usr/bin/python3", "-c", code],
            }]
        });
        let mut manifest = goal_with_acc["acceptance"].clone();
        manifest["repository"] = goal_with_acc["repository"].clone();
        manifest["verifier_executable"] = json!(BIN);
        let eval_res = run_acceptance(
            &manifest,
            &env.goal_root.join("whole-goal-state"),
            &env.commit,
            Path::new(BIN),
        );
        assert_eq!(eval_res["passed"], true, "{eval_res}");
        save(
            &env.goal_root.join("goal-evaluation.json"),
            &json!({
                "goal_id": env.goal["id"],
                "commit": env.commit,
                "result": eval_res,
            }),
        )
        .unwrap();

        let res = pub7.publish(&b, &goal_with_acc).unwrap();
        assert_eq!(res["state"], "confirmed");

        // Tamper with stdout evidence -> audit_result rejects
        let stdout_path = eval_res["receipts"][0]["stdout"].as_str().unwrap();
        fs::write(stdout_path, "tampered").unwrap();
        assert!(pub7.publish(&b, &goal_with_acc).is_err());
    }

    // 8. Delivery audit tamper controls: forged verification_id, corrupt archive, active worker
    {
        let mut pub8 = env.publisher("state-tamper", FakeRemote::new(&env.fixture.base));

        // Forged receipt ID in result.json
        let orig_result = read_json(&env.result_path).unwrap();
        let mut forged = orig_result.clone();
        forged["acceptance"]["active_verification_receipt"]["verification_id"] = json!("forged-id");
        save(&env.result_path, &forged).unwrap();
        assert!(pub8.publish(&env.binding, &env.goal).is_err());
        save(&env.result_path, &orig_result).unwrap();

        // Corrupt delivered archive
        let archive_path = PathBuf::from(
            orig_result["acceptance"]["delivery"]["destination"]
                .as_str()
                .unwrap(),
        );
        let orig_archive = fs::read(&archive_path).unwrap();
        let mut corrupted = orig_archive.clone();
        corrupted.extend_from_slice(b"corrupt");
        fs::write(&archive_path, &corrupted).unwrap();
        assert!(pub8.publish(&env.binding, &env.goal).is_err());
        fs::write(&archive_path, &orig_archive).unwrap();

        // Active execution in status.json
        let status_path = env.goal_root.join("status.json");
        let orig_status = read_json(&status_path).unwrap();
        let mut active_status = orig_status.clone();
        active_status["current"] = json!({"owned": true});
        save(&status_path, &active_status).unwrap();
        assert!(pub8.publish(&env.binding, &env.goal).is_err());
        save(&status_path, &orig_status).unwrap();
    }
}

#[test]
fn github_adapter_check_runs_reruns_and_branch_guards() {
    let target = "a".repeat(40);
    let b = json!({
        "repository": "lhemerly/ink-detector",
        "repository_id": 1422424015,
        "private": true,
        "branch": "main",
        "remote_url": "git@github.com:lhemerly/ink-detector.git",
        "initial_head": target,
        "required_checks": ["controller"],
    });

    let suites = vec![
        json!({"id": 10, "head_branch": format!("factory/x-{target}"), "head_sha": target}),
        json!({"id": 20, "head_branch": "main", "head_sha": target}),
    ];
    let rows = vec![
        json!({"id": 100, "name": "controller", "status": "completed", "conclusion": "success", "head_sha": target, "check_suite": {"id": 10}}),
        json!({"id": 200, "name": "controller", "status": "completed", "conclusion": "failure", "head_sha": target, "check_suite": {"id": 20}}),
        json!({"id": 300, "name": "controller", "status": "completed", "conclusion": "success", "head_sha": target, "check_suite": {"id": 20}}),
        json!({"id": 400, "name": "controller", "status": "completed", "conclusion": "failure", "head_sha": "f".repeat(40), "check_suite": {"id": 20}}),
        json!({"id": 500, "name": "controller", "status": "completed", "conclusion": "failure", "head_sha": target, "check_suite": {"id": 999}}),
    ];

    let target_clone = target.clone();
    let mut gh = GitHub::with_api(PathBuf::from(BIN), move |path| match path {
        "repos/lhemerly/ink-detector" => Ok(json!({
            "id": 1422424015,
            "full_name": "lhemerly/ink-detector",
            "private": true,
            "default_branch": "main",
        })),
        "repos/lhemerly/ink-detector/git/ref/heads/main" => {
            Ok(json!({"object": {"sha": target_clone}}))
        }
        p if p.contains("/check-runs") => Ok(json!({
            "total_count": rows.len(),
            "check_runs": rows,
        })),
        p if p.contains("/check-suites") => Ok(json!({
            "total_count": suites.len(),
            "check_suites": suites,
        })),
        p if p.contains("/status") => Ok(json!({
            "total_count": 1,
            "statuses": [{"context": "ci", "state": "success"}],
        })),
        other => bail!("unexpected path {other}"),
    });

    let snap = gh.snapshot(&b, Some(&target)).unwrap();
    let main_checks = snap["main_checks"].as_array().unwrap();
    assert_eq!(main_checks.len(), 1);
    assert_eq!(main_checks[0]["conclusion"], "success");
    // Status context is in checks, but not in main_checks
    assert_eq!(snap["checks"].as_array().unwrap().len(), 2);

    // Queued rerun without timestamps fences prior green
    let target2 = "b".repeat(40);
    let target2_clone = target2.clone();
    let mut gh_rerun = GitHub::with_api(PathBuf::from(BIN), move |path| match path {
        "repos/lhemerly/ink-detector" => Ok(json!({
            "id": 1422424015,
            "full_name": "lhemerly/ink-detector",
            "private": true,
            "default_branch": "main",
        })),
        "repos/lhemerly/ink-detector/git/ref/heads/main" => {
            Ok(json!({"object": {"sha": target2_clone}}))
        }
        p if p.contains("/check-runs") => Ok(json!({
            "total_count": 2,
            "check_runs": [
                {"id": 1, "name": "controller", "status": "completed", "conclusion": "success", "head_sha": target2_clone, "check_suite": {"id": 20}},
                {"id": 2, "name": "controller", "status": "queued", "conclusion": null, "head_sha": target2_clone, "check_suite": {"id": 20}}
            ],
        })),
        p if p.contains("/check-suites") => Ok(json!({
            "total_count": 1,
            "check_suites": [{"id": 20, "head_branch": "main", "head_sha": target2_clone}],
        })),
        p if p.contains("/status") => Ok(json!({"total_count": 0, "statuses": []})),
        other => bail!("unexpected path {other}"),
    });
    let snap2 = gh_rerun.snapshot(&b, Some(&target2)).unwrap();
    let (st, _) = ci_state(&b, &snap2["main_checks"]).unwrap();
    assert_eq!(st, "pending");
    assert_eq!(snap2["checks"][0]["status"], "queued");
}

#[test]
fn large_1_5mb_patch_admitted_while_over_2mb_binary_and_scope_rejected() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(repo.join("src")).unwrap();

    let git = |args: &[&str]| -> String {
        let out = Command::new("/usr/bin/git")
            .args(["-c", "core.hooksPath=/dev/null", "-C"])
            .arg(&repo)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {:?} failed", args);
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };

    git(&["init", "-q"]);
    git(&["config", "user.name", "Fixture"]);
    git(&["config", "user.email", "fixture@localhost"]);
    fs::write(repo.join("src/product.py"), "print(1)\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "base"]);
    let base = git(&["rev-parse", "HEAD"]);

    let b = json!({
        "repository": "lhemerly/ink-detector",
        "repository_id": 1422424015,
        "private": true,
        "branch": "main",
        "remote_url": "git@github.com:lhemerly/ink-detector.git",
        "initial_head": base,
        "goal_id": "goal",
        "goal_root": tmp.path().to_string_lossy(),
        "allowed_paths": ["src"],
        "protected_paths": ["src/frozen.py"],
        "forbidden_patterns": ["private-runtime-value"],
        "required_checks": [],
    });

    let publisher = Publisher::open_with_remote(
        &tmp.path().join("state"),
        FakeRemote::new(&base),
        PathBuf::from(BIN),
    )
    .unwrap();

    // 1. Zero diff rejected
    assert!(publisher.content(&b, &repo, &base, &base).is_err());

    // 2. Text source naming binary markers admitted
    fs::write(
        repo.join("src/detectors.py"),
        "binary=any(m in patch for m in (b'GIT binary patch',b'Binary files '))\n",
    )
    .unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "text source names binary markers"]);
    let text_commit = git(&["rev-parse", "HEAD"]);
    let digest = publisher.content(&b, &repo, &base, &text_commit).unwrap();
    assert_eq!(digest.len(), 64);

    // 3. 1.5MB valid patch (>1MiB supervisor stdout cap, <=2MB publication diff cap) admitted
    let line = "x".repeat(99) + "\n";
    let payload_1_5mb = line.repeat(15_000); // 1,500,000 bytes
    fs::write(repo.join("src/large_valid.txt"), &payload_1_5mb).unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "1.5MB text patch"]);
    let commit_1_5mb = git(&["rev-parse", "HEAD"]);
    let digest_1_5 = publisher
        .content(&b, &repo, &text_commit, &commit_1_5mb)
        .unwrap();
    assert_eq!(digest_1_5.len(), 64);

    // 4. >2MB patch rejected
    let payload_2_1mb = line.repeat(21_000); // 2,100,000 bytes
    fs::write(repo.join("src/too_large.txt"), &payload_2_1mb).unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "2.1MB text patch"]);
    let commit_2_1mb = git(&["rev-parse", "HEAD"]);
    let err_2_1 = publisher
        .content(&b, &repo, &commit_1_5mb, &commit_2_1mb)
        .unwrap_err();
    assert!(format!("{err_2_1:#}").contains("text scope"));

    // 5. Real binary diff rejected
    fs::write(repo.join("src/payload.bin"), b"\0binary\xff\0payload").unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "binary payload"]);
    let binary_commit = git(&["rev-parse", "HEAD"]);
    let err_bin = publisher
        .content(&b, &repo, &commit_2_1mb, &binary_commit)
        .unwrap_err();
    assert!(format!("{err_bin:#}").contains("text scope"));

    // 6. Private artifact / scope / credential pattern rejected
    fs::write(repo.join("src/.env.local"), "SECRET=1\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "env file"]);
    let env_commit = git(&["rev-parse", "HEAD"]);
    assert!(
        publisher
            .content(&b, &repo, &binary_commit, &env_commit)
            .is_err()
    );
}

#[test]
fn verifier_default_vs_explicit_owner_auth_isolation() {
    const SUBPROCESS_MARKER: &str = "FACTORY_TEST_AUTH_ISOLATION_CHILD";
    if std::env::var_os(SUBPROCESS_MARKER).is_some() {
        let tmp = TempDir::new().unwrap();
        let script = "import json,os; print(json.dumps({k:os.environ.get(k) for k in ['HOME','GH_CONFIG_DIR','SSH_AUTH_SOCK','GH_TOKEN','GITHUB_TOKEN','UNRELATED_SECRET','GIT_TERMINAL_PROMPT','GH_PROMPT_DISABLED']}))";

        // Default verifier mode MUST NOT inherit owner auth variables even when ambiently set
        let v_res = supervised_command(
            Path::new(BIN),
            Path::new("/usr/bin/python3"),
            &["-c", script],
            tmp.path(),
            10,
            0,
        )
        .unwrap();
        let v_env: Value = serde_json::from_slice(&v_res.stdout).unwrap();
        assert!(v_env["HOME"].is_null());
        assert!(v_env["GH_CONFIG_DIR"].is_null());
        assert!(v_env["SSH_AUTH_SOCK"].is_null());
        assert!(v_env["GH_TOKEN"].is_null());
        assert!(v_env["GITHUB_TOKEN"].is_null());
        assert!(v_env["UNRELATED_SECRET"].is_null());

        // Explicit owner mode forwards only the approved owner auth variables and forces non-interactive prompts
        let o_res = supervised_owner_command(
            Path::new(BIN),
            Path::new("/usr/bin/python3"),
            &["-c", script],
            tmp.path(),
            10,
            0,
        )
        .unwrap();
        let o_env: Value = serde_json::from_slice(&o_res.stdout).unwrap();
        assert_eq!(o_env["HOME"], "synthetic-home");
        assert_eq!(o_env["GH_CONFIG_DIR"], "synthetic-gh-config");
        assert_eq!(o_env["SSH_AUTH_SOCK"], "synthetic-ssh-sock");
        assert_eq!(o_env["GH_TOKEN"], "synthetic-gh-token-value");
        assert_eq!(o_env["GITHUB_TOKEN"], "synthetic-github-token-value");
        assert!(o_env["UNRELATED_SECRET"].is_null());
        assert_eq!(o_env["GIT_TERMINAL_PROMPT"], "0");
        assert_eq!(o_env["GH_PROMPT_DISABLED"], "1");

        // Verify token value never appears in receipt JSON
        let receipt_str = serde_json::to_string(&o_res.receipt).unwrap();
        assert!(!receipt_str.contains("synthetic-gh-token-value"));
        assert!(!receipt_str.contains("synthetic-github-token-value"));
        return;
    }

    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "verifier_default_vs_explicit_owner_auth_isolation",
            "--test-threads=1",
        ])
        .env(SUBPROCESS_MARKER, "1")
        .env("HOME", "synthetic-home")
        .env("GH_CONFIG_DIR", "synthetic-gh-config")
        .env("SSH_AUTH_SOCK", "synthetic-ssh-sock")
        .env("GH_TOKEN", "synthetic-gh-token-value")
        .env("GITHUB_TOKEN", "synthetic-github-token-value")
        .env("UNRELATED_SECRET", "must-not-leak")
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn owner_supervisor_enforces_output_cap_timeout_and_descendant_cleanup() {
    let tmp = TempDir::new().unwrap();
    let run_owner_plan = |script: &str, seconds: u64| -> ProcessReceipt {
        let dir = tmp.path().join(uuid::Uuid::new_v4().simple().to_string());
        fs::create_dir_all(&dir).unwrap();
        let plan = Plan {
            execution_id: uuid::Uuid::new_v4().simple().to_string(),
            argv: vec!["/usr/bin/python3".into(), "-c".into(), script.into()],
            cwd: dir.clone(),
            log: dir.join("stderr"),
            stdout: Some(dir.join("stdout")),
            commit: "0".repeat(40),
            seconds,
            receipt: dir.join("process.json"),
        };
        let plan_path = dir.join("plan.json");
        supervisor::write_json(&plan_path, &plan).unwrap();
        let mut child = Command::new(BIN)
            .args(["owner-supervisor", "--plan", plan_path.to_str().unwrap()])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let control = child.stdin.take().unwrap();
        let deadline = Instant::now() + Duration::from_secs(seconds + 5);
        while child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "owner-supervisor timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(control);
        supervisor::read_json(&plan.receipt).unwrap()
    };

    // 1. >1MiB stdout triggers output_limit and truncates
    let r_out = run_owner_plan("import sys; sys.stdout.write('x' * 1200000)", 5);
    assert!(!r_out.passed());
    assert!(r_out.quiescent);
    assert!(r_out.output_truncated);
    assert_eq!(r_out.termination_reason, "output_limit");

    // 2. Timeout reaps detached descendants
    let r_timeout = run_owner_plan(
        "import os,time; p=os.fork();\nif p==0: os.setsid()\ntime.sleep(30)",
        1,
    );
    assert!(!r_timeout.passed());
    assert!(r_timeout.quiescent);
    assert_eq!(r_timeout.termination_reason, "timeout");

    // 3. Leaked descendant after root exit invalidates zero exit and is reaped
    let r_leak = run_owner_plan(
        "import os,time; p=os.fork();\nif p==0:\n os.setsid(); time.sleep(30)\nos._exit(0)",
        2,
    );
    assert!(!r_leak.passed());
    assert!(r_leak.quiescent);
    assert!(r_leak.leaked_descendants);
    assert_eq!(
        r_leak.termination_reason,
        "owned_descendants_after_root_exit"
    );
}

#[tokio::test]
async fn delivery_audit_inspect_job_and_failed_cycle_fences() {
    let env = RealDeliveryEnv::setup().await;
    let cycle_ctrl = env.result_path.parent().unwrap().join("controller.json");
    let inspected = inspect_job(Path::new(BIN), Path::new(BIN), &cycle_ctrl, "job-pub-1").unwrap();
    assert_eq!(inspected["state"], "done");
    assert_eq!(inspected["outcome"], "verified_delivery");

    // audit_cycle on all_free_unavailable returns Ok(None)
    let mut unavail = json!({
        "goal_id": "fixture",
        "cleanup_confirmed": true,
        "status": "all_free_unavailable",
        "_result_path": env.result_path.to_string_lossy(),
    });
    assert!(
        audit_cycle(Path::new(BIN), Path::new(BIN), &env.goal, &mut unavail)
            .unwrap()
            .is_none()
    );

    // audit_cycle on failed attempt with violating accounting fences
    let mut violating = json!({
        "goal_id": "fixture",
        "cleanup_confirmed": true,
        "status": "failed",
        "job_id": "job-pub-1",
        "_result_path": env.result_path.to_string_lossy(),
        "recovery": {
            "stop_id": "stop-1",
            "execution_stop": {"status": "confirmed"},
            "accounting": {"status": "violating"},
        }
    });
    assert!(audit_cycle(Path::new(BIN), Path::new(BIN), &env.goal, &mut violating).is_err());
}
