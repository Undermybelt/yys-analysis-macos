# MuMu6（Android 15）原始御魂 JSON 采集脚本；只保存桥接回传内容，不修改游戏内存或执行游戏操作。
[CmdletBinding()]
param(
    # MuMu ADB 可执行文件；未提供时尝试从运行中的 MuMu 安装目录和常见路径查找。
    [string]$AdbPath,
    # MuMu ADB 设备端点；多开时必须明确指定，避免采集到错误游戏实例。
    [string]$Endpoint,
    # 游戏客户端版本；未提供时从阴阳师包信息读取，只允许桥接已验证的版本。
    [string]$GameVersion,
    # 阴阳师进程号；未提供时读取所有包含统一包名片段的渠道包进程。
    [int]$GameProcessId,
    # 现有只读桥接程序路径；未提供时按开发和发布目录顺序查找。
    [string]$BridgePath,
    # 原始捕获文件路径；默认写入当前目录并带时间戳。
    [string]$OutputPath,
    # 允许覆盖同名捕获文件；默认拒绝覆盖，避免误删上一份原始样本。
    [switch]$Force
)

$ErrorActionPreference = "Stop"

function Resolve-AdbExecutable {
    <#
    .SYNOPSIS
    定位可执行的 ADB；优先使用用户明确传入的路径，再检查运行中的 MuMu 根目录。
    #>
    param([string]$ConfiguredPath)

    $candidates = [System.Collections.Generic.List[string]]::new()
    if (-not [string]::IsNullOrWhiteSpace($ConfiguredPath)) {
        $candidates.Add($ConfiguredPath)
    }

    # 运行中的 MuMu 路径最可靠；进程路径读取失败时忽略该进程，不影响其他候选路径。
    $runningProcesses = @(Get-Process -ErrorAction SilentlyContinue |
        Where-Object { $_.ProcessName -match 'MuMu|Nemu' })
    $runningRoots = [System.Collections.Generic.List[string]]::new()
    foreach ($runningProcess in $runningProcesses) {
        try {
            $path = $runningProcess.Path
            if ($path) {
                $root = Split-Path (Split-Path $path -Parent) -Parent
                if ($root -and -not $runningRoots.Contains($root)) {
                    $runningRoots.Add($root)
                }
            }
        } catch {
            # 受保护进程可能无法读取路径；这里只跳过该候选，不中断采集。
        }
    }

    foreach ($root in $runningRoots) {
        $candidates.Add((Join-Path $root 'vmonitor\bin\adb_server.exe'))
        $candidates.Add((Join-Path $root 'shell\adb.exe'))
        # MuMu6（Android 15）的 ADB 位于 nx_device\15.0；这是当前版本的实际布局。
        $candidates.Add((Join-Path $root 'nx_device\15.0\shell\adb.exe'))
    }

    $candidates.Add('D:\game\MuMuPlayer\nx_device\15.0\shell\adb.exe')
    $candidates.Add('adb.exe')

    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
        if ($candidate -eq 'adb.exe' -and (Get-Command adb.exe -ErrorAction SilentlyContinue)) {
            return (Get-Command adb.exe).Source
        }
    }

    throw '找不到 ADB。请使用 -AdbPath 指定 MuMu 自带的 adb.exe 或 adb_server.exe。'
}

function Resolve-Endpoint {
    <#
    .SYNOPSIS
    读取已授权设备端点；多开时要求用户明确指定，避免混采不同游戏档案。
    #>
    param(
        [string]$Adb,
        [string]$ConfiguredEndpoint
    )

    if (-not [string]::IsNullOrWhiteSpace($ConfiguredEndpoint)) {
        return $ConfiguredEndpoint.Trim()
    }

    # 使用参数数组调用 ADB，避免路径中的空格或特殊字符被当成 PowerShell 代码执行。
    $devices = & $Adb devices 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw ("无法读取 ADB 设备列表：{0}" -f ($devices -join [Environment]::NewLine))
    }
    # 重新按行解析，只有第二列明确为 device 的端点才允许进入读取会话。
    $ready = @(
        $devices |
            Where-Object { $_ -match '^\s*[^\s]+\s+device\s*$' } |
            ForEach-Object { ($_ -split '\s+')[0] }
    )
    if ($ready.Count -eq 0) {
        throw '没有发现已授权的 MuMu ADB 设备，请先启动模拟器并开启 ADB。'
    }
    if ($ready.Count -gt 1) {
        throw "检测到多个 MuMu 实例：$($ready -join '、')；请使用 -Endpoint 明确指定。"
    }
    return $ready[0]
}

