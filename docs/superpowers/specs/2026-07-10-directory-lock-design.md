# 工作目录独占文件锁设计

## 概述

为 robit 项目实现工作目录独占文件锁机制，确保在同一工作目录下，只能有一个 robit 前端程序（TUI/GUI/QQ Bot）同时运行。

## 目标

1. **按工作目录独占**：同一工作目录下，只能启动一个 robit 程序（TUI/GUI/QQ Bot 中的任意一个）
2. **自动清理孤立锁**：检测并自动清理异常终止留下的锁文件
3. **提供诊断信息**：锁文件包含基本信息便于调试

## 非目标

- 示例程序（`examples/`）不需要锁控制
- 不限制同一程序在不同工作目录下的多实例运行

## 设计

### 模块位置

新增模块：`crates/robit-agent/src/lock.rs`

### 核心数据结构

```rust
/// 文件锁守护对象，RAII 模式，析构时自动释放锁
pub struct DirectoryLock {
    /// 锁文件路径
    lock_path: PathBuf,
    /// 锁文件句柄（保持打开以维持操作系统级锁）
    file: Option<File>,
    /// 锁定时的信息
    info: LockInfo,
}

/// 存储在锁文件中的基本信息
#[derive(Debug, Serialize, Deserialize)]
pub struct LockInfo {
    /// 程序名称（如 "robit-tui", "robit-qq", "robit-gui"）
    pub program: String,
    /// 进程 ID
    pub pid: u32,
    /// 启动时间（ISO 8601 格式）
    pub started_at: String,
    /// 启动用户名
    pub username: String,
}
```

### 错误类型

```rust
#[derive(Debug, Error)]
pub enum LockError {
    /// 目录已被另一个进程锁定
    #[error("Directory is already locked by {program} (PID {pid}, user {username}, started at {started_at})")]
    AlreadyLocked {
        program: String,
        pid: u32,
        username: String,
        started_at: String,
    },
    /// IO 错误
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    /// 序列化/反序列化错误
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// 无法创建 .robit 目录
    #[error("Failed to create .robit directory: {0}")]
    CreateDir(std::io::Error),
}
```

### 核心方法

#### `DirectoryLock::acquire(workdir: &Path, program_name: &str) -> Result<Self, LockError>`

获取工作目录的独占锁。

**流程：**
1. 计算锁文件路径：`{workdir}/.robit/LOCK`
2. 确保 `{workdir}/.robit` 目录存在，不存在则创建
3. 尝试创建/打开锁文件（读写模式）
4. 使用 `fs2` 获取排他锁（非阻塞模式）
5. 如果获取锁失败：
   - 读取锁文件内容，解析 `LockInfo`
   - 检查 PID 是否还在运行
   - 如果 PID 不在运行，说明是孤立锁，删除锁文件后重试
   - 如果 PID 还在运行，返回 `AlreadyLocked` 错误
6. 获取锁成功后：
   - 截断文件
   - 写入当前进程的 `LockInfo` JSON
   - 刷新文件确保内容写入磁盘
7. 返回 `DirectoryLock` 对象

**检测 PID 是否运行的跨平台实现：**

- **Unix (Linux/macOS):** `kill(pid, 0)`
  - 返回 `0` → 进程存在且有权限
  - 返回 `ESRCH` → 进程不存在
  - 返回 `EPERM` → 进程存在但无权限发信号（也算存在）

- **Windows:** `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid)`
  - 成功返回句柄 → 进程存在
  - 返回 `NULL` 且 `GetLastError() == ERROR_INVALID_PARAMETER` → 进程不存在
  - 其他错误（如 `ERROR_ACCESS_DENIED`）→ 进程存在但权限不足（也算存在）

**注意：** 只要能确定进程存在（无论是否有权限与其交互），就认为锁仍然有效。

#### `DirectoryLock::info() -> &LockInfo`

获取当前锁的信息。

#### RAII 析构

`Drop` trait 实现：
- 关闭文件句柄（自动释放操作系统级锁）
- 锁文件保留在磁盘上作为诊断信息（下次启动时会自动检测并处理）

### 与现有代码集成

在各前端程序的 `main.rs` 中，在任何其他初始化操作之前获取锁：

```rust
fn main() -> Result<()> {
    // 1. 解析 CLI 参数，确定工作目录
    let cli = Cli::parse();
    let working_dir = resolve_working_dir(cli.workdir)?;
    
    // 2. 获取目录锁（在任何其他操作之前）
    let _lock = DirectoryLock::acquire(&working_dir, "robit-tui")?;
    
    // 3. 原有初始化逻辑...
    let config = load_config(Some(&working_dir))?;
    // ...
    
    // 4. 运行主循环
    // ...
    
    // 5. _lock 析构时自动释放锁
}
```

**需要集成的程序：**
- `crates/robit-tui/src/main.rs` - "robit-tui"
- `crates/robit-gui/src/main.rs` - "robit-gui"
- `crates/robit-qq/src/main.rs` - "robit-qq"

**不需要集成的程序：**
- `examples/robit-chat/src/main.rs`
- `examples/robit-agent/src/main.rs`

### 依赖变更

在 `crates/robit-agent/Cargo.toml` 中添加：

```toml
[dependencies]
fs2 = "0.4"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
```

### 模块导出

在 `crates/robit-agent/src/lib.rs` 中添加：

```rust
pub mod lock;
pub use lock::{DirectoryLock, LockError, LockInfo};
```

## 边缘情况处理

| 情况 | 处理方式 |
|------|----------|
| 进程被 `SIGKILL` 杀死 | 锁文件残留，但下次启动检测 PID 不存在会清理 |
| 两个进程同时启动 | 操作系统级锁保证只有一个能成功获取 |
| 网络文件系统（NFS） | fs2 在 NFS 上可能不可靠，但这是已知限制 |
| `Ctrl+C` 退出 | `Drop` 实现确保锁被释放 |
| 程序 panic | `Drop` 实现确保锁被释放（如果 panic 不终止进程） |
| `.robit` 目录不存在 | 自动创建该目录 |
| 锁文件内容损坏 | 视为孤立锁，直接覆盖 |

## 锁文件格式

JSON 格式，示例：

```json
{
  "program": "robit-tui",
  "pid": 12345,
  "username": "alice",
  "started_at": "2026-07-10T14:30:00Z"
}
```

## 替代方案考虑

### 方案 B：纯文件 + PID 检测（未采用）

不使用操作系统级锁，仅通过文件存在性 + PID 检测来实现。

**缺点：** 存在竞态条件（两个进程同时检查并创建锁文件）

### 方案 C：混合方案（未采用）

同时使用操作系统级锁和 PID 文件。

**缺点：** 实现过于复杂，方案 A 已足够。
