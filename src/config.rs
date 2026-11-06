//! Loadout selection, persisted as JSON next to 30XX.exe.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    /// Master switch: when false, runs start exactly like vanilla.
    pub enabled: bool,
    /// Item ids per local player slot (Player 1, Player 2). Order is kept.
    pub players: [Vec<i32>; 2],
}

impl Default for Config {
    fn default() -> Self {
        Self { enabled: true, players: [Vec::new(), Vec::new()] }
    }
}

pub fn game_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn path() -> PathBuf {
    game_dir().join("loadout30xx.json")
}

impl Config {
    pub fn load() -> Self {
        std::fs::read_to_string(path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path(), s);
        }
    }
}
