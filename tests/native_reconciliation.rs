//! Regression cases discovered while independently reconciling native production paths.
//! Fake GitHub reads and temporary repositories exercise the real pstack and supervisor.
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use code_factory_controller::{
    dispatch_queue as queue,
    github_review::{GitHub, ReviewRemote, feedback},
    goal_acceptance,
    goal_state::save,
    recovery::supervised_command,
};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

const CONTROLLER: &str = env!("CARGO_BIN_EXE_code-factory-controller");

#[test]
fn legacy_rollback_fences_unknown_controller_mode_before_any_effect() -> Result<()> {
    for mode in [None, Some(0o644)] {
        let tmp = tempfile::tempdir()?;
        let q = queue::Queue::open(&tmp.path().join("queue"))?;
        let controller = tmp.path().join("controller");
        if let Some(mode) = mode {
            fs::write(&controller, b"new controller")?;
            fs::set_permissions(&controller, fs::Permissions::from_mode(mode))?;
        }
        let text = tmp.path().join("text");
        fs::write(&text, b"new text")?;
        let journal = json!({"manifest_path":tmp.path().join("manifest"),"state_path":tmp.path().join("state"),"manifest":{"controller":controller},"state":{},"backups":[{"path":text,"bytes":STANDARD.encode(b"old text")},{"path":controller,"bytes":STANDARD.encode(b"old controller")}]});
        let jp = q.root.join("update-journal.json");
        save(&jp, &journal)?;
        let before = fs::read(&jp)?;
        let error = queue::restore(&q, &journal).unwrap_err();
        assert!(format!("{error:#}").contains("lacks prior executable mode"));
        assert_eq!(fs::read(&jp)?, before);
        assert_eq!(fs::read(&text)?, b"new text");
        assert!(!tmp.path().join("manifest").exists());
        if mode.is_some() {
            assert_eq!(fs::read(&controller)?, b"new controller");
        } else {
            assert!(!controller.exists());
        }
    }
    Ok(())
}

#[test]
fn recorded_mode_restores_missing_executable_and_legacy_text() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let q = queue::Queue::open(&tmp.path().join("queue"))?;
    let controller = tmp.path().join("controller");
    let text = tmp.path().join("text");
    let journal = json!({"manifest_path":tmp.path().join("manifest"),"state_path":tmp.path().join("state"),"manifest":{"controller":controller},"state":{"prior":true},"backups":[{"path":text,"bytes":STANDARD.encode(b"old text")},{"path":controller,"bytes":STANDARD.encode(b"#!/bin/sh\nexit 0\n"),"mode":0o755}]});
    save(&q.root.join("update-journal.json"), &journal)?;
    queue::restore(&q, &journal)?;
    assert_eq!(fs::read(&text)?, b"old text");
    assert_eq!(
        fs::metadata(&controller)?.permissions().mode() & 0o777,
        0o755
    );
    assert!(!q.root.join("update-journal.json").exists());
    Ok(())
}

fn executable(path: &Path, text: &str) -> Result<()> {
    fs::write(path, text)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
fn node() -> Result<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|p| p.join("node"))
        .find(|p| p.is_file())
        .context("Node is required by production pstack")
}

#[test]
fn setup_and_large_snapshot_use_real_owned_execution() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let node = node()?;
    for kind in ["setup", "snapshot"] {
        let root = tmp.path().join(kind);
        fs::create_dir(&root)?;
        let request = root.join("request.json");
        save(&request, &json!({"kind":kind,"root":root,"node":node}))?;
        let input = format!("CF_NATIVE_RECONCILIATION={}", request.display());
        let exe = std::env::current_exe()?;
        let result = supervised_command(
            Path::new(CONTROLLER),
            Path::new("/usr/bin/env"),
            &[
                &input,
                exe.to_str().unwrap(),
                "--exact",
                "isolated_regression_child",
                "--nocapture",
            ],
            tmp.path(),
            90,
            0,
        )?;
        assert_eq!(result.receipt["quiescent"], true);
        assert_eq!(result.receipt["leaked_descendants"], false);
        assert_eq!(result.receipt["termination_reason"], "root_exit");
    }
    Ok(())
}

#[test]
fn isolated_regression_child() -> Result<()> {
    let Some(request) = std::env::var_os("CF_NATIVE_RECONCILIATION") else {
        return Ok(());
    };
    let value: Value = serde_json::from_slice(&fs::read(request)?)?;
    let root = Path::new(value["root"].as_str().unwrap());
    if value["kind"] == "setup" {
        setup_fixture(root)?;
    } else {
        snapshot_fixture(root, Path::new(value["node"].as_str().unwrap()))?;
    }
    Ok(())
}

