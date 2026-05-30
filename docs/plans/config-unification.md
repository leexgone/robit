# 配置统一：从 llms.toml + settings.toml 切换到 config/robit.toml

> **目标**：将分散的 `~/.robit/llms.toml` + `~/.robit/settings.toml` + `~/.robit/.env` 三个文件合并为单一 `robit.toml`，简化配置管理。

## 决策

| 项目 | 决定 |
|------|------|
| 加载路径 | 项目本地优先：先找 `cwd/config/robit.toml`，再找 `~/.robit/robit.toml` |
| `${ENV_VAR}` | 支持，`api_key = "${DEEPSEEK_API_KEY}"` 从环境变量读取 |
| 旧格式 | 不保留兼容，完全切换到新格式 |

## 新配置结构

```toml
# config/robit.toml

[llm]
default_profile = "default"  # 默认使用的 profile 名称

[llm.profiles.default]
model = "deepseek-chat"
base_url = "https://api.deepseek.com"
api_key = "${DEEPSEEK_API_KEY}"
max_tokens = 4096
temperature = 0.0
context_window = 65536        # 可选，上下文窗口大小

[llm.profiles.chat]
model = "deepseek-chat"
base_url = "https://api.deepseek.com"
api_key = "${DEEPSEEK_API_KEY}"

[llm.profiles.reasoner]
model = "deepseek-reasoner"
base_url = "https://api.deepseek.com"
api_key = "${DEEPSEEK_API_KEY}"

[app]
log_level = "DEBUG"
max_steps = 10
enabled_tools = ["read", "bash"]

[app.context]
max_output_lines = 500
max_output_bytes = 51200
reserve_ratio = 0.2

[app.retry]
max_retries = 3
initial_backoff_ms = 1000
max_backoff_ms = 30000
```

## 类型定义

```rust
// === robit.toml ===
pub struct RobitConfig {
    pub llm: LlmSection,
    pub app: Option<AppConfig>,
}

pub struct LlmSection {
    pub default_profile: Option<String>,
    pub profiles: HashMap<String, LlmProfile>,
}

pub struct LlmProfile {
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub context_window: Option<u64>,       // 可选，供上下文管理使用
    pub max_output_tokens: Option<u64>,
}

pub struct AppConfig {
    pub log_level: Option<String>,
    pub max_steps: Option<usize>,
    pub enabled_tools: Option<Vec<String>>,
    pub context: Option<ContextConfig>,
    pub retry: Option<RetryConfig>,
}
// ContextConfig / RetryConfig 保持不变
```

## 改动文件

### `crates/robit-ai/src/config.rs` — 核心重写

| 旧 | 新 |
|----|-----|
| `LlmConfig` (providers/models) | `RobitConfig` → `LlmSection` → `LlmProfile` |
| `ProviderConfig` + `ModelConfig` | 移除 |
| `SettingsConfig` | `AppConfig` |
| `load_llm_config()` + `load_settings()` | 合并为 `load_config()` |
| `load_env()` (加载 `.env`) | 保留但简化，`load_config` 内部调用 |
| `resolve_model(llm, settings)` | 简化为 `resolve_profile(config, name?)` |
| `ResolvedModel` | 保留（provider_key → profile_name） |

加载流程：
```
1. 查找 config 文件：cwd/config/robit.toml → ~/.robit/robit.toml
2. 解析为 RobitConfig
3. 遍历所有 profiles，将 api_key 中的 ${ENV_VAR} 替换为环境变量值
4. 确定使用哪个 profile：参数指定 > llm.default_profile > "default"
```

### `crates/robit-ai/src/client.rs`

- `LlmClient::from_config(config, profile_name?)` 新签名
- 内部调用 `resolve_profile`
- `max_tokens` / `temperature` 从 profile 读取，传入请求

### `crates/robit-ai/src/lib.rs`

- 更新 re-exports：`RobitConfig`, `LlmSection`, `LlmProfile`, `AppConfig`
- 移除旧的 `LlmConfig`, `ProviderConfig`, `ModelConfig`, `SettingsConfig`

### `examples/robit-chat/src/main.rs`

```rust
// 旧
load_env();
let llm_config = load_llm_config()?;
let settings = load_settings()?;
let client = LlmClient::from_config(&llm_config, &settings)?;

// 新
let config = load_config()?;
let client = LlmClient::from_config(&config, None)?;
```

### `examples/robit-agent/src/main.rs`

同上简化，`context_config` 从 `config.app.as_ref().and_then(|a| a.context.as_ref())` 获取。

### `config/robit.toml`

更新为完整示例配置（明文 api_key → `${ENV_VAR}` 格式）。

### `CLAUDE.md` + `docs/roadmap.md`

更新配置文档部分。

## 不在范围内

- `docs/llm-config.md` — 旧文档，标记为过时但不删除（保留历史参考）
- `docs/specs/` — 历史规格文档不动
- 重试策略实际执行（仅配置解析，不在 LlmClient 中实现）
- `max_tokens` / `temperature` 在请求中的实际传递（当前 `CreateChatCompletionRequest` 已有字段但未使用，此次顺手加上）