function Resolve-GameVersion {
    <#
    .SYNOPSIS
    从游戏包信息提取版本；未知版本必须交给桥接拒绝，不能猜测偏移。
    #>
    param(
        [string]$Adb,
        [string]$Device,
        [string]$ConfiguredVersion
    )

    if (-not [string]::IsNullOrWhiteSpace($ConfiguredVersion)) {
        return $ConfiguredVersion.Trim()
    }

    $packages = Get-OnmyojiPackageNames -Adb $Adb -Device $Device
    foreach ($package in $packages) {
        $dump = & $Adb -s $Device shell dumpsys package $package 2>$null
        if ($LASTEXITCODE -ne 0) { continue }
        $match = $dump | Select-String -Pattern 'versionName=([^\s]+)' | Select-Object -First 1
        if ($match -and $match.Matches.Count -gt 0) {
            return $match.Matches[0].Groups[1].Value
        }
    }
    throw '无法读取阴阳师版本。请使用 -GameVersion 指定已验证版本（例如 1.8.62）。'
}

function Get-OnmyojiPackageNames {
    <#
    .SYNOPSIS
    枚举设备上已安装的阴阳师包；官方包和渠道包统一按包名片段识别。
    #>
    param(
        [string]$Adb,
        [string]$Device
    )

    $marker = 'com.netease.onmyoji'
    $packageOutput = & $Adb -s $Device shell pm list packages 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw ("无法读取阴阳师包名：{0}" -f ($packageOutput -join [Environment]::NewLine))
    }

    $names = [System.Collections.Generic.List[string]]::new()
    foreach ($line in $packageOutput) {
        if ($line -notmatch '^\s*package:(\S+)\s*$') { continue }
        $packageName = $Matches[1]
        if ($packageName.IndexOf($marker, [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -and
            -not $names.Contains($packageName)) {
            $names.Add($packageName)
        }
    }
    return @($names.ToArray())
}

function Resolve-GameProcessId {
    <#
    .SYNOPSIS
    读取当前阴阳师 PID；只允许读取进程状态，不向游戏发送操作。
    #>
    param(
        [string]$Adb,
        [string]$Device,
        [int]$ConfiguredProcessId
    )

    if ($ConfiguredProcessId -gt 0) {
        return $ConfiguredProcessId
    }

    $packages = Get-OnmyojiPackageNames -Adb $Adb -Device $Device
    foreach ($package in $packages) {
        $pidOutput = & $Adb -s $Device shell pidof $package 2>$null
        $pidText = if ($null -eq $pidOutput) { '' } else { $pidOutput.Trim() }
        # PowerShell 内置的 $PID 是只读进程号，不能复用该名称保存游戏进程号。
        $processIdValue = $pidText -split '\s+' | Where-Object { $_ -match '^\d+$' } | Select-Object -First 1
        if ($processIdValue) { return [int]$processIdValue }
    }
    throw '没有发现阴阳师游戏进程，请先启动游戏后再执行采集脚本。'
}

function Get-FridaServerPids {
    <#
    .SYNOPSIS
    读取指定 MuMu 实例上的 Frida Server PID，用于只清理本次采集启动的服务。
    #>
    param(
        [string]$Adb,
        [string]$Device
    )

    $pidOutput = & $Adb -s $Device shell pidof yys-export-s 2>$null
    if ($null -eq $pidOutput) { return @() }
    return @($pidOutput.Trim() -split '\s+' | Where-Object { $_ -match '^\d+$' })
}

function Resolve-BridgeExecutable {
    <#
    .SYNOPSIS
    定位已经构建好的只读桥接程序；脚本不从网络下载或生成注入代码。
    #>
    param([string]$ConfiguredPath)

    $repoRoot = Split-Path $PSScriptRoot -Parent
    $candidates = @()
    if (-not [string]::IsNullOrWhiteSpace($ConfiguredPath)) {
        $candidates += $ConfiguredPath
    }
    $candidates += @(
        (Join-Path $repoRoot 'target\debug\resources\mumu\mumu-frida-bridge.exe'),
        (Join-Path $repoRoot 'target\release\resources\mumu\mumu-frida-bridge.exe'),
        (Join-Path $repoRoot 'src-tauri\resources\mumu\mumu-frida-bridge.exe'),
        (Join-Path $repoRoot 'resources\mumu\mumu-frida-bridge.exe')
    )

    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
    }
    throw '找不到 mumu-frida-bridge.exe，请先构建应用或使用 -BridgePath 指定桥接程序。'
}

