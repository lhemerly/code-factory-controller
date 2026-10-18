//! Stable native installation. Activation changes only future CLI invocations.
use crate::{
    goal_state::{Lock, resolve, save, write_bytes_atomic},
    recovery::{read_json as read, sha},
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    time::Duration,
};
pub const REPOSITORY: &str = "lhemerly/code-factory-controller";
pub fn release_tag(tag: &str) -> Result<[u64; 3]> {
    let Some(version) = tag.strip_prefix('v') else {
        bail!("only stable semantic-version release tags are supported")
    };
    let parts = version.split('.').collect::<Vec<_>>();
    ensure!(
        parts.len() == 3
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit())),
        "only stable semantic-version release tags are supported"
    );
    Ok([parts[0].parse()?, parts[1].parse()?, parts[2].parse()?])
}
fn atomic_link(target: &Path, link: &Path) -> Result<()> {
    let temp = link.with_file_name(format!(
        ".{}.new-{}",
        link.file_name()
            .context("link basename required")?
            .to_string_lossy(),
        uuid::Uuid::new_v4()
    ));
    symlink(target, &temp)?;
    fs::rename(&temp, link)?;
    fs::File::open(link.parent().context("link parent required")?)?.sync_all()?;
    Ok(())
}
#[derive(Debug, Clone)]
pub struct Installer {
    pub root: PathBuf,
    pub bin: PathBuf,
    pub executable: PathBuf,
}
impl Installer {
    pub fn for_user() -> Result<Self> {
        let home = PathBuf::from(std::env::var_os("HOME").context("HOME required")?);
        Ok(Self {
            root: home.join(".local/share/code-factory"),
            bin: home.join(".local/bin"),
            executable: std::env::current_exe()?,
        })
    }
    fn command(&self, command: &str, args: &[&str], cwd: &Path, timeout: u64) -> Result<Vec<u8>> {
        let mut argv = vec![];
        for k in [
            "PATH",
            "HOME",
            "RUSTUP_HOME",
            "CARGO_HOME",
            "RUSTUP_TOOLCHAIN",
            "TMPDIR",
        ] {
            if let Some(v) = std::env::var_os(k) {
                argv.push(format!("{k}={}", v.to_string_lossy()))
            }
        }
        argv.push(command.into());
        argv.extend(args.iter().map(|s| s.to_string()));
        let refs = argv.iter().map(String::as_str).collect::<Vec<_>>();
        let out = crate::recovery::supervised_command(
            &self.executable,
            Path::new("/usr/bin/env"),
            &refs,
            cwd,
            timeout,
            0,
        )?;
        Ok(out.stdout)
    }
    pub async fn latest(&self) -> Result<String> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(Duration::from_secs(30))
            .build()?;
        let result = async {
            let response = client
                .get(format!(
                    "https://api.github.com/repos/{REPOSITORY}/releases/latest"
                ))
                .header("User-Agent", "code-factory-updater")
                .header("Accept", "application/vnd.github+json")
                .send()
                .await?
                .error_for_status()?;
            ensure!(
                response.content_length().unwrap_or(0) <= 1_048_576,
                "release response too large"
            );
            let mut response = response;
            let mut bytes = vec![];
            while let Some(chunk) = response.chunk().await? {
                ensure!(
                    bytes.len() + chunk.len() <= 1_048_576,
                    "release response too large"
                );
                bytes.extend_from_slice(&chunk)
            }
            Ok::<Value, anyhow::Error>(serde_json::from_slice(&bytes)?)
        }
        .await;
        let release = match result {
            Ok(v) => v,
            Err(http_error) => {
                let out = self
                    .command(
                        "gh",
                        &["api", &format!("repos/{REPOSITORY}/releases/latest")],
                        &self.root,
                        30,
                    )
                    .with_context(|| format!("release lookup failed: {http_error}"))?;
                serde_json::from_slice(&out)?
            }
        };
        ensure!(
            release["draft"] != true && release["prerelease"] != true,
            "latest release must be stable"
        );
        let tag = release["tag_name"]
            .as_str()
            .context("release tag missing")?
            .to_owned();
        release_tag(&tag)?;
        Ok(tag)
    }
    pub async fn update(&self, requested: Option<&str>) -> Result<String> {
        fs::create_dir_all(&self.root)?;
        let _lock = Lock::acquire(&self.root.join("update.lock"), true)?;
        self.recover_activation()?;
        let tag = if let Some(tag) = requested {
            tag.to_owned()
        } else {
            self.latest().await?
        };
        self.update_locked(&tag, |destination| self.build(&tag, destination))
    }
    /// Injection boundary is the release builder; production always uses pinned Git + Cargo.
    pub fn update_with(
        &self,
        tag: &str,
        build: impl FnOnce(&Path) -> Result<()>,
    ) -> Result<String> {
        fs::create_dir_all(&self.root)?;
        let _lock = Lock::acquire(&self.root.join("update.lock"), true)?;
        self.recover_activation()?;
        self.update_locked(tag, build)
    }
    fn update_locked(&self, tag: &str, build: impl FnOnce(&Path) -> Result<()>) -> Result<String> {
        let requested = release_tag(tag)?;
        let current = self.root.join("current");
        if current.exists() {
            let installed = resolve(&current)?
                .file_name()
                .context("installed version missing")?
                .to_string_lossy()
                .into_owned();
            if requested <= release_tag(&installed)? {
                return Ok(installed);
            }
        }
        let versions = self.root.join("versions");
        fs::create_dir_all(&versions)?;
        let destination = versions.join(tag);
        if !destination.exists() {
            let staging = versions.join(format!(".install-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&staging)?;
            let result = (|| -> Result<()> {
                build(&staging)?;
                self.validate_staged(&staging, tag)?;
                fs::rename(&staging, &destination)?;
                fs::File::open(&versions)?.sync_all()?;
                Ok(())
            })();
            if result.is_err() {
                let _ = fs::remove_dir_all(&staging);
            }
            result?
        } else {
            self.validate_staged(&destination, tag)?
        }
        self.activate(&destination)?;
        Ok(tag.into())
    }
    fn build(&self, tag: &str, staging: &Path) -> Result<()> {
        release_tag(tag)?;
        let source = staging.join("source");
        let source_s = source.to_str().context("UTF-8 source required")?;
        self.command(
            "git",
            &[
                "clone",
                "--quiet",
                "--depth",
                "1",
                "--branch",
                tag,
                &format!("https://github.com/{REPOSITORY}.git"),
                source_s,
            ],
            staging,
            120,
        )?;
        let commit = String::from_utf8(self.command(
            "git",
            &["-c", "core.hooksPath=/dev/null", "rev-parse", "HEAD"],
            &source,
            10,
        )?)?
        .trim()
        .to_owned();
        ensure!(
            commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()),
            "exact release commit required"
        );
        let tagged = String::from_utf8(self.command(
            "git",
            &[
                "-c",
                "core.hooksPath=/dev/null",
                "rev-parse",
                &format!("refs/tags/{tag}^{{commit}}"),
            ],
            &source,
            10,
        )?)?;
        ensure!(tagged.trim() == commit, "release tag/checkout mismatch");
        let cache = self.root.join("build-cache");
        let target = cache.to_str().context("UTF-8 cache required")?;
        self.command(
            "cargo",
            &["build", "--locked", "--release", "--target-dir", target],
            &source,
            1800,
        )?;
        self.command(
            "cargo",
            &[
                "test",
                "--locked",
                "--release",
                "--target-dir",
                target,
                "--test",
                "goal_runtime",
                "--test",
                "dispatch_runtime",
            ],
            &source,
            1800,
        )?;
        let binary = cache.join("release/code-factory-controller");
        for args in [
            vec!["--version"],
            vec!["update", "--help"],
            vec!["research-dispatch", "--help"],
        ] {
            self.command(
                binary.to_str().context("UTF-8 binary required")?,
                &args,
                &source,
                10,
            )?;
        }
        fs::create_dir(staging.join("bin"))?;
        fs::copy(&binary, staging.join("bin/code-factory-controller"))?;
        let share = staging.join("share/code-factory");
        fs::create_dir_all(share.join("scripts"))?;
        for entry in fs::read_dir(source.join("scripts"))? {
            let p = entry?.path();
            let name = p
                .file_name()
                .context("runtime basename missing")?
                .to_string_lossy();
            if p.extension().is_some_and(|s| s == "mjs")
                && !name.starts_with("test-")
                && !matches!(name.as_ref(), "live-delivery.mjs" | "plugin-contract.mjs")
            {
                fs::copy(&p, share.join("scripts").join(name.as_ref()))?;
            }
        }
        copy_distribution(&source.join("vendor"), &share.join("vendor"))?;
        copy_distribution(&source.join("contracts"), &share.join("contracts"))?;
        let mut runtime_files = serde_json::Map::new();
        distribution_hashes(staging, &share, &mut runtime_files)?;
        save(
            &staging.join("release.json"),
            &json!({"tag":tag,"repository":REPOSITORY,"commit":commit,"binary_sha256":sha(&binary)?,"native_cli":true,"runtime_files":runtime_files}),
        )?;
        fs::remove_dir_all(&source)?;
        Ok(())
    }
    fn validate_staged(&self, destination: &Path, tag: &str) -> Result<()> {
        let release = read(&destination.join("release.json"))?;
        ensure!(
            release["tag"] == tag
                && release["repository"] == REPOSITORY
                && release["native_cli"] == true
                && release["commit"]
                    .as_str()
                    .is_some_and(|c| c.len() == 40 && c.bytes().all(|b| b.is_ascii_hexdigit()))
                && release["binary_sha256"]
                    == sha(&destination.join("bin/code-factory-controller"))?,
            "installed release identity/hash mismatch"
        );
        for (relative, expected) in release["runtime_files"]
            .as_object()
            .context("installed runtime manifest missing")?
        {
            let p = Path::new(relative);
            ensure!(
                !p.is_absolute()
                    && !p
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir))
                    && p.starts_with("share/code-factory"),
                "installed runtime path mismatch"
            );
            ensure!(
                sha(&destination.join(p))? == expected.as_str().context("runtime hash required")?,
                "installed JS/runtime artifact changed"
            );
        }
        ensure!(
            fs::metadata(destination.join("bin/code-factory-controller"))?
                .permissions()
                .mode()
                & 0o111
                != 0,
            "installed controller is not executable"
        );
        Ok(())
    }
    fn snapshot(&self, p: &Path) -> Result<Value> {
        match fs::symlink_metadata(p) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    Ok(json!({"path":p,"kind":"link","target":fs::read_link(p)?}))
                } else {
                    ensure!(meta.is_file(), "activation target must be file or symlink");
                    use base64::Engine as _;
                    Ok(
                        json!({"path":p,"kind":"file","bytes":base64::engine::general_purpose::STANDARD.encode(fs::read(p)?),"mode":meta.permissions().mode()&0o777}),
                    )
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(json!({"path":p,"kind":"absent"}))
            }
            Err(e) => Err(e.into()),
        }
    }
    pub fn recover_activation(&self) -> Result<()> {
        let journal = self.root.join("activation-journal.json");
        if !journal.exists() {
            return Ok(());
        }
        let saved = read(&journal)?;
        for item in saved["previous"]
            .as_array()
            .context("activation journal invalid")?
        {
            let p = Path::new(item["path"].as_str().context("activation path missing")?);
            ensure!(
                [
                    self.root.join("current"),
                    self.bin.join("code-factory"),
                    self.bin.join("code-factory-controller")
                ]
                .contains(&p.to_owned()),
                "activation journal target mismatch"
            );
            match item["kind"].as_str() {
                Some("link") => atomic_link(
                    Path::new(item["target"].as_str().context("old link target missing")?),
                    p,
                )?,
                Some("file") => {
                    use base64::Engine as _;
                    write_bytes_atomic(
                        p,
                        &base64::engine::general_purpose::STANDARD
                            .decode(item["bytes"].as_str().context("old bytes missing")?)?,
                    )?;
                    fs::set_permissions(
                        p,
                        fs::Permissions::from_mode(
                            item["mode"].as_u64().context("old mode missing")? as u32,
                        ),
                    )?
                }
                Some("absent") => crate::dispatch_queue::unlink(p)?,
                _ => bail!("invalid activation journal"),
            }
        }
        crate::dispatch_queue::unlink(&journal)
    }
    fn activate(&self, destination: &Path) -> Result<()> {
        fs::create_dir_all(&self.bin)?;
        let current = self.root.join("current");
        let wrapper = self.bin.join("code-factory");
        let binary = self.bin.join("code-factory-controller");
        let previous = [&current, &wrapper, &binary]
            .iter()
            .map(|p| self.snapshot(p))
            .collect::<Result<Vec<_>>>()?;
        let journal = self.root.join("activation-journal.json");
        save(
            &journal,
            &json!({"version":1,"previous":previous,"destination":destination}),
        )?;
        let result = (|| -> Result<()> {
            // Point wrappers at stable current path; active processes retain their executable.
            atomic_link(
                &self.root.join("current/bin/code-factory-controller"),
                &wrapper,
            )?;
            atomic_link(
                &self.root.join("current/bin/code-factory-controller"),
                &binary,
            )?;
            atomic_link(destination, &current)?;
            crate::dispatch_queue::unlink(&journal)
        })();
        if let Err(e) = result {
            self.recover_activation()
                .context("activation rollback failed; preserve journal")?;
            return Err(e);
        }
        Ok(())
    }
    pub fn enable_updates(&self) -> Result<()> {
        let home = PathBuf::from(std::env::var_os("HOME").context("HOME required")?);
        let units = home.join(".config/systemd/user");
        fs::create_dir_all(&units)?;
        write_bytes_atomic(&units.join("code-factory-update.service"),b"[Unit]\nDescription=Update code-factory from stable GitHub releases\n\n[Service]\nType=oneshot\nEnvironment=PATH=%h/.cargo/bin:%h/.local/bin:/usr/local/bin:/usr/bin:/bin\nExecStart=%h/.local/bin/code-factory update\n")?;
        write_bytes_atomic(&units.join("code-factory-update.timer"),b"[Unit]\nDescription=Daily code-factory release check\n\n[Timer]\nOnBootSec=10min\nOnCalendar=daily\nRandomizedDelaySec=1h\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n")?;
        self.command("systemctl", &["--user", "daemon-reload"], &units, 30)?;
        self.command(
            "systemctl",
            &["--user", "enable", "--now", "code-factory-update.timer"],
            &units,
            30,
        )?;
        Ok(())
    }
}
#[derive(clap::Args, Debug)]
pub struct Options {
    #[arg(long)]
    pub tag: Option<String>,
    #[arg(long)]
    pub auto_update: bool,
}
pub async fn run_cli(options: Options) -> Result<()> {
    let installer = Installer::for_user()?;
    let tag = installer.update(options.tag.as_deref()).await?;
    if options.auto_update {
        installer.enable_updates()?
    }
    println!(
        "Installed/current {tag}: {}",
        installer.bin.join("code-factory").display()
    );
    Ok(())
}

fn copy_distribution(source: &Path, destination: &Path) -> Result<()> {
    let m = fs::symlink_metadata(source)?;
    ensure!(
        !m.file_type().is_symlink(),
        "distribution symlinks require explicit review"
    );
    if m.is_dir() {
        fs::create_dir_all(destination)?;
        for e in fs::read_dir(source)? {
            let e = e?;
            copy_distribution(&e.path(), &destination.join(e.file_name()))?;
        }
    } else {
        ensure!(m.is_file(), "distribution must contain regular files");
        fs::copy(source, destination)?;
    }
    Ok(())
}

fn distribution_hashes(
    root: &Path,
    dir: &Path,
    out: &mut serde_json::Map<String, Value>,
) -> Result<()> {
    for e in fs::read_dir(dir)? {
        let p = e?.path();
        if p.is_dir() {
            distribution_hashes(root, &p, out)?;
        } else {
            out.insert(
                p.strip_prefix(root)?
                    .to_str()
                    .context("UTF-8 runtime path required")?
                    .into(),
                json!(sha(&p)?),
            );
        }
    }
    Ok(())
}