fn setup_fixture(root: &Path) -> Result<()> {
    let repo = root.join("repository");
    fs::create_dir(&repo)?;
    let git = |args: &[&str]| -> Result<String> {
        let output = Command::new("/usr/bin/git")
            .args(["-c", "core.hooksPath=/dev/null"])
            .args(args)
            .current_dir(&repo)
            .output()?;
        ensure!(
            output.status.success(),
            "git fixture: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(String::from_utf8(output.stdout)?.trim().to_owned())
    };
    git(&["init", "-q"])?;
    fs::write(repo.join("product.txt"), b"approved")?;
    git(&["add", "."])?;
    git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@invalid",
        "commit",
        "-qm",
        "fixture",
    ])?;
    let commit = git(&["rev-parse", "HEAD"])?;
    let bin = root.join("bin");
    fs::create_dir(&bin)?;
    executable(
        &bin.join("git"),
        "#!/bin/sh\ntest \"$CI\" = true && test \"$GIT_CONFIG_GLOBAL\" = /dev/null && test \"$GIT_CONFIG_NOSYSTEM\" = 1 || exit 91\n/usr/bin/git \"$@\" || exit $?\ncase \" $* \" in *\" clone \"*) /bin/sleep 30 & ;; esac\nexit 0\n",
    )?;
    let manifest = json!({"repository":repo,"verification_environment":{"PATH":format!("{}:/usr/bin:/bin",bin.display())},"required_evidence":["probe"],"completion_checks":[{"id":"probe","evidence_type":"probe","argv":["/bin/false"]}]});
    let state = root.join("state");
    let result = goal_acceptance::run_acceptance(&manifest, &state, &commit, Path::new(CONTROLLER));
    assert_eq!(result["passed"], false, "{result}");
    assert_eq!(result["phase"], "setup", "{result}");
    assert_eq!(result["setup"].as_array().unwrap().len(), 1);
    let receipt = &result["setup"][0]["executor"];
    assert_eq!(receipt["root_exit_status"], 0, "{result}");
    assert_eq!(
        receipt["termination_reason"], "owned_descendants_after_root_exit",
        "{result}"
    );
    assert_eq!(receipt["leaked_descendants"], true);
    assert_eq!(receipt["quiescent"], true);
    assert_eq!(result["execution_unresolved"], false);
    // A restart can independently audit the retained setup plan and kernel receipt.
    assert_eq!(goal_acceptance::audit_processes(&state)?, 1);
    Ok(())
}

fn snapshot_fixture(root: &Path, node: &Path) -> Result<()> {
    let bin = root.join("bin");
    fs::create_dir(&bin)?;
    // Launch a subprocess with a private PATH; no process-global env mutation in tests.
    executable(
        &bin.join("gh"),
        &format!(
            r#"#!{}
const a=process.argv.slice(2);let out;
if(a[0]==='pr'&&a[1]==='view')out={{mergeable:'MERGEABLE',mergeStateStatus:'CLEAN',reviewDecision:'',headRefOid:'a'.repeat(40),headRefName:'fix',baseRefName:'main',state:'OPEN',mergedAt:null,isDraft:false}};
else if(a.includes('graphql'))out={{data:{{repository:{{pullRequest:{{reviewThreads:{{nodes:[],pageInfo:{{hasNextPage:false,endCursor:null}}}}}}}}}}}};
else{{const path=a.at(-1);if(path.includes('/issues/1/comments'))out=[...Array.from({{length:20}},(_,i)=>({{id:i+100,body:'x'.repeat(80000),updated_at:'1',user:{{login:'ignored'}}}})),{{id:1,body:'Review retained',updated_at:'2',user:{{login:'reviewer'}}}}];else if(path==='repos/owner/project')out={{id:42,full_name:'owner/project',private:true}};else if(path==='repos/owner/project/pulls/1')out={{number:1,state:'open',merged_at:null,base:{{repo:{{id:42}},ref:'main'}},head:{{repo:{{id:42}},ref:'fix',sha:'a'.repeat(40)}}}};else out=[];}}
process.stdout.write(JSON.stringify(out));
"#,
            node.display()
        ),
    )?;
    // Run the actual Rust backend and existing pstack reader with only fake gh changed.
    let request = root.join("snapshot-child.json");
    save(
        &request,
        &json!({"root":root,"node":node,"kind":"snapshot_child"}),
    )?;
    let exe = std::env::current_exe()?;
    let input = format!("CF_SNAPSHOT_CHILD={}", request.display());
    let search = format!(
        "PATH={}:{}:/usr/bin:/bin",
        bin.display(),
        node.parent().unwrap().display()
    );
    supervised_command(
        Path::new(CONTROLLER),
        Path::new("/usr/bin/env"),
        &[
            &input,
            &search,
            exe.to_str().unwrap(),
            "--exact",
            "large_snapshot_child",
            "--nocapture",
        ],
        root,
        60,
        0,
    )?;
    Ok(())
}

#[test]
fn large_snapshot_child() -> Result<()> {
    let Some(path) = std::env::var_os("CF_SNAPSHOT_CHILD") else {
        return Ok(());
    };
    let v: Value = serde_json::from_slice(&fs::read(path)?)?;
    let binding = json!({"repository":"owner/project","pr":1,"ignored_authors":["ignored"],"allowed_paths":["src"],"protected_paths":[]});
    let mut remote = GitHub {
        node: PathBuf::from(v["node"].as_str().unwrap()),
        executable: PathBuf::from(CONTROLLER),
    };
    let snapshot = remote.snapshot(&binding)?;
    assert!(serde_json::to_vec(&snapshot)?.len() > 1_500_000);
    let admitted = feedback(&snapshot, &binding, &"a".repeat(40))?;
    assert_eq!(admitted.as_array().unwrap().len(), 1);
    assert_eq!(admitted[0]["body"], "Review retained");
    Ok(())
}