function Write-CaptureFile {
    <#
    .SYNOPSIS
    以 UTF-8 原子写入捕获文件；每收到一批就落盘，避免中途停止丢失已收到的原始数据。
    #>
    param(
        [string]$Path,
        [hashtable]$Document
    )

    $json = $Document | ConvertTo-Json -Depth 100
    $temporaryPath = "$Path.tmp"
    [System.IO.File]::WriteAllText(
        $temporaryPath,
        $json,
        [System.Text.UTF8Encoding]::new($false)
    )
    Move-Item -LiteralPath $temporaryPath -Destination $Path -Force
}

function Get-PayloadFingerprint {
    <#
    .SYNOPSIS
    为原始 payload 生成稳定指纹；相同计算被 Hook 重复回调时只保留一份。
    #>
    param([object]$Payload)

    # 先压缩序列化再计算 SHA-256，不改变 payload，只用于判断两批是否完全相同。
    $canonical = $Payload | ConvertTo-Json -Depth 100 -Compress
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($canonical)
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        return ([System.BitConverter]::ToString($sha.ComputeHash($bytes))).Replace('-', '').ToLowerInvariant()
    } finally {
        $sha.Dispose()
    }
}

function Stop-BridgeProcess {
    <#
    .SYNOPSIS
    结束本次捕获启动的桥接进程；只清理本次会话在指定端点启动的新 Frida Server。
    #>
    param(
        [System.Diagnostics.Process]$Process,
        [string]$Adb,
        [string]$Device,
        [string[]]$ExistingServerPids
    )

    if ($null -ne $Process) {
        try {
            if (-not $Process.HasExited) {
                $Process.Kill()
                $Process.WaitForExit(5000) | Out-Null
            }
        } catch {
            Write-Warning "结束桥接进程时出现异常：$($_.Exception.Message)"
        }
    }

    try {
        $current = @(Get-FridaServerPids $Adb $Device)
        $newPids = @($current | Where-Object { $ExistingServerPids -notcontains $_ })
        if ($newPids.Count -gt 0) {
            & $Adb -s $Device shell kill -9 @newPids 2>$null | Out-Null
        }
    } catch {
        Write-Warning "清理本次 Frida Server 时出现异常：$($_.Exception.Message)"
    }
}

function ConvertTo-ProcessArgument {
    <#
    .SYNOPSIS
    为 Windows PowerShell 5.1 的 ProcessStartInfo 生成安全参数字符串。
    #>
    param([string]$Value)

    if ($Value -match '[\s"]') {
        return '"' + $Value.Replace('"', '\"') + '"'
    }
    return $Value
}

