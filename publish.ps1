# Robit publish script (PowerShell)
# Publishes all crates in dependency order, skipping versions already on crates.io.
#
# NOTE: keep this file pure ASCII. Windows PowerShell 5.1 reads BOM-less .ps1 as
# the ANSI codepage (GBK on zh-CN), which garbles non-ASCII text and can even
# break string parsing. ASCII-only output avoids the problem entirely.

Write-Host "[start] Publishing Robit crates..." -ForegroundColor Green

# Read the unified version from the workspace Cargo.toml.
$workspaceToml = Get-Content "Cargo.toml" -Raw
if ($workspaceToml -match 'version\s*=\s*"([^"]+)"') {
    $workspaceVersion = $matches[1]
}
else {
    Write-Host "[error] Cannot read version from workspace Cargo.toml" -ForegroundColor Red
    exit 1
}

# Crates to publish, in dependency order, as "path:package-name".
$crates = @(
    "crates/robit-ai:robit-ai",
    "crates/robit-agent:robit-agent",
    "crates/robit-chatbot:robit-chatbot",
    "crates/robit-web:robit-web",
    "crates/robit-tui:robit",
    "crates/robit-qq:robit-qq",
    "crates/robit-wechat:robit-wechat"
)

foreach ($crateEntry in $crates) {
    $parts = $crateEntry -split ":"
    $crate = $parts[0]
    $crateName = $parts[1]
    $crateVersion = $workspaceVersion

    Write-Host "`n[package] Checking $crate ..." -ForegroundColor Yellow

    # Skip crates marked publish = false.
    $tomlContent = Get-Content "$crate/Cargo.toml" -Raw
    if ($tomlContent -match "publish\s*=\s*false") {
        Write-Host "[skip] $crate (publish = false)" -ForegroundColor Gray
        continue
    }

    Write-Host "   Checking whether $crateName v$crateVersion is already published..." -ForegroundColor Cyan

    # Skip versions that are already on crates.io.
    try {
        $response = Invoke-RestMethod -Uri "https://crates.io/api/v1/crates/$crateName" -ErrorAction Stop
        $versions = $response.versions | Select-Object -ExpandProperty num
        if ($versions -contains $crateVersion) {
            Write-Host "[ok] $crateName v$crateVersion already published, skipping" -ForegroundColor Green
            continue
        }
    }
    catch {
        # The crate does not exist yet, so it can be published.
        Write-Host "   $crateName is not on crates.io yet" -ForegroundColor Cyan
    }

    Write-Host "   Publishing $crateName v$crateVersion ..." -ForegroundColor Yellow

    Push-Location $crate
    cargo publish
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[error] Failed to publish $crate !" -ForegroundColor Red
        Pop-Location
        exit $LASTEXITCODE
    }
    Pop-Location

    Write-Host "[ok] $crateName v$crateVersion published" -ForegroundColor Green

    # Give the crates.io index a moment to update.
    Start-Sleep -Seconds 15
}

Write-Host "`n[done] All crates published" -ForegroundColor Green