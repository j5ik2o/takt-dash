// issue #317 の色彩と狭い幅・列幅の契約（D29 / D23 / D25 / D31）。
//
// globals.css の :root トークンと対象セレクタの宣言を読み、コントラスト比は色から算出する。
// ブラウザでの実測が必要な項目（実際の重なり・行数・高さ）は e2e/tests/issue-317-layout.spec.ts
// で Playwright の boundingBox() を使い、ここでは選べる方式の範囲で観測できる宣言を固定する。
import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { contrastRatio } from "../support/contrast.ts";

const cssPath = fileURLToPath(new URL("../../app/globals.css", import.meta.url));

const readCss = (): Promise<string> => readFile(cssPath, "utf8");

const escapeRegExp = (value: string): string => value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

type CssRule = { selectors: string; body: string };

// 入れ子のない通常の規則だけを取り出す。@media の中の規則は、そのブロックを
// 切り出してから同じ関数へ渡す。
const cssRules = (css: string): CssRule[] => {
  const rules: CssRule[] = [];
  const pattern = /([^{}]+)\{([^{}]*)\}/g;
  let match = pattern.exec(css);
  while (match !== null) {
    rules.push({ selectors: match[1] ?? "", body: match[2] ?? "" });
    match = pattern.exec(css);
  }
  return rules;
};

const blockAt = (css: string, matchIndex: number): string | undefined => {
  const open = css.indexOf("{", matchIndex);
  if (open < 0) {
    return undefined;
  }
  let depth = 0;
  for (let index = open; index < css.length; index += 1) {
    if (css[index] === "{") {
      depth += 1;
    } else if (css[index] === "}") {
      depth -= 1;
      if (depth === 0) {
        return css.slice(open + 1, index);
      }
    }
  }
  return undefined;
};

const mediaBlockFor = (css: string, width: number): string | undefined => {
  const pattern = new RegExp(`@media[^{]*max-width:\\s*${width}px[^{]*\\{`, "i");
  const match = pattern.exec(css);
  return match === null ? undefined : blockAt(css, match.index);
};

const declaration = (body: string, property: string): string | undefined => {
  const match = new RegExp(`(?:^|;)\\s*${escapeRegExp(property)}\\s*:\\s*([^;]+)`, "i").exec(body);
  return match?.[1]?.trim();
};

// 同じセレクタの宣言は後勝ち（後方の規則が優先）なので、該当する規則を全て集めて最後の値を使う。
const declarationsMatching = (css: string, selector: RegExp, property: string): string[] => {
  const values: string[] = [];
  for (const rule of cssRules(css)) {
    const matched = rule.selectors.split(",").some((token) => selector.test(token.trim()));
    if (!matched) {
      continue;
    }
    const value = declaration(rule.body, property);
    if (value !== undefined) {
      values.push(value);
    }
  }
  return values;
};

const declarationMatching = (css: string, selector: RegExp, property: string): string | undefined => {
  const values = declarationsMatching(css, selector, property);
  return values.length === 0 ? undefined : values[values.length - 1];
};

const requireDeclaration = (css: string, selector: RegExp, property: string, label: string): string => {
  const value = declarationMatching(css, selector, property);
  if (value === undefined) {
    assert.fail(`${label} の ${property} が定義されていない`);
  }
  return value;
};

// 参照トークン（var(--x)）を :root の値へ解決し、hex の色まで畳む。
const rootValue = (css: string, token: string): string => {
  const value = requireDeclaration(css, /^:root$/, token, ":root");
  const alias = /^var\((--[a-z0-9-]+)\)$/i.exec(value);
  if (alias === null) {
    return value;
  }
  return rootValue(css, alias[1]!);
};

// セレクタの色宣言を、:root のトークンを解決した hex で返す。
const resolveColor = (css: string, selector: RegExp, property: string, label: string): string => {
  const value = requireDeclaration(css, selector, property, label);
  const token = /^var\((--[a-z0-9-]+)\)$/i.exec(value);
  if (token === null) {
    assert.fail(`${label} の ${property} がトークン参照ではない: ${value}`);
  }
  return rootValue(css, token[1]!);
};

test("サイドバーの見出しの文字色は白地で 4.5:1 以上のコントラストを持つ", async () => {
  const css = await readCss();
  const color = resolveColor(css, /^\.side-title$/, "color", ".side-title");
  const surface = rootValue(css, "--surface");

  const ratio = contrastRatio(color, surface);
  assert.ok(
    ratio >= 4.5,
    `.side-title の文字色 ${color} は白地 ${surface} に対し ${ratio.toFixed(2)}:1 で 4.5:1 を下回る`,
  );

  // 変更前の --faint（#9aa1a7）は白地で約 2.62:1。同じ値へ戻す変更を拒否する。
  const before = contrastRatio("#9aa1a7", surface);
  assert.ok(ratio > before, `変更前の ${before.toFixed(2)}:1 から改善していない`);
});

