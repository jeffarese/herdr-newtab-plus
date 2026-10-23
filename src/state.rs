//! What the picker remembers between opens.
//!
//! Persisted under the plugin state directory. Three jobs: order the resting
//! list so the folder you keep coming back to is the first Enter, remember
//! which agent that folder wants so the chip row is already right when the
//! picker opens, and hold the create mode the toggle was last left on.
//!
//! The mode is one value for the whole picker rather than one per folder,
//! because it is a preference about how you like new tabs to land.

use crate::pyjson::{self, Map, Value};
use crate::pyos;
use crate::session::{mode_of, normalize};
use std::io::Write;

pub const MAX_ENTRIES: usize = 80;

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: String,
    pub agent: String,
    /// Whatever number the file held, kept as written so a save gives it
    /// back unchanged.
    pub ts: Value,
}

#[derive(Debug)]
pub struct Recents {
    pub path: Option<String>,
    pub entries: Vec<Entry>,
    create: Option<&'static str>,
    // Writing the file is skipped unless something actually changed, so the
    // picker can save on the way out of either exit without a cancel
    // rewriting the same bytes.
    dirty: bool,
}

impl Recents {
    pub fn new(path: Option<String>) -> Recents {
        let mut recents = Recents { path, entries: Vec::new(), create: None, dirty: false };
        recents.load();
        recents
    }

    fn load(&mut self) {
        let Some(path) = self.path.as_deref() else { return };
        let Ok(data) = std::fs::read(path) else { return };
        let Ok(text) = String::from_utf8(data) else { return };
        let Ok(Value::Dict(data)) = pyjson::loads(&text) else { return };
        if let Some(create) = data.get("create").and_then(|v| v.as_str()).and_then(mode_of) {
            self.create = Some(create);
        }
        let Some(Value::List(entries)) = data.get("entries") else { return };
        let mut cleaned: Vec<Entry> = Vec::new();
        for entry in entries {
            if entry.as_dict().is_none() {
                continue;
            }
            let Some(path) = entry.get("path").as_str().filter(|p| !p.is_empty()) else { continue };
            let ts = entry.get("ts");
            let ts = match ts {
                Value::Int(_) | Value::BigInt(_) | Value::Bool(_) | Value::Float(_) => ts.clone(),
                _ => Value::Int(0),
            };
            cleaned.push(Entry {
                path: normalize(path),
                agent: entry.get("agent").as_str().unwrap_or("").to_string(),
                ts,
            });
        }
        cleaned.truncate(MAX_ENTRIES);
        self.entries = cleaned;
    }

    /// Recent folders, most recent first.
    pub fn folders(&self) -> Vec<String> {
        self.entries.iter().map(|entry| entry.path.clone()).collect()
    }

    /// The agent last started in this folder, or None.
    pub fn agent_for(&self, folder: &str) -> Option<String> {
        let target = normalize(folder);
        let entry = self.entries.iter().find(|entry| entry.path == target)?;
        if entry.agent.is_empty() {
            None
        } else {
            Some(entry.agent.clone())
        }
    }

    /// Every remembered folder that was last opened with an agent, and which.
    pub fn agents(&self) -> std::collections::HashMap<String, String> {
        let mut found = std::collections::HashMap::new();
        for entry in &self.entries {
            if !entry.agent.is_empty() {
                found.entry(entry.path.clone()).or_insert_with(|| entry.agent.clone());
            }
        }
        found
    }

    /// The most recently started agent anywhere -- the default for a new folder.
    pub fn last_agent(&self) -> Option<String> {
        self.entries.iter().find(|entry| !entry.agent.is_empty()).map(|entry| entry.agent.clone())
    }

    /// The mode the toggle was last left on, or None if it never was.
    pub fn create_mode(&self) -> Option<&'static str> {
        self.create
    }

    pub fn remember_create_mode(&mut self, mode: &str) {
        let Some(mode) = mode_of(mode) else { return };
        if Some(mode) == self.create {
            return;
        }
        self.create = Some(mode);
        self.dirty = true;
    }

    pub fn touch(&mut self, folder: &str, agent: &str) {
        let target = normalize(folder);
        if target.is_empty() {
            return;
        }
        self.entries.retain(|entry| entry.path != target);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i128)
            .unwrap_or(0);
        self.entries.insert(0, Entry { path: target, agent: agent.to_string(), ts: Value::Int(now) });
        self.entries.truncate(MAX_ENTRIES);
        self.dirty = true;
    }

    /// The file's contents, as `json.dump` writes them.
    pub fn payload(&self) -> String {
        let mut payload = Map::new();
        payload.insert("version", Value::Int(1));
        let entries = self
            .entries
            .iter()
            .map(|entry| {
                let mut map = Map::new();
                map.insert("path", Value::str(entry.path.clone()));
                map.insert("agent", Value::str(entry.agent.clone()));
                map.insert("ts", entry.ts.clone());
                Value::Dict(map)
            })
            .collect();
        payload.insert("entries", Value::List(entries));
        if let Some(create) = self.create {
            payload.insert("create", Value::str(create));
        }
        pyjson::dumps(&Value::Dict(payload), false)
    }

    pub fn save(&mut self) {
        let Some(path) = self.path.clone() else { return };
        if !self.dirty {
            return;
        }
        // Losing the recent list is not worth failing a create over.
        if write_atomically(&path, self.payload().as_bytes()).is_ok() {
            self.dirty = false;
        }
    }
}

/// Write beside the target, sync, then rename over it: a reader sees the old
/// file or the new one, never half of one. The temporary file is private to
/// its owner, as Python's NamedTemporaryFile made it, and so the state file is.
fn write_atomically(path: &str, data: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let directory = pyos::dirname(path);
    if !directory.is_empty() {
        std::fs::create_dir_all(&directory)?;
    }
    let folder = if directory.is_empty() { ".".to_string() } else { directory };
    let mut attempt = 0u32;
    let (temp, mut handle) = loop {
        let name = format!("{}/.recent-{}.json", folder, random_suffix(attempt));
        match std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&name) {
            Ok(handle) => break (name, handle),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && attempt < 100 => attempt += 1,
            Err(error) => return Err(error),
        }
    };
    let written = handle.write_all(data).and_then(|_| handle.flush()).and_then(|_| handle.sync_all());
    drop(handle);
    if let Err(error) = written.and_then(|_| std::fs::rename(&temp, path)) {
        // Python's NamedTemporaryFile(delete=False) left a failed one behind;
        // there is no reason to copy that.
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    Ok(())
}

fn random_suffix(attempt: u32) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64 ^ d.as_secs())
        .unwrap_or(0);
    let mut seed = nanos ^ ((std::process::id() as u64) << 32) ^ (attempt as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_";
    (0..8)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            ALPHABET[(seed % ALPHABET.len() as u64) as usize] as char
        })
        .collect()
}
