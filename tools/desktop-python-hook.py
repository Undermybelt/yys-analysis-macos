"""Windows 桌面版阴阳师 Python 运行时只读 Hook 诊断器。

本工具参考 MuMu 桥接的运行时读取思路，通过 Frida 临时附加到桌面版主进程，
监听内置 CPython 导出的字典写入 API，收集御魂压缩字段。它不调用网络、不模拟
输入、不修改游戏字段；但 Frida 附加本身会在目标进程内安装临时拦截器，因此只作为
用户主动启动的版本诊断工具，不作为桌面版正式读取适配器。
"""

from __future__ import annotations

import argparse
import ctypes
import datetime as dt
import json
import logging
import os
import platform
import subprocess
import sys
import time
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence


# 桌面版当前已确认的主进程候选；显式 PID 优先，避免误附加其他 Python 程序。
DEFAULT_PROCESS_NAMES = ("onmyoji_future.exe", "onmyoji.exe")

# Hook 脚本只读取这些压缩字段；字段值最终以字符串传回宿主，避免 JS 64 位整数精度丢失。
SOUL_FIELDS = (
    "id",
    "uid",
    "soulId",
    "soul_id",
    "short_id",
    "base_r",
    "base_rindex",
    "others",
    "rattr",
    "sattr",
    "single_attr",
)


