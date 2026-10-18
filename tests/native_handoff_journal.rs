//! Durable pause failure and user-override boundaries using the real native CLI.
use anyhow::Result;
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
    let output = tmp.path().join("output");
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/test-native-handoff-journal.py");
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
    assert_eq!(result.receipt["quiescent"], true);
    assert_eq!(result.receipt["leaked_descendants"], false);
    Ok(serde_json::from_slice(&fs::read(
        output.join("observations.json"),
    )?)?)
}
#[test]
fn failed_journal_cannot_leave_an_unrecorded_active_pause() -> Result<()> {
    let o = observe("journal-failure")?;
    let r = &o["records"][0];
    assert_eq!(r["failed_before_pause"], true, "{r}");
    assert_eq!(r["owner_preserved"], true);
    assert_eq!(r["replacement_started"], false);
    assert_eq!(r["unrelated_pause_preserved"], true);
    Ok(())
}
#[test]
fn user_replacement_or_inplace_edit_is_never_released_or_resumed() -> Result<()> {
    let o = observe("user-pause")?;
    for r in o["records"].as_array().unwrap() {
        assert_eq!(r["helper_exit"], 0, "{r}");
        assert_eq!(r["user_pause_preserved"], true, "{r}");
        assert_eq!(r["goal_stays_paused"], true, "{r}");
        assert_eq!(r["no_goal_resume_event"], true, "{r}");
        assert_eq!(r["unrelated_pause_preserved"], true);
        assert_eq!(r["replacement_started"], true);
    }
    Ok(())
}
#[test]
fn coordinator_death_wakes_without_state_event_and_fences_uncertain_work() -> Result<()> {
    let o = observe("owner-death")?;
    let r = &o["records"][0];
    assert_eq!(r["owner_death_woke_handoff"], true, "{r}");
    assert_eq!(r["uncertain_admission_fenced"], true, "{r}");
    assert_eq!(r["replacement_started"], false);
    assert_eq!(r["unrelated_pause_preserved"], true);
    Ok(())
}

#[test]
fn maintenance_handoff_does_not_reopen_an_exhausted_one_cycle_goal() -> Result<()> {
    let o = observe("bounded")?;
    let r = &o["records"][0];
    assert_eq!(r["helper_exit"], 0, "{r}");
    assert_eq!(r["owned_pause_detached"], true, "{r}");
    assert_eq!(
        r["bounded_scope_preserved"], true,
        "Maintenance does not authorize another coding cycle: {r}"
    );
    assert_eq!(r["replacement_started"], true);
    assert_eq!(r["unrelated_pause_preserved"], true);
    Ok(())
}
