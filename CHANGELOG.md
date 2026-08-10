# Changelog

本项目所有重要变更记录于此文件。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

> 历史版本（v0.1.16 及更早）未回溯记录，自 v0.1.17 起维护。发版流程见 `CLAUDE.md` 的「发版流程」一节。

## [Unreleased]

## [0.1.17] - 2026-08-10

### Fixed

- **robit-qq**：WebSocket 重连不再重启进程。QQ 网关约每 ~30 分钟要求重连，此前适配器发出 `Disconnected` 事件导致 `ChatbotManager` 退出、进程被外部 supervisor 重启，所有进行中的异步任务（如生图）被中途丢弃。现在重连逻辑收进适配器内部的后台 supervisor：dispatch / heartbeat 任一退出时取消另一个并按指数退避重建 WebSocket，不再发出 `Disconnected`，manager 与其持有的 Agent 跨重连存活。
- **robit-agent**：`image_gen::truncate_str` 按字节下标切片字符串会 panic（`end byte index 2000 is not a char boundary; it is inside '着'`），日志记录含中文的 DashScope 响应体时触发。改用 `floor_char_boundary` 在最近的字符边界切割。
- **robit-agent**：`AsyncTaskRunner::submit` 无 panic 兜底——work future panic 会跳出 `select!`、跳过 `done_tx.send`，导致 Agent 永远收不到结果、`query_task` 永久 pending。现在用 `AssertUnwindSafe(...).catch_unwind()` 捕获 panic，记录 ERROR 日志并投递 error 结果给 Agent。

> 上述后两条共同导致了「万象生图请求被拦截无响应、智能体得不到超时提醒」的现场现象：截断 panic 发生在异步生图任务里，而该任务无 panic 兜底，于是静默死掉、永不回报。

[Unreleased]: https://github.com/leexgone/robit/compare/v0.1.17...HEAD
[0.1.17]: https://github.com/leexgone/robit/compare/v0.1.16...v0.1.17