def build_agent_script() -> str:
    """生成注入目标进程的最小 Frida 脚本；脚本只调用 CPython 只读 API。"""
    fields_json = json.dumps(list(SOUL_FIELDS), ensure_ascii=True)
    return f"""
'use strict';

// 只从主模块导出表查找 CPython API，不猜测固定地址；客户端更新后找不到导出就立即失败。
const moduleCandidates = ['onmyoji_future.exe', 'onmyoji.exe'];
let pythonModule = null;
for (const name of moduleCandidates) {{
  try {{
    pythonModule = Process.getModuleByName(name);
    break;
  }} catch (error) {{
    // 当前名称不存在时继续检查另一个桌面版名称。
  }}
}}
if (pythonModule === null) {{
  send({{type: 'error', message: '未找到桌面版主模块'}});
}} else {{
  const requiredExports = [
    'Py_GetVersion',
    'PyDict_SetItemString',
    'PyDict_GetItemString',
    'PyDict_SetItem',
    'PyObject_SetItem',
    'PyObject_Str',
    'PyObject_Repr',
    'PyUnicode_AsUTF8',
    'PyBytes_AsString',
    'PyErr_Clear',
    'PyGILState_Ensure',
    'PyGILState_Release',
  ];
  const exports = {{}};
  const missing = [];
  for (const name of requiredExports) {{
    try {{
      exports[name] = pythonModule.getExportByName(name);
    }} catch (error) {{
      missing.push(name);
    }}
  }}
  if (missing.length > 0) {{
    send({{type: 'error', message: '桌面版缺少 CPython 导出：' + missing.join(',')}});
  }} else {{
    const pyGetVersion = new NativeFunction(exports.Py_GetVersion, 'pointer', []);
    const pyDictSetItemString = new NativeFunction(
      exports.PyDict_SetItemString,
      'int',
      ['pointer', 'pointer', 'pointer'],
    );
    const pyDictGetItemString = new NativeFunction(
      exports.PyDict_GetItemString,
      'pointer',
      ['pointer', 'pointer'],
    );
    const pyDictSetItem = new NativeFunction(
      exports.PyDict_SetItem,
      'int',
      ['pointer', 'pointer', 'pointer'],
    );
    const pyObjectStr = new NativeFunction(exports.PyObject_Str, 'pointer', ['pointer']);
    const pyObjectRepr = new NativeFunction(exports.PyObject_Repr, 'pointer', ['pointer']);
    const pyUnicodeAsUtf8 = new NativeFunction(
      exports.PyUnicode_AsUTF8,
      'pointer',
      ['pointer'],
    );
    const pyBytesAsString = new NativeFunction(
      exports.PyBytes_AsString,
      'pointer',
      ['pointer'],
    );
    const pyErrClear = new NativeFunction(exports.PyErr_Clear, 'void', []);
    const pyGilStateEnsure = new NativeFunction(exports.PyGILState_Ensure, 'uint32', []);
    const pyGilStateRelease = new NativeFunction(
      exports.PyGILState_Release,
      'void',
      ['uint32'],
    );

    const fieldNames = {fields_json};
    const trackedFields = new Set(fieldNames);
    const fieldPointers = {{}};
    for (const name of fieldNames) {{
      fieldPointers[name] = Memory.allocUtf8String(name);
    }}
    const dictStates = new Map();
    const candidatePointers = new Set();
    let insideCapture = false;
    let fieldWriteCount = 0;
    let strongWriteCount = 0;
    let sequence = 0;
    // Hook 回调只把裸指针放入队列；Python C API 必须在回调返回后、取得 GIL 的批处理中调用。
    const pendingWrites = [];
    const pendingKeys = new Set();
    const maxPendingWrites = 20000;
    const maxWritesPerTick = 100;
    let processingPending = false;

    function safeReadUtf8(pointer, limit) {{
      if (pointer.isNull()) return null;
      try {{
        return pointer.readUtf8String(limit);
      }} catch (error) {{
        return null;
      }}
    }}

    // Python 3 的字符串和 bytes 分开处理；每次失败后清空 Python 异常，避免污染游戏解释器。
    function pythonText(objectPointer, useRepr) {{
      if (objectPointer.isNull()) return null;
      let text = null;
      try {{
        const bytesPointer = pyBytesAsString(objectPointer);
        text = safeReadUtf8(bytesPointer, 1024 * 1024);
      }} catch (error) {{
        pyErrClear();
      }}
      if (text !== null) return text;
      try {{
        const textObject = useRepr ? pyObjectRepr(objectPointer) : pyObjectStr(objectPointer);
        const utf8Pointer = pyUnicodeAsUtf8(textObject);
        text = safeReadUtf8(utf8Pointer, 1024 * 1024);
      }} catch (error) {{
        pyErrClear();
      }}
      return text;
    }}

    // Python 字典键通常是 Unicode；先走无副作用的字符串转换，避免对每次写入都调用对象 repr。
    function pythonKeyText(objectPointer) {{
      if (objectPointer.isNull()) return null;
      try {{
        const utf8Pointer = pyUnicodeAsUtf8(objectPointer);
        const text = safeReadUtf8(utf8Pointer, 256);
        if (text !== null) return text;
      }} catch (error) {{
        pyErrClear();
      }}
      try {{
        const bytesPointer = pyBytesAsString(objectPointer);
        return safeReadUtf8(bytesPointer, 256);
      }} catch (error) {{
        pyErrClear();
        return null;
      }}
    }}

    function readField(dictPointer, fieldName) {{
      try {{
        const valuePointer = pyDictGetItemString(dictPointer, fieldPointers[fieldName]);
        if (valuePointer.isNull()) return null;
        return pythonText(valuePointer, fieldName === 'rattr');
      }} catch (error) {{
        pyErrClear();
        return null;
      }}
    }}

    function captureSoulDictionary(dictPointer, triggerField) {{
      if (insideCapture) return;
      insideCapture = true;
      try {{
        const pointerKey = dictPointer.toString();
        const previous = dictStates.get(pointerKey) || {{}};
        const fields = {{...previous}};
        // 上一次签名只用于去重，不能再次参与本轮签名，否则每次字段写入都会产生伪变化。
        delete fields.__signature;
        for (const fieldName of fieldNames) {{
          const value = readField(dictPointer, fieldName);
          if (value !== null) fields[fieldName] = value;
        }}
        // 只有同时出现压缩主字段和副属性串才进入候选集合，排除普通 Python 配置字典。
        if (fields.others === undefined || fields.sattr === undefined) {{
          dictStates.set(pointerKey, fields);
          return;
        }}
        candidatePointers.add(pointerKey);
        const signature = JSON.stringify(fields);
        if (previous.__signature === signature) return;
        fields.__signature = signature;
        dictStates.set(pointerKey, fields);
        sequence += 1;
        send({{
          type: 'soul-candidate',
          sequence,
          dict: pointerKey,
          trigger: triggerField,
          fields,
        }});
      }} finally {{
        insideCapture = false;
      }}
    }}

    function rawPythonKeyType(objectPointer) {{
      // 只读取 CPython PyObject/PyTypeObject 的固定头部，不调用 Python API，保证 Hook 回调无重入。
      try {{
        const typePointer = objectPointer.add(Process.pointerSize).readPointer();
        const namePointer = typePointer.add(Process.pointerSize * 3).readPointer();
        return safeReadUtf8(namePointer, 64);
      }} catch (error) {{
        return null;
      }}
    }}

    function enqueueWrite(kind, dictPointer, keyPointer, keyText, valuePointer) {{
      const dictKey = dictPointer.toString();
      const valueKey = valuePointer.toString();
      const keyKey = keyText !== null ? ('text:' + keyText) : ('pointer:' + keyPointer.toString());
      const queueKey = kind + ':' + dictKey + ':' + keyKey + ':' + valueKey;
      if (pendingKeys.has(queueKey)) return;
      if (pendingWrites.length >= maxPendingWrites) {{
        const dropped = pendingWrites.shift();
        if (dropped !== undefined) pendingKeys.delete(dropped.queueKey);
      }}
      pendingKeys.add(queueKey);
      pendingWrites.push({{
        queueKey,
        kind,
        dictKey,
        keyPointer: keyPointer === null ? null : keyPointer.toString(),
        keyText,
        valueKey,
      }});
    }}

    // Hook 回调只读取 C 字符串或对象指针并入队，严禁在目标线程内调用 PyDict/PyObject API。
    function enqueueCStringWrite(args) {{
      const keyText = safeReadUtf8(args[1], 256);
      if (keyText === null) return;
      if (!trackedFields.has(keyText) && !/^[0-9a-fA-F]{{24}}$/.test(keyText)) return;
      enqueueWrite('cstring', args[0], null, keyText, args[2]);
    }}

    function enqueueObjectWrite(args) {{
      const keyType = rawPythonKeyType(args[1]);
      if (keyType !== 'str' && keyType !== 'bytes') return;
      enqueueWrite('object', args[0], args[1], null, args[2]);
    }}

    function processPendingWrites() {{
      if (processingPending || pendingWrites.length === 0) return;
      processingPending = true;
      let gilState = null;
      try {{
        // Frida 事件线程先取得 CPython GIL，再读取字典和 Python 对象，避免与游戏线程并发访问解释器。
        gilState = pyGilStateEnsure();
        let processed = 0;
        while (pendingWrites.length > 0 && processed < maxWritesPerTick) {{
          const item = pendingWrites.shift();
          pendingKeys.delete(item.queueKey);
          processed += 1;
          const keyText = item.keyText !== null
            ? item.keyText
            : pythonKeyText(ptr(item.keyPointer));
          if (keyText === null) continue;
          if (trackedFields.has(keyText)) {{
            fieldWriteCount += 1;
            if (keyText === 'sattr' || keyText === 'others') strongWriteCount += 1;
            captureSoulDictionary(ptr(item.dictKey), keyText);
            continue;
          }}
          // 已确认的御魂字典被顶层 24 位 ID 映射收纳时，补上稳定 ID；没有该事件仍保留指针兜底。
          if (/^[0-9a-fA-F]{{24}}$/.test(keyText) && candidatePointers.has(item.valueKey)) {{
            send({{type: 'soul-id-link', dict: item.valueKey, id: keyText}});
          }}
        }}
      }} catch (error) {{
        pyErrClear();
      }} finally {{
        if (gilState !== null) pyGilStateRelease(gilState);
        processingPending = false;
      }}
    }}

    // 覆盖三条公开写入路径，但所有 Python API 调用都延迟到 GIL 批处理，不在回调中重入。
    Interceptor.attach(exports.PyDict_SetItem, {{
      onEnter(args) {{
        enqueueObjectWrite(args);
      }},
    }});
    Interceptor.attach(exports.PyObject_SetItem, {{
      onEnter(args) {{
        enqueueObjectWrite(args);
      }},
    }});
    Interceptor.attach(exports.PyDict_SetItemString, {{
      onEnter(args) {{
        enqueueCStringWrite(args);
      }},
    }});
    setInterval(processPendingWrites, 10);
    setInterval(function () {{
      send({{
        type: 'stats',
        counters: {{fieldWriteCount, strongWriteCount, pendingWrites: pendingWrites.length}},
      }});
    }}, 1000);

    let pythonVersion = null;
    try {{
      pythonVersion = safeReadUtf8(pyGetVersion(), 128);
    }} catch (error) {{
      pyErrClear();
    }}
    send({{
      type: 'ready',
      mode: 'deferred-gil-read',
      module: pythonModule.name,
      pythonVersion,
      exports: requiredExports,
      counters: {{fieldWriteCount, strongWriteCount}},
    }});
  }}
}}
"""


