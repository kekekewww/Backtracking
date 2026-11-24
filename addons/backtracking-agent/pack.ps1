# Builds Backtracking Agent and packs it as target\backtracking-agent.btaddon.
# Run from anywhere: powershell -ExecutionPolicy Bypass -File addons\backtracking-agent\pack.ps1
# Needs cargo and python (for the licence notices) on PATH.
param(
    [string]$Output = ''
)
$ErrorActionPreference = 'Stop'
$crate = $PSScriptRoot
$repo = (Resolve-Path (Join-Path $crate '..\..')).Path
if (-not $Output) { $Output = Join-Path $crate 'target\backtracking-agent.btaddon' }

Push-Location $crate
try {
    & cargo build --locked --release
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

    $stage = Join-Path $crate 'target\package'
    if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
    New-Item -ItemType Directory -Path (Join-Path $stage 'bin') -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $crate 'package\addon.json') -Destination $stage
    Copy-Item -LiteralPath (Join-Path $crate 'target\release\backtracking-agent.exe') -Destination (Join-Path $stage 'bin')
    Copy-Item -LiteralPath (Join-Path $repo 'LICENSE') -Destination (Join-Path $stage 'LICENSE.txt')
    & python -I (Join-Path $crate 'licenses.py') (Join-Path $stage 'THIRD_PARTY_LICENSES.md')
    if ($LASTEXITCODE -ne 0) { throw 'licenses.py failed' }

    if (Test-Path -LiteralPath $Output) { Remove-Item -LiteralPath $Output -Force }
    & cargo run --locked --release --quiet --manifest-path (Join-Path $repo 'crates\backtracking-addon\Cargo.toml') --features install --bin btaddon-pack -- $stage $Output
    if ($LASTEXITCODE -ne 0) { throw 'btaddon-pack failed' }
} finally {
    Pop-Location
}
