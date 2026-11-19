import { createHash } from "node:crypto";
import type { Locator } from "@playwright/test";
import { expect, test } from "../support/test";
import { startStack } from "../support/stack";
import { buildMockState, buildRunFiles, RUN_SLUG } from "../support/fixture";

const backbone = ["mark_in_progress", "develop", "machine_gate", "aislop_gate"];
const fixes = ["fmt", "clippy", "test", "typecheck", "lint", "proto_check"].map((suffix) => `fix_machine_gate_${suffix}`);

function counts(gate: number, fmt: number) {
  return [["mark_in_progress", 1], ["develop", 1], ["machine_gate", gate], [fixes[0], fmt], [fixes[2], 1]].map(([step, count]) => ({
    identity: createHash("sha256").update(JSON.stringify([[], "e2e-ref", step])).digest("hex"), workflow: "e2e", step, count,
  }));
}

function event(type: string, step: string, iteration: number, matchedRuleIndex?: number) {
  return { type, step, workflow: "e2e", persona: "coder", status: "done", timestamp: new Date().toISOString(), iteration, matchedRuleIndex,
    stack: [{ step, workflow: "e2e", workflow_ref: "e2e-ref", kind: "agent", occurrence: 1 }] };
}

const mockState = (now: Date) => ({
  ...buildMockState(now),
  workflows: { e2e: { initialStep: backbone[0], steps: [
    { name: backbone[0], kind: "agent", rules: [{ next: backbone[1] }] },
    { name: backbone[1], kind: "workflow_call", rules: [{ next: backbone[2] }] },
    { name: backbone[2], kind: "system", rules: [...fixes.map((next) => ({ next })), { next: backbone[3] }] },
    { name: backbone[3], kind: "system", rules: [{ next: "COMPLETE" }, { next: "ABORT" }] },
    ...fixes.map((name) => ({ name, kind: "agent", rules: [{ next: backbone[2] }] })),
  ] } },
});

const runFiles = (now: Date) => ({
  ...buildRunFiles(now),
  meta: { ...buildRunFiles(now).meta, currentStep: "machine_gate", currentIteration: 9, stepExecutions: counts(3, 1) },
  bodyLogRecords: [
    { type: "workflow_start", workflow: "e2e" },
    event("step_complete", "mark_in_progress", 1, 0),
    event("step_complete", "develop", 2, 0),
    event("step_complete", "machine_gate", 7, 0),
    event("step_complete", fixes[0], 8, 0),
    event("step_start", "machine_gate", 9),
  ],
});

async function expectRepairConnection(repair: Locator) {
  await expect.poll(() => repair.evaluate((element) => {
    const source = element.parentElement!.querySelector(".workflow-main-node > [data-workflow-node]")!.getBoundingClientRect();
    const bounds = element.getBoundingClientRect();
    const style = getComputedStyle(element);
    const rails = ["::before", "::after"].map((pseudo) => {
      const line = getComputedStyle(element, pseudo);
      // Absolute insets are resolved against the details padding box.
      const top = bounds.top + parseFloat(style.borderTopWidth) + parseFloat(line.top);
      const left = bounds.left + parseFloat(style.borderLeftWidth) + parseFloat(line.left);
      const width = parseFloat(line.borderLeftWidth);
      return {
        painted: line.content !== "none" && line.display !== "none" && line.visibility === "visible" && line.borderLeftStyle === "solid" && width > 0,
        sourceGap: Math.abs(top - source.bottom),
        repairGap: Math.abs(top + parseFloat(line.height) - bounds.top),
        sideGap: pseudo === "::before" ? Math.abs(left - source.left) : Math.abs(left + width - source.right),
      };
    });
    return {
      railsReachSource: rails.every((rail) => rail.painted && rail.sourceGap < 1 && rail.repairGap < 1 && rail.sideGap < 1),
      returnBorder: style.borderLeftStyle === "solid" && style.borderRightStyle === "solid" && style.borderBottomStyle === "solid" && [style.borderLeftWidth, style.borderRightWidth, style.borderBottomWidth].every((width) => parseFloat(width) > 0),
      rails,
    };
  })).toMatchObject({ railsReachSource: true, returnBorder: true });
}

