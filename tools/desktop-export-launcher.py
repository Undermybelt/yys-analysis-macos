"""桌面版御魂导出单文件启动器。

PyInstaller 使用当前机器已有的 Python 构建本启动器；真正加载旧 `_cstructs.pyd` 时，
转交给随 EXE 一起打包的 Python 3.8 x64 运行时，避免 Python ABI 不匹配。
"""

import os
import subprocess
import sys
import ctypes
from pathlib import Path
from typing import List


def bundled_runtime_root() -> Path:
    """定位 PyInstaller 临时解包目录中的 Python 3.8 运行时目录。"""
    frozen_root = getattr(sys, "_MEIPASS", None)
    if frozen_root:
        return Path(frozen_root) / "runtime"
    return Path(__file__).resolve().parent.parent / ".scratch" / "desktop-export-build" / "runtime"


def build_command(arguments: List[str]) -> List[str]:
    """拼接隔离运行命令；显式传入旧核心路径，确保单文件模式不依赖当前目录。"""
    runtime_root = bundled_runtime_root()
    python_executable = runtime_root / "python.exe"
    script_path = runtime_root / "desktop-export-diagnostic.py"
    command = [str(python_executable), str(script_path)]
    if "--core" not in arguments:
        command.extend(["--core", str(runtime_root / "_cstructs.pyd")])
    command.extend(arguments)
    return command


def is_admin() -> bool:
    """判断当前 EXE 是否已获得管理员令牌；旧核心需要它读取游戏进程内存。"""
    try:
        return bool(ctypes.windll.shell32.IsUserAnAdmin())
    except (AttributeError, OSError):
        return False


def elevate_if_needed(arguments: List[str]) -> bool:
    """复刻旧版 runas 行为；返回 True 表示已启动提升后的副本，当前副本应退出。"""
    if not getattr(sys, "frozen", False) or is_admin() or "--no-elevate" in arguments:
        return False
    forwarded = [argument for argument in arguments if argument != "--no-elevate"]
    parameters = subprocess.list2cmdline(forwarded)
    result = ctypes.windll.shell32.ShellExecuteW(
        None,
        "runas",
        sys.executable,
        parameters,
        str(Path.cwd()),
        1,
    )
    if result <= 32:
        print(f"管理员权限提升失败，ShellExecuteW 返回码={result}", file=sys.stderr)
        return False
    print("已请求管理员权限，原进程退出；请在提升后的窗口中继续查看日志。")
    return True


def main() -> int:
    """校验打包资源后启动诊断脚本，并把其退出码原样传给调用方。"""
    arguments = list(sys.argv[1:])
    if elevate_if_needed(arguments):
        return 0
    arguments = [argument for argument in arguments if argument != "--no-elevate"]
    command = build_command(arguments)
    runtime_root = bundled_runtime_root()
    missing = [path for path in (Path(command[0]), Path(command[1]), runtime_root / "_cstructs.pyd") if not path.is_file()]
    if missing:
        print("桌面版导出工具资源缺失：", file=sys.stderr)
        for path in missing:
            print(f"  {path}", file=sys.stderr)
        return 1
    environment = os.environ.copy()
    environment["PYTHONUNBUFFERED"] = "1"
    try:
        return int(subprocess.call(command, cwd=str(Path.cwd()), env=environment))
    except KeyboardInterrupt:
        return 130
    except Exception as error:
        print(f"启动诊断脚本失败：{type(error).__name__}: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
