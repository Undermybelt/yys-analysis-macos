param(
    [string]$ProjectRoot
)

# 在参数块中读取脚本目录在不同 PowerShell 启动方式下可能为空，因此在正文中补齐默认项目根目录。
# 使用布尔判断兼容 Windows PowerShell 5.1，避免不同版本对静态方法表达式的解析差异。
if (-not $ProjectRoot) {
    $ProjectRoot = Split-Path -Parent $PSScriptRoot
}
# 使用 PyInstaller 生成独立探针；探针只包含 Python 标准库和 Windows 只读 API 调用。
$scriptPath = Join-Path $ProjectRoot "tools\desktop-memory-probe.py"
$distPath = Join-Path $ProjectRoot "dist\desktop-memory-probe"
$buildPath = Join-Path $ProjectRoot ".scratch\desktop-memory-probe-build"
$specPath = Join-Path $buildPath "spec"

if (-not (Test-Path -LiteralPath $scriptPath -PathType Leaf)) {
    throw "找不到桌面内存探针：$scriptPath"
}

New-Item -ItemType Directory -Force -Path $distPath, $buildPath, $specPath | Out-Null
py -3 -m PyInstaller `
    --noconfirm `
    --clean `
    --onefile `
    --console `
    --noupx `
    --name "yys-desktop-memory-probe" `
    --distpath $distPath `
    --workpath $buildPath `
    --specpath $specPath `
    $scriptPath

Write-Output "已生成：$(Join-Path $distPath 'yys-desktop-memory-probe.exe')"
