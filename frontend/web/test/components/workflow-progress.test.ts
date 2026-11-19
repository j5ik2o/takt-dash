import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { renderToStaticMarkup } from "react-dom/server";
import { JSDOM } from "jsdom";
import { WorkflowProgress as Progress, WorkflowNodeState as State, DashboardState } from "../../src/proto/taktdash.ts";
import { WorkflowProgress, WorkflowProgressSummary } from "../../src/components/workflow-progress.tsx";
import { RunDetailContent } from "../../src/components/run-detail-content.tsx";
import { OverviewContent } from "../../src/components/overview-content.tsx";
import { buildOverviewView } from "../../src/state/dashboard.ts";
import { restoreDashboardState } from "../../src/state/restore-state.ts";
import { createDashboardState, createRunDetailResponse } from "../support/fixtures.ts";

const now = new Date("2026-11-19T00:00:00Z");

function progress(): Progress {
  return Progress.create({
    initialStep: "plan", currentStep: "gate", currentPosition: { value: 3 },
    definition: ["plan", "develop", "gate", "audit", "fix_gate_fmt", "fix_gate_test", "fix_audit"].map((name) => ({ name, kind: "agent" })),
    backbone: ["plan", "develop", "gate", "audit"],
    nodes: [
      { name: "plan", state: State.WORKFLOW_NODE_STATE_COMPLETED, executionCount: { value: 1 }, logIndex: 0 },
      { name: "develop", state: State.WORKFLOW_NODE_STATE_COMPLETED, executionCount: { value: 1 } },
      { name: "gate", state: State.WORKFLOW_NODE_STATE_RUNNING, executionCount: { value: 3 }, startedAt: new Date(now.getTime() - 60_000) },
      { name: "audit", state: State.WORKFLOW_NODE_STATE_UNVISITED, executionCount: { value: 0 } },
      { name: "fix_gate_fmt", state: State.WORKFLOW_NODE_STATE_COMPLETED, executionCount: { value: 1 }, logIndex: 1 },
      { name: "fix_gate_test", state: State.WORKFLOW_NODE_STATE_UNVISITED, executionCount: { value: 0 } },
      { name: "fix_audit", state: State.WORKFLOW_NODE_STATE_UNVISITED, executionCount: { value: 0 } },
    ],
    loops: [
      { source: "gate", target: "gate", steps: ["fix_gate_fmt", "fix_gate_test"], executionCount: { value: 1 } },
      { source: "audit", target: "gate", steps: ["fix_audit"], executionCount: { value: 0 } },
    ],
    connections: [
      {source:"plan",target:"develop",traversed:true},
      {source:"develop",target:"gate",traversed:true},
      {source:"gate",target:"audit",traversed:false},
    ],
    edges: [
      { source: "plan", target: "develop", traversals: { value: 1 } },
      { source: "develop", target: "gate", traversals: { value: 1 } },
      { source: "gate", target: "audit", traversals: { value: 0 } },
      { source: "gate", target: "develop", backward: true, traversals: { value: 2 } },
      { source: "audit", target: "plan", backward: true, traversals: { value: 0 } },
      { source: "audit", target: "develop", ruleIndex: 1, backward: true },
    ],
  });
}

for (const [state, label] of [
  [State.WORKFLOW_NODE_STATE_COMPLETED, "✓ 完了"],
  [State.WORKFLOW_NODE_STATE_RUNNING, "実行中"],
  [State.WORKFLOW_NODE_STATE_UNVISITED, "未到達"],
  [State.WORKFLOW_NODE_STATE_SKIPPED, "通らなかった"],
  [State.WORKFLOW_NODE_STATE_FAILED, "× 失敗"],
  [State.WORKFLOW_NODE_STATE_ABORTED, "中止"],
] as const) {
  test(`進行図は ${label} を文字で区別し、到達回数を併記する`, () => {
    const data = Progress.create({ definition: [{ name: "gate" }], nodes: [{ name: "gate", state, executionCount: { value: 3 } }], terminal: state === State.WORKFLOW_NODE_STATE_ABORTED ? "ABORT" : undefined, backbone: ["gate"] });
    const html = renderToStaticMarkup(createElement(WorkflowProgress, { progress: data, now }));
    assert.ok(html.includes(label));
    assert.match(html, state === State.WORKFLOW_NODE_STATE_RUNNING ? /3 回目/ : /3 回/);
    if (state === State.WORKFLOW_NODE_STATE_ABORTED) assert.match(html, /ABORT/);
  });
}

