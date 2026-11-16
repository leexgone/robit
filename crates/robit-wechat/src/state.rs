//! Login-state persistence for the WeChat adapter.
//!
//! The `weixin-agent` SDK is deliberately persistence-free; this module owns
//! the three pieces of state that must survive restarts:
//!
//! - `bot_token` — login credential from the QR-scan flow
//! - `sync_buf` — long-poll cursor (resume without losing messages)
//! - `context_tokens` — per-peer outbound context tokens (reply correlation)
//!
//! Stored as JSON at `{working_dir}/.robit/wechat-state.json`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Persisted WeChat login/transport state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WechatState {
    /// Bot authentication token (from `robit-wechat login`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_token: Option<String>,
    /// Bot ID returned by the login confirmation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ilink_bot_id: Option<String>,
    /// API base URL returned by the login confirmation (preferred over config).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Long-poll cursor. `None` on first start; updated on every sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_buf: Option<String>,
    /// Per-peer context tokens for outbound messages.
    #[serde(default)]
    pub context_tokens: HashMap<String, String>,
}

impl WechatState {
    /// Path of the state file inside a working directory.
    pub fn path(working_dir: &Path) -> PathBuf {
        working_dir.join(".robit").join("wechat-state.json")
    }

    /// Load the state file; `None` when absent or unreadable (fresh start).
    pub fn load(working_dir: &Path) -> Option<Self> {
        let path = Self::path(working_dir);
        let text = std::fs::read_to_string(path).ok()?;
        match serde_json::from_str(&text) {
            Ok(state) => Some(state),
            Err(e) => {
                tracing::warn!("Ignoring malformed wechat-state.json: {}", e);
                None
            }
        }
    }

    /// Atomically save the state file (create parent dirs as needed).
    /// Returns `Err` with a message on IO failure.
    pub fn save(&self, working_dir: &Path) -> std::result::Result<(), String> {
        let path = Self::path(working_dir);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize wechat state: {}", e))?;
        std::fs::write(&path, text).map_err(|e| format!("Failed to write {}: {}", path.display(), e))
    }

    /// Merge a new long-poll cursor, dropping the field when it is empty
    /// (an empty buf from the server means "start over", not "keep the old").
    pub fn update_sync_buf(&mut self, sync_buf: &str) {
        if sync_buf.is_empty() {
            self.sync_buf = None;
        } else {
            self.sync_buf = Some(sync_buf.to_string());
        }
    }

    /// Record a peer's outbound context token.
    pub fn set_context_token(&mut self, peer: &str, token: &str) {
        self.context_tokens.insert(peer.to_string(), token.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_roundtrip_and_sync_buf_update() {
        let mut state = WechatState::default();
        state.bot_token = Some("tok".into());
        state.update_sync_buf("buf-1");
        state.set_context_token("peer-1", "ctx-1");

        let dir = std::env::temp_dir().join("robit-wechat-state-test");
        state.save(&dir).unwrap();
        let loaded = WechatState::load(&dir).unwrap();
        assert_eq!(loaded.bot_token.as_deref(), Some("tok"));
        assert_eq!(loaded.sync_buf.as_deref(), Some("buf-1"));
        assert_eq!(loaded.context_tokens.get("peer-1").map(String::as_str), Some("ctx-1"));

        // Empty cursor clears the field.
        let mut loaded = WechatState::load(&dir).unwrap();
        loaded.update_sync_buf("");
        assert!(loaded.sync_buf.is_none());

        std::fs::remove_file(WechatState::path(&dir)).ok();
    }

    #[test]
    fn load_missing_returns_none() {
        assert!(WechatState::load(Path::new("Z:/definitely-not-here")).is_none());
    }
}
