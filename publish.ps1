# Robit 发布脚本 - PowerShell 版本
# 按依赖顺序发布所有 crates，自动跳过已发布的版本

Write-Host "🚀 开始发布 Robit crates..." -ForegroundColor Green

# 从 workspace Cargo.toml 读取统一版本号
$workspaceToml = Get-Content "Cargo.toml" -Raw
if ($workspaceToml -match 'version\s*=\s*"([^"]+)"') {
    $workspaceVersion = $matches[1]
}
else {
    Write-Host "❌ 无法从 workspace Cargo.toml 读取版本号" -ForegroundColor Red
    exit 1
}

# 需要发布的 crates（按依赖顺序），格式："路径:包名"
$crates = @(
    "crates/robit-ai:robit-ai",
    "crates/robit-agent:robit-agent",
    "crates/robit-chatbot:robit-chatbot",
    "crates/robit-tui:robit",
    "crates/robit-qq:robit-qq"
)

foreach ($crateEntry in $crates) {
    $parts = $crateEntry -split ":"
    $crate = $parts[0]
    $crateName = $parts[1]
    $crateVersion = $workspaceVersion

    Write-Host "`n📦 检查 $crate ..." -ForegroundColor Yellow

    # 检查是否有 publish = false
    $tomlContent = Get-Content "$crate/Cargo.toml" -Raw
    if ($tomlContent -match "publish\s*=\s*false") {
        Write-Host "⏭️  跳过 $crate (publish = false)" -ForegroundColor Gray
        continue
    }

    Write-Host "   检查 $crateName v$crateVersion 是否已发布..." -ForegroundColor Cyan

    # 检查版本是否已发布
    try {
        $response = Invoke-RestMethod -Uri "https://crates.io/api/v1/crates/$crateName" -ErrorAction Stop
        $versions = $response.versions | Select-Object -ExpandProperty num
        if ($versions -contains $crateVersion) {
            Write-Host "✅ $crateName v$crateVersion 已发布，跳过" -ForegroundColor Green
            continue
        }
    }
    catch {
        # crate 不存在，可以发布
        Write-Host "   $crateName 尚未在 crates.io 上发布" -ForegroundColor Cyan
    }

    Write-Host "   开始发布 $crateName v$crateVersion ..." -ForegroundColor Yellow

    # 发布
    Push-Location $crate
    cargo publish
    if ($LASTEXITCODE -ne 0) {
        Write-Host "❌ 发布 $crate 失败！" -ForegroundColor Red
        Pop-Location
        exit $LASTEXITCODE
    }
    Pop-Location

    Write-Host "✅ $crateName v$crateVersion 发布成功" -ForegroundColor Green

    # 等待一下让 crates.io 索引更新
    Start-Sleep -Seconds 15
}

Write-Host "`n🎉 所有 crates 发布完成！" -ForegroundColor Green
