# 工作目录独占文件锁实施计划

## 概述

基于 [2026-07-10-directory-lock-design.md](../specs/2026-07-10-directory-lock-design.md) 的设计，实现工作目录独占文件锁机制。

## 任务清单

### 1. 更新 `robit-agent` 依赖

**文件：** `crates/robit-agent/Cargo.toml`

- 添加 `fs2 = "0.4"`
- 确认 `serde` 和 `serde_json` 已存在（应该已存在）

### 2. 创建 `lock.rs` 模块

**文件：** `crates/robit-agent/src/lock.rs`

实现内容：
- `LockInfo` 结构体（program, pid, username, started_at）
- `LockError` 错误枚举
- `DirectoryLock` 结构体
- `DirectoryLock::acquire()` 方法
- `DirectoryLock::info()` 方法
- `Drop` 实现
- 跨平台 PID 检测函数（`is_process_running(pid: u32) -> bool`）
- 获取当前用户名的函数（跨平台）

### 3. 导出模块

**文件：** `crates/robit-agent/src/lib.rs`

- 添加 `pub mod lock;`
- 添加 `pub use lock::{DirectoryLock, LockError, LockInfo};`

### 4. 集成到 `robit-tui`

**文件：** `crates/robit-tui/src/main.rs`

- 在 `main()` 函数开头，解析工作目录后，获取锁
- 使用 `"robit-tui"` 作为程序名
- 处理 `LockError`，友好显示错误信息并退出

### 5. 集成到 `robit-gui`

**文件：** `crates/robit-gui/src/main.rs`

- 在 `main()` 函数开头，解析工作目录后，获取锁
- 使用 `"robit-gui"` 作为程序名
- 处理 `LockError`，友好显示错误信息并退出

### 6. 集成到 `robit-qq`

**文件：** `crates/robit-qq/src/main.rs`

- 在 `main()` 函数开头，解析工作目录后，获取锁
- 使用 `"robit-qq"` 作为程序名
- 处理 `LockError`，友好显示错误信息并退出

### 7. 测试

- 测试同一工作目录下不能同时启动两个 TUI
- 测试同一工作目录下不能同时启动 TUI 和 QQ Bot
- 测试异常终止后锁文件能被正确识别和清理
- 测试不同工作目录可以同时运行
