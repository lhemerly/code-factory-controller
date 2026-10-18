use anyhow::{Result, bail};
use code_factory_controller::github_review::{Adapter, ReviewRemote};
use serde_json::{Value, json};
use std::path::Path;
fn binding() -> Value {
    json!({"repository":"owner/project","repository_id":42,"pr":1,"expected_head":"a".repeat(40),"remote_url":"git@github.com:owner/project.git","head_branch":"fix","base_branch":"main","goal_id":"goal","approved_task":"Fix scoped product bug","allowed_paths":["src","docs","tests"],"protected_paths":["src/frozen.py"],"seed_pins":{},"publication":{"title":"Product fix","summary_path":"docs/change.json","forbidden_patterns":["session[_ -]?id","harness"]}})
}
fn snapshot() -> Value {
    json!({"repository":{"id":42,"full_name":"owner/project","private":true},"facts":{"headRefOid":"a".repeat(40)},"pr":{"number":1,"state":"open","merged_at":null,"base":{"repo":{"id":42},"ref":"main"},"head":{"repo":{"id":42},"ref":"fix","sha":"a".repeat(40)}},"comments":[{"id":1,"body":"Please fix error","updated_at":"1","user":{"login":"reviewer"}}],"reviews":[],"inline":[]})
}
struct Remote {
    snapshot: Value,
    pushes: usize,
    fail: &'static str,
}
impl ReviewRemote for Remote {
    fn snapshot(&mut self, _: &Value) -> Result<Value> {
        Ok(self.snapshot.clone())
    }
    fn push(&mut self, _: &Value, _: &Path, commit: &str) -> Result<()> {
        self.pushes += 1;
        if self.fail == "before" {
            bail!("lost before push")
        }
        self.snapshot["pr"]["head"]["sha"] = json!(commit);
        self.snapshot["facts"]["headRefOid"] = json!(commit);
        if self.fail == "after" {
            bail!("lost response")
        }
        Ok(())
    }
    fn describe(&mut self, b: &Value, body: &str) -> Result<()> {
        self.snapshot["pr"]["body"] = json!(body);
        self.snapshot["pr"]["title"] = b["publication"]["title"].clone();
        Ok(())
    }
}

#[test]
fn crash_retained_intent_never_repushes_and_reconciles_only_exact_batch_commit_body() -> Result<()>
{
    for landed in [false, true] {
        let tmp = tempfile::tempdir()?;
        let mut adapter = Adapter::open(
            &tmp.path().join("state"),
            Remote {
                snapshot: snapshot(),
                pushes: 0,
                fail: "",
            },
            &std::env::current_exe()?,
        )?;
        adapter.bind(&binding())?;
        let key = adapter.ingest()?.unwrap();
        let commit = "b".repeat(40);
        let body = "Product summary";
        let mut state = adapter.state()?;
        state["batches"][&key]["status"] = json!("queued");
        state["publication"] = json!({"state":"push_intent","batch":key,"commit":commit,"previous_head":"a".repeat(40),"body_sha256":code_factory_controller::goal_state::digest(&json!(body))});
        adapter.persist(&state)?;
        if landed {
            adapter.remote.snapshot["pr"]["head"]["sha"] = json!(commit);
            adapter.remote.snapshot["facts"]["headRefOid"] = json!(commit);
        }
        for changed in ["changed body", "Product summary"] {
            let result = adapter.publish_verified(&key, Path::new("/unused"), &commit, changed);
            if landed && changed == body {
                assert_eq!(result?["state"], "confirmed");
            } else {
                assert!(result.is_err());
            }
            assert_eq!(adapter.remote.pushes, 0);
        }
    }
    Ok(())
}
