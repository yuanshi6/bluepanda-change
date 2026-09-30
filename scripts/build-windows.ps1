$ErrorActionPreference = 'Stop'
$workspace = Resolve-Path (Join-Path $PSScriptRoot '..')
Set-Location $workspace
cargo build --workspace --release
Write-Output "Built CLI: $workspace/target/release/bluepanda-change.exe"
Write-Output "Built GUI: $workspace/target/release/bluepanda-gui.exe"
