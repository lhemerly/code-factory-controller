use anyhow::{Result, bail};
use code_factory_controller::{
    cli_update::{Installer, REPOSITORY, release_tag},
    goal_state::save,
    recovery::sha,
};
use serde_json::json;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
};
fn stage(p: &Path, tag: &str) -> Result<()> {
    fs::create_dir(p.join("bin"))?;
    let binary = p.join("bin/code-factory-controller");
    fs::write(&binary, b"frozen native binary fixture")?;
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))?;
    save(
        &p.join("release.json"),
        &json!({"tag":tag,"repository":REPOSITORY,"commit":"a".repeat(40),"binary_sha256":sha(&binary)?,"native_cli":true,"runtime_files":{}}),
    )
}
#[test]
fn stable_versions_only() {
    for tag in [
        "main",
        "v1.0.0-beta",
        "../../tmp",
        "v1;echo bad",
        "v1.2.3.4",
        "v1.-2.3",
    ] {
        assert!(release_tag(tag).is_err(), "{tag}")
    }
    assert_eq!(release_tag("v12.2.0").unwrap(), [12, 2, 0]);
}
#[test]
fn failed_build_and_downgrade_preserve_active_version() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("install");
    let old = root.join("versions/v0.2.0");
    fs::create_dir_all(&old)?;
    symlink(&old, root.join("current"))?;
    let updater = Installer {
        root: root.clone(),
        bin: temp.path().join("bin"),
        executable: std::env::current_exe()?,
    };
    assert!(
        updater
            .update_with("v0.3.0", |_| bail!("build failed"))
            .is_err()
    );
    assert_eq!(fs::canonicalize(root.join("current"))?, old);
    for tag in ["v0.2.0", "v0.1.0"] {
        assert_eq!(
            updater.update_with(tag, |_| bail!("must not build"))?,
            "v0.2.0"
        );
    }
    Ok(())
}
#[test]
fn native_activation_and_retained_journal_rollback() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let updater = Installer {
        root: temp.path().join("install"),
        bin: temp.path().join("bin"),
        executable: std::env::current_exe()?,
    };
    updater.update_with("v0.3.0", |p| stage(p, "v0.3.0"))?;
    assert_eq!(
        fs::read_link(updater.bin.join("code-factory"))?,
        updater.root.join("current/bin/code-factory-controller")
    );
    let old = fs::read_link(updater.root.join("current"))?;
    save(
        &updater.root.join("activation-journal.json"),
        &json!({"version":1,"previous":[{"path":updater.root.join("current"),"kind":"link","target":old}]}),
    )?;
    fs::remove_file(updater.root.join("current"))?;
    symlink(
        temp.path().join("bad-release"),
        updater.root.join("current"),
    )?;
    updater.recover_activation()?;
    assert_eq!(fs::read_link(updater.root.join("current"))?, old);
    assert!(!updater.root.join("activation-journal.json").exists());
    let bad = updater.root.join("versions/v0.4.0");
    fs::create_dir(&bad)?;
    stage(&bad, "v0.4.0")?;
    fs::write(bad.join("bin/code-factory-controller"), b"tampered")?;
    assert!(
        updater
            .update_with("v0.4.0", |_| bail!("existing stage must be audited"))
            .is_err()
    );
    assert_eq!(fs::read_link(updater.root.join("current"))?, old);
    Ok(())
}
