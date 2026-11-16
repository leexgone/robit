# robit 微信 Bot（官方 iLink Bot 渠道）实现方案

状态：已实现（2026-11-16，v0.2.1 开发分支）。crate：`crates/robit-wechat`。

## 渠道选型调研（2026-11-16）

"OpenClaw 的微信机器人"实际存在两条路线，本方案选择官方渠道：

| 路线 | 形态 | 结论 |
| --- | --- | --- |
| **微信官方 iLink AI Bot**（选定） | 腾讯官方 npm 包 `@tencent-weixin/openclaw-weixin`（维护者均为 @tencent.com），协议为 `ilinkai.weixin.qq.com` 长轮询 + sendMessage + 加密 CDN | 官方协议、无封号风险、长轮询天然适配内网部署（无需公网回调） |
| freestylefly/openclaw-wechat | 苍何服务云 iPad/Mac 协议代理，付费 API Key + webhook 回调 | 非官方、封号风险、需内网穿透，排除 |

协议层依赖 crates.io `weixin-agent` v0.3.0（SpenserCai/weixin-agent-sdk-rs，MIT，基于官方包 v2.4.6 的等价移植，纯协议层不耦合 OpenClaw）。风险对策：锁定精确版本；若协议不兼容，fallback 是把 SDK 的 api/cdn/messaging/qr_login 模块 vendored 进本仓库（代码量小且 MIT）。

## 认证模型

一次性扫码换长期 `bot_token`（非 OAuth、非 app_id/app_secret、无需开发者资质）：

```text
robit-wechat login
  └─ POST ilink/bot/get_bot_qrcode (bot_type=3) → 终端渲染二维码
  └─ 轮询 get_qrcode_status:
       wait → scaned → (need_verifycode: 手机配对码回传) → confirmed
       confirmed 返回 bot_token / ilink_bot_id / base_url
  └─ 写入 {working_dir}/.robit/wechat-state.json
```

> ⚠️ 二维码编码的内容是响应里的 `qrcode_img_content`（`https://…` 登录链接）；
> `qrcode` 字段是**轮询 token**（拼在 `get_qrcode_status?qrcode=` 后面），两者勿混淆——
> 编错字段会导致手机扫出一串原始编码（已踩坑修复，对照官方包 `login-qr.js` 校准）。

- 重扫码携带 `local_token_list`（≤10 个旧 token），服务器识别已绑定返回 `binded_redirect`（不再重复发凭证）
- token 失效（errcode -14）→ `on_token_stale` → adapter 发 `Disconnected` 令进程退出，提示重新 login
- state.json 持久化三类状态：`bot_token`（凭证）、`sync_buf`（长轮询游标，重启续收）、`context_tokens`（会话出站 token，SDK `ContextTokenStore` export/import）

## 架构

照抄 robit-qq 四文件模板，连接生命周期归平台 crate：

| 文件 | 职责 |
| --- | --- |
| `platform.rs` | `WechatPlatformAdapter`（`PlatformAdapter` 实现）+ `WechatHandlerBridge`（SDK `MessageHandler` → mpsc 事件通道）+ poll supervisor |
| `state.rs` | `wechat-state.json` 读写（原子性由覆盖写保证） |
| `login.rs` | 扫码登录交互流程（含 IDC 重定向、配对码、二维码刷新） |
| `main.rs` | clap（`login` 子命令 + 常规运行），流程与 robit-qq main 一致 |

关键设计：

- **长轮询 vs QQ 的 WebSocket**：SDK 的 poll loop 自带传输/接口错误退避，supervisor 只处理意外退出（指数退避 1s→30s 重启）与 Ctrl+C
- **chat_id 编码**：`private:{ilink_user_id}`（渠道仅 1:1 私聊，无群聊概念）
- **出站**：`client.send_text/send_media(peer, …, context_token)`，context token 优先取 client 内存 store（poll loop 自动刷新），回退 state 持久化 map
- **媒体入站**（微信 CDN AES-128-ECB 加密）：adapter 在 `on_message` 中用 `ctx.download_media` 解密落盘 `media/`，`MediaAttachment.local_path` 直接置位——为此 manager 的附件下载契约调整见下
- **媒体出站**：微信无独立 upload API（SDK `send_media` 上传+发送一步完成），`upload_file` 返回本地路径占位，`send_media_message` 消费之，`SendFileTool` 链路闭合
- **typing**：SDK 有 `send_typing`，当前未映射（后续增强项）

## 基座改动（robit-chatbot / robit-ai）

1. `manager.rs` 附件下载契约：`att.local_path.is_some()` 时跳过通用 URL 下载（微信 CDN 密文场景，adapter 必须自行解密）。QQ 行为不变（adapter 恒置 None）
2. `manager.rs` `generate_title` 空消息 fallback 由硬编码 `"QQ 会话"` 改为 `"{platform} 会话"`（platform 来自 adapter 的 `platform_name()`）
3. `adapter.rs` 新增 `PlatformCaps::wechat()`（纯文本、不支持编辑、2000 字、图片/文件 20MB）与 `MarkdownFeatures::wechat()`（空集——SDK 出站自动过滤 Markdown）
4. `robit-ai/config.rs` 新增 `[channels.wechat_bot]`（`bot_token` 可选、支持 `${ENV_VAR}`；`base_url`/`cdn_base_url` 可选覆盖），token 解析优先级：config → state 文件

## 使用流程

```bash
robit-wechat login            # 一次性扫码
robit-wechat -w ~/mybot       # 日常运行（长轮询，内网可用）
# [channels.wechat_bot] bot_token = "${WECHAT_BOT_TOKEN}"  # 可选覆盖（CI/多实例）
```

## 验证清单

- [x] `cargo check/clippy/test`（workspace，22 套件全绿）
- [x] 真机扫码登录（M0 冒烟，2026-11-16）：任何微信号可直接创建 bot、无资格门槛；配对码/IDC 重定向路径已对照官方实现校准
- [x] 微信内发消息 → 回复 / 会话创建（标题自动生成）/ 多轮上下文 / 工具调用（bash 执行成功）
- [x] 图片入站：AES 解密落盘 + vision 生效（`downgrade_history_images` 确认图片进入过上下文）
- [ ] 图片/文件出站（send_file 回传）
- [ ] 重启进程 sync_buf 续传不丢消息；token 失效提示重新 login
- [ ] 同目录 robit-qq + robit-wechat 双实例目录锁与会话 source 隔离
