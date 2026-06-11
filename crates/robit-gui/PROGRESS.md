# robit-gui 开发进度

**日期**: 2026-06-11  
**状态**: ✅ 核心功能完成，工具调用持久化支持已实现

## 已完成 ✅

### 1. 设计文档
- [x] `docs/superpowers/specs/2026-06-10-robit-gui-design.md` - 设计规格
- [x] `docs/superpowers/plans/2026-06-10-robit-gui-implementation.md` - 实现计划

### 2. Rust 后端 (100%)
- [x] 项目骨架搭建 (`Cargo.toml`, `tauri.conf.json`, `build.rs`)
- [x] 事件类型定义 (`events.rs`)
- [x] SQLite 数据库层 (`db.rs`)
- [x] 应用状态管理 (`state.rs`)
- [x] Frontend trait 实现 (`frontend.rs`)
- [x] Tauri commands (`commands.rs`)
- [x] 配置模块 (`config.rs`)
- [x] 主程序入口 (`main.rs`, `lib.rs`)
- [x] 工具调用持久化支持 (保存 ToolCallRequested 和 ToolCallResult 到数据库)
- [x] 修复 Windows 命令输出乱码问题 (添加 encoding_rs 依赖，支持 GBK 解码)
- [x] 修复 auto_approve 配置支持 (工具调用自动批准)

### 3. 集成测试 (100%)
- [x] `tests/integration.rs` - 7 个测试用例全部通过
  - DB 初始化测试
  - Session CRUD 操作测试
  - Message 操作测试
  - 事件序列化测试

### 4. React 前端骨架 (100%)
- [x] 项目配置 (`package.json`, `tsconfig.json`, `vite.config.ts`)
- [x] shadcn/ui 组件库 (Button, Input, DropdownMenu, AlertDialog, Tooltip, ScrollArea, Separator)
- [x] Zustand 状态管理 (`store.ts`)
- [x] 类型定义 (`types.ts`)
- [x] Tauri commands 封装 (`commands.ts`)
- [x] 核心组件:
  - StatusBar - 顶部状态栏
  - ThemeToggle - 主题切换
  - SessionSidebar - 会话侧边栏
  - SessionItem - 会话列表项
  - ChatPanel - 聊天面板
  - MessageList - 消息列表 (支持从数据库读取 tool 消息并渲染为 ToolCard)
  - UserMessage - 用户消息
  - AssistantMessage - AI 消息 (支持 Markdown)
  - ToolCard - 工具调用卡片 (支持状态显示，从 tool_info 字段读取)
  - InputArea - 输入区域 (自动在发送后重新获取焦点)
- [x] App.tsx 主应用 (支持事件监听)
- [x] main.tsx 入口文件
- [x] 自动滚动到底部 (会话切换和新消息时)
- [x] 工具调用卡片与消息混合显示 (按时间顺序)

### 5. 验证与测试 (2026-06-11)
- [x] 前端依赖安装完成 (`npm install`)
- [x] 前端生产构建成功 (`npm run build`)
- [x] 前端开发服务器可正常启动 (http://localhost:1420/)
- [x] 创建示例配置文件 (`.robit/robit.toml`)
- [x] 更新 CLAUDE.md 文档
- [x] 工具调用持久化测试
- [x] 历史会话重新打开时工具卡片完整显示
- [x] 完整对话流程测试 (用户消息 → 工具调用 → 助手响应)

## 项目结构

```
crates/robit-gui/
├── Cargo.toml              # Rust 依赖
├── tauri.conf.json         # Tauri 配置
├── build.rs                # 构建脚本
├── .gitignore              # Git 忽略
├── capabilities/           # Tauri 权限配置
├── icons/                  # 应用图标
├── src/
│   ├── main.rs             # 主程序入口
│   ├── lib.rs              # 库声明
│   ├── events.rs           # UiEvent 类型
│   ├── db.rs               # SQLite 数据库
│   ├── state.rs            # AppState 管理
│   ├── frontend.rs         # Frontend trait 实现
│   ├── commands.rs         # Tauri 命令
│   └── config.rs           # 配置模块
├── tests/
│   └── integration.rs      # 集成测试 (7个测试通过)
├── ui/
│   ├── package.json
│   ├── tsconfig.json
│   ├── vite.config.ts
│   ├── index.html
│   └── src/
│       ├── main.tsx
│       ├── App.tsx
│       ├── components/
│       ├── lib/
│       └── styles/
└── dist/                   # 前端构建输出 (生产构建已完成)
```

## 如何运行

### 开发模式

1. 启动前端开发服务器:
   ```bash
   cd crates/robit-gui/ui
   npm run dev
   ```

2. 在另一个终端启动 Tauri 应用:
   ```bash
   cd crates/robit-gui
   npx tauri dev
   ```

### 生产构建

```bash
cd crates/robit-gui/ui
npm run build
cd ..
npx tauri build
```

## 配置

确保在 `~/.robit/robit.toml` 或项目目录的 `config/robit.toml` 中有配置文件。
示例配置见 `.robit/robit.toml`。

## 下一步建议

- [ ] 配置 API Key 环境变量
- [ ] 添加更多工具调用的测试
- [ ] 优化 UI 样式，深色模式适配
- [ ] 添加错误提示
- [ ] 添加会话导出功能
- [ ] 添加 Markdown 代码高亮

## 关键修改记录 (2026-06-11)

### 1. 工具调用持久化
- 在数据库 `messages` 表添加 `tool_info` 列，存储完整的工具调用信息
- 修改 `state.rs` 中的事件桥接任务，在收到 `ToolCallRequested` 和 `ToolCallResult` 时保存/更新到数据库
- 更新前端 `MessageList`，优先从 `pendingConfirms` 读取实时状态，历史会话从数据库读取

### 2. Windows 编码问题修复
- 在 `robit-agent` 的 `Cargo.toml` 添加 `encoding_rs` 依赖
- 修改 `bash.rs`，在 Windows 上先用 GBK 解码，失败后再用 UTF-8 降级
- 添加 `decode_output` 辅助函数处理编码

### 3. auto_approve 支持
- 在 `GuiFrontend` 中添加 `auto_approve` 字段
- 在 `request_tool_confirmation` 中检查配置，自动批准时直接返回 true
- 创建 `GuiFrontend` 时从 `AppState` 传递配置

### 4. UI 体验优化
- 发送消息后自动重新聚焦输入框
- 切换会话和新消息时自动滚动到底部
- 工具卡片与普通消息按顺序混合显示
