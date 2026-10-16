mod common;
use code_factory_controller::{agy::AgyConfig, agy_stream, engine::Engine, execution, model::*};
use common::*;
use serde_json::{Value, json};
const USAGE: &str = r#"{"input_tokens":47119,"output_tokens":1392,"thinking_tokens":0,"cache_read_tokens":53370,"total_tokens":48511}"#;
fn trace(extra: Value) -> Vec<u8> {
    let mut result = json!({"conversation_id":"native","status":"SUCCESS","num_turns":1,"usage":serde_json::from_str::<Value>(USAGE).unwrap()});
    for (k, v) in extra.as_object().unwrap() {
        result[k] = v.clone();
    }
    format!("{}\n{}\n",json!({"event":"init","conversation_id":"native","init":{"model":"gemini-4-argon-low","permission_mode":"always-proceed","cwd":"/work"}}),json!({"event":"result","result":result})).into_bytes()
}
fn parse(bytes: &[u8]) -> anyhow::Result<Value> {
    agy_stream::parse(bytes, "gemini-4-argon-low", "always-proceed", "/work")
}
#[test]
fn native_system_message_is_observation_without_tool_or_error_authority() {
    let original = String::from_utf8(trace(json!({}))).unwrap();
    let (init, terminal) = original.split_once('\n').unwrap();
    let system = json!({"conversation_id":"native","step_index":52,"state":"DONE","step_type":"system_message","duration_seconds":0.000054822});
    let stream = |step: &Value| {
        format!(
            "{init}\n{}\n{terminal}",
            json!({"event":"step_update","step_update":step})
        )
    };
    let parsed = parse(stream(&system).as_bytes()).unwrap();
    assert_eq!(parsed["steps"][0]["step_type"], "system_message");
    for change in [
        "active",
        "error",
        "tool",
        "tool_info",
        "subagent",
        "malformed_subagent",
        "unknown_subagent",
        "top_error",
        "unknown_type",
    ] {
        let mut invalid = system.clone();
        match change {
            "active" => invalid["state"] = json!("ACTIVE"),
            "error" => invalid["state"] = json!("ERROR"),
            "tool" => invalid["tool_name"] = json!("run_command"),
            "tool_info" => invalid["tool_info"] = json!({"error":{"type":"PERMISSION_DENIED"}}),
            "subagent" => invalid["subagent_info"] = json!({"subagents":[{"id":"foreign"}]}),
            "malformed_subagent" => invalid["subagent_info"] = json!({"subagents":"foreign"}),
            "unknown_subagent" => invalid["subagent_info"] = json!({"agent_id":"foreign"}),
            "top_error" => invalid["error"] = json!({"type":"ERROR"}),
            _ => invalid["step_type"] = json!("unrecognized_system_variant"),
        }
        assert!(parse(stream(&invalid).as_bytes()).is_err(), "{change}");
    }
}
#[test]
fn raw_cache_is_not_added_to_totals_or_interpreted_as_subset() {
    let r = parse(&trace(json!({}))).unwrap();
    assert_eq!(r["usage"]["fields"]["cache_read_tokens"], 53370);
    assert_eq!(r["usage"]["fields"]["total_tokens"], 48511);
    assert!(r["usage"]["fields"]["cache_write_tokens"].is_null());
    assert_eq!(r["usage"]["cache_overlap"], "unknown");
    assert!(r["usage"]["measured_spend"].is_null());
}
#[test]
fn absent_usage_is_unknown_not_zero() {
    let r = parse(&trace(json!({"usage":null}))).unwrap();
    assert_eq!(r["accounting_status"], "unknown");
    assert!(r["usage"]["fields"]["input_tokens"].is_null());
}
#[test]
fn malformed_duplicate_foreign_and_truncated_results_fail() {
    let t = trace(json!({}));
    assert!(parse(&t[..t.len() - 1]).is_err());
    assert!(parse(&[t.clone(), b"{}\n".to_vec()].concat()).is_err());
    assert!(parse(&trace(json!({"conversation_id":"foreign"}))).is_err());
    assert!(parse(&trace(json!({"num_turns":2}))).is_err());
    assert!(parse(b"bad\n").is_err());
    assert!(parse(&trace(json!({"usage":{"input_tokens":-1}}))).is_err());
}
#[test]
fn repeated_active_done_steps_do_not_double_count() {
    let t = String::from_utf8(trace(json!({}))).unwrap();
    let lines = t.lines().collect::<Vec<_>>();
    let step = json!({"event":"step_update","step_update":{"conversation_id":"native","step_index":1,"state":"DONE","step_type":"agent_response","usage":serde_json::from_str::<Value>(USAGE).unwrap()}});
    let raw = format!("{}\n{}\n{}\n{}\n", lines[0], step, step, lines[1]);
    let r = parse(raw.as_bytes()).unwrap();
    assert_eq!(r["steps"].as_array().unwrap().len(), 1);
    assert_eq!(r["usage"]["fields"]["total_tokens"], 48511);
    assert_eq!(r["step_totals_summed"], false);
}
#[test]
fn pending_tools_permission_denials_and_subagents_fail() {
    let t = String::from_utf8(trace(json!({}))).unwrap();
    let l = t.lines().collect::<Vec<_>>();
    for step in [
        json!({"conversation_id":"native","step_index":1,"state":"ACTIVE","step_type":"tool"}),
        json!({"conversation_id":"native","step_index":1,"state":"DONE","step_type":"tool","tool_info":{"error":{"type":"permission_denied"}}}),
        json!({"conversation_id":"native","step_index":1,"state":"DONE","step_type":"tool","subagent_info":{"subagents":[{}]}}),
    ] {
        assert!(
            parse(
                format!(
                    "{}\n{}\n{}\n",
                    l[0],
                    json!({"event":"step_update","step_update":step}),
                    l[1]
                )
                .as_bytes()
            )
            .is_err()
        );
    }
}
async fn fixture(body: &str) -> Fixture {
    let mut f = Fixture::new().await;
    let adapter = f.temp.path().join("adapter");
    std::fs::create_dir_all(adapter.join("plugin/skills/poteto-mode/references")).unwrap();
    execution::git(&adapter, &["init", "-b", "main"])
        .await
        .unwrap();
    std::fs::write(
        adapter.join("plugin/skills/poteto-mode/SKILL.md"),
        "---\nname: poteto-mode\n---\nController fixture workflow\n",
    )
    .unwrap();
    std::fs::write(
        adapter.join("plugin/skills/poteto-mode/references/principles.md"),
        "Fixture principle\n",
    )
    .unwrap();
    std::fs::write(
        adapter.join("plugin/LICENSE"),
        "MIT License\nFixture copyright\n",
    )
    .unwrap();
    execution::git(&adapter, &["add", "."]).await.unwrap();
    execution::git(&adapter, &["commit", "-m", "pinned fixture adapter"])
        .await
        .unwrap();
    let commit = execution::git(&adapter, &["rev-parse", "HEAD"])
        .await
        .unwrap();
    let home = f.temp.path().join("native-home");
    std::fs::create_dir_all(home.join(".gemini/antigravity-cli")).unwrap();
    std::fs::write(
        home.join(".gemini/antigravity-cli/settings.json"),
        r#"{"toolPermission":"always-proceed"}"#,
    )
    .unwrap();
    let cli = f.temp.path().join("fake-agy");
    std::fs::write(&cli,format!(r##"#!/usr/bin/python3
import sys,os,json,pathlib,time,subprocess
if '--version' in sys.argv:
 print('1.3.6');sys.exit(0)
home=pathlib.Path(os.environ['HOME']);cwd=pathlib.Path.cwd()
with (home/'submissions').open('a') as log:log.write('one\n')
assert not any(k.startswith('FACTORY_') for k in os.environ),os.environ.keys()
assert '--dangerously-skip-permissions' not in sys.argv and '--sandbox' in sys.argv
assert sys.argv[sys.argv.index('--mode')+1]=='accept-edits'
assert sys.argv[sys.argv.index('-p')+1].startswith('/poteto-mode ')
assert (cwd/'.agents/skills/poteto-mode/SKILL.md').read_text().endswith('\n')
def emit(e):print(json.dumps(e),flush=True)
model=sys.argv[sys.argv.index('--model')+1]
init={{'event':'init','conversation_id':'native','init':{{'cwd':str(cwd),'model':model,'permission_mode':'always-proceed'}}}}
emit(init)
usage={USAGE}
{body}
"##)).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    f.config.data_dir = f.temp.path().join("agy-controller");
    f.config.max_workers = 1;
    f.config.policy.models = vec!["agy/gemini-4-argon-low".into()];
    f.config.agy = Some(AgyConfig {
        executable: cli,
        home,
        adapter_repository: adapter,
        adapter_commit: commit,
        model: "gemini-4-argon-low".into(),
        workflow: "poteto-mode".into(),
        timeout_seconds: 3,
        expected_tool_permission: "always-proceed".into(),
        included_quota_approved: true,
    });
    f.config.profiles[0].checks[0].argv = vec![
        "/usr/bin/python3".into(),
        "-c".into(),
        "import pathlib; assert pathlib.Path('result.txt').read_text() == 'verified result\\n'"
            .into(),
    ];
    f.engine = Engine::open(f.config.clone()).unwrap();
    f
}
const SUCCESS: &str = "(cwd/'result.txt').write_text('verified result\\n')\nemit({'event':'result','result':{'conversation_id':'native','num_turns':1,'status':'SUCCESS','usage':usage}})";
async fn run(f: &Fixture, id: &str) {
    f.engine.admit(f.input(id)).await.unwrap();
    f.engine.run(id, "agy").await.unwrap();
    f.engine.adapter_tick(id).await.unwrap();
}
fn submissions(f: &Fixture) -> usize {
    std::fs::read_to_string(f.config.agy.as_ref().unwrap().home.join("submissions"))
        .unwrap_or_default()
        .lines()
        .count()
}
#[tokio::test]
async fn complete_process_uses_independent_verification_integration_delivery_and_dedup() {
    let mut f = fixture(SUCCESS).await;
    run(&f, "one").await;
    assert_eq!(f.engine.get("one").unwrap().state, "verifying");
    assert_eq!(submissions(&f), 1);
    let record = f.engine.agy_session("one").unwrap().unwrap();
    assert_eq!(record["usage"]["fields"]["cache_read_tokens"], 53370);
    for _ in 0..8 {
        f.engine.advance("one").await.unwrap();
        f.engine.adapter_tick("one").await.unwrap();
    }
    let j = f.engine.get("one").unwrap();
    assert_eq!(j.state, "done");
    assert_eq!(j.receipt.as_ref().unwrap().outcome, "passed");
    assert!(j.integrated_commit.is_some());
    let delivery = j.delivery.unwrap();
    let bytes = std::fs::read(delivery["destination"].as_str().unwrap()).unwrap();
    assert_eq!(digest(bytes), delivery["sha256"]);
    assert_eq!(submissions(&f), 1);
    let cfg = f.config.clone();
    drop(f.engine);
    f.engine = Engine::open(cfg).unwrap();
    f.engine.adapter_tick("one").await.unwrap();
    assert_eq!(submissions(&f), 1);
}
#[tokio::test]
async fn background_command_system_notification_does_not_interrupt_validated_workflow() {
    let f = fixture(&format!("emit({{'event':'step_update','step_update':{{'conversation_id':'native','step_index':52,'state':'DONE','step_type':'system_message','duration_seconds':0.000054822}}}})\n{SUCCESS}")).await;
    run(&f, "system-notification").await;
    for _ in 0..8 {
        f.engine.advance("system-notification").await.unwrap();
        f.engine.adapter_tick("system-notification").await.unwrap();
    }
    assert_eq!(f.engine.get("system-notification").unwrap().state, "done");
    assert_eq!(submissions(&f), 1);
}

#[tokio::test]
async fn prepared_night_budget_is_immutable_scoped_and_survives_restart() {
    use code_factory_controller::agy::AgyAttemptBudget;
    let mut f = fixture(SUCCESS).await;
    f.engine.admit(f.input("night-budget")).await.unwrap();
    let prepared = f.engine.prepare("night-budget", "agy").await.unwrap();
    let request = AgyAttemptBudget {
        job_id: "night-budget".into(),
        attempt_id: prepared.attempt.unwrap().attempt_id,
        timeout_seconds: 21_600,
        reason: "Explicit overnight research runtime; included quota only".into(),
    };
    let record = f.engine.set_agy_attempt_budget(request.clone()).unwrap();
    assert_eq!(submissions(&f), 0);
    assert_eq!(
        f.engine.set_agy_attempt_budget(request.clone()).unwrap(),
        record
    );
    let lease =
        chrono::DateTime::parse_from_rfc3339(record["lease_expires_at"].as_str().unwrap()).unwrap();
    assert!(lease > chrono::Utc::now() + chrono::Duration::hours(5));
    for (seconds, attempt) in [
        (0, request.attempt_id.clone()),
        (28_801, request.attempt_id.clone()),
        (120, "foreign".into()),
        (120, request.attempt_id.clone()),
    ] {
        let mut invalid = request.clone();
        invalid.timeout_seconds = seconds;
        invalid.attempt_id = attempt;
        assert!(f.engine.set_agy_attempt_budget(invalid).is_err());
    }
    // Simulate a late attended arming, while the prepared lease is still active.
    f.engine
        .store
        .transaction(|tx| {
            let mut job = code_factory_controller::store::Store::load(tx, "night-budget")?;
            job.attempt.as_mut().unwrap().lease_expires_at =
                (chrono::Utc::now() + chrono::Duration::seconds(3)).to_rfc3339();
            code_factory_controller::store::Store::save(tx, &mut job, "fixture delayed arming")
        })
        .unwrap();
    let cfg = f.config.clone();
    drop(f.engine);
    f.engine = Engine::open(cfg).unwrap();
    f.engine.run("night-budget", "agy").await.unwrap();
    let armed = f.engine.get("night-budget").unwrap();
    let armed_lease =
        chrono::DateTime::parse_from_rfc3339(&armed.attempt.unwrap().lease_expires_at).unwrap();
    assert!(armed_lease > chrono::Utc::now() + chrono::Duration::hours(5));
    f.engine
        .heartbeat("night-budget", &request.attempt_id)
        .unwrap();
    let renewed = f.engine.get("night-budget").unwrap();
    assert!(
        chrono::DateTime::parse_from_rfc3339(&renewed.attempt.unwrap().lease_expires_at).unwrap()
            >= armed_lease
    );
    assert!(f.engine.set_agy_attempt_budget(request.clone()).is_err());
    f.engine.adapter_tick("night-budget").await.unwrap();
    let plan: Value = serde_json::from_slice(
        &std::fs::read(
            f.config
                .data_dir
                .join("agy")
                .join(&request.attempt_id)
                .join("plan.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(plan["seconds"], 21_610);
    assert!(plan["argv"].as_array().unwrap().contains(&json!("21600s")));
    assert_eq!(submissions(&f), 1);
    for _ in 0..8 {
        f.engine.advance("night-budget").await.unwrap();
        f.engine.adapter_tick("night-budget").await.unwrap();
    }
    assert_eq!(f.engine.get("night-budget").unwrap().state, "done");
}

#[tokio::test]
async fn heartbeat_during_owned_agy_execution_does_not_lose_completed_candidate() {
    let f = fixture(&format!(
        "(home/'ready').write_text('yes')\ntime.sleep(0.5)\n{SUCCESS}"
    ))
    .await;
    f.engine.admit(f.input("heartbeat-run")).await.unwrap();
    let job = f.engine.run("heartbeat-run", "agy").await.unwrap();
    let attempt = job.attempt.unwrap().attempt_id;
    let runtime = f.engine.adapter_tick("heartbeat-run");
    let heartbeat = async {
        let ready = f.config.agy.as_ref().unwrap().home.join("ready");
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while !ready.exists() {
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        f.engine.heartbeat("heartbeat-run", &attempt).unwrap();
    };
    let (result, ()) = tokio::join!(runtime, heartbeat);
    result.unwrap();
    assert_eq!(f.engine.get("heartbeat-run").unwrap().state, "verifying");
    for _ in 0..8 {
        f.engine.advance("heartbeat-run").await.unwrap();
        f.engine.adapter_tick("heartbeat-run").await.unwrap();
    }
    assert_eq!(f.engine.get("heartbeat-run").unwrap().state, "done");
    assert_eq!(submissions(&f), 1);
}

#[tokio::test]
async fn uncapped_agy_work_stops_when_ownership_lease_is_not_renewed() {
    let mut f = fixture("(home/'ready').write_text('yes')\ntime.sleep(60)").await;
    f.config.agy.as_mut().unwrap().timeout_seconds = 0;
    f.config.data_dir = f.temp.path().join("uncapped-lease-stop");
    drop(f.engine);
    f.engine = Engine::open(f.config.clone()).unwrap();
    f.engine.admit(f.input("lease-stop")).await.unwrap();
    f.engine.run("lease-stop", "agy").await.unwrap();
    let runtime = f.engine.adapter_tick("lease-stop");
    let expiry = async {
        let ready = f.config.agy.as_ref().unwrap().home.join("ready");
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while !ready.exists() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "owned fake CLI did not start"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        // Do not save a new revision: this simulates wall-clock lease expiry.
        f.engine
            .store
            .transaction(|tx| {
                let mut job = code_factory_controller::store::Store::load(tx, "lease-stop")?;
                job.attempt.as_mut().unwrap().lease_expires_at =
                    (chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339();
                tx.execute(
                    "UPDATE jobs SET data=?1 WHERE id=?2",
                    rusqlite::params![serde_json::to_string(&job)?, "lease-stop"],
                )?;
                Ok(())
            })
            .unwrap();
    };
    let (result, ()) = tokio::join!(runtime, expiry);
    result.unwrap();
    let observation = f.engine.agy_session("lease-stop").unwrap().unwrap();
    assert_eq!(observation["execution_stop"], "confirmed");
    assert_eq!(
        observation["process"]["termination_reason"],
        "controller_disconnected"
    );
    assert_eq!(observation["process"]["quiescent"], true);
    assert_eq!(f.engine.get("lease-stop").unwrap().state, "blocked");
}

#[tokio::test]
async fn native_timeout_gets_bounded_terminal_result_grace() {
    let mut f = fixture(&format!("time.sleep(1.2)\n{SUCCESS}")).await;
    f.config.agy.as_mut().unwrap().timeout_seconds = 1;
    f.config.data_dir = f.temp.path().join("grace-controller");
    drop(f.engine);
    f.engine = Engine::open(f.config.clone()).unwrap();
    run(&f, "terminal-grace").await;
    let observation = f.engine.agy_session("terminal-grace").unwrap().unwrap();
    assert_eq!(observation["accounting_status"], "reported_raw");
    assert_eq!(observation["process"]["termination_reason"], "root_exit");
    assert_eq!(f.engine.get("terminal-grace").unwrap().state, "verifying");
}

#[tokio::test]
async fn explicit_flash_low_policy_uses_same_independent_acceptance() {
    let mut f = fixture(SUCCESS).await;
    f.config.agy.as_mut().unwrap().model = "gemini-3.8-flash-low".into();
    // Selecting a cheaper model still requires the exact admitted model policy.
    assert!(f.config.validate().is_err());
    f.config.policy.models = vec!["agy/gemini-3.8-flash-low".into()];
    f.config.data_dir = f.temp.path().join("explicit-flash-controller");
    drop(f.engine);
    f.engine = Engine::open(f.config.clone()).unwrap();
    run(&f, "flash-smoke").await;
    for _ in 0..8 {
        f.engine.advance("flash-smoke").await.unwrap();
        f.engine.adapter_tick("flash-smoke").await.unwrap();
    }
    let job = f.engine.get("flash-smoke").unwrap();
    assert_eq!(job.state, "done");
    assert_eq!(job.receipt.unwrap().outcome, "passed");
    assert!(job.integrated_commit.is_some());
    assert!(job.delivery.is_some());
    assert_eq!(submissions(&f), 1);
}

#[tokio::test]
async fn model_success_with_bad_artifact_fails_independent_acceptance() {
    let f = fixture(&SUCCESS.replace("verified result", "wrong result")).await;
    run(&f, "bad").await;
    f.engine.advance("bad").await.unwrap();
    f.engine.adapter_tick("bad").await.unwrap();
    let j = f.engine.get("bad").unwrap();
    assert_eq!(j.state, "blocked");
    assert_eq!(j.candidate_receipt.unwrap().outcome, "failed");
    assert!(j.delivery.is_none());
}
#[tokio::test]
async fn wrong_model_policy_or_missing_usage_cannot_accept() {
    for body in [
        format!(
            "emit({{'event':'init','conversation_id':'other','init':{{'cwd':str(cwd),'model':'paid-other','permission_mode':'always-proceed'}}}})\n{SUCCESS}"
        ),
        SUCCESS.replace("'usage':usage", "'usage':None"),
    ] {
        let f = fixture(&body).await;
        run(&f, "policy").await;
        let j = f.engine.get("policy").unwrap();
        assert_eq!(j.state, "blocked");
        assert!(j.candidate.is_none());
        assert!(f.engine.action("policy", "requeue").is_err());
    }
}
#[tokio::test]
async fn protected_write_and_changed_commit_history_are_rejected() {
    for body in [
        format!("(cwd/'README.md').write_text('changed')\n{SUCCESS}"),
        format!(
            "{SUCCESS}\nsubprocess.run(['git','-c','user.name=worker','-c','user.email=w@x','add','.']);subprocess.run(['git','-c','user.name=worker','-c','user.email=w@x','commit','-m','worker'])"
        ),
    ] {
        let f = fixture(&body).await;
        run(&f, "scope").await;
        assert_eq!(f.engine.get("scope").unwrap().state, "blocked");
        assert!(f.engine.get("scope").unwrap().candidate.is_none());
    }
}
#[tokio::test]
async fn terminal_artifact_tampering_and_external_bind_are_rejected() {
    let f = fixture(SUCCESS).await;
    run(&f, "tamper").await;
    assert!(f.engine.bind("tamper", "foreign", TOKEN).is_err());
    let a = f.engine.get("tamper").unwrap().attempt.unwrap();
    std::fs::write(
        f.config
            .data_dir
            .join("agy")
            .join(a.attempt_id)
            .join("stdout.ndjson"),
        trace(json!({})),
    )
    .unwrap();
    assert!(f.engine.advance("tamper").await.is_err());
    assert!(f.engine.get("tamper").unwrap().delivery.is_none());
}
#[tokio::test]
async fn cancellation_stops_owned_descendants_and_leaves_unknown_usage_fenced() {
    let f=fixture("child=subprocess.Popen(['/usr/bin/sleep','30'],start_new_session=True)\n(home/'ready').write_text(str(child.pid))\ntime.sleep(30)").await;
    f.engine.admit(f.input("cancel")).await.unwrap();
    f.engine.run("cancel", "agy").await.unwrap();
    let engine = f.engine.clone();
    let tick = tokio::spawn(async move { engine.adapter_tick("cancel").await });
    wait_ready(&f).await;
    let mut unrelated = tokio::process::Command::new("/usr/bin/sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let j = f.engine.abort_worker("cancel").await.unwrap();
    tick.await.unwrap().unwrap();
    assert_eq!(j.state, "blocked");
    assert_eq!(j.attempt.unwrap().dispatch_state, "agy_stopped_unsettled");
    assert!(f.engine.action("cancel", "requeue").is_err());
    assert!(unrelated.try_wait().unwrap().is_none());
    unrelated.kill().await.unwrap();
    let r = f.engine.agy_session("cancel").unwrap().unwrap();
    assert_eq!(r["execution_stop"], "confirmed");
    assert_eq!(r["accounting_status"], "unknown");
}
async fn wait_ready(f: &Fixture) {
    for _ in 0..200 {
        if f.config.agy.as_ref().unwrap().home.join("ready").exists() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("worker never became ready")
}
#[tokio::test]
async fn dropped_request_and_restart_never_repeat_uncertain_execution() {
    let mut f = fixture("(home/'ready').write_text('ready')\ntime.sleep(30)").await;
    f.engine.admit(f.input("restart")).await.unwrap();
    f.engine.run("restart", "agy").await.unwrap();
    let engine = f.engine.clone();
    let tick = tokio::spawn(async move { engine.adapter_tick("restart").await });
    wait_ready(&f).await;
    tick.abort();
    let _ = tick.await;
    let cfg = f.config.clone();
    drop(f.engine);
    f.engine = Engine::open(cfg).unwrap();
    f.engine.adapter_tick("restart").await.unwrap();
    assert_eq!(f.engine.get("restart").unwrap().state, "blocked");
    assert_eq!(submissions(&f), 1);
    let a = f.engine.get("restart").unwrap().attempt.unwrap();
    let path = f
        .config
        .data_dir
        .join("agy")
        .join(a.attempt_id)
        .join("process.json");
    for _ in 0..200 {
        if path.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    f.engine.reconcile_agy("restart").await.unwrap();
    assert_eq!(submissions(&f), 1);
    assert!(f.engine.action("restart", "requeue").is_err());
}
#[tokio::test]
async fn timeout_and_root_zero_with_detached_work_cannot_propose() {
    for body in [
        "time.sleep(30)".to_string(),
        format!("subprocess.Popen(['/usr/bin/sleep','30'],start_new_session=True)\n{SUCCESS}"),
    ] {
        let f = fixture(&body).await;
        run(&f, "owned").await;
        assert_eq!(f.engine.get("owned").unwrap().state, "blocked");
        assert!(f.engine.get("owned").unwrap().candidate.is_none());
        let r = f.engine.agy_session("owned").unwrap().unwrap();
        assert_eq!(r["execution_stop"], "confirmed");
        assert_eq!(r["process_passed"], false);
    }
}
#[tokio::test]
async fn preflight_never_changes_existing_permission_or_runs_a_model() {
    let f = fixture(SUCCESS).await;
    let settings = f
        .config
        .agy
        .as_ref()
        .unwrap()
        .home
        .join(".gemini/antigravity-cli/settings.json");
    std::fs::write(&settings, r#"{"toolPermission":"request-review"}"#).unwrap();
    run(&f, "preflight").await;
    assert_eq!(submissions(&f), 0);
    assert!(
        std::fs::read_to_string(settings)
            .unwrap()
            .contains("request-review")
    );
    assert_eq!(
        f.engine
            .abort_worker("preflight")
            .await
            .unwrap()
            .attempt
            .unwrap()
            .dispatch_state,
        "resolved"
    );
}

#[test]
fn contradictory_success_with_permission_error_is_rejected() {
    assert!(parse(&trace(json!({"error":{"type":"permission_denied"}}))).is_err());
}
#[tokio::test]
async fn cleanup_rejects_parent_symlink_without_deleting_external_files() {
    let f = fixture("outside=home/'outside';outside.mkdir()\nsubprocess.run(['cp','-a',str(cwd/'.agents/skills'),str(outside/'skills')],check=True)\nos.rename(cwd/'.agents',cwd/'.agents-original')\nos.symlink(outside,cwd/'.agents')\n").await;
    run(&f, "symlink").await;
    assert_eq!(f.engine.get("symlink").unwrap().state, "blocked");
    assert!(f.engine.get("symlink").unwrap().candidate.is_none());
    let sentinel = f
        .config
        .agy
        .as_ref()
        .unwrap()
        .home
        .join("outside/skills/poteto-mode/SKILL.md");
    assert!(
        std::fs::read_to_string(sentinel)
            .unwrap()
            .contains("Controller fixture workflow")
    );
    assert!(f.engine.reconcile_agy("symlink").await.is_err());
}
#[tokio::test]
async fn cached_foreign_process_and_forged_hashes_cannot_qualify_execution() {
    let f = fixture(SUCCESS).await;
    run(&f, "foreign").await;
    let j = f.engine.get("foreign").unwrap();
    let a = j.attempt.unwrap();
    let receipt = f
        .config
        .data_dir
        .join("agy")
        .join(&a.attempt_id)
        .join("process.json");
    let mut p: Value = serde_json::from_slice(&std::fs::read(&receipt).unwrap()).unwrap();
    p["execution_id"] = json!("foreign-execution");
    std::fs::write(&receipt, serde_json::to_vec(&p).unwrap()).unwrap();
    let mut record = f.engine.agy_session("foreign").unwrap().unwrap();
    record["process"] = p;
    record["receipt_sha256"] = json!(digest(std::fs::read(&receipt).unwrap()));
    f.engine
        .store
        .transaction(|tx| {
            tx.execute(
                "UPDATE metadata SET value=?1 WHERE key=?2",
                rusqlite::params![
                    serde_json::to_string(&record)?,
                    format!("agy-result:{}", a.attempt_id)
                ],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(f.engine.advance("foreign").await.is_err());
    assert!(f.engine.get("foreign").unwrap().delivery.is_none());
}

#[tokio::test]
async fn supervisor_spawn_failure_is_proven_not_submitted_and_recoverable() {
    let mut f = fixture(SUCCESS).await;
    f.config.verifier_executable = Some(f.temp.path().join("missing-supervisor"));
    f.config.data_dir = f.temp.path().join("spawn-failure-controller");
    drop(f.engine);
    f.engine = Engine::open(f.config.clone()).unwrap();
    run(&f, "spawn-failure").await;
    assert_eq!(submissions(&f), 0);
    assert_eq!(f.engine.get("spawn-failure").unwrap().state, "blocked");
    let r = f.engine.agy_session("spawn-failure").unwrap().unwrap();
    assert_eq!(r["accounting_status"], "not_submitted");
    assert!(r["usage"].is_null());
    assert_eq!(
        f.engine
            .abort_worker("spawn-failure")
            .await
            .unwrap()
            .attempt
            .unwrap()
            .dispatch_state,
        "resolved"
    );
    assert_eq!(
        f.engine
            .reconcile_agy("spawn-failure")
            .await
            .unwrap()
            .attempt
            .unwrap()
            .dispatch_state,
        "resolved"
    );
    assert_eq!(
        f.engine
            .abort_worker("spawn-failure")
            .await
            .unwrap()
            .attempt
            .unwrap()
            .dispatch_state,
        "resolved"
    );
    assert_eq!(submissions(&f), 0);
}
#[tokio::test]
async fn activation_collision_preserves_existing_files_and_recovers_without_inference() {
    let f = fixture(SUCCESS).await;
    f.engine.admit(f.input("collision")).await.unwrap();
    let j = f.engine.run("collision", "agy").await.unwrap();
    let work = j.attempt.unwrap().worktree;
    let skill = work.join(".agents/skills/poteto-mode/SKILL.md");
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::fs::write(&skill, "unrelated existing workflow").unwrap();
    f.engine.adapter_tick("collision").await.unwrap();
    assert_eq!(submissions(&f), 0);
    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        "unrelated existing workflow"
    );
    assert_eq!(
        f.engine
            .abort_worker("collision")
            .await
            .unwrap()
            .attempt
            .unwrap()
            .dispatch_state,
        "resolved"
    );
    assert_eq!(
        std::fs::read_to_string(skill).unwrap(),
        "unrelated existing workflow"
    );
}

#[tokio::test]
async fn rejected_config_cannot_broaden_approved_native_policy() {
    let f = fixture(SUCCESS).await;
    for change in ["permission", "model", "parallel", "supervisor"] {
        let mut c = f.config.clone();
        match change {
            "permission" => c.agy.as_mut().unwrap().expected_tool_permission = "dangerous".into(),
            "model" => c.agy.as_mut().unwrap().model = "auto".into(),
            "parallel" => c.max_workers = 2,
            _ => c.verifier_executable = None,
        }
        assert!(c.validate().is_err());
    }
}

#[tokio::test]
async fn prepared_worktree_waits_for_explicit_run_and_can_abort_without_submission() {
    let mut f = fixture(SUCCESS).await;
    // The wrong-mode check must not pass merely because the legacy endpoint is absent.
    f.config.opencode_url = Some("http://127.0.0.1:1".into());
    f.config.data_dir = f.temp.path().join("prepared-controller");
    drop(f.engine);
    f.engine = Engine::open(f.config.clone()).unwrap();
    f.engine.admit(f.input("prepare-only")).await.unwrap();
    let j = f.engine.prepare("prepare-only", "agy").await.unwrap();
    assert!(j.attempt.as_ref().unwrap().worktree.is_dir());
    for _ in 0..3 {
        f.engine.adapter_tick("prepare-only").await.unwrap();
    }
    assert_eq!(submissions(&f), 0);
    assert!(f.engine.run("prepare-only", "adapter").await.is_err());
    assert_eq!(
        f.engine
            .get("prepare-only")
            .unwrap()
            .attempt
            .unwrap()
            .dispatch_state,
        "agy_awaiting_run"
    );
    assert_eq!(
        f.engine
            .abort_worker("prepare-only")
            .await
            .unwrap()
            .attempt
            .unwrap()
            .dispatch_state,
        "resolved"
    );
    assert_eq!(
        f.engine
            .abort_worker("prepare-only")
            .await
            .unwrap()
            .attempt
            .unwrap()
            .dispatch_state,
        "resolved"
    );
    assert_eq!(submissions(&f), 0);
}

#[test]
fn ordinary_tool_error_permission_context_is_recoverable() {
    let t = String::from_utf8(trace(json!({}))).unwrap();
    let l = t.lines().collect::<Vec<_>>();
    let step = json!({"conversation_id":"native","step_index":1,"state":"ERROR","step_type":"tool","tool_name":"view_file","tool_info":{"error":{"type":"TOOL_ERROR","message":"declaring permissions: convert tool call for permissions: failed to read file: no such file or directory"}}});
    let bytes = format!(
        "{}\n{}\n{}\n",
        l[0],
        json!({"event":"step_update","step_update":step}),
        l[1]
    );
    assert!(
        agy_stream::early_check(
            bytes.as_bytes(),
            "gemini-4-argon-low",
            "always-proceed",
            "/work"
        )
        .is_ok()
    );
    let parsed = parse(bytes.as_bytes()).unwrap();
    assert_eq!(parsed["provider_status"], "SUCCESS");
    assert_eq!(parsed["steps"][0]["state"], "ERROR");
}
#[test]
fn actual_denials_still_fail_and_conflicting_error_replays_are_rejected() {
    let t = String::from_utf8(trace(json!({}))).unwrap();
    let l = t.lines().collect::<Vec<_>>();
    for error in [
        json!({"type":"permission_denied"}),
        json!({"type":"TOOL_ERROR","message":"permission request was denied"}),
        json!({"type":"TOOL_ERROR","message":"workspace trust required"}),
        json!({"type":"TOOL_ERROR","message":"requires approval"}),
    ] {
        let step = json!({"conversation_id":"native","step_index":1,"state":"ERROR","step_type":"tool","tool_info":{"error":error}});
        let bytes = format!(
            "{}\n{}\n{}\n",
            l[0],
            json!({"event":"step_update","step_update":step}),
            l[1]
        );
        assert!(parse(bytes.as_bytes()).is_err());
    }
    let step = json!({"conversation_id":"native","step_index":1,"state":"ERROR","step_type":"tool","tool_info":{"error":{"type":"TOOL_ERROR","message":"no such file or directory"}}});
    let mut changed = step.clone();
    changed["tool_name"] = json!("different");
    let bytes = format!(
        "{}\n{}\n{}\n{}\n",
        l[0],
        json!({"event":"step_update","step_update":step}),
        json!({"event":"step_update","step_update":changed}),
        l[1]
    );
    assert!(parse(bytes.as_bytes()).is_err());
}
#[tokio::test]
async fn worker_recovers_from_missing_file_and_still_requires_independent_acceptance() {
    let body = format!(
        "emit({{'event':'step_update','step_update':{{'conversation_id':'native','step_index':1,'state':'ERROR','step_type':'tool','tool_name':'view_file','tool_info':{{'error':{{'type':'TOOL_ERROR','message':'declaring permissions: no such file or directory'}}}}}}}})\n{SUCCESS}"
    );
    let f = fixture(&body).await;
    run(&f, "recover-tool").await;
    assert_eq!(f.engine.get("recover-tool").unwrap().state, "verifying");
    for _ in 0..8 {
        f.engine.advance("recover-tool").await.unwrap();
        f.engine.adapter_tick("recover-tool").await.unwrap();
    }
    assert_eq!(f.engine.get("recover-tool").unwrap().state, "done");
    assert_eq!(submissions(&f), 1);
}

#[test]
fn non_tool_and_unexplained_error_cannot_be_hidden_by_success() {
    let t = String::from_utf8(trace(json!({}))).unwrap();
    let l = t.lines().collect::<Vec<_>>();
    for kind in ["agent_response", "user_input", "checkpoint", "tool"] {
        let error = if kind == "tool" {
            serde_json::Value::Null
        } else {
            json!({"type":"TOOL_ERROR","message":"ordinary non-tool failure"})
        };
        let step = json!({"conversation_id":"native","step_index":1,"state":"ERROR","step_type":kind,"tool_info":{"error":error}});
        let bytes = format!(
            "{}\n{}\n{}\n",
            l[0],
            json!({"event":"step_update","step_update":step}),
            l[1]
        );
        assert!(parse(bytes.as_bytes()).is_err());
    }
}
#[test]
fn generic_native_denial_messages_still_stop_early() {
    for message in [
        "user denied tool execution",
        "user rejected the tool call",
        "operation not permitted",
    ] {
        let event = json!({"event":"step_update","step_update":{"step_type":"tool","tool_info":{"error":{"type":"TOOL_ERROR","message":message}}}});
        let bytes = format!("{event}\n");
        assert!(
            agy_stream::early_check(
                bytes.as_bytes(),
                "gemini-4-argon-low",
                "always-proceed",
                "/work"
            )
            .is_err()
        );
    }
}

fn recovery_request(f: &Fixture, id: &str) -> code_factory_controller::agy::AgyUnknownRecovery {
    let job = f.engine.get(id).unwrap();
    let a = job.attempt.unwrap();
    let intent: Value = f
        .engine
        .store
        .transaction(|tx| {
            let s: String = tx.query_row(
                "SELECT value FROM metadata WHERE key=?1",
                [format!("agy-intent:{}", a.attempt_id)],
                |r| r.get(0),
            )?;
            Ok(serde_json::from_str(&s)?)
        })
        .unwrap();
    let receipt = f
        .config
        .data_dir
        .join("agy")
        .join(&a.attempt_id)
        .join("process.json");
    code_factory_controller::agy::AgyUnknownRecovery {recovery_id:new_id(),job_id:id.into(),attempt_id:a.attempt_id,execution_id:intent["plan"]["execution_id"].as_str().unwrap().into(),binding:intent["binding"].as_str().unwrap().into(),receipt_sha256:digest(std::fs::read(receipt).unwrap()),unknown_accounting_acknowledged:true,included_quota_approved:true,ai_credits_off_observed:true,quota_remaining_percent:90.0,quota_observed_at:now(),reason:"Explicitly continue the approved goal in a distinct attempt after confirmed owned stop; retain unknown usage".into()}
}
#[tokio::test]
async fn explicit_unknown_recovery_preserves_history_and_qualifies_only_a_distinct_attempt() {
    let body =
        format!("if (home/'submissions').read_text().count('one')==1: sys.exit(0)\n{SUCCESS}");
    let mut f = fixture(&body).await;
    run(&f, "unknown-recovery").await;
    let first = f.engine.get("unknown-recovery").unwrap().attempt.unwrap();
    let original = f.engine.agy_session("unknown-recovery").unwrap().unwrap();
    assert_eq!(original["accounting_status"], "unknown");
    assert!(f.engine.action("unknown-recovery", "requeue").is_err());
    let request = recovery_request(&f, "unknown-recovery");
    let recovery = f.engine.recover_agy_unknown(request.clone()).await.unwrap();
    assert_eq!(recovery["accounting_status"], "unknown");
    assert!(recovery["measured_spend"].is_null());
    assert_eq!(
        f.engine.agy_session("unknown-recovery").unwrap().unwrap(),
        original
    );
    assert_eq!(
        f.engine.recover_agy_unknown(request.clone()).await.unwrap(),
        recovery
    );
    f.engine.advance("unknown-recovery").await.unwrap();
    assert!(
        f.engine
            .get("unknown-recovery")
            .unwrap()
            .candidate
            .is_none()
    );
    assert_eq!(submissions(&f), 1);
    f.engine.reconcile_agy("unknown-recovery").await.unwrap();
    f.engine.abort_worker("unknown-recovery").await.unwrap();
    assert_eq!(
        f.engine.agy_session("unknown-recovery").unwrap().unwrap(),
        original
    );
    let mut refresh = request.clone();
    refresh.recovery_id = new_id();
    refresh.reason =
        "Fresh attended quota/credits observation for the same distinct-attempt allowance".into();
    f.engine.recover_agy_unknown(refresh).await.unwrap();
    assert_eq!(
        f.engine.agy_history("unknown-recovery").unwrap()[0]["recovery_events"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let cfg = f.config.clone();
    drop(f.engine);
    f.engine = Engine::open(cfg).unwrap();
    f.engine.action("unknown-recovery", "requeue").unwrap();
    assert!(f.engine.action("unknown-recovery", "requeue").is_err());
    assert!(f.engine.recover_agy_unknown(request).await.is_err());
    f.engine.run("unknown-recovery", "agy").await.unwrap();
    f.engine.adapter_tick("unknown-recovery").await.unwrap();
    let second = f.engine.get("unknown-recovery").unwrap().attempt.unwrap();
    assert_ne!(first.attempt_id, second.attempt_id);
    let plan: Value = serde_json::from_slice(
        &std::fs::read(
            f.config
                .data_dir
                .join("agy")
                .join(&second.attempt_id)
                .join("plan.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(
        plan["argv"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .as_str()
            .unwrap()
            .contains(
                "Fresh attended quota/credits observation for the same distinct-attempt allowance"
            )
    );
    assert_eq!(submissions(&f), 2);
    for _ in 0..8 {
        f.engine.advance("unknown-recovery").await.unwrap();
        f.engine.adapter_tick("unknown-recovery").await.unwrap();
    }
    let j = f.engine.get("unknown-recovery").unwrap();
    assert_eq!(j.state, "done");
    assert!(j.delivery.is_some());
    let history = f.engine.agy_history("unknown-recovery").unwrap();
    assert_eq!(history.as_array().unwrap().len(), 2);
    assert_eq!(history[0]["observation"], original);
    assert_eq!(
        history[0]["unknown_recovery"]["accounting_status"],
        "unknown"
    );
    assert!(history[0]["recovery_consumed_at"].as_str().is_some());
    assert_eq!(
        history[1]["observation"]["accounting_status"],
        "reported_raw"
    );
}
#[tokio::test]
async fn unknown_recovery_rejects_missing_ack_stale_quota_foreign_identity_and_evidence_changes() {
    let f = fixture("pass").await;
    run(&f, "recovery-guards").await;
    let request = recovery_request(&f, "recovery-guards");
    for mutation in [
        "ack",
        "credits",
        "quota",
        "stale",
        "future",
        "attempt",
        "execution",
        "binding",
        "receipt",
    ] {
        let mut bad = request.clone();
        match mutation {
            "ack" => bad.unknown_accounting_acknowledged = false,
            "credits" => bad.ai_credits_off_observed = false,
            "quota" => bad.quota_remaining_percent = 0.,
            "stale" => bad.quota_observed_at = "2020-01-01T00:00:00Z".into(),
            "future" => bad.quota_observed_at = "2099-01-01T00:00:00Z".into(),
            "attempt" => bad.attempt_id = "foreign".into(),
            "execution" => bad.execution_id = "foreign".into(),
            "binding" => bad.binding = "forged".into(),
            _ => bad.receipt_sha256 = "forged".into(),
        }
        assert!(
            f.engine.recover_agy_unknown(bad).await.is_err(),
            "{mutation}"
        );
    }
    f.engine.recover_agy_unknown(request.clone()).await.unwrap();
    let mut conflicting = request;
    conflicting.reason = "different operator decision".into();
    assert!(f.engine.recover_agy_unknown(conflicting).await.is_err());
    let a = f.engine.get("recovery-guards").unwrap().attempt.unwrap();
    let stdout = f
        .config
        .data_dir
        .join("agy")
        .join(a.attempt_id)
        .join("stdout.ndjson");
    std::fs::write(stdout, b"tampered\n").unwrap();
    assert!(f.engine.action("recovery-guards", "requeue").is_err());
    assert_eq!(submissions(&f), 1);
}
#[tokio::test]
async fn recovery_cannot_take_over_active_or_candidate_execution() {
    let f = fixture(SUCCESS).await;
    run(&f, "recovery-candidate").await;
    let request = recovery_request(&f, "recovery-candidate");
    assert!(f.engine.recover_agy_unknown(request).await.is_err());
    assert_eq!(
        f.engine.get("recovery-candidate").unwrap().state,
        "verifying"
    );
    let g = fixture("pass").await;
    run(&g, "recovery-foreign-receipt").await;
    let mut request = recovery_request(&g, "recovery-foreign-receipt");
    let a = g
        .engine
        .get("recovery-foreign-receipt")
        .unwrap()
        .attempt
        .unwrap();
    let path = g
        .config
        .data_dir
        .join("agy")
        .join(a.attempt_id)
        .join("process.json");
    let mut p: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    p["execution_id"] = json!("foreign");
    std::fs::write(&path, serde_json::to_vec(&p).unwrap()).unwrap();
    request.receipt_sha256 = digest(std::fs::read(&path).unwrap());
    assert!(g.engine.recover_agy_unknown(request).await.is_err());
    assert_eq!(submissions(&g), 1);
}

#[tokio::test]
async fn expired_recovery_cannot_requeue_until_fresh_attestation_is_appended() {
    let f = fixture("pass").await;
    run(&f, "recovery-expired").await;
    let request = recovery_request(&f, "recovery-expired");
    let mut recovery = f.engine.recover_agy_unknown(request.clone()).await.unwrap();
    // Simulate time passing without sleeping or changing the machine clock.
    // Keep pointer and immutable event identical so only freshness can reject.
    recovery["request"]["quota_observed_at"] = json!("2020-01-01T00:00:00Z");
    f.engine
        .store
        .transaction(|tx| {
            for key in [
                format!("agy-unknown-recovery:{}", request.attempt_id),
                format!(
                    "agy-unknown-recovery-event:{}:{}",
                    request.attempt_id, request.recovery_id
                ),
            ] {
                tx.execute(
                    "UPDATE metadata SET value=?1 WHERE key=?2",
                    rusqlite::params![serde_json::to_string(&recovery)?, key],
                )?;
            }
            Ok(())
        })
        .unwrap();
    let err = f.engine.action("recovery-expired", "requeue").unwrap_err();
    assert!(err.to_string().contains("fresh included quota"), "{err}");
    assert_eq!(submissions(&f), 1);
    let mut fresh = request;
    fresh.recovery_id = new_id();
    fresh.quota_observed_at = now();
    f.engine.recover_agy_unknown(fresh).await.unwrap();
    f.engine.action("recovery-expired", "requeue").unwrap();
    assert_eq!(
        f.engine.agy_history("recovery-expired").unwrap()[0]["recovery_events"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(submissions(&f), 1);
}

#[tokio::test]
async fn gemini_38_included_research_has_no_native_or_supervisor_wall_deadline() {
    // Actual owned supervisor + fake provider; never calls a live model.
    let mut f = fixture(&format!("time.sleep(6.5)\n{SUCCESS}")).await;
    f.config.data_dir = f.temp.path().join("gemini-38-unlimited");
    f.config.policy.models = vec!["agy/gemini-3.8-flash-high".into()];
    let agy = f.config.agy.as_mut().unwrap();
    agy.model = "gemini-3.8-flash-high".into();
    agy.timeout_seconds = 0;
    f.engine = Engine::open(f.config.clone()).unwrap();
    run(&f, "gemini-38").await;
    let job = f.engine.get("gemini-38").unwrap();
    let attempt = job.attempt.unwrap();
    let plan: Value = serde_json::from_slice(
        &std::fs::read(
            f.config
                .data_dir
                .join("agy")
                .join(attempt.attempt_id)
                .join("plan.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(plan["seconds"], 0);
    assert!(plan["argv"].as_array().unwrap().contains(&json!("0s")));
    assert!(
        plan["argv"]
            .as_array()
            .unwrap()
            .contains(&json!("gemini-3.8-flash-high"))
    );
    for _ in 0..8 {
        f.engine.advance("gemini-38").await.unwrap();
        f.engine.adapter_tick("gemini-38").await.unwrap();
    }
    assert_eq!(f.engine.get("gemini-38").unwrap().state, "done");
    assert_eq!(submissions(&f), 1);
}

#[tokio::test]
async fn unqualified_140_preflight_reports_observed_version_without_submitting_inference() {
    let f = fixture(SUCCESS).await;
    let cli = &f.config.agy.as_ref().unwrap().executable;
    let script = std::fs::read_to_string(cli)
        .unwrap()
        .replace("print('1.3.6')", "print('1.4.0')");
    std::fs::write(cli, script).unwrap();
    let error = f.engine.agy_preflight().await.unwrap_err().to_string();
    assert!(error.contains("1.4.0"));
    assert!(error.contains("not qualified"));
    assert!(error.contains("effective permissions"));
    assert_eq!(submissions(&f), 0);
}

#[test]
fn informational_background_completion_preserves_raw_usage_and_cannot_rewrite_completed_tools() {
    // Synthetic compatibility stress case motivated by official1.4.0 background-output fixes.
    // This is not an observed native1.4.0 stream.
    let original = String::from_utf8(trace(json!({}))).unwrap();
    let (init, terminal) = original.split_once('\n').unwrap();
    let tool = json!({"event":"step_update","step_update":{"conversation_id":"native","step_index":1,"step_type":"tool","state":"DONE","tool_name":"run_command","tool_info":{"output":"original completed output"}}});
    let note = json!({"event":"step_update","step_update":{"conversation_id":"native","step_index":2,"step_type":"system_message","state":"DONE","duration_seconds":0.1}});
    let bytes = format!("{init}\n{tool}\n{note}\n{terminal}");
    let parsed = parse(bytes.as_bytes()).unwrap();
    assert_eq!(parsed["usage"]["fields"]["input_tokens"], 47119);
    assert_eq!(parsed["usage"]["fields"]["cache_read_tokens"], 53370);
    assert_eq!(parsed["step_totals_summed"], false);
    let mut changed = tool.clone();
    changed["step_update"]["tool_info"]["output"] = json!("rewritten later");
    let conflict = format!("{init}\n{tool}\n{note}\n{changed}\n{terminal}");
    assert!(parse(conflict.as_bytes()).is_err());
}
