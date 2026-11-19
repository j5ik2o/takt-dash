"use client";

import { useEffect, useId, useRef, useState, type CSSProperties, type ReactElement } from "react";
import type { WorkflowNodeState, WorkflowProgress as Progress, WorkflowProgressNode, WorkflowProgressConnection } from "../proto/taktdash";
import { WORKFLOW_NODE_STATE } from "../state/proto-values";
import { formatDuration } from "../state/format";

const stateText: Record<WorkflowNodeState, string> = {
  [WORKFLOW_NODE_STATE.UNKNOWN]: "状態は不明です",
  [WORKFLOW_NODE_STATE.UNVISITED]: "未到達",
  [WORKFLOW_NODE_STATE.RUNNING]: "実行中",
  [WORKFLOW_NODE_STATE.COMPLETED]: "✓ 完了",
  [WORKFLOW_NODE_STATE.SKIPPED]: "通らなかった",
  [WORKFLOW_NODE_STATE.FAILED]: "× 失敗",
  [WORKFLOW_NODE_STATE.ABORTED]: "中止",
  [WORKFLOW_NODE_STATE.UNRECOGNIZED]: "状態は不明です",
};

function countText(node: WorkflowProgressNode): string {
  const count = node.executionCount?.value;
  if (count === undefined) return "回数は不明です";
  return node.state === WORKFLOW_NODE_STATE.RUNNING ? `${count} 回目` : `${count} 回`;
}

function ProgressNode({ node, label, now }: { node: WorkflowProgressNode; label: string; now: Date }): ReactElement {
  const running = node.state === WORKFLOW_NODE_STATE.RUNNING;
  return (
    <div data-workflow-node={node.name} className={`workflow-node workflow-state-${node.state}`} aria-current={running ? "step" : undefined}>
      <span title={node.name} className="mono">
        {node.logIndex === undefined ? label : <a href={`#run-step-log-${node.logIndex}`}>{label}</a>}
      </span>
      {node.activeChildren.length > 0 ? <span>{node.name} ▸ {node.activeChildren.join("、")}</span> : null}
      <span>{stateText[node.state]}</span>
      <span>{countText(node)}</span>
      {running && node.startedAt !== undefined ? <span>経過 {formatDuration(Math.max(0, (now.getTime() - node.startedAt.getTime()) / 1000))}</span> : null}
    </div>
  );
}

function repairLabel(name: string, source: string): string {
  const prefix = `fix_${source}_`;
  return name.startsWith(prefix) ? name.slice(prefix.length) : name;
}

function ProgressEdge({ source, target, traversed }: WorkflowProgressConnection): ReactElement {
  return <span className={`workflow-edge ${traversed === undefined ? "unknown" : traversed ? "passed" : "pending"}`} aria-label={`${source} → ${target}：${traversed === undefined ? "通過は不明です" : traversed ? "通過済み" : "未通過"}`} />;
}

type ReturnArc = { key: string; path: string; label: string };

