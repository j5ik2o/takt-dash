// レイアウトの基本部品の契約。
//
// globals.css に、縦積み（.l-stack）、横並び（.l-cluster）、主と従の2カラム（.l-split）、
// 全幅の表の枠（.l-table）、節の見出しと本文の組（.l-section）を、@media の外の規則として定義する。
// 部品の規則は、余白に --space-*、文字サイズに --font-size-*、角丸に --radius-* の変数を使い、
// 長さと色を直書きせず、列数を固定しない。globals.css を直接読み、ブラウザ自動化は使わない。
import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const cssPath = fileURLToPath(new URL("../../app/globals.css", import.meta.url));

const readCss = async (): Promise<string> => (await readFile(cssPath, "utf8")).replace(/\/\*[\s\S]*?\*\//g, "");

const PARTS = ["l-stack", "l-cluster", "l-split", "l-table", "l-section"] as const;

type Declaration = { selectors: string; property: string; value: string };

// :root 以外の規則の宣言。@media の中の規則も、内側の規則として取り出す。
const ruleDeclarations = (css: string): Declaration[] => {
  const declarations: Declaration[] = [];
  for (const rule of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const selectors = (rule[1] ?? "").trim();
    if (selectors === ":root") {
      continue;
    }
    for (const match of (rule[2] ?? "").matchAll(/([a-z-]+)\s*:\s*([^;]+)/g)) {
      declarations.push({ selectors, property: match[1] ?? "", value: (match[2] ?? "").trim() });
    }
  }
  return declarations;
};

// 開き括弧から対応する閉じ括弧までの範囲（括弧を含む）。
const blockRange = (css: string, from: number): { start: number; end: number } | undefined => {
  const open = css.indexOf("{", from);
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
        return { start: open, end: index + 1 };
      }
    }
  }
  return undefined;
};

// @media のブロックを、中身ごと取り除いた CSS と、取り除いたブロックの中身。
const splitMedia = (css: string): { outside: string; media: string[] } => {
  let outside = "";
  const media: string[] = [];
  let cursor = 0;
  for (;;) {
    const at = css.indexOf("@media", cursor);
    if (at < 0) {
      break;
    }
    const range = blockRange(css, at);
    if (range === undefined) {
      assert.fail("@media のブロックの終わりが見つからない");
    }
    outside += css.slice(cursor, at);
    media.push(css.slice(range.start + 1, range.end - 1));
    cursor = range.end;
  }
  outside += css.slice(cursor);
  return { outside, media };
};

const selectorTokens = (selectors: string): string[] => selectors.split(",").map((token) => token.trim());

const classesIn = (selector: string): string[] =>
  [...selector.matchAll(/\.(-?[_a-zA-Z][_a-zA-Z0-9-]*)/g)].map((match) => match[1] ?? "");

const isPartSelector = (selector: string): boolean => classesIn(selector).some((name) => name.startsWith("l-"));

const partDeclarations = (css: string): Declaration[] =>
  ruleDeclarations(css).filter((declaration) => selectorTokens(declaration.selectors).some(isPartSelector));

const rootVariableNames = (css: string): Set<string> => {
  const match = /:root\s*\{([^{}]*)\}/.exec(css);
  if (match === null) {
    assert.fail(":root の規則が見つからない");
  }
  return new Set([...(match[1] ?? "").matchAll(/(--[a-z0-9-]+)\s*:/g)].map((found) => found[1] ?? ""));
};

test("5つの基本部品は、@media の外にある単独セレクタの規則で display を宣言している", async () => {
  const { outside } = splitMedia(await readCss());
  const declarations = ruleDeclarations(outside);
  const missing = PARTS.filter(
    (part) =>
      !declarations.some(
        (declaration) =>
          selectorTokens(declaration.selectors).includes(`.${part}`) && declaration.property === "display",
      ),
  );
  assert.deepEqual(missing, []);
});

