#!/bin/bash
# Robit 发布脚本 - Bash 版本
# 按依赖顺序发布所有 crates，自动跳过已发布的版本

set -e

echo "🚀 开始发布 Robit crates..."

# 从 workspace Cargo.toml 读取统一版本号
workspaceVersion=$(grep '^version\s*=' Cargo.toml | sed -E 's/version\s*=\s*"([^"]+)"/\1/')
if [ -z "$workspaceVersion" ]; then
    echo "❌ 无法从 workspace Cargo.toml 读取版本号"
    exit 1
fi

# 需要发布的 crates（按依赖顺序），格式："路径:包名"
crates=(
    "crates/robit-ai:robit-ai"
    "crates/robit-agent:robit-agent"
    "crates/robit-chatbot:robit-chatbot"
    "crates/robit-tui:robit"
    "crates/robit-qq:robit-qq"
)

for crateEntry in "${crates[@]}"; do
    IFS=":" read -r crate crateName <<< "$crateEntry"
    crateVersion="$workspaceVersion"

    echo -e "\n📦 检查 $crate ..."

    # 检查是否有 publish = false
    if grep -q "publish\s*=\s*false" "$crate/Cargo.toml"; then
        echo "⏭️  跳过 $crate (publish = false)"
        continue
    fi

    echo "   检查 $crateName v$crateVersion 是否已发布..."

    # 检查版本是否已发布
    if command -v curl &> /dev/null; then
        response=$(curl -s "https://crates.io/api/v1/crates/$crateName" 2>/dev/null)
        if echo "$response" | grep -q "\"num\":\"$crateVersion\""; then
            echo "✅ $crateName v$crateVersion 已发布，跳过"
            continue
        fi
    fi

    echo "   开始发布 $crateName v$crateVersion ..."

    # 发布
    (cd "$crate" && cargo publish)

    echo "✅ $crateName v$crateVersion 发布成功"

    # 等待一下让 crates.io 索引更新
    sleep 15
done

echo -e "\n🎉 所有 crates 发布完成！"
