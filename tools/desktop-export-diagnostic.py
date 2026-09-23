"""桌面版御魂导出诊断包装器。

本脚本只包装朋友旧工具提取出的 `_cstructs` 读取核心，不重新猜测游戏内存偏移。
目标是保留旧版 JSON 导出能力，同时把进程发现、权限、异常和输出文件路径记录完整，
方便在无法安装开发环境的虚拟机中复现问题。
"""

from __future__ import annotations

import argparse
import ctypes
import datetime as dt
import json
import logging
import os
import platform
import re
import shutil
import struct
import subprocess
import sys
import time
import traceback
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Tuple


# 旧版核心写死的桌面客户端候选进程名；按顺序尝试，避免误打开无关进程。
PROCESS_CANDIDATES = ("onmyoji_future.exe", "onmyoji.exe", "client.exe")
LOG_PREFIX = "[YYS-DESKTOP-DIAG]"
DEFAULT_CORE_NAME = "_cstructs.pyd"
DEFAULT_TIMEOUT_SECONDS = 180


class TerminateProcessGuard:
    """旧核心的进程结束保护钩子；只替换 `_cstructs.pyd` 的导入表，不修改系统 DLL。"""

    def __init__(self, callback: Callable[..., int], patched_addresses: List[int]) -> None:
        # 必须持有回调引用直到核心调用结束，否则 ctypes 回调被回收后会导致原生崩溃。
        self.callback = callback
        self.patched_addresses = patched_addresses


def read_u32_from_bytes(data: bytes, offset: int) -> int:
    """读取 PE 文件中的无符号 32 位字段，并在越界时中止保护安装。"""
    if offset < 0 or offset + 4 > len(data):
        raise RuntimeError("PE 字段越界，拒绝安装进程结束保护")
    return int.from_bytes(data[offset:offset + 4], "little")


def read_u64_from_bytes(data: bytes, offset: int) -> int:
    """读取 PE 文件中的无符号 64 位字段。"""
    if offset < 0 or offset + 8 > len(data):
        raise RuntimeError("PE 64 位字段越界，拒绝安装进程结束保护")
    return int.from_bytes(data[offset:offset + 8], "little")


def read_u64_from_address(address: int) -> int:
    """读取已加载模块 IAT 中的 64 位函数地址。"""
    return ctypes.c_uint64.from_address(address).value


def read_ascii_from_bytes(data: bytes, offset: int) -> str:
    """读取 PE 文件中的零结尾 ASCII 字符串。"""
    if offset < 0 or offset >= len(data):
        raise RuntimeError("PE 字符串越界，拒绝安装进程结束保护")
    end = data.find(b"\0", offset)
    if end < 0:
        raise RuntimeError("PE 字符串没有结束符，拒绝安装进程结束保护")
    return data[offset:end].decode("ascii", errors="replace")


def pe_rva_to_file_offset(data: bytes, pe_offset: int, rva: int) -> int:
    """按 PE 节表把 RVA 转为磁盘文件偏移，供导入表名称解析使用。"""
    number_of_sections = int.from_bytes(data[pe_offset + 6:pe_offset + 8], "little")
    optional_size = int.from_bytes(data[pe_offset + 20:pe_offset + 22], "little")
    section_table = pe_offset + 24 + optional_size
    for index in range(number_of_sections):
        section = section_table + index * 40
        virtual_size = read_u32_from_bytes(data, section + 8)
        virtual_address = read_u32_from_bytes(data, section + 12)
        raw_size = read_u32_from_bytes(data, section + 16)
        raw_pointer = read_u32_from_bytes(data, section + 20)
        section_size = max(virtual_size, raw_size)
        if virtual_address <= rva < virtual_address + section_size:
            offset = raw_pointer + (rva - virtual_address)
            if offset < len(data):
                return offset
    raise RuntimeError("PE RVA 不在任何节中，拒绝安装进程结束保护")