def is_admin() -> bool:
    """读取当前权限，仅用于决定是否请求 UAC，不修改目标进程权限。"""
    if sys.platform != "win32":
        return False
    try:
        return bool(ctypes.windll.shell32.IsUserAnAdmin())
    except (AttributeError, OSError):
        return False


def maybe_elevate(argv: Sequence[str]) -> bool:
    """非管理员运行时重新启动同一 Hook 工具；拒绝 UAC 则交由主流程报告。"""
    if sys.platform != "win32" or is_admin() or "--no-elevate" in argv or "--help" in argv:
        return False
    executable = sys.executable
    parameters = subprocess.list2cmdline(list(argv)) if getattr(sys, "frozen", False) else subprocess.list2cmdline([str(Path(sys.argv[0]).resolve()), *argv])
    result = ctypes.windll.shell32.ShellExecuteW(None, "runas", executable, parameters, str(Path.cwd()), 1)
    if result <= 32:
        return False
    print("已请求管理员权限，原 Hook 进程退出；请在提升后的窗口查看报告。")
    return True


def process_names(values: Optional[Sequence[str]]) -> List[str]:
    """规范化进程候选名，避免附加到同名脚本或其他客户端。"""
    raw = values or DEFAULT_PROCESS_NAMES
    result = []
    for value in raw:
        name = Path(value.strip()).name.lower()
        if name and not name.endswith(".exe"):
            name += ".exe"
        if name and name not in result:
            result.append(name)
    return result


