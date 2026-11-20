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
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use weixin_agent::{LoginStatus, QrLoginSession, StandaloneQrLogin, WeixinConfig};

use crate::state::WechatState;

/// Poll interval for the login status endpoint (the endpoint itself
/// long-polls; this is the retry cadence on early returns).
const LOGIN_POLL_INTERVAL: Duration = Duration::from_millis(1500);

/// Maximum QR refreshes (expiry / verify-code lockout) before giving up.
const MAX_QR_REFRESH: u32 = 3;

/// Error text returned when the user aborts the flow with Ctrl+C.
const CANCELLED_MSG: &str = "已取消微信登录（收到 Ctrl+C），未保存任何登录状态";

/// Spawn a background Ctrl+C listener that cancels the returned token.
///
/// `tokio::signal::ctrl_c()` installs the Ctrl+C handler eagerly on first
/// await, so it is ready before the flow reaches its first `await` — a
/// Ctrl+C pressed during startup is not lost to the default SIGINT action.
fn spawn_ctrl_c(task: &'static str) -> CancellationToken {
    let token = CancellationToken::new();
    let cancel = token.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("\n收到 Ctrl+C，正在取消{}…", task);
            cancel.cancel();
        }
    });
    token
}

/// Wait for a future to finish, or for Ctrl+C to be cancelled.
enum Interrupted<T> {
    Done(T),
    CtrlC,
}

async fn or_ctrl_c<T>(
    fut: impl std::future::Future<Output = T>,
    token: &CancellationToken,
) -> Interrupted<T> {
    tokio::select! {
        v = fut => Interrupted::Done(v),
        _ = token.cancelled() => Interrupted::CtrlC,
    }
}

/// Run the interactive QR login flow for `working_dir`.
///
/// Uses `[channels.wechat_bot]` (base_url overrides) and any existing state
/// file (re-login passes the old token so the server recognizes the binding).
///
/// Ctrl+C aborts the flow at any point (including while waiting for the pair
/// code on stdin) and is reported as an error, so the caller exits non-zero
/// without writing a partial state file.
pub async fn run_login(working_dir: &Path, config: &RobitConfig) -> Result<(), String> {
    let ctrl_c = spawn_ctrl_c("微信登录");

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
        let session = match or_ctrl_c(login.start(None, &local_tokens), &ctrl_c).await {
            Interrupted::Done(Ok(s)) => s,
            Interrupted::Done(Err(e)) => return Err(format!("获取二维码失败: {}", e)),
            Interrupted::CtrlC => return Err(CANCELLED_MSG.to_string()),
        };
        render_qr(&session);

        let mut scan_announced = false;
        let mut verify_code: Option<String> = None;
        let mut login = login;

        loop {
            // Sleep, then poll — both interruptible by Ctrl+C.
            match or_ctrl_c(tokio::time::sleep(LOGIN_POLL_INTERVAL), &ctrl_c).await {
                Interrupted::Done(()) => {}
                Interrupted::CtrlC => return Err(CANCELLED_MSG.to_string()),
            }
            let status = match or_ctrl_c(
                login.poll_status(&session, verify_code.as_deref()),
                &ctrl_c,
            )
            .await
            {
                Interrupted::Done(Ok(s)) => s,
                Interrupted::Done(Err(e)) => {
                    // Transient poll failure — keep waiting, the QR stays valid.
                    eprintln!("登录状态查询失败（将继续重试）: {}", e);
                    continue;
                }
                Interrupted::CtrlC => return Err(CANCELLED_MSG.to_string()),
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
                    // A watched channel lets the cancellation abort a pending
                    // stdin read, so Ctrl+C is not ignored while the user is
                    // expected to type the pair code.
                    let (input_tx, input_rx) = watch::channel(false);
                    let cancel = ctrl_c.clone();
                    tokio::spawn(async move {
                        cancel.cancelled().await;
                        let _ = input_tx.send(true);
                    });

                    let prompt = if verify_code.is_some() {
                        "❌ 配对码不匹配，请重新输入手机微信上显示的数字:"
                    } else {
                        "📱 安全验证：请输入手机微信上显示的配对码:"
                    };
                    println!("{}", prompt);
                    match read_line_interruptible(input_rx).await {
                        Ok(line) => {
                            // The read may have completed at the same moment as
                            // the cancellation; prefer the user's abort.
                            if ctrl_c.is_cancelled() {
                                return Err(CANCELLED_MSG.to_string());
                            }
                            verify_code = Some(line.trim().to_string());
                        }
                        Err(ReadLineError::Cancelled) => return Err(CANCELLED_MSG.to_string()),
                        Err(ReadLineError::Io(e)) => {
                            return Err(format!("读取输入失败: {}", e));
                        }
                    }
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

/// Outcome of an interruptible pair-code read.
#[derive(Debug)]
enum ReadLineError {
    /// Ctrl+C arrived while waiting for input.
    Cancelled,
    /// stdin closed or a read error occurred.
    Io(std::io::Error),
}

/// Read one line from stdin (used only for the pair code), abortable on Ctrl+C.
///
/// `cancelled` flips to `true` when the flow is interrupted. The blocking stdin
/// read runs on the blocking pool; once cancelled, the outcome is resolved
/// right away and the orphaned read task is left to finish on its own.
async fn read_line_interruptible(
    mut cancelled: watch::Receiver<bool>,
) -> Result<String, ReadLineError> {
    let mut read = tokio::spawn(async {
        let mut line = String::new();
        let mut stdin = BufReader::new(tokio::io::stdin());
        stdin.read_line(&mut line).await.map(|_| line)
    });

    tokio::select! {
        joined = &mut read => match joined {
            Ok(Ok(line)) => Ok(line),
            Ok(Err(e)) => Err(ReadLineError::Io(e)),
            Err(e) => Err(ReadLineError::Io(std::io::Error::other(e))),
        },
        _ = cancelled.wait_for(|c| *c) => Err(ReadLineError::Cancelled),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_sdk_config_allows_empty_token() {
        let cfg = sdk_config_for_login("https://ilinkai.weixin.qq.com/").unwrap();
        assert_eq!(cfg.token, "");
    }

    #[tokio::test]
    async fn ctrl_c_listener_cancels_immediately() {
        let token = spawn_ctrl_c("测试");
        assert!(!token.is_cancelled());
        // Cancel exactly the way the listener would, and verify the helper
        // races correctly against a never-ready future.
        token.cancel();
        assert!(matches!(
            or_ctrl_c(std::future::pending::<()>(), &token).await,
            Interrupted::CtrlC
        ));
    }

    #[tokio::test]
    async fn pair_code_read_is_aborted_by_cancellation() {
        let (tx, rx) = watch::channel(false);
        tx.send(true).unwrap();
        // The pre-cancelled receiver resolves without touching stdin.
        assert!(matches!(
            read_line_interruptible(rx).await,
            Err(ReadLineError::Cancelled)
        ));
    }
}
