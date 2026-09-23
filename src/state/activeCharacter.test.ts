import { beforeEach, describe, expect, it, vi } from "vitest";

const { listCharacterArchivesMock, setActiveCharacterMock } = vi.hoisted(() => ({
  // 用桩函数观察启动恢复角色时是否同步调用后端激活接口。
  listCharacterArchivesMock: vi.fn(),
  setActiveCharacterMock: vi.fn(),
}));

vi.mock("../api/client", () => ({
  desktopApi: {
    listCharacterArchives: listCharacterArchivesMock,
    setActiveCharacter: setActiveCharacterMock,
  },
}));

import { syncActiveCharacter } from "./activeCharacter";

describe("syncActiveCharacter", () => {
  const storage = new Map<string, string>();

  beforeEach(() => {
    // 启动恢复依赖浏览器本地存储；测试用内存实现隔离每个用例的角色选择。
    storage.clear();
    vi.stubGlobal("localStorage", {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => storage.set(key, value),
      removeItem: (key: string) => storage.delete(key),
    });
    vi.stubGlobal("window", {
      // 角色状态广播只需验证不抛异常，具体监听逻辑由页面集成测试覆盖。
      dispatchEvent: vi.fn(),
    });
    listCharacterArchivesMock.mockReset();
    setActiveCharacterMock.mockReset();
  });

  it("应用重启恢复川源时应同步后端激活档案，避免御魂查询返回空列表", async () => {
    storage.set("yys-active-character-id", "profile-chuanyuan");
    listCharacterArchivesMock.mockResolvedValue({
      message: "已读取角色档案",
      characters: [
        {
          accountId: "account-chuanyuan",
          profileId: "profile-chuanyuan",
          displayName: "川源",
          playerId: null,
          serverLabel: "测试服",
          lastUpdatedAt: null,
          isCurrent: false,
          readable: true,
          warning: null,
          soulCount: 5000,
          shikigamiCount: 100,
          sourceKind: "native",
          sourceLabel: "本地导入",
        },
      ],
    });
    setActiveCharacterMock.mockResolvedValue(undefined);

    await syncActiveCharacter();

    expect(setActiveCharacterMock).toHaveBeenCalledWith({
      profileId: "profile-chuanyuan",
    });
  });
});
