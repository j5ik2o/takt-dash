import assert from "node:assert/strict";

export function assertUnreadRunList(root: ParentNode): void {
  const list = root.querySelector('section[aria-labelledby="h-runs"]');
  assert.ok(list, "run 一覧を描画する");
  const content = list.textContent ?? "";
  assert.match(content, /run.*(?:不明|分かりません)/, "run 一覧内で存在不明と分かる");
  assert.doesNotMatch(content, /走っている\s*run\s*はありません|run\s*0\s*件/, "未読の run 一覧で空と断定しない");
}

export function assertRejectsUnreadEmptyClaim(root: Element): void {
  const copy = root.ownerDocument.createElement("div");
  copy.innerHTML = root.innerHTML;
  const list = copy.querySelector('section[aria-labelledby="h-runs"]');
  assert.ok(list);
  const empty = root.ownerDocument.createElement("p");
  empty.textContent = "走っている run はありません。";
  list.append(empty);
  assert.throws(() => assertUnreadRunList(copy), { code: "ERR_ASSERTION" });
}
