import type { Shikigami } from "./api/contracts";

// Wiki 页面按式神正式名称命名；运行时只生成链接，不在离线应用中请求 Wiki 内容。
const SHIKIGAMI_WIKI_BASE_URL = "https://wiki.biligame.com/yys/";

/** 将目录中的式神名称编码为对应 Wiki 页面地址，确保中文和特殊字符不会破坏 URL。 */
export function getShikigamiWikiUrl(entry: Pick<Shikigami, "name">): string {
  return `${SHIKIGAMI_WIKI_BASE_URL}${encodeURIComponent(entry.name)}`;
}
