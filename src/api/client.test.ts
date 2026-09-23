import { beforeEach, describe, expect, it, vi } from "vitest";

const { invokeMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invokeMock,
}));

import { desktopApi } from "./client";

describe("desktopApi.lookupShikigamiShards", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("按稳定式神 ID 调用跨角色碎片查询且不发送当前角色", async () => {
    const result = { shikigamiId: "398", characters: [] };
    invokeMock.mockResolvedValue(result);

    await expect(desktopApi.lookupShikigamiShards("398")).resolves.toEqual(result);
    expect(invokeMock).toHaveBeenCalledWith("lookup_shikigami_shards_across_archives", {
      request: { shikigamiId: "398" },
    });
  });
});
