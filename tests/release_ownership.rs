//! Admission and coordinator identities are independent of generic process liveness.
use anyhow::Result;
use code_factory_controller::{
    model::digest,
    queue_handoff::{handoff_resume_allowed, validate_owner},
    recovery::sha,
    research_dispatch::{identity, owner_binding},
};
use serde_json::{Value, json};
use std::{fs, path::Path};
#[test]
fn handoff_reopens_only_its_exact_causal_pause_in_the_same_unexhausted_admission() -> Result<()> {
    let txn = json!("123456781234123412341234567890ab");
    let text = "owned maintenance pause\n";
    let entry = json!({"text":text,"expected_sequence":4});
    let gs = json!({"state":"paused"});
    let status = json!({"sequence":4,"pause_reason":"Operator PAUSE file","pause_observation":{"sequence":4,"transaction":txn,"text_sha256":digest(text)}});
    assert!(handoff_resume_allowed(&gs, &status, &entry, &txn)?);
    for (object, path, value) in [
        ("gs", "admission_exhausted", json!({"sequence":4})),
        (
            "gs",
            "reason",
            json!("Bounded implementation cycle ended; independent review required"),
        ),
        ("gs", "state", json!("complete")),
        ("gs", "state", json!("publishing")),
        ("status", "sequence", json!(5)),
        ("status", "pause_reason", json!("Provider usage unknown")),
    ] {
        let mut g = gs.clone();
        let mut s = status.clone();
        if object == "gs" {
            g[path] = value
        } else {
            s[path] = value
        };
        assert!(!handoff_resume_allowed(&g, &s, &entry, &txn)?, "{g} {s}");
    }
    for key in ["transaction", "text_sha256", "sequence"] {
        let mut s = status.clone();
        s["pause_observation"][key] = Value::Null;
        assert!(!handoff_resume_allowed(&gs, &s, &entry, &txn)?);
    }
    assert!(!handoff_resume_allowed(
        &gs,
        &json!({"sequence":4,"pause_reason":"Operator PAUSE file"}),
        &entry,
        &txn
    )?);
    assert!(handoff_resume_allowed(
        &json!({"state":"ready"}),
        &status,
        &entry,
        &txn
    )?);
    Ok(())
}
#[test]
fn coordinator_binding_rejects_foreign_manifest_state_pool_queue_source_and_legacy_records()
-> Result<()> {
    let t = tempfile::tempdir()?;
    let source = std::env::current_exe()?;
    let h = sha(&source)?;
    let m = json!({"state":t.path().join("state.json"),"pool_lock":t.path().join("pool.lock"),"trusted_files":{source.to_str().unwrap():h}});
    let manifest = t.path().join("manifest.json");
    let queue = t.path().join("q");
    fs::create_dir(&queue)?;
    let binding = owner_binding(&manifest, &queue, &m, &source)?;
    let owner =
        json!({"process":identity(std::process::id()),"dispatcher_sha256":h,"binding":binding});
    validate_owner(&owner, &manifest, &queue, &m)?;
    assert!(validate_owner(&owner, &t.path().join("other-manifest.json"), &queue, &m).is_err());
    assert!(validate_owner(&owner, &manifest, &t.path().join("other-q"), &m).is_err());
    for field in ["state", "pool_lock"] {
        let mut other = m.clone();
        other[field] = json!(t.path().join("foreign"));
        assert!(validate_owner(&owner, &manifest, &queue, &other).is_err());
    }
    let mut bad = owner.clone();
    bad["binding"]["source"] = json!(Path::new("/bin/sleep"));
    assert!(validate_owner(&bad, &manifest, &queue, &m).is_err());
    let mut legacy = owner;
    legacy.as_object_mut().unwrap().remove("binding");
    assert!(validate_owner(&legacy, &manifest, &queue, &m).is_err());
    Ok(())
}