test("親が後続へ進んでも中止した呼出しを完了へ変えずに表示する", () => {
  const data = progress();
  data.nodes = data.nodes.map((node) => node.name === "develop" ? { ...node, state: State.WORKFLOW_NODE_STATE_ABORTED } : node);
  const dom = new JSDOM(renderToStaticMarkup(createElement(WorkflowProgress, { progress: data, now })));
  try {
    const call = dom.window.document.querySelector('[data-workflow-node="develop"]')!;
    assert.ok([...call.querySelectorAll("span")].some((span) => span.textContent === "中止"));
    assert.equal(call.getAttribute("aria-current"), null);
    assert.equal(dom.window.document.querySelector('[data-workflow-node="gate"]')!.getAttribute("aria-current"), "step");
  } finally { dom.window.close(); }
});

test("輪は閉じた要約を持ち、実行済みの短縮名・正式名・未実行名と異なる戻り先を表示する", () => {
  const dom = new JSDOM(renderToStaticMarkup(createElement(WorkflowProgress, { progress: progress(), now })));
  try {
    const repairs = [...dom.window.document.querySelectorAll("details")];
    assert.equal(repairs.length, 2);
    assert.ok(repairs.every((repair) => !repair.open));
    assert.equal(repairs[0]!.closest("[data-workflow-source]")?.getAttribute("data-workflow-source"),"gate");
    assert.match(repairs[0]!.querySelector("summary")!.textContent!, /修正 1 回/);
    assert.match(repairs[0]!.querySelector("summary")!.textContent!, /ほか 1 種は未実行/);
    const link = repairs[0]!.querySelector('a[href="#run-step-log-1"]');
    assert.equal(link?.textContent, "fmt");
    assert.equal(link?.parentElement?.getAttribute("title"), "fix_gate_fmt");
    assert.match(repairs[0]!.textContent!, /未実行：fix_gate_test/);
    assert.equal(repairs[0]!.querySelectorAll(".workflow-node").length, 1);
    assert.match(repairs[1]!.textContent!, /gate へ戻る/);
  } finally { dom.window.close(); }
});

test("進行図は通過辺と未通過辺を区別し、通った差し戻しだけを回数付きで描く", () => {
  const dom = new JSDOM(renderToStaticMarkup(createElement(WorkflowProgress, { progress: progress(), now })));
  try {
    assert.equal(dom.window.document.querySelector('[aria-label="plan → develop：通過済み"]')?.classList.contains("passed"), true);
    assert.equal(dom.window.document.querySelector('[aria-label="gate → audit：未通過"]')?.classList.contains("pending"), true);
    const returns = [...dom.window.document.querySelectorAll(".workflow-return")];
    assert.equal(returns.length, 1);
    assert.match(returns[0]!.textContent!, /gate → develop：差し戻し 2 回/);
  } finally { dom.window.close(); }
});

test("親ノードの活動名と回数を表示し、線なしの縮退でも既知のノードを残す", () => {
  const data = Progress.create({ definition: [{ name: "develop" }, { name: "review" }], nodes: [{ name: "develop", state: State.WORKFLOW_NODE_STATE_RUNNING, executionCount: { value: 1 }, activeChildren: ["plan"] }, { name: "review", state: State.WORKFLOW_NODE_STATE_UNKNOWN }], edges: [{ source: "develop", target: "review" }] });
  const dom = new JSDOM(renderToStaticMarkup(createElement(WorkflowProgress, { progress: data, now })));
  try {
    assert.match(dom.window.document.body.textContent!, /develop ▸ plan/);
    assert.match(dom.window.document.body.textContent!, /回数は不明です/);
    assert.equal(dom.window.document.querySelectorAll(".workflow-node").length, 2);
    assert.equal(dom.window.document.querySelectorAll(".workflow-edge").length, 0);
  } finally { dom.window.close(); }
});

test("詳細では図がログ一覧の前にあり、記録済みノードのリンクが対応ログへ移動する", () => {
  const data = { ...createRunDetailResponse(), progress: progress() };
  const dom = new JSDOM(renderToStaticMarkup(createElement(RunDetailContent, { detail: data, now })));
  try {
    const graph = dom.window.document.querySelector('[aria-label="ワークフローの進行"]')!;
    const logs = dom.window.document.getElementById("h-steps")!;
    assert.ok(graph.compareDocumentPosition(logs) & dom.window.Node.DOCUMENT_POSITION_FOLLOWING);
    for (const link of graph.querySelectorAll("a")) assert.ok(dom.window.document.querySelector(link.getAttribute("href")!));
    assert.equal(graph.querySelector('[title="audit"] a'), null);
    assert.match(graph.textContent!, /4 段階中 3 つ目、gate、3 回目/);
    assert.match(graph.textContent!, /経過 1分/);
  } finally { dom.window.close(); }
});