def find_import_iat_address(core_path: Path, module_base: int, imported_dll: str, imported_name: str) -> Optional[int]:
    """从磁盘 PE 导入名表解析内存 IAT 地址，兼容 OriginalFirstThunk 已被清零的旧核心。"""
    data = core_path.read_bytes()
    if len(data) < 0x40 or data[:2] != b"MZ":
        raise RuntimeError("旧核心不是有效 PE 文件，拒绝安装进程结束保护")
    pe_offset = read_u32_from_bytes(data, 0x3C)
    if data[pe_offset:pe_offset + 4] != b"PE\0\0":
        raise RuntimeError("旧核心 PE 签名无效，拒绝安装进程结束保护")
    optional_header = pe_offset + 24
    magic = int.from_bytes(data[optional_header:optional_header + 2], "little")
    if magic != 0x20B:
        raise RuntimeError("旧核心不是 64 位 PE，无法安全安装进程结束保护")
    # PE32+ 数据目录从可选头 +112 开始，导入目录是第 1 项，每项 8 字节。
    import_rva = read_u32_from_bytes(data, optional_header + 112 + 8)
    if import_rva == 0:
        return None
    descriptor = pe_rva_to_file_offset(data, pe_offset, import_rva)
    while True:
        original_thunk_rva = read_u32_from_bytes(data, descriptor)
        name_rva = read_u32_from_bytes(data, descriptor + 12)
        first_thunk_rva = read_u32_from_bytes(data, descriptor + 16)
        if not original_thunk_rva and not name_rva and not first_thunk_rva:
            return None
        dll_name = read_ascii_from_bytes(data, pe_rva_to_file_offset(data, pe_offset, name_rva))
        if dll_name.lower() == imported_dll.lower():
            # 某些旧 PE 将 OriginalFirstThunk 置零，文件中的 FirstThunk 仍保存名称 RVA。
            lookup_rva = original_thunk_rva or first_thunk_rva
            index = 0
            while True:
                lookup_offset = pe_rva_to_file_offset(data, pe_offset, lookup_rva + index * 8)
                lookup_value = read_u64_from_bytes(data, lookup_offset)
                if lookup_value == 0:
                    break
                # 64 位导入表最高位为序号标记；按名称导入时值是 Hint/Name RVA。
                if not (lookup_value & (1 << 63)):
                    name_offset = pe_rva_to_file_offset(data, pe_offset, lookup_value + 2)
                    function_name = read_ascii_from_bytes(data, name_offset)
                    if function_name == imported_name:
                        return module_base + first_thunk_rva + index * 8
                index += 1
        descriptor += 20


def install_terminate_process_guard(core_path: Path, logger: logging.Logger) -> Optional[TerminateProcessGuard]:
    """把旧核心 IAT 中的 TerminateProcess 替换为记录并拒绝的回调，保护真实客户端不被结束。"""
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.GetModuleHandleW.argtypes = [ctypes.c_wchar_p]
    kernel32.GetModuleHandleW.restype = ctypes.c_void_p
    module_base = int(kernel32.GetModuleHandleW(core_path.name) or 0)
    if not module_base:
        logger.warning("%s 未找到已加载核心模块，无法安装 TerminateProcess 保护", LOG_PREFIX)
        return None
    iat_address = find_import_iat_address(core_path, module_base, "KERNEL32.DLL", "TerminateProcess")
    if not iat_address:
        logger.info("%s 核心未导入 TerminateProcess，跳过进程结束保护", LOG_PREFIX)
        return None
    original_address = read_u64_from_address(iat_address)
    callback_type = ctypes.WINFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_uint)

    def deny_terminate(process_handle: int, exit_code: int) -> int:
        """拒绝旧核心结束任何进程；失败信息由父进程日志统一记录。"""
        logger.warning(
            "%s 已拦截旧核心 TerminateProcess(handle=0x%x, exitCode=%d)",
            LOG_PREFIX,
            int(process_handle or 0),
            int(exit_code),
        )
        ctypes.set_last_error(5)
        return 0

    callback = callback_type(deny_terminate)
    new_address = ctypes.cast(callback, ctypes.c_void_p).value
    if not new_address:
        raise RuntimeError("无法取得 TerminateProcess 保护回调地址")
    kernel32.VirtualProtect.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_ulong, ctypes.POINTER(ctypes.c_ulong)]
    kernel32.VirtualProtect.restype = ctypes.c_int
    old_protection = ctypes.c_ulong(0)
    if not kernel32.VirtualProtect(ctypes.c_void_p(iat_address), ctypes.c_size_t(8), 0x40, ctypes.byref(old_protection)):
        raise ctypes.WinError(ctypes.get_last_error())
    ctypes.memmove(iat_address, ctypes.byref(ctypes.c_uint64(new_address)), 8)
    restored_protection = ctypes.c_ulong(0)
    kernel32.VirtualProtect(ctypes.c_void_p(iat_address), ctypes.c_size_t(8), old_protection.value, ctypes.byref(restored_protection))
    logger.info(
        "%s 已安装 TerminateProcess 保护 core=%s iat=0x%x original=0x%x guard=0x%x",
        LOG_PREFIX,
        core_path,
        iat_address,
        original_address,
        new_address,
    )
    return TerminateProcessGuard(callback, [iat_address])


