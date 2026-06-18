//! QQ Official Bot WebSocket gateway protocol types (opcodes, payloads).
//!
//! The QQ Bot gateway is Discord-like: a WebSocket connection exchanging JSON
//! [`GatewayPayload`] frames keyed by `op` (opcode). The client identifies,
//! heartbeats, and receives `op=0` dispatch events (`C2C_MESSAGE_CREATE`,
//! `GROUP_AT_MESSAGE_CREATE`, etc.). Message sending is done over HTTP.
//!
//! Reference: QQ Official Bot API (https://bot.q.qq.com).

use serde::{Deserialize, Serialize};

// ===========================================================================
// Opcodes
// ===========================================================================

/// Gateway opcode values.
pub mod op {
    /// Server pushes an event.
    pub const DISPATCH: u32 = 0;
    /// Client sends / server replies to heartbeat.
    pub const HEARTBEAT: u32 = 1;
    /// Client authenticates the connection.
    pub const IDENTIFY: u32 = 2;
    /// Client resumes a broken session.
    pub const RESUME: u32 = 6;
    /// Server asks the client to reconnect.
    pub const RECONNECT: u32 = 7;
    /// Server indicates the session is invalid.
    pub const INVALID_SESSION: u32 = 9;
    /// Server sends the heartbeat interval (Hello).
    pub const HELLO: u32 = 10;
    /// Server acknowledges a heartbeat.
    pub const HEARTBEAT_ACK: u32 = 11;
}

/// Dispatch event type strings (the `t` field of an `op=0` payload).
pub mod event_type {
    /// Ready event — sent after a successful Identify.
    pub const READY: &str = "READY";
    /// Resumed event — sent after a successful Resume.
    pub const RESUMED: &str = "RESUMED";
    /// C2C (private) chat message.
    pub const C2C_MESSAGE_CREATE: &str = "C2C_MESSAGE_CREATE";
    /// Group @-mention message.
    pub const GROUP_AT_MESSAGE_CREATE: &str = "GROUP_AT_MESSAGE_CREATE";
}

// ===========================================================================
// Intent bitflags
// ===========================================================================

/// Intent for C2C (private) messages.
pub const INTENT_C2C: u32 = 1 << 12;
/// Intent for group @-mention messages.
pub const INTENT_GROUP_AT_MESSAGE: u32 = 1 << 25;

// ===========================================================================
// Gateway payload
// ===========================================================================

/// Top-level gateway payload, exchanged over the WebSocket connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayPayload {
    /// Opcode (see [`op`]).
    pub op: u32,
    /// Data payload.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub d: Option<serde_json::Value>,
    /// Sequence number (for dispatch / resume).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub s: Option<u64>,
    /// Event type (for `op=0` Dispatch).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub t: Option<String>,
}

impl GatewayPayload {
    /// Build an Identify (`op=2`) payload.
    pub fn identify(access_token: &str, intents: u32) -> Self {
        Self {
            op: op::IDENTIFY,
            d: Some(serde_json::json!({
                "token": format!("QQBot {}", access_token),
                "intents": intents,
                "shard": [0, 1],
            })),
            s: None,
            t: None,
        }
    }

    /// Build a Heartbeat (`op=1`) payload carrying the last sequence number.
    pub fn heartbeat(last_seq: Option<u64>) -> Self {
        Self {
            op: op::HEARTBEAT,
            d: Some(serde_json::Value::from(last_seq)),
            s: None,
            t: None,
        }
    }
}

/// `d` payload of the Hello (`op=10`) event.
#[derive(Debug, Clone, Deserialize)]
pub struct HelloData {
    pub heartbeat_interval: u64,
}

// ===========================================================================
// Message events
// ===========================================================================

