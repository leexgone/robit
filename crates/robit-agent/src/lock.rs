//! 工作目录独占文件锁
//!
//! 确保同一工作目录下只能有一个 robit 程序运行。

use std::fs::{File, OpenOptions, remove_file};
use std::io::{Write, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process;

use fs2::FileExt;
use serde::{Serialize, Deserialize};
use thiserror::Error;
use time::OffsetDateTime;

/// 存储在锁文件中的基本信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockInfo {
    /// 程序名称（如 "robit-tui", "robit-qq", "robit-gui"）
    pub program: String,
    /// 进程 ID
    pub pid: u32,
    /// 启动用户名
    pub username: String,
    /// 启动时间（ISO 8601 格式）
    pub started_at: String,
}

/// 文件锁错误
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
    /// IO 错误（携带具体操作与路径：Windows 的 os error 5/32/33 在不同
    /// API 上含义不同，不带上下文根本无从判断是哪一步被拒绝）
    #[error("I/O error while {action} {path}: {source}")]
    Io {
        /// 失败的操作（如 "opening lock file"）
        action: &'static str,
        /// 涉及的路径
        path: PathBuf,
        /// 底层 IO 错误
        #[source]
        source: std::io::Error,
    },
    /// 序列化/反序列化错误
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// 文件锁守护对象，RAII 模式，析构时自动释放锁
pub struct DirectoryLock {
    /// 锁文件路径
    lock_path: PathBuf,
    /// 锁文件句柄（保持打开以维持操作系统级锁）
    file: Option<File>,
    /// 锁定时的信息
    info: LockInfo,
}

impl DirectoryLock {
    /// 获取工作目录的独占锁
    ///
    /// # Arguments
    /// * `workdir` - 工作目录路径
    /// * `program_name` - 程序名称（如 "robit-tui"）
    pub fn acquire(workdir: &Path, program_name: &str) -> Result<Self, LockError> {
        let robit_dir = workdir.join(".robit");
        let lock_path = robit_dir.join("LOCK");

        tracing::debug!("Acquiring directory lock at: {}", lock_path.display());

        // 确保 .robit 目录存在
        if !robit_dir.exists() {
            tracing::debug!("Creating .robit directory at: {}", robit_dir.display());
            std::fs::create_dir_all(&robit_dir)
                .map_err(|e| Self::io_err("creating directory", &robit_dir, e))?;
        }

        // 获取当前用户名
        let username = get_username().unwrap_or_else(|| "unknown".to_string());

        // 创建锁信息
        let info = LockInfo {
            program: program_name.to_string(),
            pid: process::id(),
            username,
            started_at: OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| "unknown".to_string()),
        };

        // 尝试获取锁，最多重试一次（处理孤立锁）
        match Self::try_acquire(&lock_path, &info) {
            Ok(lock) => Ok(lock),
            Err(LockError::AlreadyLocked { program, pid, username, started_at }) => {
                // 检查进程是否还在运行
                if !is_process_running(pid) {
                    // 进程不存在，删除旧锁文件并重试
                    tracing::warn!(
                        "Found stale lock file from {} (PID {}, user {}, started at {}), cleaning up",
                        program,
                        pid,
                        username,
                        started_at
                    );
                    let _ = remove_file(&lock_path);
                    Self::try_acquire(&lock_path, &info)
                } else {
                    Err(LockError::AlreadyLocked { program, pid, username, started_at })
                }
            }
            Err(e) => Err(e),
        }
    }

    /// 把所有 IO 错误补上“正在做什么 + 哪个路径”，否则 Windows 上只会看到
    /// 一句 “os error 5”，无法判断是准备目录、打开锁文件还是写锁文件失败。
    fn io_err(action: &'static str, path: &Path, source: std::io::Error) -> LockError {
        LockError::Io {
            action,
            path: path.to_path_buf(),
            source,
        }
    }

    /// 尝试获取锁（单次尝试）
    fn try_acquire(lock_path: &Path, info: &LockInfo) -> Result<Self, LockError> {
        // 打开或创建锁文件
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
            .map_err(|e| Self::io_err("opening lock file", lock_path, e))?;

        // 尝试获取排他锁（非阻塞）
        match file.try_lock_exclusive() {
            Ok(_) => {
                // 获取锁成功，写入信息
                file.set_len(0)
                    .map_err(|e| Self::io_err("truncating lock file", lock_path, e))?;
                file.seek(SeekFrom::Start(0))
                    .map_err(|e| Self::io_err("seeking in lock file", lock_path, e))?;
                serde_json::to_writer_pretty(&mut file, info)?;
                file.flush()
                    .map_err(|e| Self::io_err("writing lock info to", lock_path, e))?;
                file.sync_data()
                    .map_err(|e| Self::io_err("flushing lock file", lock_path, e))?;

                tracing::info!("Acquired directory lock at: {}", lock_path.display());
                tracing::debug!("Lock info: {:?}", info);

                Ok(Self {
                    lock_path: lock_path.to_path_buf(),
                    file: Some(file),
                    info: info.clone(),
                })
            }
            Err(_) => {
                // 获取锁失败，读取现有锁信息
                let mut content = String::new();
                file.seek(SeekFrom::Start(0))
                    .map_err(|e| Self::io_err("seeking in lock file", lock_path, e))?;
                file.read_to_string(&mut content)
                    .map_err(|e| Self::io_err("reading lock file", lock_path, e))?;

                let existing_info: LockInfo = match serde_json::from_str(&content) {
                    Ok(info) => info,
                    Err(_) => {
                        // 锁文件内容损坏，视为可清理
                        LockInfo {
                            program: "unknown".to_string(),
                            pid: 0,
                            username: "unknown".to_string(),
                            started_at: "unknown".to_string(),
                        }
                    }
                };

                Err(LockError::AlreadyLocked {
                    program: existing_info.program,
                    pid: existing_info.pid,
                    username: existing_info.username,
                    started_at: existing_info.started_at,
                })
            }
        }
    }

    /// 获取当前锁的信息
    pub fn info(&self) -> &LockInfo {
        &self.info
    }

    /// 手动释放锁（通常不需要调用，RAII 会自动处理）
    pub fn release(&mut self) {
        if let Some(file) = self.file.take() {
            let _ = file.unlock();
        }
    }
}