def resolve_pid(frida_module: Any, pid: Optional[int], names: Sequence[str]) -> int:
    """通过 Frida 本机设备枚举目标 PID；显式 PID 仍需校验进程名。"""
    if pid is not None:
        return int(pid)
    device = frida_module.get_local_device()
    rows = device.enumerate_processes()
    for row in rows:
        if row.name.lower() in names:
            return int(row.pid)
    raise RuntimeError("未找到桌面版阴阳师进程")


def default_output_path() -> Path:
    """按启动时间创建独立报告，避免覆盖前一次 Hook 结果。"""
    stamp = dt.datetime.now().strftime("%Y%m%d_%H%M%S")
    return Path.cwd() / f"desktop-python-hook-{stamp}.json"


def run_hook(args: argparse.Namespace) -> int:
    """附加一次短时 Python Hook，会话结束时卸载脚本并分离 Frida。"""
    try:
        import frida  # type: ignore[import-not-found]
    except ImportError as error:
        raise RuntimeError("当前 Python 环境缺少 frida 包；请使用已安装 Frida 的 Python 环境") from error

    output_path = (args.output or default_output_path()).resolve()
    started_at = dt.datetime.now(dt.timezone.utc).isoformat(timespec="milliseconds")
    report: Dict[str, Any] = {
        "format": "yys-desktop-python-hook-v2",
        "capturedAt": started_at,
        "platform": platform.platform(),
        "isAdmin": is_admin(),
        "readOnlyIntent": True,
        "network": False,
        "simulateInput": False,
        "requestedPid": args.pid,
        "requestedProcessNames": process_names(args.process_name),
        "targetPid": None,
        "hook": None,
        "latestStats": None,
        "events": [],
        "souls": [],
        "errors": [],
    }
    session = None
    script = None
    records: Dict[str, Dict[str, Any]] = {}

    def on_message(message: Dict[str, Any], _data: Any) -> None:
        """接收 Hook 端的 JSON 事件；原始字段只写入本地报告，不发送到网络。"""
        if message.get("type") != "send":
            report["errors"].append(str(message))
            return
        payload = message.get("payload") or {}
        event_type = payload.get("type")
        if event_type == "soul-candidate":
            dict_key = str(payload.get("dict"))
            current = records.setdefault(dict_key, {"hookPointer": dict_key})
            fields = payload.get("fields")
            if isinstance(fields, dict):
                current.update({key: value for key, value in fields.items() if not key.startswith("__")})
            current["lastTrigger"] = payload.get("trigger")
            current["lastSequence"] = payload.get("sequence")
            return
        if event_type == "soul-id-link":
            dict_key = str(payload.get("dict"))
            current = records.setdefault(dict_key, {"hookPointer": dict_key})
            current["id"] = payload.get("id")
            return
        if event_type == "stats":
            report["latestStats"] = payload.get("counters")
            return
        if event_type == "ready":
            report["hook"] = payload
        report["events"].append(payload)

    try:
        target_pid = resolve_pid(frida, args.pid, process_names(args.process_name))
        report["targetPid"] = target_pid
        device = frida.get_local_device()
        session = device.attach(target_pid)
        script = session.create_script(build_agent_script())
        script.on("message", on_message)
        script.load()
        deadline = time.monotonic() + max(1.0, float(args.duration))
        print(f"Hook 已附加到 PID={target_pid}，请在此期间打开御魂页面；无需翻页。")
        while time.monotonic() < deadline:
            time.sleep(0.2)
    except Exception as error:  # noqa: BLE001 - 诊断工具必须保留完整错误到报告
        report["errors"].append(str(error))
        return_code = 3
    else:
        return_code = 0
    finally:
        if script is not None:
            try:
                script.unload()
            except Exception as error:  # noqa: BLE001 - 清理失败仍需写出报告
                report["errors"].append(f"卸载 Hook 失败：{error}")
        if session is not None:
            try:
                session.detach()
            except Exception as error:  # noqa: BLE001 - 清理失败仍需写出报告
                report["errors"].append(f"分离 Frida 会话失败：{error}")
        report["souls"] = list(records.values())
        report["summary"] = {
            "candidateCount": len(report["souls"]),
            "completeCount": sum(
                1
                for record in report["souls"]
                if record.get("others") is not None and record.get("sattr") is not None
            ),
            "idCount": sum(1 for record in report["souls"] if record.get("id")),
            "eventCount": len(report["events"]),
        }
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
        print(f"Hook 完成：候选 {report['summary']['candidateCount']} 条，报告={output_path}")
    return return_code


