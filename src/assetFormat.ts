// 金币达到一千万后按整数万展示，达到一亿后拆分为“亿 + 万”，不足一万的尾数不显示。
export function formatCoin(value: number): string {
  if (value >= 100_000_000) {
    const hundredMillion = Math.floor(value / 100_000_000);
    const tenThousands = Math.floor((value % 100_000_000) / 10_000);
    return tenThousands === 0
      ? `${hundredMillion}亿`
      : `${hundredMillion}亿${tenThousands}万`;
  }

  if (value >= 10_000_000) {
    return `${Math.floor(value / 10_000)}万`;
  }

  return value.toLocaleString("zh-CN");
}
