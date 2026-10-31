//! Historial de restauraciones en `~/.local/state/odotui/history.json`.

use std::path::PathBuf;

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

const KEEP: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub at: DateTime<Local>,
    pub profile: String,
    pub backup: String,
    pub db: String,
    pub ok: bool,
    pub message: String,
}

fn path() -> Option<PathBuf> {
    Some(dirs::state_dir()?.join("odotui/history.json"))
}

pub fn load() -> Vec<Entry> {
    path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Escritura atómica; un fallo aquí nunca es fatal.
pub fn push(entries: &mut Vec<Entry>, e: Entry) {
    entries.insert(0, e);
    entries.truncate(KEEP);
    let Some(p) = path() else { return };
    let Ok(raw) = serde_json::to_string_pretty(entries) else {
        return;
    };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, raw).is_ok() {
        let _ = std::fs::rename(tmp, p);
    }
}
