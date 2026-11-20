// 色のコントラスト比（WCAG 2.x）を算出する小さな補助関数。
//
// globals.css の :root トークンが hex で定義されている前提で、文字色と地色の
// 相対輝度から (L1 + 0.05) / (L2 + 0.05) を返す。色の意味づけは持たず、
// 呼び出したテストが対象のセレクタと地色を指定する。
const channel = (value: number): number => (value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4);

const relativeLuminance = (hex: string): number => {
  const raw = hex.trim().replace(/^#/, "");
  const normalized =
    raw.length === 3 ? raw.split("").map((digit) => digit + digit).join("").toLowerCase() : raw;
  if (!/^[0-9a-fA-F]{6}$/.test(normalized)) {
    throw new Error(`hex の色ではない: ${hex}`);
  }
  const [r, g, b] = [0, 2, 4].map((offset) => channel(parseInt(normalized.slice(offset, offset + 2), 16) / 255));
  return 0.2126 * r! + 0.7152 * g! + 0.0722 * b!;
};

export const contrastRatio = (foreground: string, background: string): number => {
  const first = relativeLuminance(foreground);
  const second = relativeLuminance(background);
  const lighter = Math.max(first, second);
  const darker = Math.min(first, second);
  return (lighter + 0.05) / (darker + 0.05);
};
