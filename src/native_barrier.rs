//! Independently audit the existing native execution records before a milestone.
use crate::{
    goal_acceptance::string,
    goal_state::resolve,
    recovery::{read_json, sha},
};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};
fn load_execution(
    directory: &Path,
    id: &str,
    work: &Path,
    base: &str,
) -> Result<(Value, Value, Value)> {
    ensure!(
        id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()),
        "Native execution ID invalid"
    );
    let root = directory.join(format!("native-execution-{id}"));
    ensure!(
        resolve(&root)?.parent() == Some(resolve(directory)?.as_path()),
        "Native execution path escaped cycle"
    );
    let plan = read_json(&root.join("plan.json"))?;
    let started = read_json(&root.join("process.started.json"))?;
    let stopped = read_json(&root.join("process.json"))?;
    ensure!(
        plan["execution_id"] == id
            && plan["cwd"] == work.to_str().context("worktree UTF-8")?
            && plan["commit"] == base
            && plan["seconds"] == 0
            && plan["stdout"] == root.join("stdout.log").to_str().context("stdout UTF-8")?
            && plan["log"] == root.join("stderr.log").to_str().context("stderr UTF-8")?
            && plan["receipt"]
                == root
                    .join("process.json")
                    .to_str()
                    .context("receipt UTF-8")?,
        "Native plan binding changed"
    );
    ensure!(
        started["quiescent"] == false && started["root_exit_status"].is_null(),
        "Native start snapshot is not an active start"
    );
    for receipt in [&started, &stopped] {
        ensure!(
            receipt["version"] == 1
                && receipt["execution_id"] == id
                && receipt["observer"] == "linux_subreaper_waitpid_pidfd"
                && receipt["supervisor"]["pid"].is_u64()
                && receipt["supervisor"]["start_ticks"].is_u64()
                && receipt["supervisor"]["boot_id"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty())
                && receipt["root_pid"].is_u64(),
            "Native execution identity unknown"
        );
    }
    ensure!(
        started["supervisor"] == stopped["supervisor"]
            && started["root_pid"] == stopped["root_pid"]
            && stopped["quiescent"] == true
            && stopped["leaked_descendants"] == false
            && stopped["cleanup_error"].is_null()
            && stopped["output_truncated"] == false
            && stopped["root_exit_status"].is_i64(),
        "Native independent stop unconfirmed"
    );
    Ok((plan, started, stopped))
}
fn audit_mount(
    plan: &Value,
    directory: &Path,
    work: &Path,
    native: &str,
    readonly: bool,
) -> Result<()> {
    let argv = plan["argv"]
        .as_array()
        .context("Native argv missing")?
        .iter()
        .map(|v| v.as_str().context("Native argv entry"))
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        argv.first() == Some(&"/usr/bin/env"),
        "Native environment wrapper changed"
    );
    let offset = argv
        .iter()
        .skip(1)
        .position(|v| !v.contains('='))
        .context("Native command missing")?
        + 1;
    ensure!(
        argv[offset] == "/usr/bin/bwrap",
        "Native containment executable changed"
    );
    let work = work.to_str().context("worktree UTF-8")?;
    let profile = directory.join("native");
    let profile = profile.to_str().context("profile UTF-8")?;
    let admission = PathBuf::from(work).join(".opencode/pstack-admission");
    let admission = admission.to_str().context("admission UTF-8")?;
    let mut found = false;
    let mut chdir = false;
    let mut i = offset + 1;
    let mut flags = HashSet::new();
    while i < argv.len() {
        flags.insert(argv[i]);
        match argv[i] {
            "--bind" | "--ro-bind" => {
                let pair = argv
                    .get(i + 1..i + 3)
                    .context("Native mount operands missing")?;
                ensure!(pair[0] == pair[1], "Native mount alias changed");
                if pair[1] == work {
                    ensure!(
                        argv[i] == if readonly { "--ro-bind" } else { "--bind" },
                        "Native source mount is not the required mode"
                    );
                    found = true;
                }
                if argv[i] == "--bind" {
                    ensure!(
                        pair[1] == profile
                            || pair[1] == admission
                            || (!readonly && pair[1] == work),
                        "Native plan grants an unexpected writable mount"
                    );
                }
                i += 3;
            }
            "--chdir" => {
                ensure!(argv.get(i + 1) == Some(&work), "Native cwd mount changed");
                chdir = true;
                i += 2;
            }
            "--symlink" => {
                ensure!(
                    argv.get(i + 1..i + 3) == Some(&["usr/bin", "/bin"][..]),
                    "Native system symlink changed"
                );
                i += 3;
            }
            "--cap-drop" => {
                ensure!(
                    argv.get(i + 1) == Some(&"ALL"),
                    "Native capability boundary changed"
                );
                i += 2;
            }
            "--proc" | "--dev" | "--tmpfs" => {
                let expected = match argv[i] {
                    "--proc" => "/proc",
                    "--dev" => "/dev",
                    _ => "/tmp",
                };
                ensure!(
                    argv.get(i + 1) == Some(&expected),
                    "Native special mount changed"
                );
                i += 2;
            }
            "--unshare-user" | "--unshare-pid" | "--unshare-ipc" | "--unshare-uts"
            | "--new-session" | "--die-with-parent" => i += 1,
            executable => {
                ensure!(
                    executable == native && argv.get(i + 1) == Some(&"serve") && found && chdir,
                    "Native server/mount contract changed"
                );
                ensure!(
                    [
                        "--unshare-user",
                        "--unshare-pid",
                        "--unshare-ipc",
                        "--unshare-uts",
                        "--new-session",
                        "--die-with-parent",
                        "--cap-drop"
                    ]
                    .iter()
                    .all(|f| flags.contains(f)),
                    "Native isolation flags missing"
                );
                return Ok(());
            }
        }
    }
    anyhow::bail!("Native server invocation missing")
}
pub fn audit(directory: &Path, out: &Value, config: &Value, authoritative: &Value) -> Result<()> {
    ensure!(
        out["native_source_barrier_version"] == 1,
        "Native source barrier version required"
    );
    let work = resolve(Path::new(string(out, "worktree")?))?;
    ensure!(
        authoritative["current_attempt"]["worktree_binding"]
            == work.to_str().context("worktree UTF-8")?
            && authoritative["current_attempt"]["attempt_id"] == out["attempt"],
        "Native barrier attempt binding mismatch"
    );
    let base = string(out, "base")?;
    ensure!(
        authoritative["current_attempt"]["approved_base_commit"] == base,
        "Native barrier base binding mismatch"
    );
    let native = string(config, "native")?;
    let before = &out["native_completion_stop"];
    let before_id = string(before, "execution_id")?;
    let (plan, _, stop) = load_execution(directory, before_id, &work, base)?;
    ensure!(
        before["receipt"] == stop
            && before["process_receipt"] == plan["receipt"]
            && before["plan_sha256"]
                == sha(&directory.join(format!("native-execution-{before_id}/plan.json")))?
            && stop["termination_reason"] == "controller_disconnected",
        "Writable native cutover lacks its exact stop receipt"
    );
    audit_mount(&plan, directory, &work, native, false)?;
    let after = &out["native_readonly_execution"];
    let after_id = string(after, "execution_id")?;
    ensure!(before_id != after_id, "Native cutover reused an execution");
    let (plan, started, stopped) = load_execution(directory, after_id, &work, base)?;
    ensure!(
        after["started"] == started
            && after["plan_sha256"]
                == sha(&directory.join(format!("native-execution-{after_id}/plan.json")))?
            && started["supervisor"]["boot_id"] == stop["supervisor"]["boot_id"]
            && started["supervisor"]["start_ticks"].as_u64()
                >= stop["supervisor"]["start_ticks"].as_u64()
            && (stopped["termination_reason"] == "controller_disconnected"
                || (stopped["termination_reason"] == "root_exit"
                    && stopped["root_exit_status"] == 0)),
        "Read-only native execution receipt changed"
    );
    audit_mount(&plan, directory, &work, native, true)?;
    Ok(())
}