def parse_args(argv: Optional[Sequence[str]] = None) -> argparse.Namespace:
    """解析短时 Hook 参数；默认 60 秒给用户打开页面和等待库存加载。"""
    parser = argparse.ArgumentParser(description="阴阳师桌面版 CPython 只读 Hook 诊断器")
    parser.add_argument("--pid", type=int, help="显式指定游戏进程 PID")
    parser.add_argument("--process-name", action="append", help="覆盖进程候选名，可重复传入")
    parser.add_argument("--duration", type=float, default=60.0, help="Hook 持续秒数")
    parser.add_argument("--output", type=Path, help="报告 JSON 路径")
    parser.add_argument("--no-elevate", action="store_true", help="不自动请求管理员权限")
    return parser.parse_args(argv)


def main(argv: Optional[Sequence[str]] = None) -> int:
    """组织 UAC、Frida 附加和清理，确保 Hook 不会在工具退出后常驻。"""
    raw_args = list(sys.argv[1:] if argv is None else argv)
    if maybe_elevate(raw_args):
        return 0
    try:
        return run_hook(parse_args(raw_args))
    except Exception as error:  # noqa: BLE001 - 顶层错误必须让用户看到原因而不是闪退
        print(f"Hook 失败：{error}", file=sys.stderr)
        return 4


if __name__ == "__main__":
    raise SystemExit(main())
