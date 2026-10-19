//! Release-only versioned-controller promotion; frozen dispatch tests remain unchanged.
use anyhow::{Result, bail};
use code_factory_controller::{
    dispatch_queue as q,
    goal_state::save,
    recovery::{read_json as read, sha},
    research_dispatch::Dispatcher,
};
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt, path::Path};
fn fixture(root: &Path) -> Result<(Value, std::path::PathBuf)> {
    let goalroot = root.join("g");
    fs::create_dir_all(goalroot.join("repository"))?;
    let source = root.join("code/cycle.mjs");
    fs::create_dir_all(source.parent().unwrap())?;
    fs::write(&source, b"approved runtime")?;
    fs::set_permissions(&source, fs::Permissions::from_mode(0o750))?;
    let state = root.join("state.json");
    let manifest = json!({"version":1,"worker_slots":1,"state":state,"pool_lock":root.join("pool.lock"),"controller":"/usr/bin/true","trusted_files":{source.to_str().unwrap():sha(&source)?},"goals":[{"id":"approved","root":goalroot,"repository":goalroot.join("repository"),"cycle_command":["node",source,goalroot],"admission_mode":"one_cycle","acceptance":{}}]});
    Ok((manifest, state))
}
#[test]
fn versioned_controller_promotes_before_audit_and_retains_old_identity() -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let tmp = tempfile::tempdir()?;
    let (mut manifest, state) = fixture(tmp.path())?;
    // Inert artifacts exercise promotion/rollback, with no executable launch.
    let owner = tmp.path().join("controller-v1");
    let candidate = tmp.path().join("candidate");
    let destination = tmp.path().join("controller-v2");
    fs::write(&owner, b"immutable owner")?;
    fs::set_permissions(&owner, fs::Permissions::from_mode(0o750))?;
    fs::write(&candidate, b"versioned controller")?;
    fs::set_permissions(&candidate, fs::Permissions::from_mode(0o750))?;
    let identity = fs::metadata(&owner)?;
    let original_identity = (identity.dev(), identity.ino(), sha(&owner)?);
    manifest["controller"] = json!(owner);
    manifest["trusted_files"][owner.to_str().unwrap()] = json!(sha(&owner)?);
    let mut dispatcher = Dispatcher::open(manifest.clone(), &state)?;
    dispatcher.s["unknown_accounting"] = json!({"cost":null,"tokens":null});
    dispatcher.persist()?;
    let before_state = fs::read(&state)?;
    let mp = tmp.path().join("manifest.json");
    save(&mp, &manifest)?;
    let report = tmp.path().join("verification.json");
    save(
        &report,
        &json!({"passed":true,"permissions_unchanged":true,"tests":[{"passed":true}]}),
    )?;
    let mut proposed = manifest.clone();
    proposed["controller"] = json!(destination);
    proposed["trusted_files"][destination.to_str().unwrap()] = json!(sha(&candidate)?);
    let plan = json!({"previous_manifest":manifest,"manifest":proposed,"files":[{"source":candidate,"destination":destination,"sha256":sha(&candidate)?}],"verification":{"path":report,"sha256":sha(&report)?}});
    let queue = q::Queue::open(&tmp.path().join("q"))?;
    queue.stage(&plan)?;
    let verify_promoted = |proposed: &Value| -> Result<()> {
        // This rejects the discarded draft's audit-before-promotion ordering.
        q::pins(&proposed["trusted_files"])?;
        assert_eq!(fs::read(&destination)?, b"versioned controller");
        assert_eq!(fs::metadata(&destination)?.mode() & 0o777, 0o750);
        Ok(())
    };
    assert!(
        q::boundary_updates(&queue, &mp, &state, |proposed| {
            verify_promoted(proposed)?;
            bail!("independent audit rejected")
        })
        .is_err()
    );
    assert!(!destination.exists());
    assert_eq!(read(&mp)?, manifest);
    assert_eq!(fs::read(&state)?, before_state);
    assert!(!queue.root.join("update-journal.json").exists());
    q::boundary_updates(&queue, &mp, &state, verify_promoted)?;
    assert_eq!(read(&mp)?, proposed);
    assert_eq!(
        read(&state)?["unknown_accounting"],
        json!({"cost":null,"tokens":null})
    );
    let retained = fs::metadata(&owner)?;
    assert_eq!(
        (retained.dev(), retained.ino(), sha(&owner)?),
        original_identity
    );
    q::pins(&manifest["trusted_files"])?;
    let after_state = fs::read(&state)?;
    q::boundary_updates(&queue, &mp, &state, |_| {
        bail!("must not repeat committed update")
    })?;
    assert_eq!(fs::read(&state)?, after_state);
    Ok(())
}
