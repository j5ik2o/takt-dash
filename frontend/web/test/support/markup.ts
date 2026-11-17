// 描画結果の HTML 文字列から、属性で指定した要素を取り出す補助。
//
// 同じ名前の要素が入れ子になっても、開始タグと終了タグの対応を数えて外側の要素を丸ごと返す。
// 取り出した要素の中だけで観測し、ページの別の場所の一致に紛れないようにする。
import assert from "node:assert/strict";

const escapeRegExp = (value: string): string => value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

const VOID_ELEMENTS = new Set(["area", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr"]);

type Element = { name: string; openingTag: string; outer: string };

const elementAt = (html: string, start: number, name: string): Element => {
  const openEnd = html.indexOf(">", start) + 1;
  const openingTag = html.slice(start, openEnd);
  if (VOID_ELEMENTS.has(name) || openingTag.endsWith("/>")) {
    return { name, openingTag, outer: openingTag };
  }
  const tags = new RegExp(`<(/?)${name}\\b[^>]*>`, "gi");
  tags.lastIndex = openEnd;
  let depth = 1;
  let match = tags.exec(html);
  while (match !== null) {
    depth += match[1] === "/" ? -1 : 1;
    if (depth === 0) {
      return { name, openingTag, outer: html.slice(start, match.index + match[0].length) };
    }
    match = tags.exec(html);
  }
  assert.fail(`<${name}> の終了タグが見つからない`);
};

// 指定した属性の値が完全に一致する最初の要素を返す。無ければ失敗にする。
export const elementWithAttribute = (html: string, attribute: string, value: string): Element => {
  const pattern = new RegExp(`<([a-z][a-z0-9]*)\\b[^>]*\\s${escapeRegExp(attribute)}="${escapeRegExp(value)}"`, "i");
  const match = pattern.exec(html);
  if (match === null) {
    assert.fail(`${attribute}="${value}" を持つ要素が描画されていない`);
  }
  return elementAt(html, match.index, match[1]!.toLowerCase());
};

// class 属性に指定名をクラスのトークンとして含む要素を、文書順にすべて返す。
export const elementsWithClass = (html: string, className: string): Element[] => {
  const pattern = new RegExp(
    `<([a-z][a-z0-9]*)\\b[^>]*\\sclass="(?:[^"]*\\s)?${escapeRegExp(className)}(?:\\s[^"]*)?"`,
    "gi",
  );
  return Array.from(html.matchAll(pattern), (match) => elementAt(html, match.index ?? 0, match[1]!.toLowerCase()));
};

// タグを除いた表示文字列。実体参照は読みやすさのために代表的なものだけ戻す。
export const textOf = (markup: string): string =>
  markup
    .replace(/<[^>]+>/g, "")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#x27;/g, "'")
    .replace(/&amp;/g, "&");

export const hasClassToken = (openingTag: string, className: string): boolean =>
  new RegExp(`\\sclass="(?:[^"]*\\s)?${escapeRegExp(className)}(?:\\s[^"]*)?"`).test(openingTag);
