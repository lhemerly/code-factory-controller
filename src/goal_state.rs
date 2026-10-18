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
pub fn python_float(f: f64) -> String {
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
}

pub fn python_number(n: &serde_json::Number) -> String {
    if n.is_f64() {
        python_float(n.as_f64().expect("float"))
    } else {
        n.to_string()
    }
}

/// RawJson parses JSON without reordering object keys, preserving exact insertion/source order.
#[derive(Clone, Debug, PartialEq)]
pub enum RawJson {
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Array(Vec<RawJson>),
    Object(Vec<(String, RawJson)>),
}

impl RawJson {
    pub fn parse(raw: &[u8]) -> Result<Self> {
        let mut de = serde_json::Deserializer::from_slice(raw);
        let val = Self::deserialize(&mut de).map_err(|e| anyhow::anyhow!("{e}"))?;
        de.end().map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(val)
    }

    pub fn read(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&bytes).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn from_value(value: &Value) -> Self {
        match value {
            Value::Null => RawJson::Null,
            Value::Bool(b) => RawJson::Bool(*b),
            Value::Number(n) => RawJson::Number(n.clone()),
            Value::String(s) => RawJson::String(s.clone()),
            Value::Array(arr) => RawJson::Array(arr.iter().map(Self::from_value).collect()),
            Value::Object(map) => RawJson::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), Self::from_value(v)))
                    .collect(),
            ),
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            RawJson::Null => Value::Null,
            RawJson::Bool(b) => Value::Bool(*b),
            RawJson::Number(n) => Value::Number(n.clone()),
            RawJson::String(s) => Value::String(s.clone()),
            RawJson::Array(arr) => Value::Array(arr.iter().map(Self::to_value).collect()),
            RawJson::Object(map) => {
                let mut obj = serde_json::Map::new();
                for (k, v) in map {
                    obj.insert(k.clone(), v.to_value());
                }
                Value::Object(obj)
            }
        }
    }

    pub fn get(&self, key: &str) -> Option<&RawJson> {
        match self {
            RawJson::Object(entries) => entries.iter().rfind(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut RawJson> {
        match self {
            RawJson::Object(entries) => entries.iter_mut().rfind(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn set(&mut self, key: impl Into<String>, val: RawJson) -> Result<()> {
        let key = key.into();
        let RawJson::Object(entries) = self else {
            anyhow::bail!("expected JSON object");
        };
        if let Some(pos) = entries.iter().rposition(|(k, _)| *k == key) {
            entries[pos].1 = val;
        } else {
            entries.push((key, val));
        }
        Ok(())
    }

    pub fn remove(&mut self, key: &str) -> Option<RawJson> {
        let RawJson::Object(entries) = self else {
            return None;
        };
        entries
            .iter()
            .rposition(|(k, _)| k == key)
            .map(|pos| entries.remove(pos).1)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            RawJson::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn is_object(&self) -> bool {
        matches!(self, RawJson::Object(_))
    }

    pub fn is_null(&self) -> bool {
        matches!(self, RawJson::Null)
    }

    fn deserialize<'de, D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::de::Deserializer<'de>,
    {
        use serde::de::*;
        struct RawVisitor;
        impl<'de> Visitor<'de> for RawVisitor {
            type Value = RawJson;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("any JSON value")
            }
            fn visit_bool<E>(self, v: bool) -> Result<RawJson, E> {
                Ok(RawJson::Bool(v))
            }
            fn visit_i64<E>(self, v: i64) -> Result<RawJson, E> {
                Ok(RawJson::Number(v.into()))
            }
            fn visit_u64<E>(self, v: u64) -> Result<RawJson, E> {
                Ok(RawJson::Number(v.into()))
            }
            fn visit_f64<E: Error>(self, v: f64) -> Result<RawJson, E> {
                let n = serde_json::Number::from_f64(v)
                    .ok_or_else(|| E::custom("nonfinite JSON number"))?;
                Ok(RawJson::Number(n))
            }
            fn visit_str<E>(self, v: &str) -> Result<RawJson, E> {
                Ok(RawJson::String(v.to_string()))
            }
            fn visit_string<E>(self, v: String) -> Result<RawJson, E> {
                Ok(RawJson::String(v))
            }
            fn visit_unit<E>(self) -> Result<RawJson, E> {
                Ok(RawJson::Null)
            }
            fn visit_none<E>(self) -> Result<RawJson, E> {
                Ok(RawJson::Null)
            }
            fn visit_some<D: Deserializer<'de>>(
                self,
                deserializer: D,
            ) -> Result<RawJson, D::Error> {
                deserializer.deserialize_any(self)
            }
            fn visit_seq<S: SeqAccess<'de>>(self, mut access: S) -> Result<RawJson, S::Error> {
                let mut vec = Vec::new();
                while let Some(elem) = access.next_element::<RawJson>()? {
                    vec.push(elem);
                }
                Ok(RawJson::Array(vec))
            }
            fn visit_map<M: MapAccess<'de>>(self, mut access: M) -> Result<RawJson, M::Error> {
                let mut entries: Vec<(String, RawJson)> = Vec::new();
                while let Some(key) = access.next_key::<String>()? {
                    let val = access.next_value::<RawJson>()?;
                    if let Some(pos) = entries.iter().position(|(k, _)| *k == key) {
                        entries[pos].1 = val;
                    } else {
                        entries.push((key, val));
                    }
                }
                Ok(RawJson::Object(entries))
            }
        }
        deserializer.deserialize_any(RawVisitor)
    }
}

impl<'de> serde::Deserialize<'de> for RawJson {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::de::Deserializer<'de>,
    {
        Self::deserialize(deserializer)
    }
}

impl serde::Serialize for RawJson {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::{SerializeMap, SerializeSeq};
        match self {
            RawJson::Null => serializer.serialize_unit(),
            RawJson::Bool(b) => serializer.serialize_bool(*b),
            RawJson::Number(n) => n.serialize(serializer),
            RawJson::String(s) => serializer.serialize_str(s),
            RawJson::Array(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            RawJson::Object(entries) => {
                let mut map = serializer.serialize_map(Some(entries.len()))?;
                for (k, v) in entries {
                    map.serialize_entry(k, v)?;
                }
                map.end()
            }
        }
    }
}

pub fn raw_json_compact_utf8(value: &RawJson) -> String {
    match value {
        RawJson::Null => "null".into(),
        RawJson::Bool(v) => v.to_string(),
        RawJson::Number(n) => python_number(n),
        RawJson::String(s) => serde_json::to_string(s).expect("JSON string"),
        RawJson::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(raw_json_compact_utf8)
                .collect::<Vec<_>>()
                .join(",")
        ),
        RawJson::Object(entries) => {
            format!(
                "{{{}}}",
                entries
                    .iter()
                    .map(|(key, val)| {
                        format!(
                            "{}:{}",
                            serde_json::to_string(key).expect("JSON string"),
                            raw_json_compact_utf8(val)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

pub fn python_json_compact_utf8(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(v) => v.to_string(),
        Value::Number(n) => python_number(n),
        Value::String(s) => serde_json::to_string(s).expect("JSON string"),
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(python_json_compact_utf8)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(map) => {
            let items: Vec<_> = map.iter().collect();
            format!(
                "{{{}}}",
                items
                    .into_iter()
                    .map(|(key, val)| {
                        format!(
                            "{}:{}",
                            serde_json::to_string(key).expect("JSON string"),
                            python_json_compact_utf8(val)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

pub fn python_json_compact_ascii(value: &Value) -> String {
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
        Value::Number(n) => python_number(n),
        Value::String(s) => string(s),
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(python_json_compact_ascii)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(map) => {
            let mut items: Vec<_> = map.iter().collect();
            items.sort_by_key(|(key, _)| *key);
            format!(
                "{{{}}}",
                items
                    .into_iter()
                    .map(|(key, val)| format!("{}:{}", string(key), python_json_compact_ascii(val)))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}
pub fn raw_json_pretty(value: &RawJson) -> String {
    fn string_ascii(value: &str) -> String {
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
    fn fmt(v: &RawJson, indent: usize) -> String {
        let pad = " ".repeat(indent);
        let next_pad = " ".repeat(indent + 2);
        match v {
            RawJson::Null => "null".into(),
            RawJson::Bool(b) => b.to_string(),
            RawJson::Number(n) => python_number(n),
            RawJson::String(s) => string_ascii(s),
            RawJson::Array(items) => {
                if items.is_empty() {
                    "[]".into()
                } else {
                    let inner = items
                        .iter()
                        .map(|i| format!("{next_pad}{}", fmt(i, indent + 2)))
                        .collect::<Vec<_>>()
                        .join(",\n");
                    format!("[\n{inner}\n{pad}]")
                }
            }
            RawJson::Object(entries) => {
                if entries.is_empty() {
                    "{}".into()
                } else {
                    let inner = entries
                        .iter()
                        .map(|(k, val)| {
                            format!("{next_pad}{}: {}", string_ascii(k), fmt(val, indent + 2))
                        })
                        .collect::<Vec<_>>()
                        .join(",\n");
                    format!("{{\n{inner}\n{pad}}}")
                }
            }
        }
    }
    let mut s = fmt(value, 0);
    s.push('\n');
    s
}

pub fn digest(value: &Value) -> String {
    format!("{:x}", Sha256::digest(python_json(value).as_bytes()))
}
pub fn save(path: &Path, value: &Value) -> Result<()> {
    fs::create_dir_all(path.parent().context("state parent")?)?;
    crate::supervisor::write_json(path, value)
}
pub fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    if bytes.len() > 16_777_216 {
        anyhow::bail!("execution record exceeds limit");
    }
    let parent = path.parent().context("state parent")?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension(format!("tmp-{}", crate::model::new_id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}
pub fn save_raw(path: &Path, value: &RawJson) -> Result<()> {
    write_bytes_atomic(path, raw_json_pretty(value).as_bytes())
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
    pub(crate) fn raw_fd(&self) -> i32 {
        self._file.as_raw_fd()
    }
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