for (const width of [1100, 1920]) {
  test(`進行図は幅 ${width}px で読め、輪を開いて対応ログへ移動できる`, async ({ page }) => {
    const stack = await startStack({ mockState, runFiles });
    try {
      await page.setViewportSize({ width, height: 1000 });
      await page.goto(`${stack.webUrl}/runs/${RUN_SLUG}`);
      const graph = page.getByRole("region", { name: "ワークフローの進行", exact: true });
      await expect(graph).toContainText("4 段階中 3 つ目、machine_gate、3 回目");
      const repair = graph.locator("details");
      await expect(repair).not.toHaveAttribute("open", "");
      await expect(repair.locator("xpath=ancestor::li[1]")).toHaveAttribute("data-workflow-source","machine_gate");
      await expect(graph.getByLabel("S → mark_in_progress",{exact:true})).toBeVisible();
      await expect(repair.locator("summary")).toContainText("修正 2 回");
      await expect(repair.locator("summary")).toContainText("ほか 4 種は未実行");
      await expectRepairConnection(repair);
      await graph.screenshot({ path: test.info().outputPath("repair-closed.png") });
      await repair.locator("summary").click();
      await expect(repair).toHaveAttribute("open", "");
      await expectRepairConnection(repair);
      await expect(repair.getByText("未実行：", { exact: false })).toContainText(fixes[1]);
      await graph.screenshot({ path: test.info().outputPath("repair-open.png") });
      await repair.locator("summary").click();
      await expect(repair).not.toHaveAttribute("open", "");
      await expect(repair.locator("summary")).toContainText("修正 2 回");
      await expectRepairConnection(repair);
      await graph.screenshot({ path: test.info().outputPath("repair-closed-again.png") });
      await repair.locator("summary").click();
      const fmt = repair.getByRole("link", { name: "fmt", exact: true });
      await expect(fmt).toBeVisible();
      await expect(fmt.locator("..")).toHaveAttribute("title", fixes[0]);
      await fmt.click();
      await expect(page).toHaveURL(new RegExp("#run-step-log-3$"));
      await expect(page.locator("#run-step-log-3")).toContainText(fixes[0]);
      const bounds = await graph.evaluate((element) => ({ right: element.getBoundingClientRect().right, scroll: document.documentElement.scrollWidth, width: document.documentElement.clientWidth }));
      expect(bounds.scroll).toBeLessThanOrEqual(bounds.width);
      expect(bounds.right).toBeLessThanOrEqual(width);
    } finally { await stack.stop(); }
  });
}

test("同じ概要カードと詳細画面が修正への移動と帰還に追従する", async ({ page }) => {
  const stack = await startStack({ mockState, runFiles });
  try {
    await page.goto(stack.webUrl);
    const card = page.locator(".run-row");
    await expect(card).toContainText("3/4");
    await expect(card).toContainText("修正 2 回");
    await card.evaluate((element) => element.setAttribute("data-test-existing", "yes"));
    await stack.updateRunMeta((meta) => ({ ...meta, currentStep: fixes[0], currentIteration: 10, stepExecutions: counts(3, 2) }));
    await stack.appendRunLog([event("step_complete", "machine_gate", 9, 0), event("step_start", fixes[0], 10)]);
    await expect(card).toContainText(fixes[0], { timeout: 15_000 });
    await expect(card).toContainText("3/4");
    await expect(card).toContainText("修正 3 回");
    await expect(card).toHaveAttribute("data-test-existing", "yes");
    await page.goto(`${stack.webUrl}/runs/${RUN_SLUG}`);
    const graph = page.getByRole("region", { name: "ワークフローの進行", exact: true });
    await expect(graph).toContainText(fixes[0]);
    await graph.evaluate((element) => element.setAttribute("data-test-existing", "yes"));
    await stack.updateRunMeta((meta) => ({ ...meta, currentStep: "machine_gate", currentIteration: 11, stepExecutions: counts(4, 2) }));
    await stack.appendRunLog([event("step_complete", fixes[0], 10, 0), event("step_start", "machine_gate", 11)]);
    await expect(graph).toContainText("4 段階中 3 つ目、machine_gate、4 回目", { timeout: 15_000 });
    await expect(graph).toHaveAttribute("data-test-existing", "yes");
    await expect(graph.locator("summary")).toContainText("修正 3 回");
  } finally { await stack.stop(); }
});

