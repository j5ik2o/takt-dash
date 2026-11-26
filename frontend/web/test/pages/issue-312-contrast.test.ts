// issue #312 の警告文字色のコントラスト契約（WCAG AA 4.5:1 以上）。
//
// globals.css の :root トークンと対象セレクタの宣言を読み、文字色と地色の
// コントラスト比を test/support/contrast.ts の contrastRatio で算出する。
import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { contrastRatio } from "../support/contrast.ts";

const cssPath = fileURLToPath(new URL("../../app/globals.css", import.meta.url));

const readCss = (): Promise<string> => readFile(cssPath, "utf8");

const escapeRegExp = (value: string): string => value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

type CssRule = { selectors: string; body: string };

const stripComments = (css: string): string => css.replace(/\/\*[\s\S]*?\*\//g, "");

const cssRules = (css: string): CssRule[] => {
  const rules: CssRule[] = [];
  const source = stripComments(css);
  const pattern = /([^{}]+)\{([^{}]*)\}/g;
  let match = pattern.exec(source);
  while (match !== null) {
    rules.push({ selectors: match[1] ?? "", body: match[2] ?? "" });
    match = pattern.exec(source);
  }
  return rules;
};

// 同じ規則内に同名プロパティが複数ある場合も、CSS の後勝ちに合わせて最後の宣言を返す。
const declaration = (body: string, property: string): string | undefined => {
  const pattern = new RegExp(`(?:^|;)\\s*${escapeRegExp(property)}\\s*:\\s*([^;]+)`, "gi");
  let latest: string | undefined;
  for (const match of body.matchAll(pattern)) {
    latest = match[1]?.trim();
  }
  return latest;
};

const declarationMatching = (css: string, selector: RegExp, property: string): string | undefined => {
  let latest: string | undefined;
  for (const rule of cssRules(css)) {
    const matched = rule.selectors.split(",").some((token) => selector.test(token.trim()));
    if (!matched) {
      continue;
    }
    const value = declaration(rule.body, property);
    if (value !== undefined) {
      latest = value;
    }
  }
  return latest;
};

const requireDeclaration = (css: string, selector: RegExp, property: string, label: string): string => {
  const value = declarationMatching(css, selector, property);
  if (value === undefined) {
    assert.fail(`${label} の ${property} が定義されていない`);
  }
  return value;
};

// 参照トークン（var(--x)）を :root の値へ解決し、hex の色まで畳む。
const rootValue = (css: string, token: string, seen = new Set<string>()): string => {
  if (seen.has(token)) {
    assert.fail(`:root のトークン参照が循環している: ${token}`);
  }
  seen.add(token);
  const value = requireDeclaration(css, /^:root$/, token, ":root");
  const alias = /^var\(\s*(--[a-z0-9-]+)\s*\)$/i.exec(value);
  if (alias === null) {
    return value;
  }
  return rootValue(css, alias[1]!, seen);
};

const resolveTokenColor = (css: string, value: string, label: string): string => {
  const token = /^var\(\s*(--[a-z0-9-]+)\s*\)$/i.exec(value);
  if (token === null) {
    assert.fail(`${label} がトークン参照ではない: ${value}`);
  }
  return rootValue(css, token[1]!, new Set());
};

const resolveColor = (css: string, selector: RegExp, property: string, label: string): string =>
  resolveTokenColor(css, requireDeclaration(css, selector, property, label), `${label} の ${property}`);

test("書き込み失敗の文は薄橙の地で 4.5:1 以上のコントラストを持つ", async () => {
  const css = await readCss();
  const color = resolveColor(css, /^\.decision-results\s+\.result\.error$/, "color", ".decision-results .result.error");
  const background = resolveColor(
    css,
    /^\.decision-results\s+\.result\.error$/,
    "background",
    ".decision-results .result.error",
  );

  const ratio = contrastRatio(color, background);
  assert.ok(
    ratio >= 4.5,
    `.decision-results .result.error の文字色 ${color} は地色 ${background} に対し ${ratio.toFixed(2)}:1 で 4.5:1 を下回る`,
  );

  // 変更前の #d9661a（--warn / --orange）は --warn-bg（#fdf1e7）上で 3.22:1。同じ値へ戻す変更を拒否する。
  const before = contrastRatio("#d9661a", background);
  assert.ok(ratio > before, `変更前の ${before.toFixed(2)}:1 から改善していない`);
});

test("会話の注意文はメッセージの白地で 4.5:1 以上のコントラストを持つ", async () => {
  const css = await readCss();
  const color = resolveColor(css, /^\.conversation-notice$/, "color", ".conversation-notice");
  const background = resolveColor(css, /^\.conversation-message$/, "background", ".conversation-message");

  const ratio = contrastRatio(color, background);
  assert.ok(
    ratio >= 4.5,
    `.conversation-notice の文字色 ${color} は地色 ${background} に対し ${ratio.toFixed(2)}:1 で 4.5:1 を下回る`,
  );

  // 変更前の #d9661a（--warn / --orange）は --surface（#fff）上で 3.58:1。同じ値へ戻す変更を拒否する。
  const before = contrastRatio("#d9661a", background);
  assert.ok(ratio > before, `変更前の ${before.toFixed(2)}:1 から改善していない`);
});

test("--warn または --orange を文字色に使う規則があれば 4.5:1 以上のコントラストを持つ", async () => {
  const css = await readCss();
  const surfaceHex = rootValue(css, "--surface");

  const offending: string[] = [];
  for (const rule of cssRules(css)) {
    const color = declaration(rule.body, "color");
    if (color === undefined) {
      continue;
    }
    const token = /^var\(\s*(--(?:warn|orange))\s*\)$/i.exec(color);
    if (token === null) {
      continue;
    }
    const fgHex = rootValue(css, token[1]!, new Set());
    const bgDecl = declaration(rule.body, "background") ?? declaration(rule.body, "background-color");
    const bgToken = bgDecl === undefined ? null : /^var\(\s*(--[a-z0-9-]+)\s*\)$/i.exec(bgDecl);
    const bgHex = bgToken === null ? surfaceHex : rootValue(css, bgToken[1]!, new Set());
    const ratio = contrastRatio(fgHex, bgHex);
    if (ratio < 4.5) {
      offending.push(`${rule.selectors.trim()} { color: ${color}; } (${fgHex} on ${bgHex}: ${ratio.toFixed(2)}:1)`);
    }
  }
  assert.deepEqual(offending, [], `4.5:1 に届かない警告文字色を使う規則がある: ${offending.join(", ")}`);
});
