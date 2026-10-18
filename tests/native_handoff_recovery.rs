//! Real native handoff faults using only directly owned disposable children.
//! Python prepares fixtures; no production orchestration depends on it.
use anyhow::{Result, ensure};
use code_factory_controller::recovery::supervised_command;
use serde_json::Value;
use std::{fs, os::unix::fs::PermissionsExt, path::Path};
const CONTROLLER: &str = env!("CARGO_BIN_EXE_code-factory-controller");
fn observe(kind: &str) -> Result<Value> {
    let tmp = tempfile::tempdir()?;
    let binary = tmp.path().join("controller");
    supervised_command(
        Path::new(CONTROLLER),
        Path::new("/usr/bin/strip"),
        &["--strip-debug", "-o", binary.to_str().unwrap(), CONTROLLER],
        tmp.path(),
        30,
        0,
    )?;
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))?;
    let output = tmp.path().join("observations");
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/test-native-handoff-recovery.py");
    let result = supervised_command(
        Path::new(CONTROLLER),
        Path::new("/usr/bin/python3"),
        &[
            script.to_str().unwrap(),
            binary.to_str().unwrap(),
            output.to_str().unwrap(),
            kind,
        ],
        tmp.path(),
        60,
        0,
    )?;
    ensure!(
        result.receipt["quiescent"] == true && result.receipt["leaked_descendants"] == false,
        "fixture process cleanup must be confirmed"
    );
    let observations: Value = serde_json::from_slice(&fs::read(output.join("observations.json"))?)?;
    Ok(observations["records"][0].clone())
}
#[test]
fn interrupted_handoff_recovers_only_its_exact_pause() -> Result<()> {
    let record = observe("crash-restart")?;
    assert_eq!(record["injected_helper_exit"], -9);
    assert_eq!(record["productive_owner_preserved_while_active"], true);
    assert_eq!(record["restart_owned_pause_retained"], true, "{record}");
    assert_eq!(record["own_pause_released"], true, "{record}");
    assert_eq!(record["unrelated_pause_unchanged"], true);
    assert_eq!(record["replacement_started"], true);
    Ok(())
}
#[test]
fn separate_state_directory_wakes_handoff_at_real_idle_boundary() -> Result<()> {
    let record = observe("separate-state-directory")?;
    assert_eq!(record["productive_owner_preserved_while_active"], true);
    assert_eq!(record["actual_state_change_woke_handoff"], true, "{record}");
    assert_eq!(record["own_pause_released"], true);
    assert_eq!(record["unrelated_pause_unchanged"], true);
    assert_eq!(record["replacement_started"], true);
    Ok(())
}