test("長い背骨は折り返し、ノードを画面外へ隠さない", async ({ page }) => {
  const names = Array.from({ length: 14 }, (_, index) => `long_workflow_step_${index}`);
  const stack = await startStack({
    mockState: (now) => ({ ...buildMockState(now), workflows: { e2e: { initialStep: names[0], steps: names.map((name, index) => ({ name, kind: "agent", rules: index === names.length - 1 ? [{next:"COMPLETE"},{next:names[0]}] : [{ next: names[index + 1] }] })) } } }),
    runFiles: (now) => ({ ...buildRunFiles(now), meta: { ...buildRunFiles(now).meta, currentStep: names[0], stepExecutions: [] }, bodyLogRecords: [{type:"workflow_start",workflow:"e2e"},event("step_complete",names.at(-1)!,1,1)] }),
  });
  try {
    await page.setViewportSize({ width: 1100, height: 1000 });
    await page.goto(`${stack.webUrl}/runs/${RUN_SLUG}`);
    const graph = page.getByRole("region", { name: "ワークフローの進行", exact: true });
    await expect(graph.locator(".workflow-node")).toHaveCount(14);
    await graph.evaluate((element) => element.setAttribute("data-test-existing", "yes"));
    let previous: { top: number; center: number }[] | undefined;
    for (const [resizeIndex, width] of [1100, 1920, 1100].entries()) {
      await page.setViewportSize({ width, height: 1000 });
      await expect(graph).toHaveAttribute("data-test-existing", "yes");
      const nodes = graph.locator(".workflow-main-node > [data-workflow-node]");
      await expect(nodes).toHaveCount(14);
      const rectangles = await nodes.evaluateAll((elements) => elements.map((element) => {
        const rect = element.getBoundingClientRect();
        return { top: rect.top, right: rect.right, center: rect.left + rect.width / 2 };
      }));
      expect(new Set(rectangles.map((rect) => rect.top)).size).toBeGreaterThan(1);
      expect(rectangles.every((rect) => rect.right <= width)).toBe(true);
      if (previous !== undefined) {
        const beforeResize = previous;
        expect(rectangles.some((rect, index) => (index === 0 || index === rectangles.length - 1) && (Math.abs(rect.top - beforeResize[index].top) > 1 || Math.abs(rect.center - beforeResize[index].center) > 1))).toBe(true);
      }
      previous = rectangles;
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
      expect(overflow).toBeLessThanOrEqual(0);
      const arc = graph.locator(".workflow-return-arcs > path");
      await expect(arc).toHaveCount(1);
      await expect(arc.locator("title")).toHaveText(`${names.at(-1)} → ${names[0]}：差し戻し 1 回`);
      await expect.poll(() => arc.evaluate((element) => {
        const path = element as SVGPathElement;
        const svg = path.ownerSVGElement!.getBoundingClientRect();
        const graph = path.closest(".workflow-progress")!;
        const nodes = [...graph.querySelectorAll<HTMLElement>(".workflow-main-node > [data-workflow-node]")];
        const source = nodes.at(-1)!.getBoundingClientRect();
        const target = nodes[0]!.getBoundingClientRect();
        const start = path.getPointAtLength(0);
        const end = path.getPointAtLength(path.getTotalLength());
        return [Math.abs(start.x + svg.left - source.left - source.width/2),Math.abs(start.y+svg.top-source.bottom),Math.abs(end.x+svg.left-target.left-target.width/2),Math.abs(end.y+svg.top-target.bottom)];
      }).then((distances) => distances.map((distance) => distance < 1))).toEqual([true, true, true, true]);
      const intersections = await arc.evaluate((element) => {
        const path = element as SVGPathElement;
        const svg = path.ownerSVGElement!.getBoundingClientRect();
        const nodes = [...path.closest(".workflow-progress")!.querySelectorAll<HTMLElement>("[data-workflow-node]")];
        const middle = nodes.slice(1, -1).map((node) => node.getBoundingClientRect());
        const radius = Number.parseFloat(getComputedStyle(path).strokeWidth) / 2 + 0.5;
        const length = path.getTotalLength();
        let collisions = 0;
        for (let distance = 0; distance <= length; distance += 0.5) {
          const point = path.getPointAtLength(distance);
          const x = point.x + svg.left, y = point.y + svg.top;
          if (middle.some((rect) => x + radius >= rect.left && x - radius <= rect.right && y + radius >= rect.top && y - radius <= rect.bottom)) collisions++;
        }
        return collisions;
      });
      expect(intersections).toBe(0);
      await expect(arc).toHaveAttribute("marker-end", /url\(#.+\)/);
      await graph.screenshot({ path: test.info().outputPath(`return-${resizeIndex}-${width}.png`) });
    }
  } finally { await stack.stop(); }
});