test("滞留した項目の待ち時間の文字色は白地と薄橙地のどちらでも 4.5:1 以上になる", async () => {
  const css = await readCss();
  const surface = rootValue(css, "--surface");
  const orangeSoft = rootValue(css, "--orange-soft");

  // 滞留していない項目の待ち時間は通常の地（白）に出す。
  const waitingColor = resolveColor(css, /^\.tray-item\s+\.wait\s+b$/, "color", ".tray-item .wait b");
  const onLight = contrastRatio(waitingColor, surface);
  assert.ok(
    onLight >= 4.5,
    `待ち時間の文字色 ${waitingColor} は白地で ${onLight.toFixed(2)}:1 となり 4.5:1 を下回る`,
  );

  // 滞留した項目の待ち時間は薄橙の地に出す。
  const staleColor = resolveColor(
    css,
    /^\.tray-item\.stale\s+\.wait\s+b$/,
    "color",
    ".tray-item.stale .wait b",
  );
  const onStale = contrastRatio(staleColor, orangeSoft);
  assert.ok(
    onStale >= 4.5,
    `滞留した項目の待ち時間の文字色 ${staleColor} は薄橙地 ${orangeSoft} で ${onStale.toFixed(2)}:1 となり 4.5:1 を下回る`,
  );

  // 変更前の白地 3.58:1（--orange）。白地の待ち時間は変更前より高いコントラストを保つ。
  const beforeLight = contrastRatio("#d9661a", surface);
  assert.ok(onLight > beforeLight, `白地で変更前の ${beforeLight.toFixed(2)}:1 から改善していない`);
  // 滞留したカードの地では、変更前の文字色（--text #262b30、3.22:1）と同程度以上の
  // コントラストを保ち、滞留の強調（橙系の濃色）を維持する。
  assert.ok(onStale >= 4.5, `薄橙地で ${onStale.toFixed(2)}:1 となり 4.5:1 を下回る`);
  assert.notEqual(
    staleColor.toLowerCase(),
    "#262b30",
    "滞留の強調を通常の文字色（--text）に戻さない",
  );

  // 滞留項目の地色が薄橙であることを前提条件として固定する（地色が変わると上の算出が無効になる）。
  const staleBackground = declarationMatching(css, /^\.tray-item\.stale$/, "background");
  assert.match(
    staleBackground ?? "",
    /var\(--orange-soft\)/,
    "滞留カードの地色は --orange-soft のまま維持する",
  );
});

test("狭い幅でも操作ボタンの押せる高さを 44px 以上にする", async () => {
  const css = await readCss();
  const minHeight = declarationMatching(css, /^\.actions\s+button$/, "min-height");
  assert.equal(minHeight, "44px", "操作ボタンに 44px 以上の最小高さを与える");
});

test("アクティビティの種類の列は文字を隣の列へ重ねない幅の指定を持つ", async () => {
  const css = await readCss();

  // 重なりの有無は 1280px・760px・390px の実描画で測る（e2e/tests/issue-317-layout.spec.ts）。
  // 要件は「重なる場合だけ列の幅を中身に合わせて直す」であり、どの幅でも重ならない構成を
  // 固定する。種類は2列目に置き、nowrap で1行に保つ前提のもとで、2列目が固定幅だと
  // 長い種類名がはみ出して本文へ重なるため、中身に合わせて伸縮する幅を必須にする。
  const shrinkable = /^(auto|minmax\(|max-content|fit-content|[\d.]+fr$)/;

  // 広い幅の .ev。
  const columns = requireDeclaration(css, /^\.ev$/, "grid-template-columns", ".ev");
  const wideTracks = columns.trim().split(/\s+/);
  assert.ok(
    shrinkable.test(wideTracks[1] ?? ""),
    `.ev の2列目（種類）が固定幅 ${wideTracks[1] ?? "(なし)"} で、長い種類名が隣の列へ重なる`,
  );

  // 1280px 以下では列構成が変わる（.ev の再定義）。ここも種類の列が固定幅だと重なる。
  const desktop = mediaBlockFor(css, 1280);
  if (desktop !== undefined) {
    const narrowColumns = declarationMatching(desktop, /^\.ev$/, "grid-template-columns");
    if (narrowColumns !== undefined) {
      const tracks = narrowColumns.trim().split(/\s+/);
      assert.ok(
        shrinkable.test(tracks[1] ?? ""),
        `1280px 以下の .ev の2列目が固定幅 ${tracks[1] ?? "(なし)"} で、長い種類名が隣の列へ重なる`,
      );
    }
  }
});

test("サーバー一覧の行はサーバー名が1行に収まる折り返しの制御を持つ", async () => {
  const css = await readCss();

  // D31: 1280px でサーバー名が2行に折り返さない。実測は e2e で行うが、名前が折り返す
  // 原因になる overflow-wrap を残したまま nowrap を足すと噛み合わないため、名称の要素へ
  // 折り返しを止める宣言があることを固定する。
  const anchorWrap = declarationMatching(css, /^\.srv-list\s+a$/, "white-space");
  const nameWrap = declarationMatching(css, /^\.srv-list\s+\.nm$/, "white-space");
  assert.equal(
    anchorWrap ?? nameWrap,
    "nowrap",
    "サーバー名を1行に保つ white-space: nowrap が無く、長い名称で2行になる",
  );

  // 折り返しを止めたまま名前を切らないよう、はみ出しを抑える指定（省略記号・最小幅 0）を持つ。
  const overflow = declarationMatching(css, /^\.srv-list\s+a$/, "overflow");
  const ellipsis = declarationMatching(css, /^\.srv-list\s+a$/, "text-overflow");
  const minWidth = declarationMatching(css, /^\.srv-list\s+a$/, "min-width");
  assert.ok(
    overflow === "hidden" || ellipsis === "ellipsis" || minWidth === "0",
    "サーバー名を1行に収める際に、あふれを抑える指定が無い",
  );
});
