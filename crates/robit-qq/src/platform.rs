//! QQ Official Bot platform adapter.
//!
//! NOTE: Full implementation lands in Phase 8.

use robit_ai::config::RobitConfig;

/// QQ Bot configuration parsed from `[channels.qq_bot]`.
#[derive(Debug, Clone)]
pub struct QqConfig {
    pub app_id: String,
    pub app_secret: String,
    pub bot_token: String,
    pub sandbox: bool,
}

impl QqConfig {
    /// Extract QQ Bot config from the loaded `RobitConfig`.
    ///
    /// NOTE: depends on Phase 2 config additions (`channels.qq_bot`).
    pub fn from_config(config: &RobitConfig) -> Result<Self, String> {
        let qq = config
            .channels
            .as_ref()
            .and_then(|c| c.qq_bot.as_ref())
            .ok_or_else(|| {
                "QQ Bot config not found. Add [channels.qq_bot] section to config.toml".to_string()
            })?;
        Ok(Self {
            app_id: qq.app_id.clone(),
            app_secret: qq.app_secret.clone(),
            bot_token: qq.bot_token.clone(),
            sandbox: false,
        })
    }

    pub fn gateway_url(&self) -> &str {
        if self.sandbox {
            "wss://sandbox.api.sgroup.qq.com/gateway"
        } else {
            "wss://api.sgroup.qq.com/gateway"
        }
    }

    pub fn api_base_url(&self) -> &str {
        if self.sandbox {
            "https://sandbox.api.sgroup.qq.com"
        } else {
            "https://api.sgroup.qq.com"
        }
    }
}

/// QQ Official Bot platform adapter.
///
/// NOTE: stub — `PlatformAdapter` impl lands in Phase 8.
pub struct QqPlatformAdapter;
