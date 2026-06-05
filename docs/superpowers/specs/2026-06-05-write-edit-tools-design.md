# 设计文档：`write` 和 `edit` 工具

> 日期：2026-06-05
> 状态：待实现
> 作者：Claude (brainstorming)

## 概述

为 `robit-agent` 工具系统添加 `write` 和 `edit` 两个核心文件操作工具，使 Agent 能够直接创建/修改文件，补齐编程能力的致命短板。

## 工具清单

| 工具 | 功能 | 默认启用 | 需确认 |
|------|------|----------|--------|
| `write` | 创建/覆盖文件 | 是 | 是 |
| `edit` | 精确字符串替换 | 是 | 是 |

> 注：根据 CLAUDE.md 中工具系统表格，`write` 和 `edit` 标记为默认启用且需用户确认。

## `write` 工具

### 参数

```json
{
  "type": "object",
  "properties": {
    "file_path": {
      "type": "string",
      "description": "目标文件路径（相对或绝对路径）"
    },
    "content": {
      "type": "string",
      "description": "写入的文件内容"
    }
  },
  "required": ["file_path", "content"]
}
```

### 行为

1. 解析 `file_path`（相对路径相对于 `ctx.working_dir`）
2. 自动创建所有不存在的父目录（`create_dir_all`）
3. 如果文件已存在则覆盖，否则创建新文件
4. 使用 `tokio::fs::write` 异步写入

### 返回值

- 成功：`ToolResult::success("已创建文件: xxx (N bytes)")` 或 `ToolResult::success("已覆盖文件: xxx (N bytes)")`
- 失败：`ToolResult::error("路径非法: xxx")` 等

### 确认

`requires_confirmation()` 返回 `true` — 写入文件需用户确认（与 `bash` 一致）。

### 错误处理

- 文件路径为空 → 报错
- 路径指向已存在的目录 → 报错（"路径是目录"）
- 写入失败（权限不足等） → 报错并返回具体原因

## `edit` 工具

### 参数

```json
{
  "type": "object",
  "properties": {
    "file_path": {
      "type": "string",
      "description": "目标文件路径（相对或绝对路径）"
    },
    "old_string": {
      "type": "string",
      "description": "要替换的原始文本（必须在文件中唯一存在）"
    },
    "new_string": {
      "type": "string",
      "description": "替换后的新文本"
    }
  },
  "required": ["file_path", "old_string", "new_string"]
}
```

### 行为

1. 解析 `file_path`，检查文件存在性
2. 读取文件全文内容
3. 使用字符串精确匹配查找 `old_string`：
   - **唯一匹配** → 替换为 `new_string`，写回文件
   - **多处匹配** → 报错，列出所有匹配位置（行号 + 前后文）
   - **零处匹配** → 报错，返回最相似的 top-3 匹配片段
4. 成功后写回文件

### 多处匹配的错误输出格式

```
old_string 在文件中出现 3 次（第 12、45、89 行），无法唯一确定替换位置。
请提供更多上下文使 old_string 唯一。

匹配位置：
---
第 12 行:
  fn example() {
>   let x = 42;
  }
---
第 45 行:
  fn test() {
>   let x = 42;
  }
---
第 89 行:
  fn demo() {
>   let x = 42;
  }
---
```

### 零匹配的错误输出格式

```
在文件中未找到完全匹配的 old_string。
以下是最相似的 3 个匹配片段，请检查是否选择错误：

相似度 1 (第 42 行):
  期望: let x = 42;
  实际: let x = 43;
```

> 实现细节：零匹配时，将文件按行分割，对每行计算与 `old_string` 的包含度或编辑距离，返回最相似的前 3 个。

### 返回值

- 成功：`ToolResult::success("已修改文件: xxx (第 N 行)")`
- 失败：见上方错误格式

### 确认

`requires_confirmation()` 返回 `true` — 修改文件需用户确认。

## 文件结构

```
crates/robit-agent/src/tool/
├── mod.rs        # 新增: pub mod write; pub mod edit;
├── bash.rs       # 已有
├── read.rs       # 已有
├── write.rs      # 新建
└── edit.rs       # 新建
```

### `mod.rs` 变更

添加模块声明：
```rust
pub mod bash;
pub mod read;
pub mod write;  // 新增
pub mod edit;   // 新增
```

### Agent 初始化变更

在 `robit-agent/src/agent.rs` 中注册工具的地方，添加：

```rust
registry.register(WriteTool::new());
registry.register(EditTool::new());
```

具体配置结构根据 app 配置中的 `enabled_tools` 列表决定。

## 与现有代码的集成

| 集成点 | 说明 |
|--------|------|
| `resolve_path` | `write.rs` 和 `edit.rs` 复用 `read.rs` 中的路径解析逻辑，考虑提取到 `mod.rs` 为共享函数 |
| `truncate_output` | `edit.rs` 错误输出可能复用 `bash.rs` 中的截断逻辑 |
| `ToolContext.working_dir` | 两个工具都使用此字段解析相对路径 |
| `CLAUDE.md` 工具表 | 更新工具系统表格，`write` 和 `edit` 状态从"计划"改为"已完成" |

## 实现顺序

1. 提取 `resolve_path` 到 `mod.rs`（避免重复代码）
2. 实现 `write.rs`
3. 实现 `edit.rs`
4. 更新 `mod.rs` 注册
5. 更新 `CLAUDE.md` 工具表
6. 编译验证 + 手动测试
