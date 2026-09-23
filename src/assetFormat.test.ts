import { describe, expect, it } from "vitest";
import { formatCoin } from "./assetFormat";

describe("formatCoin", () => {
  it("一千万以下保留完整数字", () => {
    expect(formatCoin(9_999_999)).toBe("9,999,999");
  });

  it("达到一千万后按万展示", () => {
    expect(formatCoin(10_000_000)).toBe("1000万");
    expect(formatCoin(12_345_678)).toBe("1234万");
  });

  it("达到一亿后按亿和万展示", () => {
    expect(formatCoin(100_000_000)).toBe("1亿");
    expect(formatCoin(123_456_789)).toBe("1亿2345万");
  });
});
