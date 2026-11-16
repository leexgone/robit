//! QR-code login flow for the WeChat iLink Bot channel.
//!
//! One-time interactive flow (`robit-wechat login`): fetch a QR code from the
//! iLink server, render it in the terminal, and poll until the user confirms
//! on their phone. The server then issues a long-lived `bot_token` which is
//! persisted to `wechat-state.json` — no app_id/app_secret, no developer
//! registration; the scan itself is the credential acquisition.
//!
//! Status flow (from the protocol SDK):
//! `wait` → `scaned` → (optional pair code) → `confirmed { bot_token, .. }`,
//! with `expired` / `verify_code_blocked` refreshing the QR code and
//! `binded_redirect` meaning the bot is already bound to this local token.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use robit_ai::config::RobitConfig;
use weixin_agent::{LoginStatus, QrLoginSession, StandaloneQrLogin, WeixinConfig};

use crate::state::WechatState;

/// Poll interval for the login status endpoint (the endpoint itself
/// long-polls; this is the retry cadence on early returns).
const LOGIN_POLL_INTERVAL: Duration = Duration::from_millis(1500);

/// Maximum QR refreshes (expiry / verify-code lockout) before giving up.
const MAX_QR_REFRESH: u32 = 3;

/// Run the interactive QR login flow for `working_dir`.
///
/// Uses `[channels.wechat_bot]` (base_url overrides) and any existing state
/// file (re-login passes the old token so the server recognizes the binding).
pub async fn run_login(working_dir: &Path, config: &RobitConfig) -> Result<(), String> {
    let section = config
        .channels
        .as_ref()
        .and_then(|c| c.wechat_bot.as_ref());
    let mut base_url = section
        .and_then(|s| s.base_url.clone())
        .unwrap_or_else(|| weixin_agent::config::DEFAULT_BASE_URL.to_string());

    let existing = WechatState::load(working_dir);
    let local_tokens: Vec<String> = existing
        .as_ref()
        .and_then(|s| s.bot_token.clone())
        .into_iter()
        .collect();

    println!("正在获取微信登录二维码…");
    println!("提示：若此前的登录仍有效，重新扫码可能触发\"已绑定\"保护。");

    let mut qr_refreshes: u32 = 0;
    'qr: loop {
        let login = StandaloneQrLogin::new(&sdk_config_for_login(&base_url)?);
        let session = login
            .start(None, &local_tokens)
            .await
            .map_err(|e| format!("获取二维码失败: {}", e))?;
        render_qr(&session);

        let mut scan_announced = false;
        let mut verify_code: Option<String> = None;
        let mut login = login;

        loop {
            tokio::time::sleep(LOGIN_POLL_INTERVAL).await;
            let status = match login.poll_status(&session, verify_code.as_deref()).await {
                Ok(s) => s,
                Err(e) => {
                    // Transient poll failure — keep waiting, the QR stays valid.
                    eprintln!("登录状态查询失败（将继续重试）: {}", e);
                    continue;
                }
            };

            match status {
                LoginStatus::Wait => {}
                LoginStatus::Scanned => {
                    // A pending verify code accepted by the server (status moved
                    // back to `scaned`): stop re-sending it in later polls.
                    verify_code = None;
                    if !scan_announced {
                        println!("✅ 已扫码，正在验证…");
                        scan_announced = true;
                    }
                }
                LoginStatus::ScannedButRedirect { redirect_host } => {
                    if !redirect_host.is_empty() {
                        // The server returns a bare host; rebuild a full base URL.
                        let new_base = if redirect_host.starts_with("http") {
                            redirect_host
                        } else {
                            format!("https://{}", redirect_host)
                        };
                        println!("↪️ 登录服务重定向到 {}", new_base);
                        base_url = new_base;
                        // Keep polling the same QR session against the new host.
                        login = StandaloneQrLogin::new(&sdk_config_for_login(&base_url)?);
                    }
                }
                LoginStatus::NeedVerifyCode => {
                    let prompt = if verify_code.is_some() {
                        "❌ 配对码不匹配，请重新输入手机微信上显示的数字:"
                    } else {
                        "📱 安全验证：请输入手机微信上显示的配对码:"
                    };
                    println!("{}", prompt);
                    verify_code = Some(read_line()?.trim().to_string());
                    // Poll again immediately with the code.
                    continue;
                }
                LoginStatus::Expired => {
                    qr_refreshes += 1;
                    if qr_refreshes > MAX_QR_REFRESH {
                        return Err("二维码多次失效，登录流程已停止。请稍后再试".to_string());
                    }
                    println!("⏳ 二维码已过期，正在重新获取…");
                    continue 'qr;
                }
                LoginStatus::VerifyCodeBlocked => {
                    qr_refreshes += 1;
                    if qr_refreshes > MAX_QR_REFRESH {
                        return Err("配对码多次输入错误，登录流程已停止。请稍后再试".to_string());
                    }
                    println!("⚠️ 配对码错误次数过多，正在刷新二维码…");
                    continue 'qr;
                }
                LoginStatus::BindedRedirect => {
                    if let Some(token) = existing.as_ref().and_then(|s| s.bot_token.clone()) {
                        println!(
                            "ℹ️ 机器人已绑定到当前实例，现有 token 仍然有效（无需重新登录）。\n\
                             如需改绑其他账号，请先在手机微信上解绑后再试。"
                        );
                        let _ = token;
                        return Ok(());
                    }
                    return Err(
                        "机器人已绑定到其他实例，且本地没有可用 token。请先在原实例解绑，或获取原实例的 token 后重试"
                            .to_string(),
                    );
                }
                LoginStatus::Confirmed {
                    bot_token,
                    ilink_bot_id,
                    base_url: confirmed_base,
                    ilink_user_id,
                } => {
                    if bot_token.is_empty() {
                        return Err("登录确认未返回有效 token".to_string());
                    }
                    let mut state = existing.clone().unwrap_or_default();
                    state.bot_token = Some(bot_token);
                    state.ilink_bot_id = Some(ilink_bot_id);
                    if !confirmed_base.is_empty() {
                        state.base_url = Some(confirmed_base);
                    }
                    state.save(working_dir)?;

                    println!("\n🎉 微信登录成功！");
                    println!("   扫码账号: {}", ilink_user_id);
                    println!(
                        "   登录状态已保存: {}",
                        WechatState::path(working_dir).display()
                    );
                    println!("   现在可以启动 bot: robit-wechat -w <工作目录>");
                    return Ok(());
                }
                // `LoginStatus` is non-exhaustive; future statuses keep the
                // login alive instead of aborting.
                _ => {}
            }
        }
    }
}

