# LLM 提供商配置

robit 采用统一的提供商配置结构，兼容 OpenAI 协议，支持适配 DeepSeek、QWen 等多种模型提供商。配置文件位于 `~/.robit/llms.json`。

## 配置结构

```json5
{
  "default_provider": "deepseek",
  "default_model": "deepseek/deepseek-chat",
  
  "providers": {
    "deepseek": {
      "name": "DeepSeek",
      "baseUrl": "https://api.deepseek.com/v1",
      "apiKey": "${DEEPSEEK_API_KEY}",
      "models": [
        {
          "id": "deepseek-chat",
          "name": "DeepSeek Chat",
          "contextWindow": 65536,
          "maxOutputTokens": 8192,
          "supportsImages": false,
          "supportsTools": true
        },
        {
          "id": "deepseek-coder",
          "name": "DeepSeek Coder",
          "contextWindow": 65536,
          "maxOutputTokens": 8192,
          "supportsImages": false,
          "supportsTools": true
        }
      ]
    },
    
    "qwen": {
      "name": "通义千问",
      "baseUrl": "https://dashscope.aliyuncs.com/compatible-mode/v1",
      "apiKey": "${DASHSCOPE_API_KEY}",
      "models": [
        {
          "id": "qwen-max",
          "name": "Qwen Max",
          "contextWindow": 32768,
          "maxOutputTokens": 8192,
          "supportsImages": true,
          "supportsTools": true
        }
      ]
    },
    
    "openai": {
      "name": "OpenAI",
      "baseUrl": "https://api.openai.com/v1",
      "apiKey": "${OPENAI_API_KEY}",
      "models": [
        {
          "id": "gpt-4o",
          "name": "GPT-4o",
          "contextWindow": 128000,
          "maxOutputTokens": 16384,
          "supportsImages": true,
          "supportsTools": true
        }
      ]
    }
  }
}
```

## 字段说明

### 顶层字段

| 字段 | 类型 | 必填 | 说明 |
| ---- | ------ | ---- | ---- |
| `default_provider` | `string` | 否 | 默认提供商 key，对应 `providers` 中的键名 |
| `default_model` | `string` | 否 | 默认模型，格式为 `provider/model`（如 `deepseek/deepseek-chat`） |
| `providers` | `object` | 是 | 提供商配置集合 |

### Provider 字段

| 字段 | 类型 | 必填 | 说明 |
| ---- | ------ | ---- | ---- |
| `name` | `string` | 否 | 提供商显示名称 |
| `baseUrl` | `string` | 是 | API 基础地址，必须兼容 OpenAI 协议 |
| `apiKey` | `string` | 是 | API 密钥，支持 `${ENV_VAR}` 环境变量引用 |
| `models` | `array` | 是 | 该提供商下的模型列表（**不能为空**） |

### Model 字段

| 字段 | 类型 | 必填 | 说明 |
| ---- | ------ | ---- | ---- |
| `id` | `string` | 是 | 模型 ID，用于 API 调用 |
| `name` | `string` | 否 | 模型显示名称 |
| `contextWindow` | `number` | 否 | 上下文窗口大小（token 数），用于上下文管理 |
| `maxOutputTokens` | `number` | 否 | 最大输出 token 数 |
| `supportsImages` | `bool` | 否 | 是否支持图片输入，默认 `false` |
| `supportsTools` | `bool` | 否 | 是否支持工具调用，默认 `false` |

## 模型引用格式

在 robit 中引用模型时，使用 `provider/model` 格式：

- `deepseek/deepseek-chat`
- `qwen/qwen-max`
- `openai/gpt-4o`

## 凭证管理

### 环境变量方式（推荐）

在配置文件中引用环境变量：

```json5
{
  "apiKey": "${DEEPSEEK_API_KEY}"
}
```

在 `~/.robit/.env` 或系统环境变量中设置：

```txt
DEEPSEEK_API_KEY=sk-xxxxxxxxxxxxxxxx
DASHSCOPE_API_KEY=sk-xxxxxxxxxxxxxxxx
OPENAI_API_KEY=sk-xxxxxxxxxxxxxxxx
```

### 直接配置方式（不推荐）

也可以直接在配置文件中写入明文密钥，但存在安全风险：

```json5
{
  "apiKey": "sk-xxxxxxxxxxxxxxxx"
}
```

## 配置加载顺序

1. 读取 `~/.robit/llms.json`
2. 解析 `${ENV_VAR}` 引用，从 `~/.robit/.env` 或系统环境变量中取值
3. 验证配置完整性（`baseUrl`、`apiKey`、`models` 不能为空）
4. 设置默认模型（如果未指定，使用第一个可用模型）

## 扩展提供商

添加新的提供商只需在 `providers` 中新增一个 key，确保该提供商的 API 兼容 OpenAI 协议：

```json5
{
  "providers": {
    "moonshot": {
      "name": "Moonshot AI",
      "baseUrl": "https://api.moonshot.cn/v1",
      "apiKey": "${MOONSHOT_API_KEY}",
      "models": [
        {
          "id": "moonshot-v1-8k",
          "name": "Moonshot V1 8K",
          "contextWindow": 8192,
          "maxOutputTokens": 4096
        }
      ]
    }
  }
}
```

## 参考

robit 的提供商配置设计参考了 [OpenClaw 的 models.providers 配置模式](https://docs.openclaw.ai/zh-CN/concepts/model-providers)，采用统一的结构适配多种 LLM 提供商。