def build_logger(log_path: Path) -> logging.Logger:
    """创建同时写文件和控制台的诊断日志器，记录时间、级别和线程信息。"""
    logger = logging.getLogger("yys_desktop_export_diagnostic")
    logger.setLevel(logging.DEBUG)
    logger.handlers.clear()
    formatter = logging.Formatter(
        "%(asctime)s.%(msecs)03d [%(levelname)s] %(message)s",
        datefmt="%Y-%m-%d %H:%M:%S",
    )

    log_path.parent.mkdir(parents=True, exist_ok=True)
    file_handler = logging.FileHandler(log_path, encoding="utf-8")
    file_handler.setLevel(logging.DEBUG)
    file_handler.setFormatter(formatter)
    logger.addHandler(file_handler)

    console_handler = logging.StreamHandler()
    console_handler.setLevel(logging.INFO)
    console_handler.setFormatter(formatter)
    logger.addHandler(console_handler)
    return logger


def is_admin() -> bool:
    """读取当前进程的管理员状态；旧核心可能需要管理员权限打开游戏进程。"""
    try:
        return bool(ctypes.windll.shell32.IsUserAnAdmin())
    except (AttributeError, OSError):
        return False


def snapshot_processes(logger: logging.Logger) -> List[Dict[str, Any]]:
    """枚举候选进程的 PID、路径和窗口信息，帮助区分“未启动”和“名称变化”。"""
    rows: List[Dict[str, Any]] = []
    try:
        # 使用 PowerShell 仅做只读诊断；失败时仍保留核心读取尝试，不把诊断工具变成硬依赖。
        command = [
            "powershell.exe",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-CimInstance Win32_Process | "
            "Select-Object Name,ProcessId,ExecutablePath,CommandLine | "
            "ConvertTo-Json -Compress",
        ]
        completed = subprocess.run(
            command,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=8,
            check=False,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )
        logger.debug("%s 进程快照命令退出码=%s", LOG_PREFIX, completed.returncode)
        if completed.stderr.strip():
            logger.debug("%s 进程快照 stderr=%s", LOG_PREFIX, completed.stderr.strip())
        decoded = json.loads(completed.stdout) if completed.stdout.strip() else []
        if isinstance(decoded, dict):
            decoded = [decoded]
        interesting_pattern = re.compile(r"onmyoji|yys|阴阳师|网易|netease|bilibili|client", re.IGNORECASE)
        interesting_count = 0
        candidate_names = {candidate.lower() for candidate in PROCESS_CANDIDATES}
        for row in decoded:
            name = str(row.get("Name") or "")
            if interesting_pattern.search(name):
                interesting_count += 1
                logger.info(
                    "%s 相关进程 name=%s pid=%s path=%s commandLine=%s",
                    LOG_PREFIX,
                    row.get("Name"),
                    row.get("ProcessId"),
                    row.get("ExecutablePath"),
                    row.get("CommandLine"),
                )
            if name.lower() in candidate_names:
                rows.append(row)
        logger.info(
            "%s 进程总数=%d，相关进程数=%d，候选进程数=%d，候选名=%s",
            LOG_PREFIX,
            len(decoded),
            interesting_count,
            len(rows),
            PROCESS_CANDIDATES,
        )
        for row in rows:
            logger.info(
                "%s 候选进程 name=%s pid=%s path=%s commandLine=%s",
                LOG_PREFIX,
                row.get("Name"),
                row.get("ProcessId"),
                row.get("ExecutablePath"),
                row.get("CommandLine"),
            )
    except Exception:
        logger.exception("%s 进程快照失败，继续尝试旧核心", LOG_PREFIX)
    return rows


