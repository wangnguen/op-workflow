# Fails if any crate under core/ depends (directly or transitively) on Tauri or a webview stack.
# PowerShell twin of check-core-deps.sh for Windows shells without bash.
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

$forbidden = '^(tauri|tauri-[a-z-]+|wry|tao|webview2-com)$'
$failed = $false
Get-ChildItem core -Directory | ForEach-Object {
    $manifest = Join-Path $_.FullName 'Cargo.toml'
    if (-not (Test-Path $manifest)) { return }
    $crate = (Select-String -Path $manifest -Pattern '^name = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
    $deps = cargo tree -p $crate --edges normal,build --prefix none --format '{p}' 2>$null |
        ForEach-Object { ($_ -split ' ')[0] } | Sort-Object -Unique
    $hits = @($deps | Where-Object { $_ -match $forbidden })
    if ($hits.Count -gt 0) {
        Write-Output "FAIL: $crate depends on: $($hits -join ' ')"
        $failed = $true
    } else {
        Write-Output "ok:   $crate"
    }
}
if ($failed) { exit 1 }
