// 余白・文字サイズ・角丸の CSS 変数の契約。
//
// globals.css の :root に、余白（margin・padding・gap）、文字サイズ（font-size）、角丸（border-radius）の
// 値を変数として定義し、各規則はその変数を var() で参照する。変数の値は名前の数値と同じにし、
// 参照する変数はすべて :root で定義する。globals.css を直接読み、ブラウザ自動化は使わない。
import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const cssPath = fileURLToPath(new URL("../../app/globals.css", import.meta.url));

const readCss = async (): Promise<string> => (await readFile(cssPath, "utf8")).replace(/\/\*[\s\S]*?\*\//g, "");

type Family = "space" | "fontSize" | "radius";

const familyOfProperty = (property: string): Family | undefined => {
  if (/^(?:margin|padding)(?:-[a-z-]+)?$/.test(property) || /^(?:row-|column-)?gap$/.test(property)) {
    return "space";
  }
  if (property === "font-size") {
    return "fontSize";
  }
  if (property === "border-radius") {
    return "radius";
  }
  return undefined;
};

const familyOfVariable = (name: string): Family | undefined => {
  if (name.startsWith("--space-")) {
    return "space";
  }
  if (name.startsWith("--font-size-")) {
    return "fontSize";
  }
  if (name.startsWith("--radius-")) {
    return "radius";
  }
  return undefined;
};

// 変数名から決まる値。--space-N・--font-size-N・--radius-N は Npx、--radius-pill は 999px。
// それ以外の名前は値を名前から決めないため、照合しない。
const valueFromName = (name: string): string | undefined => {
  if (name === "--radius-pill") {
    return "999px";
  }
  const match = /^--(?:space|font-size|radius)-(\d+)$/.exec(name);
  return match === null ? undefined : `${match[1]}px`;
};

const rootBody = (css: string): string => {
  const match = /:root\s*\{([^{}]*)\}/.exec(css);
  if (match === null) {
    assert.fail(":root の規則が見つからない");
  }
  return match[1] ?? "";
};

const rootVariables = (css: string): Map<string, string> => {
  const variables = new Map<string, string>();
  for (const match of rootBody(css).matchAll(/(--[a-z0-9-]+)\s*:\s*([^;]+);/g)) {
    variables.set(match[1] ?? "", (match[2] ?? "").trim());
  }
  return variables;
};

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

const tokenVariables = (css: string): Map<string, string> =>
  new Map([...rootVariables(css)].filter(([name]) => familyOfVariable(name) !== undefined));

test(":root の余白・文字サイズ・角丸の変数は、名前の数値と同じ値を持つ", async () => {
  const css = await readCss();
  const checked = [...tokenVariables(css)].flatMap(([name, value]) => {
    const expected = valueFromName(name);
    return expected === undefined ? [] : [{ name, value, expected }];
  });
  assert.ok(checked.length > 0, ":root に余白・文字サイズ・角丸の変数が定義されていない");
  const mismatched = checked
    .filter(({ value, expected }) => value !== expected)
    .map(({ name, value, expected }) => `${name}: ${value}（期待値 ${expected}）`);
  assert.deepEqual(mismatched, []);
});

test("参照している余白・文字サイズ・角丸の変数は、すべて :root で定義されている", async () => {
  const css = await readCss();
  const defined = tokenVariables(css);
  const undefinedReferences = [...css.matchAll(/var\(\s*(--[a-z0-9-]+)\s*\)/g)]
    .map((match) => match[1] ?? "")
    .filter((name) => familyOfVariable(name) !== undefined && !defined.has(name));
  assert.deepEqual(undefinedReferences, []);
});

test("余白の変数は余白の宣言、文字サイズの変数は font-size、角丸の変数は border-radius だけで参照する", async () => {
  const css = await readCss();
  const misplaced = ruleDeclarations(css).flatMap((declaration) =>
    [...declaration.value.matchAll(/var\(\s*(--[a-z0-9-]+)\s*\)/g)]
      .map((match) => match[1] ?? "")
      .filter((name) => {
        const family = familyOfVariable(name);
        return family !== undefined && family !== familyOfProperty(declaration.property);
      })
      .map((name) => `${declaration.selectors} { ${declaration.property}: ${declaration.value} } -> ${name}`),
  );
  assert.deepEqual(misplaced, []);
});
