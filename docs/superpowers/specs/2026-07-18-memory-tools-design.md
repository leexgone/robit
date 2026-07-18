# 记忆工具系统设计规格

**版本**: 1.0  
**日期**: 2026-07-18  
**状态**: 设计中

## 1. 概述

为 Robit Agent 提供长期记忆能力，使 Agent 能够：
- 跨会话记住用户偏好和重要事实
- 检索相关历史记忆用于当前任务
- 选择性遗忘过时信息
- 组织和管理记忆条目

## 2. 设计目标

| 目标 | 说明 |
|------|------|
| **易用性** | 简单的工具接口，LLM 可以自然调用 |
| **持久化** | 记忆跨会话、跨重启持久保存 |
| **可检索** | 支持按关键词、时间、类型筛选检索 |
| **可扩展** | 支持不同类型的记忆（笔记、偏好、事实等） |
| **兼容性** | 复用现有 SQLite 存储架构，不引入新依赖 |

## 3. 记忆数据模型

### 3.1 Memory 实体

```rust
pub struct Memory {
    pub id: String,                    // UUID v4
    pub session_id: Option<String>,     // 关联的会话 ID（可选）
    pub chat_id: Option<String>,        // 关联的 chat ID（Bot 平台）
    pub memory_type: MemoryType,        // 记忆类型
    pub title: String,                  // 简短标题
    pub content: String,                // 记忆内容
    pub tags: Vec<String>,              // 标签列表（用于检索）
    pub is_active: bool,                // 是否激活
    pub created_at: String,             // ISO 8601 时间
    pub updated_at: String,             // ISO 8601 时间
}
```

### 3.2 MemoryType 枚举

```rust
pub enum MemoryType {
    Fact,          // 客观事实（如 "用户喜欢 Rust"）
    Preference,    // 用户偏好（如 "优先使用 deepseek-chat"）
    Note,          // 笔记（如 "项目目录结构"）
    Task,          // 任务记录（如 "上次完成了 XX"）
    Custom(String), // 自定义类型
}
```

## 4. 数据库 Schema

### 4.1 新表 `memories`

```sql
CREATE TABLE IF NOT EXISTS memories (
    id TEXT PRIMARY KEY,
    session_id TEXT,              -- 关联的会话 ID（可选）
    chat_id TEXT,                 -- Bot 平台的 chat_id（可选）
    memory_type TEXT NOT NULL,    -- 'fact' | 'preference' | 'note' | 'task' | 'custom:...'
    title TEXT NOT NULL,          -- 简短标题
    content TEXT NOT NULL,        -- 记忆内容
    tags TEXT,                    -- 逗号分隔的标签列表
    is_active INTEGER DEFAULT 1,  -- 软删除标记
    created_at TEXT NOT NULL,     -- ISO 8601
    updated_at TEXT NOT NULL,     -- ISO 8601

    -- 索引
    FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE SET NULL,
    FOREIGN KEY (chat_id) REFERENCES sessions(chat_id) ON DELETE SET NULL
);

-- 检索优化索引
CREATE INDEX IF NOT EXISTS idx_memories_type ON memories(memory_type);
CREATE INDEX IF NOT EXISTS idx_memories_tags ON memories(tags);
CREATE INDEX IF NOT EXISTS idx_memories_created ON memories(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_memories_session ON memories(session_id);
CREATE INDEX IF NOT EXISTS idx_memories_chat ON memories(chat_id);
CREATE INDEX IF NOT EXISTS idx_memories_active ON memories(is_active) WHERE is_active = 1;
```

### 4.2 Schema 版本

新增的 `memories` 表在 schema v3 中引入。

## 5. 记忆工具设计

### 5.1 `memorize` — 存储记忆

**用途**: LLM 调用此工具存储新记忆或更新现有记忆

**参数**:
```json5
{
  "title": "用户偏好的编程语言",        // 必填，简短标题
  "content": "用户更喜欢使用 Rust 开发", // 必填，详细内容
  "memory_type": "preference",         // 可选，默认 'note'
  "tags": ["用户偏好", "编程语言"],      // 可选，标签列表
  "update_if_exists": false            // 可选，若标题已存在则更新
}
```

**结果**: 返回存储的记忆 ID

**确认策略**: 不需要确认（记忆操作对用户低风险）

---

### 5.2 `recall` — 检索记忆

**用途**: LLM 调用此工具检索相关记忆

**参数**:
```json5
{
  "query": "用户偏好",                  // 可选，关键词搜索（标题/内容/标签）
  "memory_type": "preference",         // 可选，按类型筛选
  "tags": ["用户偏好"],                 // 可选，按标签筛选
  "limit": 10,                        // 可选，返回结果数上限，默认 10
  "since": null,                      // 可选，ISO 8601 时间，只返回此时间后的
  "session_id": null,                  // 可选，按会话筛选
  "chat_id": null                     // 可选，按 chat_id 筛选（Bot 平台）
}
```

**结果**: 返回匹配的记忆列表（标题 + 内容片段 + 标签 + 时间）

**确认策略**: 不需要确认

---

### 5.3 `forget` — 删除记忆

