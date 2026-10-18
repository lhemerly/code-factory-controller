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

#[test]
fn prevented_admission_and_resume_crash_are_bound_to_the_same_transaction() -> Result<()> {
    use code_factory_controller::goal_state::digest as json_digest;
    use code_factory_controller::queue_handoff::handoff_resume_replayed;
    let txn = json!("123456781234123412341234567890ab");
    let text = "owned maintenance pause\n";
    let mut entry = json!({"text":text,"observed_sequence":4,"expected_sequence":5});
    let gs = json!({"state":"paused"});
    let before = json!({"sequence":4,"base":"fixed","current":null,"status":"paused","pause_reason":"Operator PAUSE file","pause_observation":{"sequence":4,"transaction":txn,"text_sha256":digest(text)}});
    assert!(handoff_resume_allowed(&gs, &before, &entry, &txn)?);
    entry["resume_intent"] =
        json!({"transaction":txn,"before":before,"before_digest":json_digest(&before)});
    let mut after = before.clone();
    after["status"] = json!("waiting");
    after["pause_reason"] = Value::Null;
    after["operator_resume"] = json!({"at":"2026-10-18T10:00:00Z","reason":format!("Cooperative queue handoff transaction {}",txn.as_str().unwrap())});
    assert!(!handoff_resume_allowed(&gs, &after, &entry, &txn)?);
    assert!(handoff_resume_replayed(&gs, &after, &entry, &txn)?);
    for key in ["base", "sequence", "current"] {
        let mut changed = after.clone();
        changed[key] = json!("different");
        assert!(!handoff_resume_replayed(&gs, &changed, &entry, &txn)?);
    }
    let mut wrong = after.clone();
    wrong["operator_resume"]["reason"] = json!("Unrelated manual resume");
    assert!(!handoff_resume_replayed(&gs, &wrong, &entry, &txn)?);
    assert!(!handoff_resume_replayed(
        &json!({"state":"paused","admission_exhausted":{"sequence":4}}),
        &after,
        &entry,
        &txn
    )?);
    let mut stale = before;
    stale["sequence"] = json!(6);
    assert!(!handoff_resume_allowed(&gs, &stale, &entry, &txn)?);
    Ok(())
}
#[test]
fn canonical_owner_pin_accepts_the_same_installed_symlink_and_terminal_identity() -> Result<()> {
    let t = tempfile::tempdir()?;
    let source = std::env::current_exe()?;
    let link = t.path().join("installed-cli");
    std::os::unix::fs::symlink(&source, &link)?;
    let h = sha(&source)?;
    let m = json!({"state":t.path().join("state.json"),"pool_lock":t.path().join("pool.lock"),"trusted_files":{link.to_str().unwrap():h}});
    let manifest = t.path().join("manifest.json");
    let queue = t.path().join("q");
    fs::create_dir(&queue)?;
    let binding = owner_binding(&manifest, &queue, &m, &source)?;
    let owner = json!({"state":"stopped","process":null,"dispatcher_sha256":h,"binding":binding});
    validate_owner(&owner, &manifest, &queue, &m)?;
    Ok(())
}
