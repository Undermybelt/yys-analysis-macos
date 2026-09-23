param(
    [string]$ProjectRoot = (Split-Path -Parent $PSScriptRoot)
)

# 使用当前机器的 PyInstaller 构建完全独立的缓存读取 EXE；程序不携带旧版 `_cstructs.pyd`。
$scriptPath = Join-Path $ProjectRoot "tools\desktop-cache-export.py"
$distPath = Join-Path $ProjectRoot "dist\desktop-cache-export"
$buildPath = Join-Path $ProjectRoot ".scratch\desktop-cache-export-build"
$specPath = Join-Path $buildPath "spec"

if (-not (Test-Path -LiteralPath $scriptPath -PathType Leaf)) {
    throw "找不到缓存读取器脚本：$scriptPath"
}

New-Item -ItemType Directory -Force -Path $distPath, $buildPath, $specPath | Out-Null
python -m PyInstaller `
    --noconfirm `
    --clean `
    --onefile `
    --console `
    --noupx `
    --name "yys-desktop-cache-export" `
    --distpath $distPath `
    --workpath $buildPath `
    --specpath $specPath `
    $scriptPath

Write-Output "已生成：$(Join-Path $distPath 'yys-desktop-cache-export.exe')"