function useReturnArcs(progress: Progress) {
  const diagram = useRef<HTMLDivElement>(null);
  const [arcs, setArcs] = useState<ReturnArc[]>([]);
  useEffect(() => {
    const element = diagram.current;
    if (element === null) return;
    const measure = () => {
      const bounds = element.getBoundingClientRect();
      const nodes = new Map([...element.querySelectorAll<HTMLElement>("[data-workflow-node]")].map((node) => [node.dataset.workflowNode, node]));
      const slots = [...element.querySelectorAll<HTMLElement>(".workflow-node-slot")].map((slot) => slot.getBoundingClientRect());
      const next = progress.edges.filter((edge) => edge.backward && (edge.traversals?.value ?? 0) > 0).flatMap((edge, index) => {
        const sourceNode = nodes.get(edge.source);
        const targetNode = nodes.get(edge.target);
        const source = sourceNode?.getBoundingClientRect();
        const target = targetNode?.getBoundingClientRect();
        const sourceSlot = sourceNode?.closest(".workflow-node-slot")?.getBoundingClientRect();
        const targetSlot = targetNode?.closest(".workflow-node-slot")?.getBoundingClientRect();
        if (source === undefined || target === undefined || sourceSlot === undefined || targetSlot === undefined || source.width === 0 || target.width === 0) return [];
        const x1 = source.left + source.width / 2 - bounds.left;
        const y1 = source.bottom - bounds.top;
        const x2 = target.left + target.width / 2 - bounds.left;
        const y2 = target.bottom - bounds.top;
        const rowLane = (slot: DOMRect) => Math.max(...slots.filter((other) => Math.abs(other.top - slot.top) < 1).map((other) => other.bottom)) - bounds.top + 16 + index * 8;
        const sourceLane = rowLane(sourceSlot);
        const targetLane = rowLane(targetSlot);
        const sourceSide = sourceSlot.right - bounds.left + 6;
        const targetSide = targetSlot.right - bounds.left + 6;
        const side = bounds.width - 6 - index * 8;
        const betweenRows = Math.abs(sourceSlot.top - targetSlot.top) < 1
          ? `H ${targetSide}`
          : `H ${side} V ${targetLane} H ${targetSide}`;
        return [{ key: `${edge.source}:${edge.ruleIndex}`, path: `M ${x1} ${y1} V ${y1 + 6} H ${sourceSide} V ${sourceLane} ${betweenRows} V ${y2 + 6} H ${x2} V ${y2}`, label: `${edge.source} → ${edge.target}：差し戻し ${edge.traversals!.value} 回` }];
      });
      setArcs((previous) => JSON.stringify(previous) === JSON.stringify(next) ? previous : next);
    };
    const observer = typeof ResizeObserver === "undefined" ? undefined : new ResizeObserver(measure);
    observer?.observe(element);
    for (const node of element.querySelectorAll("[data-workflow-node]")) observer?.observe(node);
    element.addEventListener("toggle", measure, true);
    window.addEventListener("resize", measure);
    measure();
    return () => { observer?.disconnect(); element.removeEventListener("toggle", measure, true); window.removeEventListener("resize", measure); };
  }, [progress]);
  return { diagram, arcs };
}