**用途**: LLM 调用此工具删除或停用记忆

**参数**:
```json5
{
  "memory_id": "uuid",                // 可选，按 ID 删除（精确）
  "title": "旧记忆",                   // 可选，按标题删除（模糊匹配）
  "permanent": false                  // 可选，true 时真删除，false 时仅标记 is_active=0（默认）
}
```

**结果**: 返回删除的记忆数量

**确认策略**: 需要确认（防止误删）

---

### 5.4 `list_memories` — 列出记忆

**用途**: LLM 调用此工具查看所有活跃记忆

**参数**:
```json5
{
  "limit": 20,                        // 可选，默认 20
  "memory_type": null,                // 可选，按类型筛选
  "sort_by": "created_at"             // 可选，'created_at' | 'updated_at' | 'title'
}
```

**结果**: 返回记忆列表（精简版，仅 ID、标题、类型、时间）

**确认策略**: 不需要确认

## 6. 系统提示词集成

### 6.1 工具描述

| 工具 | 描述 |
|------|------|
| `memorize` | 存储重要信息到长期记忆。用于记录用户偏好、关键事实、项目笔记等。 |
| `recall` | 从长期记忆中检索相关信息。需要上下文时调用此工具。 |
| `forget` | 删除或停用不再需要的记忆。 |
| `list_memories` | 列出所有可用的记忆概览。 |

### 6.2 使用指南（注入系统提示词）

```markdown
## 记忆系统使用指南

你拥有长期记忆能力。请合理使用：

1. **何时记忆**：
   - 用户显式要求记住的内容
   - 用户多次提到的偏好（如语言、工具选择）
   - 重要的项目上下文（如目录结构、技术选型）
   - 会对后续会话有帮助的信息

2. **如何组织记忆**：
   - 用简洁、概括性的标题（易于检索）
   - 用标签分类（如 "用户偏好"、"项目笔记"、"技术栈"）
   - 用合适的类型标记（fact/preference/note/task）

3. **何时检索**：
   - 对话开始时，先 `list_memories` 了解已有记忆
   - 需要上下文时，用 `recall` 检索相关主题
   - 不确定信息是否已记住时，先检索再询问用户

4. **记忆示例**：
   - ✅ `{title: "用户主语言", content: "用户使用中文交流", memory_type: "preference", tags: ["语言"]}`
   - ✅ `{title: "项目技术栈", content: "使用 Rust + Actix-web + PostgreSQL", memory_type: "fact", tags: ["项目", "技术栈"]}`
   - ❌ 不要存储逐字对话（那是会话历史的职责）
```

## 7. 存储 API 设计

### 7.1 在 `robit-agent/src/storage.rs` 新增函数

```rust
// 写入操作
pub fn insert_memory(conn: &Connection, memory: &Memory) -> Result<()>
pub fn update_memory(conn: &Connection, memory: &Memory) -> Result<()>
pub fn deactivate_memory(conn: &Connection, memory_id: &str) -> Result<()>
pub fn delete_memory_permanently(conn: &Connection, memory_id: &str) -> Result<()>

// 读取操作
pub fn get_memory(conn: &Connection, memory_id: &str) -> Result<Option<Memory>>
pub fn list_memories(
    conn: &Connection,
    filter: MemoryFilter,
    limit: Option<usize>,
) -> Result<Vec<Memory>>
pub fn recall_memories(
    conn: &Connection,
    query: &str,
    filter: MemoryFilter,
    limit: usize,
) -> Result<Vec<Memory>>

// 辅助结构
pub struct MemoryFilter {
    pub memory_type: Option<MemoryType>,
    pub tags: Option<Vec<String>>,
    pub session_id: Option<String>,
    pub chat_id: Option<String>,
    pub since: Option<String>,
    pub only_active: bool,
}
```

## 8. 实现阶段

### Phase 1: 基础存储（MVP）
- [ ] 新增 `memories` 表 + schema v3 迁移
- [ ] 实现 Memory struct 与 serde
- [ ] 实现基础 CRUD 函数
- [ ] 实现简单的关键词检索

### Phase 2: 工具实现
- [ ] 实现 `memorize` tool
- [ ] 实现 `recall` tool
- [ ] 实现 `forget` tool
- [ ] 实现 `list_memories` tool
- [ ] 集成到 `ToolRegistry` 和 `bootstrap`

### Phase 3: 增强功能（未来）
- [ ] 向量检索（可选，引入向量 DB）
- [ ] 记忆自动摘要
- [ ] 记忆关联与引用
- [ ] 记忆重要性评分

## 9. 配置选项

```toml
[app.memory]
enabled = true                      # 是否启用记忆系统
auto_recall_on_start = true        # 会话开始时自动检索相关记忆
auto_memorize_insights = true      # 自动记忆重要信息（LLM 触发）
max_memories_per_retrieval = 20    # 单次检索最多返回记忆数
```

## 10. 安全与隐私

- 记忆数据存储在本地 SQLite 数据库（同会话存储）
- 默认不向 LLM 提供商发送记忆（记忆只在本地检索）
- 敏感信息建议用户不要让 Agent 记忆（或让 Agent 明确提示）
