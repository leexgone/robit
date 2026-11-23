# 上游 PR 提交件 · weixin-agent client 级 typing API

- **日期**：2026-11-23
- **关联设计**：`docs/robit-rfc-bot-typing-design.md` §5（W-D 上游 API 规格）、§7（路径 B）
- **上游仓库**：[SpenserCai/weixin-agent-sdk-rs](https://github.com/SpenserCai/weixin-agent-sdk-rs)（第三方，MIT，PR 政策 all）
- **本地克隆**：`E:\GitHub\weixin-agent-sdk-rs`，分支 `typing-client-api`，提交 `b87cabe`（作者 Steven Lee <leexgone@163.com>，基于上游 `b9332db` = 0.3.0 发布提交）
- **状态**：**PR 已提交**（2026-11-23，gh CLI 以 leexgone 账号执行）：[SpenserCai/weixin-agent-sdk-rs#4](https://github.com/SpenserCai/weixin-agent-sdk-rs/pull/4)，fork `leexgone/weixin-agent-sdk-rs` 分支 `typing-client-api` @ `b87cabe` → 上游 `main`。等待上游维护者 review/合并/发版（0.4.0）

## 1. 提交步骤

```powershell
# 1) 在 GitHub UI fork SpenserCai/weixin-agent-sdk-rs 到你的账号（<user>）
# 2) 添加 fork 远端并推送分支
git -C E:\GitHub\weixin-agent-sdk-rs remote add fork https://github.com/<user>/weixin-agent-sdk-rs.git
git -C E:\GitHub\weixin-agent-sdk-rs push fork typing-client-api
# 3) 开 PR（gh CLI，或 GitHub UI 从 fork 分支向 SpenserCai:main 发起）
gh pr create --repo SpenserCai/weixin-agent-sdk-rs `
  --head "<user>:typing-client-api" --base main `
  --title "feat: expose client-level typing API on WeixinClient" `
  --body-file <本文档 §2 内容存成的文件>
```

建议先开一个 issue 或直接 PR 均可：delta 极小（6 文件 +135/−30，零新协议代码），与已合并的 [PR #1](https://github.com/SpenserCai/weixin-agent-sdk-rs/pull/1)（`with_cancel_token`，robit-wechat 正在使用）同形——暴露既有内部能力，可在 PR 描述中引用该先例。

## 2. PR 正文（可直接粘贴）

```markdown
## Summary

Expose the typing indicator at the client level: `WeixinClient::send_typing(to, context_token)` / `cancel_typing(to, context_token)`, mirroring the existing `MessageContext` methods.

**Motivation.** Today typing can only be driven inside the `on_message` callback (`MessageContext` is not `Clone`, its `sender` field is private, and `WeixinClient` exposes no typing entry point). Integrations that manage an agent turn's lifecycle — light the indicator when work starts, refresh while busy, cancel on completion / error / while waiting for user confirmation — need to call typing *outside* the callback. This is the same shape as #1 (`with_cancel_token`): exposing an existing internal capability, no new protocol code.

## Changes

- `WeixinClient::send_typing` / `cancel_typing` (public, additive).
- Shared internal `MessageSender::set_typing(to, context_token, status)`; `MessageContext::send_typing` / `cancel_typing` now delegate to it (behavior unchanged).
- Request construction moved to `messaging::send::build_typing_request` — the module that owns request assembly per its doc comment.
- Ticket handling unchanged: internal per-user `ConfigCache` (24h TTL + jitter + backoff). Refresh/cancel policy stays with the caller; each method performs exactly one protocol call.
- Tests: typing request wire shape (status 1/2, ticket omitted when `None`).
- e2e example: new `ctyping` command exercising the client-level API (handler holds an `OnceLock` client slot filled after `build()`).
- CHANGELOG: `[Unreleased]` entry. Suggested release: **0.4.0** (purely additive).

## Verification

- `cargo fmt --check` clean.
- `cargo test --lib`: 117 passed, 1 failed — `api::client::tests::send_message_surfaces_server_ret_as_api_error` **also fails on pristine 0.3.0** in this environment (verified via `git stash`, twice), unrelated to this change.
- `cargo clippy --all-targets`: no new warnings from this change (3 pre-existing `assert_is_empty` lints in `cdn/aes_ecb.rs` / `util/net_error.rs` under clippy 1.100).

## Compatibility

Purely additive; no existing signature or behavior changes. `MessageContext` typing behavior is byte-identical (same request, same ticket path).
```

## 3. 本地验证记录（2026-11-23）

| 项 | 结果 |
|---|---|
| `cargo fmt --check` | 通过 |
| `cargo test --lib` | 117 通过 / 1 失败；失败项 `send_message_surfaces_server_ret_as_api_error` 经 `git stash` 在**原始 0.3.0 上复现同样失败**（两次），属上游既有问题，与本 delta 无关 |
| 新增测试 | `build_typing_request_shape`、`build_typing_request_cancel_wire_value` 通过 |
| `cargo clippy --all-targets` | 本 delta 涉及文件零告警；仅上游既有 3 处 `assert_is_empty`（未触碰文件）+ manifest homepage 冗余告警 |

## 4. 上游合并后的 robit 侧动作（路径 B 收尾）

1. 上游发布 **0.4.0** 到 crates.io（版本号由上游维护者定，建议 0.4.0）。
2. robit `feat/wechat-typing` 分支：根 `Cargo.toml` `weixin-agent = "0.3"` → `"0.4"`，**删除 `[patch.crates-io]`**，`cargo check` 同步 `Cargo.lock`。robit 源码零改动（从第一天按终态 API 写）。
3. 合入 main，随 0.6.0 发布（typing 默认关，真机探针门禁后另议翻转）。
4. 若上游长期无响应：功能滞留 feature 分支，main 照常发版；每个发版窗口重议（设计 §7.1 路径 C 兜底）。

## 5. 开发期依赖说明

robit `feat/wechat-typing` 分支开发期间用本地路径 patch 指向克隆：

```toml
[patch.crates-io]
weixin-agent = { path = "../weixin-agent-sdk-rs" }
```

- 克隆版本保持 **0.3.0**（满足 robit 现有 `"0.3"` 依赖要求，patch 才生效）；上游发版时再由维护者 bump。
- 用户 fork 建立后，可把 patch 换成 git URL（`{ git = "https://github.com/<user>/weixin-agent-sdk-rs", branch = "typing-client-api" }`）以便 CI/他机复现。
- **该 patch 只活在 feature 分支**：crates.io 不接受 git/路径依赖，main 必须始终依赖 registry（设计 §7.3 发布门禁）。
