//! QQ WebSocket gateway protocol types (opcodes, payloads).
//!
//! NOTE: Full implementation lands in Phase 8.

use serde::{Deserialize, Serialize};

/// Top-level gateway payload, exchanged over the WebSocket connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayPayload {
    /// Opcode (0=Dispatch, 1=Heartbeat, 2=Identify, ...).
    pub op: u32,
    /// Data payload.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub d: Option<serde_json::Value>,
    /// Sequence number (for dispatch / resume).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub s: Option<u64>,
    /// Event type (for op=0 Dispatch).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub t: Option<String>,
}

// QQ intent bitflags.
pub const INTENT_DIRECT_MESSAGE: u32 = 1 << 12;
pub const INTENT_GROUP_AT_MESSAGE: u32 = 1 << 25;
