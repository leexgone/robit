//! Tool confirmation coordinator for Bot platforms.
//!
//! Unlike GUI which uses dialog boxes, Bot confirmation happens via inline
//! chat messages: the Confirmer sends a prompt, then waits for the user to
//! reply with an approve/reject keyword (with a timeout).
//!
//! NOTE: Full implementation lands in Phase 5.

use std::sync::Arc;

/// Keywords that trigger approval or rejection.
#[derive(Debug, Clone)]
pub struct ConfirmKeywords {
    pub approve: Vec<String>,
    pub reject: Vec<String>,
}

impl Default for ConfirmKeywords {
    fn default() -> Self {
        Self {
            approve: vec![
                "确认".into(),
                "同意".into(),
                "yes".into(),
                "y".into(),
                "approve".into(),
                "ok".into(),
                "允许".into(),
            ],
            reject: vec![
                "取消".into(),
                "拒绝".into(),
                "no".into(),
                "n".into(),
                "reject".into(),
                "cancel".into(),
                "deny".into(),
            ],
        }
    }
}

/// Tool confirmation coordinator for Bot platforms.
///
/// NOTE: stub — `request`/`check_confirmation_response` implemented in Phase 5.
pub struct Confirmer {
    #[allow(dead_code)]
    keywords: ConfirmKeywords,
}

impl Confirmer {
    pub fn new(_keywords: ConfirmKeywords) -> Self {
        Self {
            keywords: _keywords,
        }
    }

    /// Placeholder handle to satisfy `Arc<Confirmer>` usage before Phase 5.
    pub fn placeholder() -> Arc<Self> {
        Arc::new(Self::new(ConfirmKeywords::default()))
    }
}