test("分類できないステップだけを別行へ残し、判別できた背骨と輪を消さない", () => {
  const data = progress();
  data.otherSteps = ["unclassified"];
  data.nodes.push({ name: "unclassified", state: State.WORKFLOW_NODE_STATE_UNKNOWN, executionCount: undefined, startedAt: undefined, activeChildren: [], logIndex: undefined });
  const dom = new JSDOM(renderToStaticMarkup(createElement(WorkflowProgress, { progress: data, now })));
  try {
    assert.equal(dom.window.document.querySelectorAll(".workflow-main-node > .workflow-node").length, 4);
    assert.equal(dom.window.document.querySelectorAll("details.workflow-repair").length, 2);
    const heading = [...dom.window.document.querySelectorAll("h3")].find((element) => element.textContent === "そのほかの step");
    assert.equal(heading?.parentElement?.querySelectorAll(".workflow-node").length, 1);
    assert.match(heading?.parentElement?.textContent ?? "", /unclassified.*状態は不明です.*回数は不明です/);
  } finally { dom.window.close(); }
});

test("同じ図と概要カードが応答更新後の現在名・辺・輪の回数へ追従する", async () => {
  const dom = new JSDOM("<div id='root'></div>", { url: "http://localhost" });
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
  const previousDocument = Object.getOwnPropertyDescriptor(globalThis, "document");
  const previousSelf = Object.getOwnPropertyDescriptor(globalThis, "self");
  Object.defineProperty(globalThis, "window", { configurable: true, value: dom.window });
  Object.defineProperty(globalThis, "document", { configurable: true, value: dom.window.document });
  Object.defineProperty(globalThis, "self", { configurable: true, value: dom.window });
  const scope = globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT?: boolean };
  const previousAct = scope.IS_REACT_ACT_ENVIRONMENT;
  scope.IS_REACT_ACT_ENVIRONMENT = true;
  const root = createRoot(dom.window.document.getElementById("root")!);
  try {
    let originalGraph: Element | null = null;
    let originalCard: Element | null = null;
    for (const [step, count] of [["gate", 1], ["fix_gate_fmt", 2], ["gate", 2]] as const) {
      const data = Progress.create({ ...progress(), connections: progress().connections.map((edge) => ({...edge,traversed:edge.source === "gate" && count === 1 ? false : true})), currentStep: step, loops: [{ source: "gate", target: "gate", steps: ["fix_gate_fmt"], executionCount: { value: count } }], edges: progress().edges.map((edge) => ({ ...edge, traversals: { value: edge.source === "gate" && edge.target === "audit" && count === 1 ? 0 : count } })) });
      const base = createDashboardState();
      const state = DashboardState.create({ ...base, runs: [{ ...base.runs[0]!, currentStepName: step, progress: data }] });
      const restored = restoreDashboardState(DashboardState.toJSON(state));
      await act(async () => { root.render(createElement("div", null,
        createElement(WorkflowProgress, { progress: data, now }),
        createElement(OverviewContent, { view: buildOverviewView(restored, now), disconnected: false }),
      )); });
      const graph = dom.window.document.querySelector('[aria-label="ワークフローの進行"]')!;
      const card = dom.window.document.querySelector(".run-row")!;
      if (originalGraph === null) originalGraph = graph; else assert.equal(graph, originalGraph);
      if (originalCard === null) originalCard = card; else assert.equal(card, originalCard);
      assert.match(card.textContent!, /3\/4/);
      assert.match(card.textContent!, new RegExp(`修正 ${count} 回`));
      assert.ok(card.textContent!.includes(step));
      assert.equal(graph.querySelector('[aria-label="plan → develop：通過済み"]')?.classList.contains("passed"), true);
      assert.ok(graph.querySelector(`[aria-label="gate → audit：${count === 1 ? "未通過" : "通過済み"}"]`));
      assert.match(graph.querySelector("summary")!.textContent!, new RegExp(`修正 ${count} 回`));
    }
  } finally {
    await act(async () => root.unmount());
    if (previousWindow) Object.defineProperty(globalThis, "window", previousWindow); else Reflect.deleteProperty(globalThis, "window");
    if (previousDocument) Object.defineProperty(globalThis, "document", previousDocument); else Reflect.deleteProperty(globalThis, "document");
    if (previousSelf) Object.defineProperty(globalThis, "self", previousSelf); else Reflect.deleteProperty(globalThis, "self");
    scope.IS_REACT_ACT_ENVIRONMENT = previousAct;
    dom.window.close();
  }
});

test("概要の短い表示は backend の背骨位置を使う", () => {
  const html = renderToStaticMarkup(createElement(WorkflowProgressSummary, { progress: progress() }));
  assert.match(html, /3\/4/);
  assert.match(html, /修正 1 回/);
});

