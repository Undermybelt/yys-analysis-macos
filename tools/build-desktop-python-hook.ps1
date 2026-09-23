param(
    [string]$ProjectRoot
)

# 从脚本目录推导项目根目录，兼容 Windows PowerShell 5.1 和 PowerShell 7。
if (-not $ProjectRoot) {
    $ProjectRoot = (Split-Path -Parent $PSScriptRoot)
}

# Frida 包含本地扩展 DLL，使用 --collect-all 确保独立 EXE 能加载同一版本的运行时。
$scriptPath = Join-Path $ProjectRoot "tools\desktop-python-hook.py"
$distPath = Join-Path $ProjectRoot "dist\desktop-python-hook"
$buildPath = Join-Path $ProjectRoot ".scratch\desktop-python-hook-build"
$specPath = Join-Path $buildPath "spec"

if (-not (Test-Path -LiteralPath $scriptPath -PathType Leaf)) {
    throw "找不到桌面 Python Hook 工具：$scriptPath"
}

New-Item -ItemType Directory -Force -Path $distPath, $buildPath, $specPath | Out-Null
py -3 -m PyInstaller `
    --noconfirm `
    --clean `
    --onefile `
    --console `
    --noupx `
    --collect-all frida `
    --name "yys-desktop-python-hook" `
    --distpath $distPath `
    --workpath $buildPath `
    --specpath $specPath `
    $scriptPath

Write-Output "已生成：$(Join-Path $distPath 'yys-desktop-python-hook.exe')"
