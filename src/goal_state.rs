//! Persistence and Python-compatible policy fingerprints for staged migration.
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::{Path, PathBuf},
};

/// Preserve json.dumps(sort_keys=True)'s separators and ASCII escaping. Policy
/// fingerprints are state compatibility, not merely a choice of JSON formatting.
pub fn python_json(value: &Value) -> String {
    fn string(value: &str) -> String {
        let encoded = serde_json::to_string(value).expect("JSON string");
        let mut out = String::new();
        for c in encoded.chars() {
            if c.is_ascii() && c != '\u{7f}' {
                out.push(c);
            } else {
                for unit in c.encode_utf16(&mut [0; 2]) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
        out
    }
    match value {
        Value::Null => "null".into(),
        Value::Bool(v) => v.to_string(),
        Value::Number(n) => {
            if n.is_f64() {
                let f = n.as_f64().expect("float");
                if f != 0.0 && (f.abs() < 1e-4 || f.abs() >= 1e16) {
                    let sci = format!("{f:e}");
                    let (mantissa, exponent) = sci.split_once('e').expect("scientific number");
                    let exponent: i32 = exponent.parse().expect("exponent");
                    format!("{mantissa}e{exponent:+03}")
                } else {
                    let result = f.to_string();
                    if result.contains('.') {
                        result
                    } else {
                        format!("{result}.0")
                    }
                }
            } else {
                n.to_string()
            }
        }
        Value::String(s) => string(s),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(python_json).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(map) => {
            let mut items: Vec<_> = map.iter().collect();
            items.sort_by_key(|(key, _)| *key);
            format!(
                "{{{}}}",
                items
                    .into_iter()
                    .map(|(key, value)| format!("{}: {}", string(key), python_json(value)))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }
}
pub fn digest(value: &Value) -> String {
    format!("{:x}", Sha256::digest(python_json(value).as_bytes()))
}
pub fn save(path: &Path, value: &Value) -> Result<()> {
    fs::create_dir_all(path.parent().context("state parent")?)?;
    crate::supervisor::write_json(path, value)
}
/// Resolve existing parent symlinks even before the target is created, matching
/// pathlib.resolve(strict=False). Missing suffixes remain lexical components.
pub fn resolve(path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut resolved = PathBuf::new();
    for component in path.components() {
        use std::path::Component;
        match component {
            Component::ParentDir => {
                resolved.pop();
            }
            Component::CurDir => (),
            other => {
                resolved.push(other.as_os_str());
                if fs::symlink_metadata(&resolved).is_ok() {
                    resolved = fs::canonicalize(&resolved)?;
                }
            }
        }
    }
    Ok(resolved)
}
pub struct Lock {
    _file: File,
}
impl Lock {
    pub fn acquire(path: &Path, wait: bool) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        ensure!(file.metadata()?.is_file(), "owner lock must be regular");
        let result = unsafe {
            libc::flock(
                file.as_raw_fd(),
                libc::LOCK_EX | if wait { 0 } else { libc::LOCK_NB },
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error()).context("another owner holds this state");
        }
        Ok(Self { _file: file })
    }
}