for (const traversed of [true,false,undefined]) {
  test(`背骨・終端・輪は通過状態 ${traversed} と代替文を一致させる`, async () => {
    const data = progress();
    data.connections = [["plan","develop"],["audit","COMPLETE"],["gate","fix_gate_fmt"],["fix_gate_fmt","gate"]].map(([source,target]) => ({source:source!,target:target!,traversed}));
    const dom = new JSDOM(renderToStaticMarkup(createElement(WorkflowProgress,{progress:data,now})));
    for (const repair of dom.window.document.querySelectorAll("details")) repair.open=true;
    const css=await readFile(new URL("../../app/globals.css",import.meta.url),"utf8");
    const stylesheet=dom.window.document.createElement("style"); stylesheet.textContent=css; dom.window.document.head.append(stylesheet);
    try {
      const expected = traversed === undefined ? "unknown" : traversed ? "passed" : "pending";
      const label = traversed === undefined ? "通過は不明です" : traversed ? "通過済み" : "未通過";
      for (const {source,target} of data.connections) {
        assert.ok(dom.window.document.querySelector(`[aria-label="${source} → ${target}：${label}"]`)?.classList.contains(expected));
        assert.equal(dom.window.getComputedStyle(dom.window.document.querySelector(`[aria-label="${source} → ${target}：${label}"]`)!).borderTopStyle,traversed === undefined ? "dashed" : traversed ? "solid" : "dotted");
      }
      assert.ok(dom.window.document.querySelector('[aria-label="S → plan"].workflow-start'));
    } finally { dom.window.close(); }
  });
}

// DOM が返す実測値を固定し、折り返し・開閉・応答更新時の測定と後片付けを検証する。
test("差し戻しの弧は実ノードの両端へ接続し、再配置と開閉に追従する", async () => {
  const dom = new JSDOM("<div id='root'></div>",{url:"http://localhost"});
  const saved = ["window","document","self","ResizeObserver","IS_REACT_ACT_ENVIRONMENT"].map((key) => [key,Object.getOwnPropertyDescriptor(globalThis,key)] as const);
  let resize: (() => void) | undefined;
  let disconnected = false;
  class Observer { constructor(callback: () => void) { resize=callback; } observe() {} disconnect() { disconnected=true; } }
  for (const [key,value] of Object.entries({window:dom.window,document:dom.window.document,self:dom.window,ResizeObserver:Observer,IS_REACT_ACT_ENVIRONMENT:true})) Object.defineProperty(globalThis,key,{configurable:true,writable:true,value});
  let wrapped = false;
  dom.window.HTMLElement.prototype.getBoundingClientRect = function () {
    const name=this.dataset.workflowNode;
    if (name === "gate") return new dom.window.DOMRect(wrapped ? 30 : 430,wrapped ? 260 : 60,100,100);
    if (name === "develop") return new dom.window.DOMRect(230,60,100,100);
    return new dom.window.DOMRect(10,20,800,500);
  };
  const root=createRoot(dom.window.document.getElementById("root")!);
  try {
    const data=progress();
    await act(async () => root.render(createElement(WorkflowProgress,{progress:data,now})));
    const path=() => dom.window.document.querySelector(".workflow-return-arcs > path")!;
    assert.match(path().getAttribute("d")!,/^M 470 140 V .* 270 V 140$/);
    assert.match(path().textContent!,/gate → develop：差し戻し 2 回/);
    wrapped=true;
    await act(async () => { resize?.(); });
    assert.match(path().getAttribute("d")!,/^M 70 340 V .* 270 V 140$/);
    wrapped=false;
    await act(async () => { dom.window.document.querySelector("details")!.dispatchEvent(new dom.window.Event("toggle")); });
    assert.match(path().getAttribute("d")!,/^M 470 140 V .* 270 V 140$/);
    const updated=Progress.create({...data,edges:data.edges.map((edge)=>({...edge,traversals:{value:0}}))});
    await act(async () => root.render(createElement(WorkflowProgress,{progress:updated,now})));
    assert.equal(dom.window.document.querySelectorAll(".workflow-return-arcs > path").length,0);
    assert.equal(dom.window.document.querySelectorAll(".workflow-return").length,0);
  } finally {
    await act(async () => root.unmount());
    assert.ok(disconnected);
    for (const [key,descriptor] of saved) { if (descriptor) Object.defineProperty(globalThis,key,descriptor); else Reflect.deleteProperty(globalThis,key); }
    dom.window.close();
  }
});

test("同じ接続先の後順位規則の通過は backend の接続状態で表示する", () => {
  const data=progress();
  data.edges=[{source:"plan",target:"develop",ruleIndex:0,traversals:{value:0},backward:false},{source:"plan",target:"develop",ruleIndex:1,traversals:{value:1},backward:false}];
  const dom=new JSDOM(renderToStaticMarkup(createElement(WorkflowProgress,{progress:data,now})));
  try { assert.equal(dom.window.document.querySelector('[aria-label="plan → develop：通過済み"]')?.classList.contains("passed"),true); }
  finally {dom.window.close();}
});