export function WorkflowProgress({ progress, now }: { progress: Progress; now: Date }): ReactElement {
  const { diagram, arcs } = useReturnArcs(progress);
  const arrow = useId();
  const connection = (source: string, target: string) => progress.connections.find((edge) => edge.source === source && edge.target === target) ?? { source, target, traversed: undefined };
  const nodes = new Map(progress.nodes.map((node) => [node.name, node]));
  const position = progress.currentPosition?.value;
  const current = nodes.get(progress.currentStep ?? "");
  const description = progress.backbone.length === 0
    ? "定義順にステップを表示します。接続線は表示しません。"
    : `${progress.backbone.length} 段階中 ${position === undefined ? "現在位置は不明です" : `${position} つ目`}、${progress.currentStep ?? ""}、${current === undefined ? "回数は不明です" : countText(current)}`;
  const names = progress.backbone.length > 0 ? progress.backbone : progress.definition.map((node) => node.name);
  const returnSpace = 24 + progress.edges.filter((edge) => edge.backward && (edge.traversals?.value ?? 0) > 0).length * 8;
  const renderRepair = (repair: Progress["loops"][number]) => {
        const visited = repair.steps.filter((name) => {
          const node = nodes.get(name);
          return node !== undefined && ((node.executionCount?.value ?? 0) > 0 || [WORKFLOW_NODE_STATE.RUNNING, WORKFLOW_NODE_STATE.COMPLETED, WORKFLOW_NODE_STATE.FAILED, WORKFLOW_NODE_STATE.ABORTED].some((state) => state === node.state));
        });
        const unvisited = repair.steps.filter((name) => !visited.includes(name) && nodes.get(name)?.executionCount?.value === 0);
        const unknown = repair.steps.filter((name) => !visited.includes(name) && nodes.get(name)?.executionCount === undefined);
        return (
          <details className="workflow-repair" key={`${repair.source}:${repair.target}`}>
            <summary>{repair.source}：修正 {repair.executionCount === undefined ? "回数は不明です" : `${repair.executionCount.value} 回`}
              （{visited.map((name) => `${repairLabel(name, repair.source)} ${nodes.get(name)?.executionCount?.value ?? "不明"}`).join("、")}
              {unvisited.length > 0 ? `、ほか ${unvisited.length} 種は未実行` : ""}）
              {repair.target !== repair.source ? ` ${repair.target} へ戻る` : ""}
            </summary>
            <div className="workflow-repair-nodes">
              {visited.map((name) => {
                const node = nodes.get(name);
                return node === undefined ? null : <div key={name}>
                  {progress.connections.filter((edge) => edge.source === repair.source && edge.target === name).map((edge) => <ProgressEdge key={`${edge.source}:${edge.target}`} {...edge} />)}
                  <ProgressNode node={node} label={repairLabel(name, repair.source)} now={now} />
                  {progress.connections.filter((edge) => edge.source === name && edge.target === repair.target).map((edge) => <ProgressEdge key={`${edge.source}:${edge.target}`} {...edge} />)}
                </div>;
              })}
            </div>
            {unvisited.length > 0 ? <p>未実行：{unvisited.map((name) => <span key={name} className={`workflow-state-${nodes.get(name)?.state}`}>{name}（{stateText[nodes.get(name)!.state]}） </span>)}</p> : null}
            {unknown.length > 0 ? <p>回数は不明です：{unknown.join("、")}</p> : null}
          </details>
        );
  };

  return (
    <section className="workflow-progress" aria-label="ワークフローの進行">
      <h2 className="sec">ワークフローの進行</h2>
      <p className="workflow-description">{description}</p>
      <div className="workflow-diagram" ref={diagram} style={{ "--workflow-return-space": `${returnSpace}px` } as CSSProperties}>
      <ol className="workflow-backbone" aria-label="成功までの主経路">
        {names.map((name, index) => {
          const node = nodes.get(name);
          const previous = index === 0 ? undefined : names[index - 1];
          const edge = progress.backbone.length === 0 || previous === undefined ? undefined : connection(previous, name);
          return node === undefined ? null : (
            <li key={name} className="workflow-node-slot" data-workflow-source={name}>
              <div className="workflow-leading">
              {index === 0 && progress.backbone.length > 0 ? <><span className="workflow-terminal">S</span><span className="workflow-edge workflow-start" aria-label={`S → ${name}`} /></> : null}
              {edge === undefined ? null : <ProgressEdge {...edge} />}
              </div>
              <div className="workflow-node-branch">
              <div className="workflow-main-node"><ProgressNode node={node} label={name} now={now} /></div>
              {progress.backbone.length === 0 ? null : progress.loops.filter((repair) => repair.source === name).map(renderRepair)}
              </div>
            </li>
          );
        })}
        {progress.backbone.length > 0 ? <li>
          {progress.terminal === "ABORT" || progress.terminal === "FAILED" ? null : <ProgressEdge {...connection(progress.backbone.at(-1)!, "COMPLETE")} />}
          <span className="workflow-terminal">{progress.terminal ?? "E"}</span>
        </li> : null}
      </ol>

      {progress.edges.filter((edge) => edge.backward && edge.traversals !== undefined && edge.traversals.value > 0).map((edge) => (
        <div className="workflow-return" key={`${edge.source}:${edge.ruleIndex}`}>

          {edge.source} → {edge.target}：差し戻し {edge.traversals?.value} 回
        </div>
      ))}
      <svg className="workflow-return-arcs" aria-label="通過した差し戻しの接続">
        <defs><marker id={arrow} markerWidth="8" markerHeight="8" refX="4" refY="4" orient="auto"><path d="M 0 0 L 4 4 L 0 8" fill="none" stroke="currentColor" /></marker></defs>
        {arcs.map((arc) => <path key={arc.key} d={arc.path} fill="none" stroke="currentColor" markerEnd={`url(#${arrow})`}><title>{arc.label}</title></path>)}
      </svg>
      </div>
      {progress.backbone.length > 0 && progress.otherSteps.length > 0 ? <div>
        <h3>そのほかの step</h3>
        <div className="workflow-repair-nodes">{progress.otherSteps.map((name) => {
          const node = nodes.get(name);
          return node === undefined ? null : <ProgressNode key={name} node={node} label={name} now={now} />;
        })}</div>
      </div> : null}
    </section>
  );
}

export function WorkflowProgressSummary({ progress }: { progress: Progress }): ReactElement {
  const position = progress.currentPosition?.value;
  return <div className="workflow-summary">
    {position === undefined ? null : <>
      <div className="workflow-segments" role="img" aria-label={`${progress.backbone.length} 段階中 ${position} つ目`}>
        {progress.backbone.map((name, index) => <span key={name} className={index + 1 <= position ? "passed" : ""} />)}
      </div>
      <span>{position}/{progress.backbone.length}</span>
    </>}
    {progress.loops.map((repair) => <span key={`${repair.source}:${repair.target}`}>{repair.source}：修正 {repair.executionCount === undefined ? "回数は不明です" : `${repair.executionCount.value} 回`}</span>)}
  </div>;
}
