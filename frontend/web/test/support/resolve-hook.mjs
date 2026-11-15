// node --test で TypeScript と拡張子なしの相対 import を解決するためのフック。
//
// 生成コード（src/proto/taktdash.ts）は拡張子なしの相対 import を使い、
// tsconfig の paths（@/*）で参照するモジュールもある。Node の ESM 解決は
// 拡張子を補わないため、失敗したときだけ .ts / .tsx を補って再解決する。
// さらに、Node の標準の型ストリップは JSX を扱えないため、.tsx は Next が
// 同梱する SWC で JavaScript へ変換して読み込む。
import { readFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";

const projectRoot = new URL("../../", import.meta.url);
const require = createRequire(import.meta.url);

const candidates = (baseHref) =>
  [".ts", ".tsx", "/index.ts", "/index.tsx"].map((suffix) => `${baseHref}${suffix}`);

export async function resolve(specifier, context, nextResolve) {
  try {
    return await nextResolve(specifier, context);
  } catch (error) {
    if (error?.code !== "ERR_MODULE_NOT_FOUND") {
      throw error;
    }
    const baseHref = specifier.startsWith("@/")
      ? new URL(specifier.slice(2), projectRoot).href
      : /^\.{1,2}\//.test(specifier)
        ? new URL(specifier, context.parentURL).href
        : undefined;
    if (baseHref === undefined) {
      // next は exports を持たないため、next/server のような拡張子なしの
      // パッケージ内パスは .js を補わないと解決できない。
      try {
        return await nextResolve(`${specifier}.js`, context);
      } catch {
        throw error;
      }
    }
    for (const candidate of candidates(baseHref)) {
      if (existsSync(fileURLToPath(candidate))) {
        try {
          return await nextResolve(candidate, context);
        } catch {
          continue;
        }
      }
    }
    throw error;
  }
}

let bindingsReady;

const loadSwc = async () => {
  if (bindingsReady === undefined) {
    const swc = require("next/dist/build/swc");
    bindingsReady = swc.loadBindings().then(() => swc);
  }
  return bindingsReady;
};

export async function load(url, context, nextLoad) {
  if (url.endsWith(".css")) {
    // ページのレイアウトはグローバル CSS を import する。Node は CSS を実行できないため
    // 空のモジュールとして扱い、ページの実行だけを観測できるようにする。
    return { format: "module", source: "", shortCircuit: true };
  }
  if (url.endsWith(".tsx")) {
    const swc = await loadSwc();
    const fileName = fileURLToPath(url);
    const source = await readFile(fileName, "utf8");
    const output = await swc.transform(source, {
      filename: fileName,
      jsc: {
        parser: { syntax: "typescript", tsx: true },
        transform: { react: { runtime: "automatic" } },
        target: "es2020",
      },
      module: { type: "es6" },
    });
    return { format: "module", source: output.code, shortCircuit: true };
  }
  return nextLoad(url, context);
}
