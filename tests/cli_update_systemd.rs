//! Regression test: Installer preserves user systemd session environment variables
//! specifically for systemctl, while keeping other child processes clean.
use anyhow::Result;
use code_factory_controller::cli_update::Installer;
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

const CONTROLLER: &str = env!("CARGO_BIN_EXE_code-factory-controller");

#[test]
fn systemctl_preserves_user_systemd_session_variables() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let script_bin = tmp.path().join("bin");
    fs::create_dir(&script_bin)?;

    // Script named systemctl that echoes session variables
    let systemctl_script = script_bin.join("systemctl");
    fs::write(
        &systemctl_script,
        b"#!/bin/sh\necho \"XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-unset}\"\necho \"DBUS_SESSION_BUS_ADDRESS=${DBUS_SESSION_BUS_ADDRESS:-unset}\"\n",
    )?;
    fs::set_permissions(&systemctl_script, fs::Permissions::from_mode(0o755))?;

    // Script named other_cmd that echoes session variables
    let other_script = script_bin.join("other_cmd");
    fs::write(
        &other_script,
        b"#!/bin/sh\necho \"XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-unset}\"\necho \"DBUS_SESSION_BUS_ADDRESS=${DBUS_SESSION_BUS_ADDRESS:-unset}\"\n",
    )?;
    fs::set_permissions(&other_script, fs::Permissions::from_mode(0o755))?;

    let installer = Installer {
        root: tmp.path().join("root"),
        bin: tmp.path().join("local_bin"),
        executable: PathBuf::from(CONTROLLER),
    };

    // Set the session variables in current environment
    unsafe {
        std::env::set_var("XDG_RUNTIME_DIR", "/run/user/12345");
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/12345/bus");
    }

    // Add script_bin to PATH so Installer can invoke systemctl and other_cmd by name
    let old_path = std::env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", script_bin.display(), old_path);
    unsafe {
        std::env::set_var("PATH", &test_path);
    }

    // 1. Invoking "systemctl" must preserve session variables
    let out = installer.command("systemctl", &[], tmp.path(), 10)?;
    let text = String::from_utf8(out)?;
    assert!(
        text.contains("XDG_RUNTIME_DIR=/run/user/12345"),
        "systemctl should receive XDG_RUNTIME_DIR, got: {text}"
    );
    assert!(
        text.contains("DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/12345/bus"),
        "systemctl should receive DBUS_SESSION_BUS_ADDRESS, got: {text}"
    );

    // 2. Invoking "other_cmd" must strip session variables
    let out_other = installer.command("other_cmd", &[], tmp.path(), 10)?;
    let text_other = String::from_utf8(out_other)?;
    assert!(
        text_other.contains("XDG_RUNTIME_DIR=unset"),
        "other commands should strip XDG_RUNTIME_DIR, got: {text_other}"
    );
    assert!(
        text_other.contains("DBUS_SESSION_BUS_ADDRESS=unset"),
        "other commands should strip DBUS_SESSION_BUS_ADDRESS, got: {text_other}"
    );

    // Restore PATH
    unsafe {
        std::env::set_var("PATH", old_path);
    }

    Ok(())
}