impl Drop for DirectoryLock {
    fn drop(&mut self) {
        tracing::debug!("Releasing directory lock at: {}", self.lock_path.display());
        if let Some(file) = self.file.take() {
            let _ = file.unlock();
        }
        // 删除锁文件
        let result = std::fs::remove_file(&self.lock_path);
        match result {
            Ok(_) => tracing::debug!("Deleted lock file: {}", self.lock_path.display()),
            Err(e) => tracing::debug!("Failed to delete lock file: {}", e),
        }
    }
}

/// 检测进程是否还在运行（跨平台简化版）
fn is_process_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        use std::process::Command;
        // 使用 `kill -0` 命令检测
        let output = Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .output();
        match output {
            Ok(output) => {
                // 成功返回 true，或者返回 EPERM（没有权限但进程存在）
                output.status.success() || output.status.code() == Some(1)
            }
            Err(_) => {
                // 命令失败，保守假设进程存在
                true
            }
        }
    }

    #[cfg(windows)]
    {
        use std::process::Command;
        // 使用 `tasklist` 命令检测
        let output = Command::new("tasklist")
            .arg("/FI")
            .arg(format!("PID eq {}", pid))
            .output();
        match output {
            Ok(output) => {
                let output_str = String::from_utf8_lossy(&output.stdout);
                output_str.contains(&pid.to_string())
            }
            Err(_) => {
                // 命令失败，保守假设进程存在
                true
            }
        }
    }

    #[cfg(not(any(unix, windows)))]
    {
        // 其他平台保守处理：假设进程存在
        true
    }
}

/// 获取当前用户名（跨平台）
fn get_username() -> Option<String> {
    // 优先从环境变量获取
    if let Ok(name) = std::env::var("USER") {
        if !name.is_empty() {
            return Some(name);
        }
    }
    if let Ok(name) = std::env::var("USERNAME") {
        if !name.is_empty() {
            return Some(name);
        }
    }

    // 环境变量都不可用时的备选方案
    #[cfg(unix)]
    {
        use std::process::Command;
        if let Ok(output) = Command::new("whoami").output() {
            if output.status.success() {
                let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !name.is_empty() {
                    return Some(name);
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_lock_acquire_and_release() {
        let dir = tempdir().unwrap();
        let lock = DirectoryLock::acquire(dir.path(), "test-program").unwrap();
        assert_eq!(lock.info().program, "test-program");
        assert!(lock.info().pid > 0);
    }

    #[test]
    fn test_username_is_set() {
        let dir = tempdir().unwrap();
        let lock = DirectoryLock::acquire(dir.path(), "test-program").unwrap();
        assert!(!lock.info().username.is_empty());
    }

    #[test]
    fn test_started_at_is_set() {
        let dir = tempdir().unwrap();
        let lock = DirectoryLock::acquire(dir.path(), "test-program").unwrap();
        assert!(!lock.info().started_at.is_empty());
    }

    #[test]
    fn test_is_process_running_with_our_pid() {
        // 我们自己的 PID 应该是在运行的
        assert!(is_process_running(std::process::id()));
    }

    #[test]
    fn test_is_process_running_with_invalid_pid() {
        // 一个很大的 PID 应该不在运行
        // 注意：这不是 100% 可靠，但对于测试来说足够了
        assert!(!is_process_running(u32::MAX));
    }
}