def load_legacy_core(core_path: Path, logger: logging.Logger) -> Any:
    """导入 Python 3.8 x64 的 `_cstructs` 核心并记录公开接口，拒绝错误位数/文件。"""
    logger.info("%s 准备加载核心=%s", LOG_PREFIX, core_path)
    if not core_path.is_file():
        raise FileNotFoundError(f"核心文件不存在：{core_path}")
    if struct.calcsize("P") != 8:
        raise RuntimeError("当前 Python 不是 64 位，旧核心要求 Python 3.8 x64")
    import importlib.util

    spec = importlib.util.spec_from_file_location("_cstructs", core_path)
    if spec is None or spec.loader is None:
        raise ImportError(f"无法创建核心加载器：{core_path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules["_cstructs"] = module
    spec.loader.exec_module(module)
    public_names = sorted(name for name in dir(module) if not name.startswith("__"))
    logger.info("%s 核心加载成功，公开接口前缀=%s", LOG_PREFIX, public_names[:30])
    return module


def run_export(core: Any, logger: logging.Logger) -> Any:
    """调用旧核心的统一入口；兼容核心返回 JSON、字典或通过文件保存的不同形式。"""
    run = getattr(core, "run", None)
    if not callable(run):
        raise AttributeError("_cstructs 未提供 run() 入口")
    logger.info("%s 开始调用旧核心 run()，后续流程由核心负责", LOG_PREFIX)
    started = time.perf_counter()
    result = run()
    logger.info("%s 旧核心 run() 返回，耗时 %.3fs，返回类型=%s", LOG_PREFIX, time.perf_counter() - started, type(result).__name__)
    return result


def resolve_core_path(requested: Path) -> Path:
    """解析脚本运行和 PyInstaller 运行两种布局下的 `_cstructs.pyd` 路径。"""
    if requested != Path(DEFAULT_CORE_NAME):
        return requested
    frozen_root = getattr(sys, "_MEIPASS", None)
    if frozen_root:
        return Path(frozen_root) / DEFAULT_CORE_NAME
    return Path.cwd() / DEFAULT_CORE_NAME


def worker_main(core_path: Path, allow_client_terminate: bool) -> int:
    """子进程安装客户端保护后调用旧核心；父进程负责超时、日志和进程生命周期控制。"""
    worker_logger = logging.getLogger("yys_desktop_export_worker")
    if not worker_logger.handlers:
        # 子进程只把加载失败和核心调用信息送到父进程；父进程统一落盘完整日志。
        worker_logger.addHandler(logging.StreamHandler())
        worker_logger.setLevel(logging.DEBUG)
    try:
        core = load_legacy_core(core_path.resolve(), worker_logger)
        # 默认拒绝旧核心结束目标进程；只有明确传入参数时才恢复原版高风险行为。
        terminate_guard = None
        if not allow_client_terminate:
            terminate_guard = install_terminate_process_guard(core_path.resolve(), worker_logger)
        else:
            worker_logger.warning("%s 已通过参数允许旧核心 TerminateProcess，可能导致客户端退出", LOG_PREFIX)
        run_export(core, worker_logger)
        # 保持回调引用直到 run() 返回，避免原生 IAT 回调地址失效。
        _ = terminate_guard
        return 0
    except KeyboardInterrupt:
        return 130
    except Exception:
        traceback.print_exc()
        return 1


def worker_command() -> List[str]:
    """生成当前脚本或当前单文件 EXE 的子进程命令，确保超时可杀死旧核心。"""
    if getattr(sys, "frozen", False):
        return [sys.executable, "--worker"]
    return [sys.executable, str(Path(__file__).resolve()), "--worker"]


def decode_child_output(raw: Optional[bytes]) -> str:
    """按 UTF-8、GB18030 顺序解码旧核心输出，避免中文日志被替换字符污染。"""
    if not raw:
        return ""
    for encoding in ("utf-8", "gb18030", "cp936"):
        try:
            decoded = raw.decode(encoding)
        except UnicodeDecodeError:
            continue
        if "\ufffd" not in decoded:
            return decoded
    return raw.decode("utf-8", errors="replace")


def run_with_timeout(core_path: Path, timeout_seconds: int, allow_client_terminate: bool, logger: logging.Logger) -> int:
    """在受控子进程中调用旧核心，转发 stdout/stderr 并在超时后强制结束。"""
    command = worker_command()
    # 无论脚本还是单文件 EXE，都显式传递核心绝对路径；单文件 EXE 的资源在临时目录中。
    command.extend(["--core", str(core_path.resolve())])
    if allow_client_terminate:
        command.append("--allow-client-terminate")
    logger.info("%s 启动核心子进程 command=%s timeout=%ss", LOG_PREFIX, command, timeout_seconds)
    environment = os.environ.copy()
    environment["PYTHONUNBUFFERED"] = "1"
    # 原版核心使用 Windows 中文控制台编码输出；固定子进程编码，避免管道转 UTF-8 时出现乱码。
    environment["PYTHONIOENCODING"] = "gb18030"
    child = subprocess.Popen(
        command,
        cwd=str(Path.cwd()),
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=environment,
        creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
    )
    try:
        stdout, stderr = child.communicate(timeout=timeout_seconds)
    except subprocess.TimeoutExpired as error:
        logger.error("%s 核心子进程超时，开始终止 pid=%s", LOG_PREFIX, child.pid)
        child.kill()
        stdout, stderr = child.communicate()
        partial_stdout = decode_child_output(error.stdout or stdout)
        partial_stderr = decode_child_output(error.stderr or stderr)
        if partial_stdout:
            logger.error("%s 核心超时前 stdout=\n%s", LOG_PREFIX, partial_stdout)
        if partial_stderr:
            logger.error("%s 核心超时前 stderr=\n%s", LOG_PREFIX, partial_stderr)
        return 124
    decoded_stdout = decode_child_output(stdout)
    decoded_stderr = decode_child_output(stderr)
    if decoded_stdout:
        logger.info("%s 核心 stdout=\n%s", LOG_PREFIX, decoded_stdout.rstrip())
    if decoded_stderr:
        logger.warning("%s 核心 stderr=\n%s", LOG_PREFIX, decoded_stderr.rstrip())
    logger.info("%s 核心子进程退出 pid=%s code=%s", LOG_PREFIX, child.pid, child.returncode)
    return int(child.returncode or 0)


def normalize_json_result(result: Any, output_path: Path, logger: logging.Logger) -> Optional[Path]:
    """将旧核心可能返回的 JSON 值写成兼容文件；已有核心文件则只记录不覆盖。"""
    if result is None:
        logger.info("%s 核心未返回 Python 对象，检查当前目录是否已生成 JSON", LOG_PREFIX)
        return None
    output_path.parent.mkdir(parents=True, exist_ok=True)
    if isinstance(result, (dict, list)):
        output_path.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    elif isinstance(result, str):
        text = result
        json.loads(text)
        output_path.write_text(text, encoding="utf-8")
    else:
        logger.warning("%s 核心返回非 JSON 类型=%s，不强行序列化", LOG_PREFIX, type(result).__name__)
        return None
    logger.info("%s 兼容 JSON 已保存 path=%s bytes=%d", LOG_PREFIX, output_path, output_path.stat().st_size)
    return output_path


def collect_changed_json(before: Dict[Path, Tuple[int, int]], logger: logging.Logger) -> List[Path]:
    """找出旧核心本次新增或改写的 JSON，兼容原版的中文文件名和时间戳文件名。"""
    changed: List[Path] = []
    for path in Path.cwd().glob("*.json"):
        try:
            stat = path.stat()
        except OSError as error:
            logger.warning("%s 无法读取 JSON 文件状态 path=%s error=%s", LOG_PREFIX, path, error)
            continue
        current = (stat.st_size, stat.st_mtime_ns)
        if before.get(path) != current and stat.st_size > 0:
            changed.append(path)
    changed.sort(key=lambda item: item.stat().st_mtime_ns, reverse=True)
    for path in changed:
        logger.info("%s 检测到新增/改写 JSON path=%s bytes=%d", LOG_PREFIX, path, path.stat().st_size)
    return changed


def snapshot_json_files() -> Dict[Path, Tuple[int, int]]:
    """保存调用旧核心前的 JSON 文件状态，用于区分本次导出的文件。"""
    snapshot: Dict[Path, Tuple[int, int]] = {}
    for path in Path.cwd().glob("*.json"):
        try:
            stat = path.stat()
        except OSError:
            continue
        snapshot[path] = (stat.st_size, stat.st_mtime_ns)
    return snapshot


def collect_environment(logger: logging.Logger) -> None:
    """记录诊断所需的非敏感运行环境，避免把账号或原始库存写入日志。"""
    logger.info(
        "%s 环境 python=%s executable=%s platform=%s pointerSize=%d admin=%s cwd=%s",
        LOG_PREFIX,
        sys.version.replace("\n", " "),
        sys.executable,
        platform.platform(),
        struct.calcsize("P"),
        is_admin(),
        Path.cwd(),
    )


def main() -> int:
    """执行环境记录、候选进程快照、旧核心导出和异常落盘，始终返回可诊断退出码。"""
    parser = argparse.ArgumentParser(description="桌面版御魂导出诊断工具")
    parser.add_argument("--core", type=Path, default=Path("_cstructs.pyd"), help="旧核心路径")
    parser.add_argument("--output", type=Path, default=None, help="可选 JSON 输出路径")
    parser.add_argument("--log", type=Path, default=None, help="日志路径")
    parser.add_argument("--timeout-seconds", type=int, default=DEFAULT_TIMEOUT_SECONDS, help="旧核心最长运行秒数")
    parser.add_argument(
        "--allow-client-terminate",
        action="store_true",
        help="危险兼容模式：允许旧核心调用 TerminateProcess，可能导致阴阳师退出",
    )
    parser.add_argument("--no-pause", action="store_true", help="结束时不等待回车，适合自动化测试")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()

    if args.worker:
        return worker_main(resolve_core_path(args.core), args.allow_client_terminate)

    stamp = dt.datetime.now().strftime("%Y%m%d_%H%M%S")
    log_path = args.log or Path.cwd() / f"yys-desktop-export-{stamp}.log"
    output_path = args.output or Path.cwd() / f"yys-export-{stamp}.json"
    logger = build_logger(log_path)
    logger.info("%s 启动，版本=diagnostic-0.1", LOG_PREFIX)
    logger.info("%s 输出候选路径=%s 日志路径=%s", LOG_PREFIX, output_path, log_path)
    pause_at_end = not args.no_pause and bool(getattr(sys.stdin, "isatty", lambda: False)())

    def finish(exit_code: int) -> int:
        """在交互式双击场景保留控制台；自动化测试通过 --no-pause 跳过等待。"""
        if pause_at_end:
            try:
                input("按回车键退出……")
            except EOFError:
                logger.debug("%s 标准输入不可用，跳过退出等待", LOG_PREFIX)
        return exit_code

    try:
        collect_environment(logger)
        candidates = snapshot_processes(logger)
        if not candidates:
            logger.error(
                "%s 未找到候选客户端，拒绝调用旧核心，避免旧核心无目标时无限等待；候选名=%s",
                LOG_PREFIX,
                PROCESS_CANDIDATES,
            )
            return finish(2)
        core_path = resolve_core_path(args.core)
        json_before = snapshot_json_files()
        exit_code = run_with_timeout(core_path, max(1, args.timeout_seconds), args.allow_client_terminate, logger)
        if exit_code != 0:
            logger.error("%s 核心子进程失败，退出码=%s", LOG_PREFIX, exit_code)
            return finish(exit_code)
        # 旧核心通常自行保存文件；包装器只扫描并记录结果，不覆盖其兼容格式。
        generated = collect_changed_json(json_before, logger)
        if generated:
            latest = generated[0]
            if args.output is not None and latest.resolve() != args.output.resolve():
                # 只有明确指定 --output 时才复制，避免破坏原版文件名和字段格式。
                args.output.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(latest, args.output)
                logger.info("%s 已按参数复制 JSON path=%s bytes=%d", LOG_PREFIX, args.output, args.output.stat().st_size)
        else:
            logger.warning("%s 核心返回成功但当前目录没有发现新增或改写的 *.json", LOG_PREFIX)
        logger.info("%s 流程结束：成功", LOG_PREFIX)
        return finish(0)
    except KeyboardInterrupt:
        logger.warning("%s 用户中断", LOG_PREFIX)
        return finish(130)
    except Exception as error:
        logger.error("%s 流程失败：%s: %s", LOG_PREFIX, type(error).__name__, error)
        logger.error("%s 完整堆栈：\n%s", LOG_PREFIX, traceback.format_exc())
        logger.error("%s 请把本日志和控制台输出一并带回，JSON 可能尚未生成", LOG_PREFIX)
        return finish(1)


if __name__ == "__main__":
    raise SystemExit(main())
