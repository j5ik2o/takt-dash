// アンカーの href と id の契約（issue #51）。
//
// Next.js は遷移時に URL のフラグメントへ decodeURIComponent を適用してから
// DOM の id を探す。id は生の名前から作るため、href 側はフラグメントを
// encodeURIComponent で符号化し、一度復号した結果が id と一致するようにする。
// 符号化しないと、名前に %20 を含む場合は復号で名前が変わって対応する要素を
// 見つけられず、100% のような名前では復号が URIError になって遷移が失敗する。
import test from "node:test";
import assert from "node:assert/strict";
import { projectAnchorId, projectHref, serverAnchorId, serverHref } from "../../src/state/anchors.ts";

const fragmentOf = (href: string): string => href.split("#")[1];

test("プロジェクトの href はフラグメントを一度復号すると id と一致する", () => {
  for (const name of ["corporate-site", "release%20candidate", "100%"]) {
    const fragment = fragmentOf(projectHref(name));
    assert.equal(decodeURIComponent(fragment), projectAnchorId(name), `名前 ${name} のリンク先が一致しない`);
  }
});

test("サーバーの href はフラグメントを一度復号すると id と一致する", () => {
  for (const serverId of ["mini", "release%20candidate", "100%"]) {
    const fragment = fragmentOf(serverHref(serverId));
    assert.equal(decodeURIComponent(fragment), serverAnchorId(serverId), `サーバー ID ${serverId} のリンク先が一致しない`);
  }
});

test("% を含む名前の href はフラグメントが符号化される", () => {
  assert.equal(projectHref("release%20candidate"), "/projects#project-release%2520candidate");
  assert.equal(projectHref("100%"), "/projects#project-100%25");
  assert.equal(serverHref("release%20candidate"), "/#server-release%2520candidate");
  assert.equal(serverHref("100%"), "/#server-100%25");
});

test("通常の名前の href は従来どおり符号化されない", () => {
  assert.equal(projectHref("corporate-site"), "/projects#project-corporate-site");
  assert.equal(serverHref("mini"), "/#server-mini");
});