/// Common fields of an incoming message event (`C2C_MESSAGE_CREATE`,
/// `GROUP_AT_MESSAGE_CREATE`).
#[derive(Debug, Clone, Deserialize)]
pub struct MessageEvent {
    /// Message ID (needed to reply via HTTP).
    pub id: String,
    /// Message content (text). For group @-messages this excludes the @mention.
    #[serde(default)]
    pub content: String,
    /// Author / sender.
    pub author: Author,
    /// For group messages: the group's openid.
    #[serde(default, rename = "group_openid")]
    pub group_openid: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Author {
    /// Sender's openid.
    #[serde(default, rename = "user_openid")]
    pub user_openid: Option<String>,
    #[serde(default, rename = "member_openid")]
    pub member_openid: Option<String>,
}

impl MessageEvent {
    /// The sender's user identifier (openid).
    pub fn user_id(&self) -> Option<&str> {
        self.author
            .user_openid
            .as_deref()
            .or(self.author.member_openid.as_deref())
    }
}

// ===========================================================================
// HTTP send-message request
// ===========================================================================

/// Message type for the send-message HTTP API.
pub mod msg_type {
    /// Plain text.
    pub const TEXT: u32 = 0;
    /// Markdown (template / raw).
    pub const MARKDOWN: u32 = 2;
}

/// Request body for POSTing a message to a group or user.
#[derive(Debug, Clone, Serialize)]
pub struct SendMessageRequest {
    pub content: String,
    pub msg_type: u32,
    /// The incoming message ID this reply references (QQ requires this for
    /// passive replies within 5 minutes of the original message).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub msg_id: Option<String>,
    /// Monotonic sequence to dedupe replies within a msg_id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub msg_seq: Option<u32>,
}

/// Response body for the send-message HTTP API.
#[derive(Debug, Clone, Deserialize)]
pub struct SendMessageResponse {
    pub id: Option<String>,
    #[serde(default)]
    pub msg_id: Option<String>,
}

// ===========================================================================
// Access token (OAuth2)
// ===========================================================================

/// Request body for the getAppAccessToken endpoint.
#[derive(Debug, Serialize)]
pub struct AccessTokenRequest {
    pub app_id: String,
    pub client_secret: String,
}

/// Response body for the getAppAccessToken endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct AccessTokenResponse {
    pub access_token: String,
    /// Seconds until expiry.
    #[serde(default)]
    pub expires_in: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hello_payload() {
        let json = r#"{"op":10,"d":{"heartbeat_interval":41250}}"#;
        let payload: GatewayPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.op, op::HELLO);
        let hello: HelloData = serde_json::from_value(payload.d.unwrap()).unwrap();
        assert_eq!(hello.heartbeat_interval, 41250);
    }

    #[test]
    fn builds_identify_payload() {
        let p = GatewayPayload::identify("tok-123", INTENT_C2C | INTENT_GROUP_AT_MESSAGE);
        assert_eq!(p.op, op::IDENTIFY);
        let d = p.d.unwrap();
        assert_eq!(d["token"], "QQBot tok-123");
        assert_eq!(d["intents"], INTENT_C2C | INTENT_GROUP_AT_MESSAGE);
    }

    #[test]
    fn builds_heartbeat_payload() {
        let p = GatewayPayload::heartbeat(Some(42));
        assert_eq!(p.op, op::HEARTBEAT);
        assert_eq!(p.d.unwrap(), 42);
    }

    #[test]
    fn parses_group_at_message_event() {
        let json = r#"{
            "id": "msg-1",
            "content": " hello",
            "author": {"member_openid": "mem-1"},
            "group_openid": "grp-1"
        }"#;
        let ev: MessageEvent = serde_json::from_str(json).unwrap();
        assert_eq!(ev.id, "msg-1");
        assert_eq!(ev.group_openid.as_deref(), Some("grp-1"));
        assert_eq!(ev.user_id(), Some("mem-1"));
    }

    #[test]
    fn parses_c2c_message_event() {
        let json = r#"{
            "id": "msg-2",
            "content": "hi",
            "author": {"user_openid": "user-1"}
        }"#;
        let ev: MessageEvent = serde_json::from_str(json).unwrap();
        assert_eq!(ev.user_id(), Some("user-1"));
        assert!(ev.group_openid.is_none());
    }

    #[test]
    fn serializes_send_message_request() {
        let req = SendMessageRequest {
            content: "hello".into(),
            msg_type: msg_type::TEXT,
            msg_id: Some("msg-1".into()),
            msg_seq: Some(1),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"content\":\"hello\""));
        assert!(json.contains("\"msg_id\":\"msg-1\""));
        assert!(json.contains("\"msg_seq\":1"));
    }

    #[test]
    fn send_message_request_omits_none_fields() {
        let req = SendMessageRequest {
            content: "hi".into(),
            msg_type: msg_type::TEXT,
            msg_id: None,
            msg_seq: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("msg_id"));
        assert!(!json.contains("msg_seq"));
    }
}
