# Changelog

本项目所有重要变更记录于此文件。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

> 历史版本（v0.1.16 及更早）未回溯记录，自 v0.1.17 起维护。发版流程见 `CLAUDE.md` 的「发版流程」一节。

## [Unreleased]

## [0.1.18] - 2026-08-11

### Added

- **robit-ai**：日志改用本地时间。时间戳和每日日志文件名（`robit-YYYY-MM-DD.log`）按系统本地时区输出（如 `2026-08-11T10:19:01.790373+08:00`、本地午夜切文件），取不到本地偏移时回退 UTC。
- **robit-ai**：新增 `app.log_retention_days` 配置（默认 14 天）。启动时扫描日志目录，删除超过保留期的 `robit-*.log`；`0` 禁用清理。此前日志按天生成但永不删除、无限累积。
- **robit-ai**：安装 panic hook，将 panic 信息经 `tracing::error!` 写入日志文件再链到默认 stderr hook。此前 detached 运行的二进制（nohup/systemd 下的 robit-qq 等）panic 只进 stderr，日志文件无痕迹，故障表现为无声停滞。

### Fixed

- **robit-qq**：修复重连 supervisor 的 `JoinHandle polled after completion` panic。`tokio::select!` 消费了先结束任务的 JoinHandle 后又 re-await 同一个句柄触发 panic；panic 只进 stderr，supervisor 静默死亡，导致每次 ~30 分钟网关轮换后不再重连、机器人掉线（"该机器人未连接服务"）。改为只 await 未被消费的另一个句柄。
- **robit-gui**：GUI 产物名长期停在 `Robit_0.1.1_*`。移除 `tauri.conf.json` 中硬编码的 `version` 字段，Tauri 改从 Cargo.toml（经 `CARGO_PKG_VERSION` 跟随 workspace）读版本，产物名与运行时版本显示均正确。

## [0.1.17] - 2026-08-10

### Fixed

- **robit-qq**：WebSocket 重连不再重启进程。QQ 网关约每 ~30 分钟要求重连，此前适配器发出 `Disconnected` 事件导致 `ChatbotManager` 退出、进程被外部 supervisor 重启，所有进行中的异步任务（如生图）被中途丢弃。现在重连逻辑收进适配器内部的后台 supervisor：dispatch / heartbeat 任一退出时取消另一个并按指数退避重建 WebSocket，不再发出 `Disconnected`，manager 与其持有的 Agent 跨重连存活。
- **robit-agent**：`image_gen::truncate_str` 按字节下标切片字符串会 panic（`end byte index 2000 is not a char boundary; it is inside '着'`），日志记录含中文的 DashScope 响应体时触发。改用 `floor_char_boundary` 在最近的字符边界切割。
- **robit-agent**：`AsyncTaskRunner::submit` 无 panic 兜底——work future panic 会跳出 `select!`、跳过 `done_tx.send`，导致 Agent 永远收不到结果、`query_task` 永久 pending。现在用 `AssertUnwindSafe(...).catch_unwind()` 捕获 panic，记录 ERROR 日志并投递 error 结果给 Agent。

> 上述后两条共同导致了「万象生图请求被拦截无响应、智能体得不到超时提醒」的现场现象：截断 panic 发生在异步生图任务里，而该任务无 panic 兜底，于是静默死掉、永不回报。

[Unreleased]: https://github.com/leexgone/robit/compare/v0.1.18...HEAD
[0.1.18]: https://github.com/leexgone/robit/compare/v0.1.17...v0.1.18
[0.1.17]: https://github.com/leexgone/robit/compare/v0.1.16...v0.1.17
