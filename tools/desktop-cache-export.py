"""只读读取阴阳师桌面版账号缓存并导出诊断 JSON。

本工具不查找游戏进程、不读取游戏内存、不冻结或注入客户端，只读取
Documents/userdata 与 Documents/cache_server_data 下的文件。当前桌面版
inventory 的属性串 sattr 使用“类型码 + 7 字节压缩浮点”布局；工具保留
原始 sattr，同时输出可核对的副属性分组与最终值。
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import logging
import math
import os
import re
import struct
import sys
import time
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional, Tuple


LOG_PREFIX = "[YYS-DESKTOP-CACHE]"
FORMAT_NAME = "yys-desktop-cache-decoded-v2"
ACCOUNT_ID_PATTERN = re.compile(r"^[0-9a-fA-F]{24}$")
ROLE_LOG_PATTERN = re.compile(
    r"\[(?P<clock>\d{2}:\d{2}:\d{2}(?:\.\d{1,6})?)\].*?"
    r"createRoleSuccessful, id (?P<account>[0-9a-fA-F]{24})",
    re.IGNORECASE,
)
MAX_CACHE_BYTES = 128 * 1024 * 1024

# 桌面版 sattr 类型码从 01 开始；属性单位沿用项目已验证的 MuMu 紧凑格式。
# valueScale 是内部规范值的单位，例如 0.03 表示 3%，114 表示生命固定值一次成长。
DESKTOP_ATTRIBUTE_SPECS: Dict[int, Tuple[str, str, float, str]] = {
    1: ("hp_flat", "生命", 114.0, "flat"),
    2: ("defense_flat", "防御", 5.0, "flat"),
    3: ("attack_flat", "攻击", 27.0, "flat"),
    4: ("hp_rate", "生命加成", 0.03, "percent"),
    5: ("defense_rate", "防御加成", 0.03, "percent"),
    6: ("attack_rate", "攻击加成", 0.03, "percent"),
    7: ("speed", "速度", 3.0, "flat"),
    8: ("crit_rate", "暴击", 0.03, "percent"),
    9: ("crit_damage", "暴击伤害", 0.04, "percent"),
    10: ("effect_hit", "效果命中", 0.04, "percent"),
    11: ("effect_resist", "效果抵抗", 0.04, "percent"),
}

# 首领御魂使用普通套装编码位；明确列出套装名称，避免把其他特殊套装误标为首领御魂。
LEADER_SET_NAMES: Dict[int, str] = {
    50: "土蜘蛛",
    51: "胧车",
    52: "荒骷髅",
    53: "地震鲶",
    54: "蜃气楼",
    77: "鬼灵歌伎",
    91: "夜荒魂",
}

# 项目图鉴中的稳定套装编码；导出器只把它用于展示，不参与缓存选择或属性计算。
SOUL_SET_NAMES: Dict[int, str] = {
    2: "雪幽魂", 3: "地藏像", 4: "蝠翼", 6: "涅槃之火", 7: "三味",
    8: "魍魉之匣", 9: "被服", 10: "招财猫", 11: "反枕", 12: "轮入道",
    13: "日女巳时", 14: "镜姬", 15: "钟灵", 18: "狰", 19: "火灵",
    20: "鸣屋", 21: "薙魂", 22: "心眼", 23: "木魅", 24: "树妖",
    26: "网切", 27: "阴摩罗", 29: "伤魂鸟", 30: "破势", 31: "镇墓兽",
    32: "珍珠", 33: "骰子鬼", 34: "蚌精", 35: "魅妖", 36: "针女",
    39: "返魂香", 48: "狂骨", 49: "幽谷响", 50: "土蜘蛛", 51: "胧车",
    52: "荒骷髅", 53: "地震鲶", 54: "蜃气楼", 55: "片叶之苇", 56: "尘冢",
    57: "油赤子", 58: "夜啼石", 59: "夜送犬", 60: "雨降", 73: "飞缘魔",
    74: "兵主部", 75: "青女房", 76: "涂佛", 77: "鬼灵歌伎", 79: "遗念火",
    80: "共潜", 81: "恶楼", 82: "贝吹坊", 83: "海月火玉", 84: "出世螺",
    85: "火之车", 86: "隐念", 87: "叠叩", 88: "应声虫", 89: "元兴寺",
    90: "钓瓶火", 91: "夜荒魂", 92: "无刀取", 93: "奉海图", 94: "八咫镜",
    95: "天羽羽斩", 96: "预言星盘", 97: "月之石", 98: "纺缘锤", 99: "稻荷穗箭",
}

# 桌面缓存的 single_attr 是首领/星痕等御魂的数值固有属性编码；保留该映射用于诊断输出。
DESKTOP_SINGLE_ATTRIBUTE_SPECS: Dict[int, Tuple[str, str, float]] = {
    1: ("attack_rate", "攻击加成", 0.08),
    2: ("hp_rate", "生命加成", 0.08),
    3: ("defense_rate", "防御加成", 0.16),
    4: ("crit_rate", "暴击", 0.08),
    5: ("effect_hit", "效果命中", 0.08),
    6: ("effect_resist", "效果抵抗", 0.08),
}

# 已验证的特殊模板号位映射；常规模板仍通过模板编号族计算号位。
SPECIAL_TEMPLATE_SLOTS: Dict[int, int] = {
    180001: 1,
    180003: 3,
    180005: 5,
    180011: 5,
    180013: 1,
    180015: 3,
    180016: 4,
    180020: 2,
    180027: 6,
}


def configure_logger(log_path: Path) -> logging.Logger:
    """创建同时写文件和控制台的详细日志；日志不写入游戏目录。"""
    # 日志路径可能由命令行指定到尚不存在的测试目录，必须先创建父目录，
    # 否则 FileHandler 初始化失败会遮蔽真正的缓存诊断结果。
    log_path.parent.mkdir(parents=True, exist_ok=True)
    logger = logging.getLogger("yys_desktop_cache_export")
    logger.setLevel(logging.DEBUG)
    logger.handlers.clear()
    formatter = logging.Formatter(
        "%(asctime)s.%(msecs)03d [%(levelname)s] %(message)s",
        datefmt="%Y-%m-%d %H:%M:%S",
    )
    file_handler = logging.FileHandler(log_path, encoding="utf-8")
    file_handler.setLevel(logging.DEBUG)
    file_handler.setFormatter(formatter)
    console_handler = logging.StreamHandler()
    console_handler.setLevel(logging.INFO)
    console_handler.setFormatter(formatter)
    logger.addHandler(file_handler)
    logger.addHandler(console_handler)
    return logger


def now_stamp() -> str:
    """生成文件名使用的本地时间戳，避免覆盖上一次诊断结果。"""
    return dt.datetime.now().strftime("%Y%m%d_%H%M%S")


def parse_clock(value: str) -> float:
    """把日志中的时分秒转换成可比较的秒数；异常值按零处理。"""
    try:
        hours, minutes, seconds = value.split(":")
        return int(hours) * 3600 + int(minutes) * 60 + float(seconds)
    except (TypeError, ValueError):
        return 0.0


def iter_nested_values(value: Any, path: str = "$") -> Iterable[Tuple[str, Any]]:
    """遍历配置 JSON，保留路径，便于日志说明 UID 的实际来源。"""
    yield path, value
    if isinstance(value, dict):
        for key, child in value.items():
            yield from iter_nested_values(child, f"{path}.{key}")
    elif isinstance(value, list):
        for index, child in enumerate(value):
            yield from iter_nested_values(child, f"{path}[{index}]")


def find_config_accounts(documents_root: Path, logger: logging.Logger) -> Dict[str, List[str]]:
    """从所有 userdata 配置收集 24 位账号 ID，不读取其他账号 inventory。"""
    accounts: Dict[str, List[str]] = {}
    userdata_root = documents_root / "userdata"
    configs = sorted(userdata_root.glob("*/config")) if userdata_root.is_dir() else []
    logger.debug("%s 扫描 userdata 配置数=%d 根目录=%s", LOG_PREFIX, len(configs), userdata_root)
    for config_path in configs:
        try:
            document = json.loads(config_path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError, json.JSONDecodeError) as error:
            logger.warning("%s 配置读取失败 path=%s error=%s", LOG_PREFIX, config_path, error)
            continue
        for json_path, value in iter_nested_values(document):
            if not isinstance(value, str) or not ACCOUNT_ID_PATTERN.fullmatch(value):
                continue
            key = json_path.rsplit(".", 1)[-1].lower()
            if key not in {"uid", "userid", "user_id", "roleid", "role_id"} and "uid" not in key:
                continue
            accounts.setdefault(value.lower(), []).append(f"{config_path}:{json_path}")
    for account_id, sources in sorted(accounts.items()):
        logger.info("%s 配置发现账号 accountId=%s sources=%s", LOG_PREFIX, account_id, sources)
    return accounts


def find_latest_log_account(documents_root: Path, logger: logging.Logger) -> Optional[str]:
    """从游戏日志中找最近一次 createRoleSuccessful，作为当前登录账号证据。"""
    log_root = documents_root / "log"
    candidates: List[Tuple[float, float, int, str, Path]] = []
    logs = sorted(log_root.glob("log*.txt")) if log_root.is_dir() else []
    logger.debug("%s 扫描游戏日志数=%d 根目录=%s", LOG_PREFIX, len(logs), log_root)
    for log_path in logs:
        try:
            text = log_path.read_text(encoding="utf-8", errors="replace")
            file_mtime = log_path.stat().st_mtime
        except OSError as error:
            logger.warning("%s 日志读取失败 path=%s error=%s", LOG_PREFIX, log_path, error)
            continue
        for match in ROLE_LOG_PATTERN.finditer(text):
            # 同一秒优先当前 log.txt，再用文件修改时间处理跨文件排序。
            current_file_priority = 1 if log_path.name.lower() == "log.txt" else 0
            candidates.append(
                (
                    parse_clock(match.group("clock")),
                    file_mtime,
                    current_file_priority,
                    match.group("account").lower(),
                    log_path,
                )
            )
    if not candidates:
        logger.warning("%s 未在游戏日志中找到 createRoleSuccessful", LOG_PREFIX)
        return None
    selected = max(candidates)
    logger.info(
        "%s 最新角色证据 accountId=%s clock=%s source=%s candidates=%d",
        LOG_PREFIX,
        selected[3],
        selected[0],
        selected[4],
        len(candidates),
    )
    return selected[3]


def read_hex_json(path: Path, logger: logging.Logger) -> Tuple[Any, bytes]:
    """读取桌面缓存的一层 ASCII 十六进制 JSON，并返回解析对象与解码后字节。"""
    raw = path.read_bytes()
    if len(raw) > MAX_CACHE_BYTES:
        raise ValueError(f"缓存文件超过安全上限 {MAX_CACHE_BYTES} 字节")
    text = raw.decode("ascii").strip()
    if len(text) % 2 != 0 or not re.fullmatch(r"[0-9a-fA-F]+", text):
        raise ValueError("缓存不是预期的 ASCII 十六进制文本")
    decoded = bytes.fromhex(text)
    document = json.loads(decoded.decode("utf-8"))
    logger.debug(
        "%s 缓存解码 path=%s outerBytes=%d innerBytes=%d jsonType=%s",
        LOG_PREFIX,
        path,
        len(raw),
        len(decoded),
        type(document).__name__,
    )
    return document, decoded


def read_stable_hex_json(path: Path, logger: logging.Logger, retries: int = 3) -> Tuple[Any, bytes, os.stat_result]:
    """在游戏可能写入缓存时读取稳定快照；变化则重试并记录，不锁文件。"""
    for attempt in range(1, retries + 1):
        before = path.stat()
        if before.st_size > MAX_CACHE_BYTES:
            raise ValueError(f"缓存文件超过安全上限 {MAX_CACHE_BYTES} 字节")
        document, decoded = read_hex_json(path, logger)
        after = path.stat()
        if (before.st_size, before.st_mtime_ns) == (after.st_size, after.st_mtime_ns):
            logger.info("%s 缓存快照稳定 attempt=%d size=%d", LOG_PREFIX, attempt, after.st_size)
            return document, decoded, after
        logger.warning(
            "%s 缓存读取期间发生变化 attempt=%d before=(%d,%d) after=(%d,%d)",
            LOG_PREFIX,
            attempt,
            before.st_size,
            before.st_mtime_ns,
            after.st_size,
            after.st_mtime_ns,
        )
        time.sleep(0.2)
    raise RuntimeError("缓存持续变化，未生成可能不一致的导出")


def compact_diagnostics(record: Dict[str, Any]) -> Dict[str, Any]:
    """解码桌面缓存的主属性、副属性事件和首领套装标识。"""
    others = record.get("others")
    sattr = record.get("sattr")
    if not isinstance(others, int):
        return {"decodeStatus": "unsupported", "reason": "others 不是整数"}
    raw_suit_code = (others >> 20) & 0xFFF
    suit_code = raw_suit_code - 992 if raw_suit_code >= 992 else None
    template_id = others & 0xFFFFF
    slot = slot_from_template(template_id)
    level = (others >> 48) & 0xF
    result: Dict[str, Any] = {
        "decodeStatus": "decoded-candidate-v2",
        "layoutMarker": f"0x{((others >> 32) & 0xFFFF):04X}",
        "templateId": template_id,
        "rawSuitCode": raw_suit_code,
        "suitCode": suit_code,
        "suitName": SOUL_SET_NAMES.get(suit_code),
        "suitCategory": "首领御魂" if suit_code in LEADER_SET_NAMES else None,
        "slot": slot,
        "cacheLevel": level,
        "mainAttribute": main_attribute_diagnostic(slot, record.get("base_rindex"), level),
    }
    if isinstance(sattr, str):
        decoded = decode_sattr(sattr)
        result.update(decoded)
        result["sattrLength"] = len(sattr)
    else:
        result["sattrDecodeStatus"] = "unsupported"
    # single_attr 在桌面版缓存中直接提供固有属性编码；未知编码只进入诊断状态，不静默伪造属性。
    single_attr = record.get("single_attr")
    if single_attr is None:
        result["fixedAttributeDecodeStatus"] = "missing"
    elif isinstance(single_attr, int) and single_attr in DESKTOP_SINGLE_ATTRIBUTE_SPECS:
        attr_type, name, value = DESKTOP_SINGLE_ATTRIBUTE_SPECS[single_attr]
        result["fixedAttribute"] = {
            "code": single_attr,
            "type": attr_type,
            "name": name,
            "value": value,
            "fixed": True,
        }
        result["fixedAttributeDecodeStatus"] = "decoded"
    else:
        result["fixedAttributeDecodeStatus"] = "unknown-code"
    return result


def slot_from_template(template_id: int) -> Optional[int]:
    """按模板编号还原御魂号位；只采用项目已有导入器验证过的模板规则。"""
    family = template_id // 10_000
    if 11 <= family <= 16:
        return family - 10
    if 190_000 <= template_id < 200_000:
        slot = template_id % 10
        return slot if 1 <= slot <= 6 else None
    return SPECIAL_TEMPLATE_SLOTS.get(template_id)


def main_attribute_diagnostic(
    slot: Optional[int], base_index: Any, level: int
) -> Optional[Dict[str, Any]]:
    """根据号位、主属性索引和等级生成主属性诊断；不匹配时返回空值而不猜测。"""
    if not isinstance(slot, int) or not isinstance(base_index, int):
        return None
    mapping = {
        (1, 0): ("attack_flat", "攻击", "flat"),
        (2, 0): ("attack_rate", "攻击加成", "percent"),
        (2, 1): ("defense_rate", "防御加成", "percent"),
        (2, 2): ("hp_rate", "生命加成", "percent"),
        (2, 3): ("speed", "速度", "flat"),
        (3, 1): ("defense_flat", "防御", "flat"),
        (4, 0): ("attack_rate", "攻击加成", "percent"),
        (4, 1): ("defense_rate", "防御加成", "percent"),
        (4, 2): ("hp_rate", "生命加成", "percent"),
        (4, 3): ("effect_hit", "效果命中", "percent"),
        (4, 4): ("effect_resist", "效果抵抗", "percent"),
        (5, 2): ("hp_flat", "生命", "flat"),
        (6, 0): ("attack_rate", "攻击加成", "percent"),
        (6, 1): ("defense_rate", "防御加成", "percent"),
        (6, 2): ("hp_rate", "生命加成", "percent"),
        (6, 3): ("crit_damage", "暴击伤害", "percent"),
        (6, 4): ("crit_rate", "暴击", "percent"),
    }
    spec = mapping.get((slot, base_index))
    if spec is None or not 0 <= level <= 15:
        return None
    attr_type, name, unit = spec
    if attr_type == "attack_flat":
        value = 81 + level * 27
    elif attr_type == "defense_flat":
        value = 14 + level * 6
    elif attr_type == "hp_flat":
        value = 342 + level * 114
    elif attr_type == "speed":
        value = 12 + level * 3
    elif attr_type == "crit_damage":
        value = 0.14 + level * 0.05
    else:
        value = 0.10 + level * 0.03
    return {
        "type": attr_type,
        "name": name,
        "value": value,
        "displayValue": format_attribute_value(value, unit),
    }


def decode_sattr(sattr: str) -> Dict[str, Any]:
    """把每 10 字符的桌面副属性事件解为类型、成长倍率和聚合最终值。"""
    if len(sattr) % 10 != 0:
        return {
            "sattrDecodeStatus": "invalid-length",
            "sattrChunkCount": None,
            "sattrChunks": [],
            "subAttributes": [],
        }
    chunks: List[Dict[str, Any]] = []
    grouped: Dict[int, Dict[str, Any]] = {}
    for index in range(0, len(sattr), 10):
        chunk = sattr[index : index + 10]
        type_code_text = chunk[:2]
        tail = chunk[3:]
        if chunk[2] != "?" or not type_code_text.isdigit():
            return {
                "sattrDecodeStatus": "invalid-chunk-layout",
                "sattrChunkCount": len(sattr) // 10,
                "sattrChunks": [],
                "subAttributes": [],
            }
        type_code = int(type_code_text)
        spec = DESKTOP_ATTRIBUTE_SPECS.get(type_code)
        if spec is None:
            return {
                "sattrDecodeStatus": "unknown-type-code",
                "sattrChunkCount": len(sattr) // 10,
                "sattrChunks": [],
                "subAttributes": [],
            }
        try:
            tail_bytes = bytes(ord(char) for char in tail)
            # 桌面端去掉 IEEE754 double 的固定首字节 0x3F，只存后 7 字节。
            growth_ratio = struct.unpack(">d", b"\x3F" + tail_bytes)[0]
        except (ValueError, struct.error):
            return {
                "sattrDecodeStatus": "invalid-float",
                "sattrChunkCount": len(sattr) // 10,
                "sattrChunks": [],
                "subAttributes": [],
            }
        if not math.isfinite(growth_ratio) or not 0 < growth_ratio <= 1.05:
            return {
                "sattrDecodeStatus": "invalid-float-range",
                "sattrChunkCount": len(sattr) // 10,
                "sattrChunks": [],
                "subAttributes": [],
            }
        attr_type, name, scale, unit = spec
        contribution = growth_ratio * scale
        chunk_info = {
            "index": index // 10,
            "typeCode": type_code,
            "type": attr_type,
            "name": name,
            "growthRatio": round(growth_ratio, 8),
            "valueContribution": round(contribution, 8),
            "rawHex": tail_bytes.hex(),
        }
        chunks.append(chunk_info)
        group = grouped.setdefault(
            type_code,
            {"typeCode": type_code, "type": attr_type, "name": name, "unit": unit, "scale": scale, "growthTotal": 0.0, "eventCount": 0},
        )
        group["growthTotal"] += growth_ratio
        group["eventCount"] += 1

    sub_attributes = []
    for type_code in sorted(grouped):
        group = grouped[type_code]
        value = group["growthTotal"] * group["scale"]
        sub_attributes.append(
            {
                "typeCode": type_code,
                "type": group["type"],
                "name": group["name"],
                "value": round(value, 8),
                "displayValue": format_attribute_value(value, group["unit"]),
                # 第一条是初始副属性，后续事件对应强化命中次数。
                "enhancementCount": max(0, group["eventCount"] - 1),
                "eventCount": group["eventCount"],
            }
        )
    return {
        "sattrDecodeStatus": "decoded",
        "sattrChunkCount": len(chunks),
        "sattrChunks": chunks,
        "subAttributes": sub_attributes,
    }


def format_attribute_value(value: float, unit: str) -> str:
    """生成核对用显示值；JSON 同时保留未格式化的数值。"""
    if unit == "percent":
        return f"{value * 100:.2f}%"
    if abs(value - round(value)) < 1e-9:
        return str(int(round(value)))
    return f"{value:.2f}"


def normalize_inventory(document: Any, logger: logging.Logger) -> Dict[str, Dict[str, Any]]:
    """校验 inventory 的稳定外形并复制当前账号记录，拒绝静默丢弃异常条目。"""
    if not isinstance(document, dict):
        raise ValueError("inventory 解码结果不是 JSON 对象")
    normalized: Dict[str, Dict[str, Any]] = {}
    for item_id, raw_record in document.items():
        if not isinstance(item_id, str) or not ACCOUNT_ID_PATTERN.fullmatch(item_id):
            raise ValueError(f"inventory 中存在非法物品 ID: {item_id!r}")
        if not isinstance(raw_record, dict):
            raise ValueError(f"物品 {item_id} 记录不是对象")
        if "base_rindex" not in raw_record or "others" not in raw_record or "sattr" not in raw_record:
            raise ValueError(f"物品 {item_id} 缺少 base_rindex/others/sattr")
        record = dict(raw_record)
        record["_desktopDiagnostics"] = compact_diagnostics(record)
        normalized[item_id.lower()] = record
    decoded_count = sum(
        record["_desktopDiagnostics"].get("sattrDecodeStatus") == "decoded"
        for record in normalized.values()
    )
    leader_count = sum(
        record["_desktopDiagnostics"].get("suitCategory") == "首领御魂"
        for record in normalized.values()
    )
    logger.info(
        "%s inventory 记录校验通过 count=%d sattrDecoded=%d leaderSouls=%d",
        LOG_PREFIX,
        len(normalized),
        decoded_count,
        leader_count,
    )
    return normalized


def select_account(documents_root: Path, logger: logging.Logger) -> Tuple[str, str, Dict[str, List[str]]]:
    """只选择当前角色目录；日志证据优先，配置 UID 用于交叉验证。"""
    accounts = find_config_accounts(documents_root, logger)
    log_account = find_latest_log_account(documents_root, logger)
    available = {
        path.name.lower()
        for path in (documents_root / "cache_server_data").iterdir()
        if path.is_dir() and ACCOUNT_ID_PATTERN.fullmatch(path.name)
    } if (documents_root / "cache_server_data").is_dir() else set()
    logger.info("%s 账号缓存目录数=%d ids=%s", LOG_PREFIX, len(available), sorted(available))
    if log_account and log_account in available:
        selected = log_account
        reason = "最新 createRoleSuccessful 日志且对应缓存目录存在"
    elif len(accounts) == 1:
        selected = next(iter(accounts))
        reason = "userdata/config 中唯一账号 UID"
    elif log_account:
        raise RuntimeError(f"日志最新账号 {log_account} 没有对应 cache_server_data 目录")
    else:
        raise RuntimeError("无法确认当前账号：没有有效日志 UID，且配置 UID 不唯一")
    config_ids = set(accounts)
    if config_ids and selected not in config_ids:
        logger.warning("%s 日志账号与配置 UID 不一致 log=%s config=%s", LOG_PREFIX, selected, sorted(config_ids))
    for skipped in sorted(available - {selected}):
        logger.info("%s 跳过其他账号缓存 accountId=%s reason=不是当前登录账号", LOG_PREFIX, skipped)
    logger.info("%s 选定当前账号 accountId=%s reason=%s", LOG_PREFIX, selected, reason)
    return selected, reason, accounts


def export_cache(game_root: Path, output_path: Path, log_path: Path) -> int:
    """执行一次只读缓存导出，并把选择依据、完整性和原始记录写入 JSON。"""
    logger = configure_logger(log_path)
    logger.info("%s 启动 format=%s version=cache-0.1", LOG_PREFIX, FORMAT_NAME)
    logger.info("%s gameRoot=%s cwd=%s pid=%d", LOG_PREFIX, game_root, Path.cwd(), os.getpid())
    documents_root = game_root / "Documents"
    if not documents_root.is_dir():
        raise FileNotFoundError(f"找不到游戏 Documents 目录: {documents_root}")
    account_id, selection_reason, config_accounts = select_account(documents_root, logger)
    inventory_path = documents_root / "cache_server_data" / account_id / "inventory"
    if not inventory_path.is_file():
        raise FileNotFoundError(f"找不到当前账号 inventory: {inventory_path}")
    document, decoded, stat_result = read_stable_hex_json(inventory_path, logger)
    inventory = normalize_inventory(document, logger)
    digest = hashlib.sha256(inventory_path.read_bytes()).hexdigest()
    captured_at = dt.datetime.now(dt.timezone.utc).isoformat(timespec="milliseconds")
    payload = {
        "format": FORMAT_NAME,
        "capturedAt": captured_at,
        "source": "desktop-cache",
        "gameRoot": str(game_root),
        "accountId": account_id,
        "accountSelection": selection_reason,
        "inventoryPath": str(inventory_path),
        "inventoryCount": len(inventory),
        "inventorySha256": digest,
        "inventoryOuterBytes": stat_result.st_size,
        "inventoryDecodedBytes": len(decoded),
        "warnings": [
            "副属性按 sattr 类型码聚合并生成核对值；原始分组、类型码和十六进制值均保留。",
            "桌面版缓存使用 single_attr 保存固有属性编码；导出保留原始字段并附带固有属性诊断。",
            "未读取其他账号 inventory；其他账号目录仅用于确认分账号结构。",
            "本次运行未查找、冻结、注入或操作游戏进程。",
        ],
        "configAccountIds": sorted(config_accounts),
        "payload": inventory,
    }
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8")
    logger.info("%s JSON 写入成功 path=%s bytes=%d", LOG_PREFIX, output_path, output_path.stat().st_size)
    logger.info("%s 完成 accountId=%s inventoryCount=%d", LOG_PREFIX, account_id, len(inventory))
    return len(inventory)


def parse_args(argv: List[str]) -> argparse.Namespace:
    """解析命令行参数；默认输出到当前工作目录，方便虚拟机双击后取文件。"""
    parser = argparse.ArgumentParser(description="只读导出阴阳师桌面版当前账号缓存")
    parser.add_argument("--game-root", type=Path, default=Path(r"D:\game\yys"), help="游戏根目录")
    parser.add_argument("--output", type=Path, help="JSON 输出路径")
    parser.add_argument("--log", type=Path, help="日志输出路径")
    parser.add_argument("--no-pause", action="store_true", help="结束时不等待按键")
    return parser.parse_args(argv)


def main(argv: Optional[List[str]] = None) -> int:
    """组织参数、导出和错误日志；失败时返回非零退出码但不触碰游戏进程。"""
    args = parse_args(list(sys.argv[1:] if argv is None else argv))
    stamp = now_stamp()
    output_path = args.output or Path.cwd() / f"yys-desktop-cache-{stamp}.json"
    log_path = args.log or Path.cwd() / f"yys-desktop-cache-{stamp}.log"
    try:
        count = export_cache(args.game_root.resolve(), output_path.resolve(), log_path.resolve())
        print(f"导出完成：{count} 条，JSON={output_path.resolve()}，日志={log_path.resolve()}")
        return 0
    except Exception as error:  # noqa: BLE001 - 顶层统一写入用户可带走的诊断日志
        logger = logging.getLogger("yys_desktop_cache_export")
        if logger.handlers:
            logger.exception("%s 导出失败 type=%s error=%s", LOG_PREFIX, type(error).__name__, error)
        else:
            print(f"导出失败：{type(error).__name__}: {error}", file=sys.stderr)
        print(f"导出失败，详情见日志：{log_path.resolve()}", file=sys.stderr)
        return 1
    finally:
        if not args.no_pause and sys.stdin.isatty():
            input("按回车退出...")


if __name__ == "__main__":
    raise SystemExit(main())