test("globals.css で定義する l- の部品クラスは、5つの基本部品だけである", async () => {
  const css = await readCss();
  const defined = new Set(
    ruleDeclarations(css).flatMap((declaration) =>
      selectorTokens(declaration.selectors).flatMap(classesIn).filter((name) => name.startsWith("l-")),
    ),
  );
  assert.deepEqual([...defined].sort(), [...PARTS].sort());
});

test("基本部品の規則は @media の中に置かない", async () => {
  const { media } = splitMedia(await readCss());
  const inMedia = media.flatMap((block) =>
    ruleDeclarations(block)
      .filter((declaration) => selectorTokens(declaration.selectors).some(isPartSelector))
      .map((declaration) => declaration.selectors),
  );
  assert.deepEqual([...new Set(inMedia)], []);
});

test("基本部品の余白は --space-* か 0、文字サイズは --font-size-*、角丸は --radius-* だけを値に持つ", async () => {
  const css = await readCss();
  const allowed = (property: string): RegExp | undefined => {
    if (/^(?:margin|padding)(?:-[a-z-]+)?$/.test(property) || /^(?:row-|column-)?gap$/.test(property)) {
      return /^(?:var\(--space-[a-z0-9-]+\)|0)$/;
    }
    if (property === "font-size") {
      return /^var\(--font-size-[a-z0-9-]+\)$/;
    }
    if (property === "border-radius") {
      return /^var\(--radius-[a-z0-9-]+\)$/;
    }
    return undefined;
  };
  const checked = partDeclarations(css).filter((declaration) => allowed(declaration.property) !== undefined);
  assert.ok(checked.length > 0, "基本部品の規則に余白・文字サイズ・角丸の宣言がない");
  const violations = checked
    .filter((declaration) => {
      const pattern = allowed(declaration.property);
      return declaration.value.split(/\s+/).some((token) => !(pattern?.test(token) ?? true));
    })
    .map((declaration) => `${declaration.selectors} { ${declaration.property}: ${declaration.value} }`);
  assert.deepEqual(violations, []);
});

test("基本部品の規則は、罫線の 1px を除いて長さを直書きしない", async () => {
  const css = await readCss();
  const violations = partDeclarations(css)
    .filter((declaration) => {
      const value = /^border(?:-[a-z]+)?$/.test(declaration.property)
        ? declaration.value.replace(/(?:^|\s)1px(?=\s)/g, " ")
        : declaration.value;
      return /(?:^|[\s(,])-?\d*\.?\d+(?:px|rem|em|vw|vh)\b/.test(value);
    })
    .map((declaration) => `${declaration.selectors} { ${declaration.property}: ${declaration.value} }`);
  assert.deepEqual(violations, []);
});

test("基本部品の規則は、色を直書きしない", async () => {
  const css = await readCss();
  const violations = partDeclarations(css)
    .filter((declaration) => /#[0-9a-fA-F]{3,8}\b|\b(?:rgba?|hsla?)\(/.test(declaration.value))
    .map((declaration) => `${declaration.selectors} { ${declaration.property}: ${declaration.value} }`);
  assert.deepEqual(violations, []);
});

test("基本部品の規則は、列数を数で固定しない", async () => {
  const css = await readCss();
  const violations = partDeclarations(css)
    .filter(
      (declaration) =>
        /^grid-template(?:-columns)?$/.test(declaration.property) && /repeat\(\s*\d+\s*,/.test(declaration.value),
    )
    .map((declaration) => `${declaration.selectors} { ${declaration.property}: ${declaration.value} }`);
  assert.deepEqual(violations, []);
});

test("基本部品の規則が参照する変数は、すべて :root で定義されている", async () => {
  const css = await readCss();
  const defined = rootVariableNames(css);
  const undefinedReferences = partDeclarations(css).flatMap((declaration) =>
    [...declaration.value.matchAll(/var\(\s*(--[a-z0-9-]+)/g)]
      .map((match) => match[1] ?? "")
      .filter((name) => !defined.has(name))
      .map((name) => `${declaration.selectors} { ${declaration.property}: ${declaration.value} } -> ${name}`),
  );
  assert.deepEqual(undefinedReferences, []);
});
