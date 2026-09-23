import { describe, expect, it } from "vitest";
import { toLocalDateTime } from "./timeFormat";

describe("toLocalDateTime", () => {
  it("把 UTC ISO 时间转成本地时区的紧凑显示", () => {
    const iso = "2026-08-19T03:24:00Z";
    const expected = new Date(iso);
    const pad = (part: number) => String(part).padStart(2, "0");
    expect(toLocalDateTime(iso)).toBe(
      `${expected.getFullYear()}-${pad(expected.getMonth() + 1)}-${pad(
        expected.getDate(),
      )} ${pad(expected.getHours())}:${pad(expected.getMinutes())}`,
    );
  });

  it("空值返回 null", () => {
    expect(toLocalDateTime(null)).toBeNull();
    expect(toLocalDateTime(undefined)).toBeNull();
  });

  it("无法解析的字符串原样返回", () => {
    expect(toLocalDateTime("not-a-time")).toBe("not-a-time");
  });
});
