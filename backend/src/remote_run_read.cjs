// SSH 側で実行する読取専用処理。パスは stdin の JSON で受け取り、shell へ埋め込まない。
const fs = require("node:fs");
const path = require("node:path");

const LIMIT = 16 * 1024 * 1024;
const contains = (root, child) => child === root || child.startsWith(root + path.sep);
function realOrMissing(target) {
  try { return fs.realpathSync(target); }
  catch (error) { if (error.code === "ENOENT") return undefined; throw error; }
}
function openFile(root, target, limit) {
  const real = realOrMissing(target);
  if (real === undefined) return undefined;
  if (!contains(root, real)) throw new Error("記録が許可ルートの外を指しています");
  const before = fs.statSync(real);
  if (!before.isFile() || before.size > limit) throw new Error("記録の種類または大きさが不正です");
  const fd = fs.openSync(real, fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW);
  const opened = fs.fstatSync(fd);
  if (opened.dev !== before.dev || opened.ino !== before.ino) {
    fs.closeSync(fd);
    throw new Error("読取中に記録が置き換わりました");
  }
  return fd;
}
function readFile(root, target) {
  const fd = openFile(root, target, LIMIT);
  if (fd === undefined) return undefined;
  try {
    const bytes = Buffer.alloc(LIMIT + 1);
    let size = 0;
    while (size < bytes.length) {
      const read = fs.readSync(fd, bytes, size, bytes.length - size, null);
      if (read === 0) break;
      size += read;
    }
    if (size > LIMIT) throw new Error("記録が上限を超えています");
    return bytes.subarray(0, size).toString("utf8");
  }
  finally { fs.closeSync(fd); }
}
async function* completeLines(input) {
  let pending = "";
  input.setEncoding("utf8");
  for await (const chunk of input) {
    pending += chunk;
    let newline;
    while ((newline = pending.indexOf("\n")) !== -1) {
      const line = pending.slice(0, newline);
      pending = pending.slice(newline + 1);
      if (Buffer.byteLength(line) > LIMIT) throw new Error("ログ行が上限を超えています");
      yield line;
    }
    if (Buffer.byteLength(pending) > LIMIT) throw new Error("ログ行が上限を超えています");
  }
  // 本体ログは追記中なので、最後の改行に達していない記録は次回へ残す。
}
async function readRecords(root, logs, names) {
  const records = [];
  let bytes = 0;
  for (const name of names) {
    const target = path.join(logs, name);
    const fd = openFile(root, target, Number.MAX_SAFE_INTEGER);
    if (fd === undefined) continue;
    const input = fs.createReadStream(target, { fd, autoClose: true });
    try {
      for await (const line of completeLines(input)) {
        if (line.trim() === "") continue;
        let record;
        try { record = JSON.parse(line); }
        catch { throw new Error("本体ログを解釈できません"); }
        if (!["workflow_start", "step_start", "step_complete", "workflow_call_start", "workflow_call_complete", "phase_start", "phase_complete"].includes(record.type)) continue;
        const selected = {};
        for (const key of ["type", "step", "workflow", "stack", "parallel", "timestamp", "iteration", "matchedRuleIndex", "status", "returnValue", "phase"]) {
          if (record[key] !== undefined) selected[key] = record[key];
        }
        if (record.error !== undefined && record.error !== null) selected.error = true;
        bytes += Buffer.byteLength(JSON.stringify(selected));
        if (bytes > LIMIT) throw new Error("実行証拠が上限を超えています");
        records.push(selected);
      }
    } finally { input.destroy(); }
  }
  return records;
}
async function main() {
  const request = JSON.parse(fs.readFileSync(0, "utf8"));
  const project = fs.realpathSync(request.project);
  const projectRuns = path.join(project, ".takt", "runs");
  const worktrees = realOrMissing(path.resolve(project, "..", "takt-worktrees")) ?? path.resolve(project, "..", "takt-worktrees");
  if (request.operation === "tasks") {
    const tasks = readFile(project, path.join(project, ".takt", "tasks.yaml"));
    if (tasks === undefined) throw new Error("対応表がありません");
    return { project, tasks };
  }
  if (request.operation !== "records" || !Array.isArray(request.paths)) throw new Error("読取要求が不正です");
  for (const candidate of request.paths) {
    const root = realOrMissing(candidate);
    if (root === undefined) continue;
    if (!contains(projectRuns, root) && !contains(worktrees, root)) throw new Error("run が許可ルートの外にあります");
    const metaText = readFile(root, path.join(root, "meta.json"));
    const logs = realOrMissing(path.join(root, "logs"));
    if (logs !== undefined && !contains(root, logs)) throw new Error("logs が run の外を指しています");
    const names = logs === undefined ? [] : fs.readdirSync(logs).sort().filter((name) => name.endsWith(".jsonl") && fs.lstatSync(path.join(logs, name)).isFile());
    if (metaText === undefined && names.length === 0) continue;
    const meta = metaText === undefined ? {} : JSON.parse(metaText);
    const records = await readRecords(root, logs, names);
    return { counts: meta.stepExecutions, records };
  }
  throw new Error("run の保存場所がありません");
}
main().then((value) => process.stdout.write(JSON.stringify(value))).catch((error) => {
  process.stderr.write(String(error.message));
  process.exitCode = 1;
});