/// SDK config for the login flow: empty token (login endpoints carry no
/// Authorization header) with an optional base_url override.
fn sdk_config_for_login(base_url: &str) -> Result<WeixinConfig, String> {
    WeixinConfig::builder()
        .token("")
        .base_url(base_url)
        .build()
        .map_err(|e| format!("SDK config error: {}", e))
}

/// Render the login QR code in the terminal (plus the URL fallback).
///
/// The scannable content is `qrcode_img_content` (a `https://…` URL);
/// `session.qrcode` is the polling token, NOT what the phone should scan.
fn render_qr(session: &QrLoginSession) {
    use qrcode::render::unicode::Dense1x2;

    if !session.qrcode_img_content.is_empty() {
        match qrcode::QrCode::new(session.qrcode_img_content.as_bytes()) {
            Ok(code) => {
                let rendered = code
                    .render::<Dense1x2>()
                    .dark_color(Dense1x2::Dark)
                    .light_color(Dense1x2::Light)
                    .build();
                println!();
                println!("{}", rendered);
                println!("请使用微信扫码登录");
            }
            Err(e) => eprintln!("终端二维码渲染失败（{}），请使用下方链接", e),
        }
    }
    if !session.qrcode_img_content.is_empty() {
        println!("二维码图片（可复制到浏览器生成后扫码）: {}", session.qrcode_img_content);
    }
    let _ = std::io::stdout().flush();
}

/// Read one line from stdin (blocking, used only for the pair code).
fn read_line() -> Result<String, String> {
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("读取输入失败: {}", e))?;
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_sdk_config_allows_empty_token() {
        let cfg = sdk_config_for_login("https://ilinkai.weixin.qq.com/").unwrap();
        assert_eq!(cfg.token, "");
    }
}
