use anyhow::{Result, bail};
use code_factory_controller::github_review::{
    Adapter, ReviewRemote, feedback, identity, product, scoped,
};
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
fn review_versions_scope_identity_and_untrusted_data_are_preserved() -> Result<()> {
    let b = binding();
    let mut snap = snapshot();
    identity(&snap, &b, &"a".repeat(40))?;
    snap["inline"] = json!([{"id":2,"body":"old","commit_id":"b".repeat(40),"path":"src/a.py"},{"id":3,"body":"scope","path":"outside.py"}]);
    let out = feedback(&snap, &b, &"a".repeat(40))?;
    assert_eq!(out[1]["state"], "stale");
    assert_eq!(out[2]["state"], "out_of_scope");
    scoped(&["src/code.rs", "docs/user.md"], &b)?;
    assert!(scoped(&["src/frozen.py"], &b).is_err());
    assert!(product("Session ID: private", &b).is_err());
    snap["repository"]["private"] = json!(false);
    assert!(identity(&snap, &b, &"a".repeat(40)).is_err());
    Ok(())
}
#[test]
fn lost_response_reconciles_exact_remote_without_repeated_push() -> Result<()> {
    for failure in ["before", "after"] {
        let tmp = tempfile::tempdir()?;
        let remote = Remote {
            snapshot: snapshot(),
            pushes: 0,
            fail: failure,
        };
        let mut adapter =
            Adapter::open(&tmp.path().join("state"), remote, &std::env::current_exe()?)?;
        adapter.bind(&binding())?;
        let key = adapter.ingest()?.unwrap();
        assert!(adapter.ingest()?.is_none());
        adapter.remote.snapshot["comments"][0]["body"] = json!("edited without new timestamp");
        assert_ne!(adapter.ingest()?.unwrap(), key);
        let mut state = adapter.state()?;
        state["batches"][&key]["status"] = json!("queued");
        adapter.persist(&state)?;
        let commit = "b".repeat(40);
        assert!(
            adapter
                .publish_verified(&key, Path::new("/unused"), &commit, "Product summary")
                .is_err()
        );
        assert_eq!(adapter.state()?["publication"]["state"], "uncertain");
        adapter.remote.fail = "";
        let retry =
            adapter.publish_verified(&key, Path::new("/unused"), &commit, "Product summary");
        if failure == "after" {
            assert_eq!(retry?["state"], "confirmed");
            adapter.publish_verified(&key, Path::new("/unused"), &commit, "Product summary")?;
        } else {
            assert!(retry.is_err());
        }
        assert_eq!(adapter.remote.pushes, 1);
    }
    Ok(())
}
