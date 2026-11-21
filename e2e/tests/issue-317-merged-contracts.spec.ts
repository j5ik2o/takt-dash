// 衝突した E2E 検証（D23 / D26 / D31）の観測点が、合流後も維持されていることを固定する。
//
// 各 E2E は実 Chrome・headless で `make e2e` のときに動く。ここではブラウザを起動せず、
// 検証対象の要素・期待値が spec から消えていないことだけを観測する。合流で main 側を
// 採用したときに、goal 側の最小差分の観測点まで落ちていないかを検出する。
import { test, expect } from "@playwright/test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

// この spec と同じディレクトリの検証対象 spec を読む。e2e は CommonJS として読み込まれ、
// import.meta を使えないため、__dirname から解決する。
const spec = readFileSync(join(__dirname, "issue-317-layout.spec.ts"), "utf8");

const matched = (pattern: RegExp): boolean => pattern.test(spec);

// D26: 狭い幅ではサイドバー下端を details.side-foot として開閉し、本文を押し下げない。
test("D26 の E2E は details.side-foot を開閉して本文の位置を測る", () => {
  expect(matched(/page\.locator\("details\.side-foot"\)/), "E2E は details.side-foot を観測する").toBe(true);
});

// D31: 1280px でサーバー名が1行に収まることを、長いサーバー名のスタックで測る。
test("D31 の E2E は longServerStack で長いサーバー名の1行表示を測る", () => {
  expect(matched(/longServerStack: stack/), "E2E は longServerStack fixture を受け取る").toBe(true);
});

// D23: アクティビティの種類列が隣の列へ重ならないことを測る。
test("D23 の E2E は種類列の重なりを測る", () => {
  expect(matched(/種類/), "E2E は種類列を対象にしている").toBe(true);
});