try {
    $adb = Resolve-AdbExecutable $AdbPath
    $device = Resolve-Endpoint $adb $Endpoint
    $version = Resolve-GameVersion $adb $device $GameVersion
    $gamePid = Resolve-GameProcessId $adb $device $GameProcessId
    $bridge = Resolve-BridgeExecutable $BridgePath

    if ([string]::IsNullOrWhiteSpace($OutputPath)) {
        $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
        $OutputPath = Join-Path (Get-Location) "mumu6-raw-$stamp.json"
    } else {
        $OutputPath = [System.IO.Path]::GetFullPath($OutputPath)
    }
    if ((Test-Path -LiteralPath $OutputPath) -and -not $Force) {
        throw "输出文件已存在：$OutputPath；如需覆盖请增加 -Force。"
    }

    $parent = Split-Path $OutputPath -Parent
    if ($parent) { New-Item -ItemType Directory -Force -Path $parent | Out-Null }

    # 捕获文件明确区分元数据和批次；payload 内部内容保持桥接返回的原始 JSON 结构。
    $document = @{
        format = 'mumu-raw-capture-v1'
        source = 'MuMu6'
        capturedAt = (Get-Date).ToUniversalTime().ToString('o')
        endpoint = $device
        gameVersion = $version
        gameProcessId = $gamePid
        batches = [System.Collections.Generic.List[object]]::new()
    }
    # 同一会话内已经保存过的原始 payload 指纹；不同内容仍允许作为新的筛选批次保存。
    $seenBatchFingerprints = [System.Collections.Generic.HashSet[string]]::new()
    Write-CaptureFile $OutputPath $document

    $existingServerPids = @(Get-FridaServerPids $adb $device)
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $bridge
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $processArguments = @(
        '--session-id', "capture-$([Guid]::NewGuid().ToString())",
        '--adb-endpoint', $device,
        '--adb-path', $adb,
        '--game-version', $version,
        '--game-process-id', [string]$gamePid,
        '--read-only', 'true'
    )
    $psi.Arguments = (($processArguments | ForEach-Object { ConvertTo-ProcessArgument $_ }) -join ' ')

    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $psi
    [void]$process.Start()
    Write-Host "原始采集已启动：$OutputPath"
    Write-Host "请在 MuMu6（Android 15）游戏内选择全部主属性后完成御魂计算；2号位需包含攻击/防御/生命/速度，4号位需包含攻击/防御/生命/效果命中/效果抵抗，6号位需包含攻击/防御/生命/暴击/暴伤。"
    Write-Host "如果游戏一次不能全选，请分别完成多次计算；不同批次会保留并按御魂 ID 合并，完全重复批次会自动跳过。"
    Write-Host '采集结束请回到此窗口按 Ctrl+C，脚本会先保存已收到的全部批次。'

    while (-not $process.HasExited) {
        $line = $process.StandardOutput.ReadLine()
        if ($null -eq $line) { break }
        try {
            $event = $line | ConvertFrom-Json
        } catch {
            Write-Warning "桥接输出不是 JSON 行，已跳过：$line"
            continue
        }

        switch ($event.type) {
            'status' {
                Write-Host "[状态] $($event.status)"
            }
            'batch' {
                # payload 不做合并、不做字段转换，方便后续判断 MuMu6 是否提供完整对象。
                $fingerprint = Get-PayloadFingerprint $event.payload
                if (-not $seenBatchFingerprints.Add($fingerprint)) {
                    Write-Host "[跳过] 收到重复批次，桥接报告 $($event.soulCount) 枚御魂；已保留第一次数据"
                    continue
                }
                $document.batches.Add([ordered]@{
                    receivedAt = (Get-Date).ToUniversalTime().ToString('o')
                    soulCount = [int]$event.soulCount
                    payload = $event.payload
                })
                Write-CaptureFile $OutputPath $document
                Write-Host "[已保存] 第 $($document.batches.Count) 批，桥接报告 $($event.soulCount) 枚御魂"
            }
            'error' {
                Write-Warning "桥接错误：$($event.message)"
            }
            default {
                Write-Host "[桥接] $line"
            }
        }
    }
} finally {
    if ($null -ne $document) {
        Write-CaptureFile $OutputPath $document
    }
    Stop-BridgeProcess $process $adb $device $existingServerPids
    if ($null -ne $process) { $process.Dispose() }
    if ($null -ne $document) {
        Write-Host "原始 JSON 已保存：$OutputPath"
    }
}
