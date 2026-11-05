# LLM 提供商配置

robit 采用统一的提供商配置结构，兼容 OpenAI 协议，支持适配 DeepSeek、QWen 等多种模型提供商。配置文件位于 `config.toml`（项目本地 `.robit/config.toml` 或全局 `~/.robit/config.toml`）。

## 配置结构

```toml
# 默认模型（provider/model 格式）
default_model = "deepseek/deepseek-chat"

# DeepSeek 提供商
[providers.deepseek]
name = "DeepSeek"
base_url = "https://api.deepseek.com/v1"
api_key = "${DEEPSEEK_API_KEY}"

[[providers.deepseek.models]]
id = "deepseek-chat"
name = "DeepSeek Chat"
context_window = 65536
max_output_tokens = 8192
supports_images = false
supports_tools = true

[[providers.deepseek.models]]
id = "deepseek-coder"
name = "DeepSeek Coder"
context_window = 65536
max_output_tokens = 8192
supports_images = false
supports_tools = true

# 通义千问提供商
[providers.qwen]
name = "通义千问"
base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1"
api_key = "${DASHSCOPE_API_KEY}"

[[providers.qwen.models]]
id = "qwen-max"
name = "Qwen Max"
context_window = 32768
max_output_tokens = 8192
supports_images = true
supports_tools = true

# OpenAI 提供商
[providers.openai]
name = "OpenAI"
base_url = "https://api.openai.com/v1"
api_key = "${OPENAI_API_KEY}"

[[providers.openai.models]]
id = "gpt-4o"
name = "GPT-4o"
context_window = 128000
max_output_tokens = 16384
supports_images = true
supports_tools = true
```

## 字段说明

### 顶层字段

| 字段 | 类型 | 必填 | 说明 |
| ---- | ------ | ---- | ---- |
| `default_model` | `string` | 否 | 默认模型，格式为 `provider/model`（如 `deepseek/deepseek-chat`） |
| `default_vision_model` | `string` | 否 | 默认识图模型，格式为 `provider/model`（如 `qwen/qwen-vl-max`），用于 `read_image` 工具。模型需定义在普通 `providers` 段中且 `supports_images = true`；仅当主模型不支持图片输入时生效 |
| `providers` | `table` | 是 | 提供商配置集合 |

### Provider 字段

| 字段 | 类型 | 必填 | 说明 |
| ---- | ------ | ---- | ---- |
| `name` | `string` | 否 | 提供商显示名称 |
| `base_url` | `string` | 是 | API 基础地址，必须兼容 OpenAI 协议 |
| `api_key` | `string` | 是 | API 密钥，支持 `${ENV_VAR}` 环境变量引用 |
| `models` | `array` | 是 | 该提供商下的模型列表（**不能为空**） |

### Model 字段

| 字段 | 类型 | 必填 | 说明 |
| ---- | ------ | ---- | ---- |
| `id` | `string` | 是 | 模型 ID，用于 API 调用 |
| `name` | `string` | 否 | 模型显示名称 |
| `context_window` | `integer` | 否 | 上下文窗口大小（token 数），用于上下文管理 |
| `max_output_tokens` | `integer` | 否 | 最大输出 token 数 |
| `temperature` | `float` | 否 | 采样温度（0.0-2.0），运行时参数 |
| `max_tokens` | `integer` | 否 | 最大生成 token 数，运行时参数 |
| `supports_images` | `bool` | 否 | 是否支持图片输入，默认 `false` |
| `supports_tools` | `bool` | 否 | 是否支持工具调用，默认 `false` |
| `thinking` | `string` | 否 | 思考过程输出控制：`auto`（默认）/ `on` / `off`，见下节 |

## 思考过程输出（thinking）

思考型模型（QWen3 系、DeepSeek-R1、Kimi 等 OpenAI 兼容面）流式输出时会在 `delta.reasoning_content` 下发思考增量。robit 的捕获与配置分两层：

- **响应侧捕获**：与配置无关——只要 provider 下发了 `reasoning_content`，`thinking = "on"/"off"` 的模型走 robit 自建 SSE 旁路捕获并以 `AgentEvent::ThinkingDelta` / wire `thinking_delta` 帧送达前端（GUI 渲染为可折叠的「思考过程」面板；TUI/Bot 暂不渲染）。思考文本不写回会话历史、不持久化。
- **请求侧开关**（`enable_thinking` body 字段，DashScope 兼容面拼写）：

| 取值 | 行为 |
| ---- | ---- |
| `auto`（默认） | 不发送 `enable_thinking`，请求体与旧版本字节级一致；走 async-openai 传统流式路径。**对未知字段报错的网关（如 DeepSeek）必须保持 `auto`** |
| `on` | 注入 `"enable_thinking": true`，显式开启思考增量下发 |
| `off` | 注入 `"enable_thinking": false`，显式关闭（对默认开启思考的模型，如 QWen3 系） |

```toml
[[providers.qwen.models]]
id = "qwen3-max"
name = "Qwen3 Max"
context_window = 262144
supports_tools = true
thinking = "on"        # 显式开启思考过程输出
```

> 注：部分 profile 在未显式设置时也会默认下发思考增量（以实际网关为准）。`auto` 下这些增量会被传统路径丢弃（与旧版本行为一致）；需要捕获请配置 `on`。

## 模型引用格式

在 robit 中引用模型时，使用 `provider/model` 格式：

- `deepseek/deepseek-chat`
- `qwen/qwen-max`
- `openai/gpt-4o`

## 凭证管理

### 环境变量方式（推荐）

在配置文件中引用环境变量：

```toml
api_key = "${DEEPSEEK_API_KEY}"
```

在 `~/.robit/.env` 或系统环境变量中设置：

```txt
DEEPSEEK_API_KEY=sk-xxxxxxxxxxxxxxxx
DASHSCOPE_API_KEY=sk-xxxxxxxxxxxxxxxx
OPENAI_API_KEY=sk-xxxxxxxxxxxxxxxx
```

### 直接配置方式（不推荐）

也可以直接在配置文件中写入明文密钥，但存在安全风险：

```toml
api_key = "sk-xxxxxxxxxxxxxxxx"
```

## 配置加载顺序

1. 加载 `~/.robit/.env`（环境变量）
2. 读取 `cwd/.robit/config.toml`（项目本地，最高优先级）→ `~/.robit/config.toml`（全局 fallback）
3. 解析 `api_key` 中的 `${ENV_VAR}` 引用，从 `.env` 或系统环境变量中取值
4. 解析 `default_model` 为 `provider/model` 格式
5. 查找对应 provider，获取 `base_url` 和 `api_key`
6. 在该 provider 的 `models` 数组中匹配 `id`，获取模型参数
7. 如果未指定 `default_model`，使用第一个可用 provider 的第一个模型

## 扩展提供商

添加新的提供商只需在 `providers` 中新增一个 table，确保该提供商的 API 兼容 OpenAI 协议：

```toml
[providers.moonshot]
name = "Moonshot AI"
base_url = "https://api.moonshot.cn/v1"
api_key = "${MOONSHOT_API_KEY}"

[[providers.moonshot.models]]
id = "moonshot-v1-8k"
name = "Moonshot V1 8K"
context_window = 8192
max_output_tokens = 4096
```

## 参考

robit 的提供商配置设计参考了 [OpenClaw 的 models.providers 配置模式](https://docs.openclaw.ai/zh-CN/concepts/model-providers)，采用统一的结构适配多种 LLM 提供商。
