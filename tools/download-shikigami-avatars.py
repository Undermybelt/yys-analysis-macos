"""下载并压缩阴阳师 Wiki 式神头像，生成随前端发布的离线资源。"""

from __future__ import annotations

import io
import json
import hashlib
from pathlib import Path
from urllib.request import Request, urlopen

from PIL import Image, ImageOps


# 头像只在构建期访问 Wiki；运行中的桌面端只读取 public 目录，不依赖网络。
ROOT = Path(__file__).resolve().parents[1]
CATALOG_PATH = ROOT / "src-tauri" / "src" / "data" / "shikigami-catalog.json"
OUTPUT_DIR = ROOT / "public" / "shikigami-avatars"
HUIJI_CDN = "https://huiji-public.huijistatic.com/yys/uploads"
USER_AGENT = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) yys-analysis-offline-catalog/1.0"

# 四个达摩是素材目录条目，不使用可培养式神的头像候选流程，固定使用 Wiki 文件图像。
MATERIAL_IMAGE_URLS = {
    "招福达摩": (
        "410",
        "https://patchwiki.biligame.com/images/yys/3/36/eojulu7ezkn1eveaoy9oyjsraoubgcj.png",
    ),
    "御行达摩": (
        "411",
        "https://patchwiki.biligame.com/images/yys/1/11/c6omlysnr96ghaigh9k36qa75mr23xw.png",
    ),
    "奉为达摩": (
        "412",
        "https://patchwiki.biligame.com/images/yys/a/a4/py5vuhgar5ztpozgnx3csq30evz854h.png",
    ),
    "大吉达摩": (
        "413",
        "https://patchwiki.biligame.com/images/yys/2/2d/0ut0f2db5g57gxncsnisiwcyqym1rm0.png",
    ),
}


def huiji_avatar_url(shikigami_id: str) -> str:
    """按角色检索页的静态文件规则生成头像地址，保持编号与项目目录一一对应。"""
    filename = f"Tx_{shikigami_id}_s.png"
    # Huiji Wiki 使用文件名 MD5 的首字符和前两字符作为上传目录的两级前缀。
    digest = hashlib.md5(filename.encode("utf-8"), usedforsecurity=False).hexdigest()
    return f"{HUIJI_CDN}/{digest[0]}/{digest[:2]}/{filename}"


def download_avatar(url: str) -> bytes:
    """下载单张原图字节；原图随后立即压缩，不会存入仓库。"""
    request = Request(
        url,
        headers={
            "User-Agent": USER_AGENT,
            "Referer": "https://yys.huijiwiki.com/wiki/角色检索",
        },
    )
    with urlopen(request, timeout=45) as response:  # noqa: S310 - URL 来自固定的 Wiki CDN 规则或素材表
        return response.read()


def write_avatar(image_bytes: bytes, output: Path) -> None:
    """统一裁切缩放为 128px 方图并保存为 WebP，控制桌面包体积。"""
    with Image.open(io.BytesIO(image_bytes)) as image:
        converted = image.convert("RGBA")
        canvas = ImageOps.fit(
            converted,
            (128, 128),
            method=Image.Resampling.LANCZOS,
            centering=(0.5, 0.5),
        )
        # 下载端若返回纯色错误页或空白图，必须视为失败，不能冒充真实头像写入目录。
        color_count = canvas.convert("RGB").getcolors(maxcolors=128 * 128)
        if color_count is not None and len(color_count) <= 3:
            raise ValueError("下载结果是纯色图片")
        canvas.save(output, format="WEBP", quality=88, method=6)


def write_fallback_avatar(rarity: str, output: Path) -> None:
    """仅在确实没有远程资源时生成带纹理的离线占位图，避免再次出现纯色图片。"""
    colors = {
        "UR": (112, 76, 145, 255),
        "SP": (126, 72, 105, 255),
        "SSR": (164, 116, 45, 255),
        "SR": (63, 126, 111, 255),
        "R": (94, 128, 166, 255),
        "N": (118, 113, 105, 255),
    }
    base = colors.get(rarity, colors["N"])
    canvas = Image.new("RGBA", (128, 128), base)
    # 斜向明暗纹理用于明确区分“资源缺失占位图”和真实头像，不能伪装成已下载立绘。
    for offset in range(-128, 256, 16):
        for point in range(128):
            x = offset + point
            y = point
            if 0 <= x < 128:
                # 使用多档明度而非单一高光色，确保占位图也不会退化成低色数纯色块。
                delta = 12 + (point // 8 % 5) * 6
                canvas.putpixel(
                    (x, y),
                    (
                        min(base[0] + delta, 255),
                        min(base[1] + delta, 255),
                        min(base[2] + delta, 255),
                        255,
                    ),
                )
    canvas.save(output, format="WEBP", quality=82, method=6)


def validate_avatar(output: Path) -> None:
    """校验头像能解码、尺寸正确且不是纯色图，防止坏资源进入发布目录。"""
    with Image.open(output) as image:
        if image.size != (128, 128):
            raise ValueError(f"头像尺寸错误：{image.size}")
        colors = image.convert("RGB").getcolors(maxcolors=4)
        if colors is not None and len(colors) <= 3:
            raise ValueError("头像是纯色图片")


def main() -> None:
    """读取目录、下载 Huiji Wiki 头像并校验每条式神都有对应离线资源。"""
    catalog = json.loads(CATALOG_PATH.read_text(encoding="utf-8"))
    entries = [
        entry
        for entry in catalog["entries"]
        if entry["rarity"] not in {"MATERIAL", "素材"}
    ]
    # 角色检索页的文件编号与项目稳定 shikigamiId 相同，不依赖页面名称或分页顺序。
    urls = {
        entry["name"]: huiji_avatar_url(entry["shikigamiId"])
        for entry in entries
    }
    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
    real_count = 0
    fallback_count = 0
    failed_names: list[str] = []
    for index, entry in enumerate(entries, start=1):
        target = OUTPUT_DIR / f"{entry['shikigamiId']}.webp"
        if entry["name"] in urls:
            try:
                write_avatar(download_avatar(urls[entry["name"]]), target)
                real_count += 1
            except Exception as error:  # noqa: BLE001 - 单张头像失败时使用离线兜底
                print(f"头像下载失败，使用纹理占位图：{entry['name']}（{error}）")
                write_fallback_avatar(entry["rarity"], target)
                fallback_count += 1
                failed_names.append(entry["name"])
        else:
            write_fallback_avatar(entry["rarity"], target)
            fallback_count += 1
            failed_names.append(entry["name"])
        print(f"[{index}/{len(entries)}] {entry['name']}")

    # 批量写入后再次逐文件校验，确保下载成功、官方备用资源和占位逻辑都不会生成坏图。
    for entry in entries:
        validate_avatar(OUTPUT_DIR / f"{entry['shikigamiId']}.webp")

    # 素材式神单独下载并校验，确保图鉴、式神录和碎片页共用的头像随包发布。
    for material_name, (material_id, image_url) in MATERIAL_IMAGE_URLS.items():
        material_target = OUTPUT_DIR / f"{material_id}.webp"
        write_avatar(download_avatar(image_url), material_target)
        validate_avatar(material_target)
        print(f"素材头像已更新：{material_name}（{material_id}）")

    print(
        f"完成：{len(entries) + len(MATERIAL_IMAGE_URLS)} 张离线头像已写入 {OUTPUT_DIR}；"
        f"真实立绘 {real_count} 张，纹理占位图 {fallback_count} 张。"
    )
    if failed_names:
        print("仍缺少真实头像：" + "、".join(failed_names))


if __name__ == "__main__":
    main()
