// Web プロセスのログ行へ backend と同じ UTC ISO 時刻を前置する（issue #186）。
//
// Next.js 自身の起動・Ready・エラー行はアプリコードから書式を制御できないため、
// Node の --import で起動時にこのモジュールを読み込み、process.stdout /
// process.stderr の書き込みを時刻付きに差し替える。
//
// 時刻を付ける単位は「改行まで、1 行につき 1 回」であり、本文の内容では判定しない。
// 本文が時刻付きの文字列で始まる行（アプリの計測行など）も、どの行にも同じ規則で
// 1 スタンプだけ付く。

/**
 * 1 行を、backend と同じ `YYYY-MM-DDTHH:MM:SS.sssZ` 形式の UTC 時刻と空白を
 * 前置した文字列へ変換する。
 * @param {string} line
 * @param {Date} now
 * @returns {string}
 */
export function formatLine(line, now) {
  return `${now.toISOString()} ${line}`;
}

/**
 * ストリームへ流れるチャンクを行単位に時刻付きへ変換する。
 * 改行で終わらない末尾の部分行は、続きのチャンクが届くか flush が呼ばれるまで
 * 保持してから、1 行につき 1 スタンプで出力する。
 * @param {{ now?: () => Date }} [options]
 * @returns {{ push: (chunk: string) => string, flush: () => string }}
 */
export function createLinePrefixer(options = {}) {
  const now = options.now ?? (() => new Date());
  let buffer = "";
  return {
    push(chunk) {
      if (chunk === "") {
        return "";
      }
      buffer += chunk;
      const pieces = buffer.split("\n");
      buffer = pieces.pop() ?? "";
      return pieces.map((line) => `${formatLine(line, now())}\n`).join("");
    },
    flush() {
      if (buffer === "") {
        return "";
      }
      const out = formatLine(buffer, now());
      buffer = "";
      return out;
    },
  };
}

/**
 * 1 つのストリームの `write` を時刻付きへ差し替える。差し替えを戻す関数を返す。
 *
 * `write` は Node の契約どおり `(chunk, encoding, callback)` を透過し、戻り値を
 * そのまま返す。終了時には保持中の部分行を flush する。差し替え対象の process は
 * テスト用に差し替えられ、省略時は実際の `process` を使う。
 * @param {{ write: (...args: unknown[]) => unknown }} stream
 * @param {{ now?: () => Date, process?: { on: Function, off: Function } }} [options]
 * @returns {() => void}
 */
export function installPrefixOnStream(stream, options = {}) {
  const now = options.now ?? (() => new Date());
  const targetProcess = options.process ?? process;
  const prefixer = createLinePrefixer({ now });
  const originalWrite = stream.write;

  // 部分行を保持している間の callback は、その write の内容を含む行の書き出しが
  // 終わるまで呼ばない。保留中の callback は write ごとに区別して保存し、行を確定
  // させた write の callback まで含めて到着順に呼ぶ。行の書き出し完了は、その行を
  // 流す `originalWrite` の callback で観測し、書き出し失敗は cb(error) で通知する。
  const pendingCallbacks = [];

  const runCallbacks = (callbacks, error) => {
    for (const cb of callbacks) {
      cb(error);
    }
  };

  stream.write = function write(chunk, encoding, callback) {
    const text = typeof chunk === "string" ? chunk : String(chunk);
    const cb = typeof encoding === "function" ? encoding : callback;
    const out = prefixer.push(text);
    if (out === "") {
      if (cb !== undefined) {
        pendingCallbacks.push(cb);
      }
      return true;
    }
    // 押し出された行はこの write 自身の内容も含む。行を確定させた write の cb も
    // 同じ行の書き出し完了後に呼ぶため、先行分と合わせて到着順に処理する。
    if (cb !== undefined) {
      pendingCallbacks.push(cb);
    }
    const callbacks = pendingCallbacks.splice(0);
    // 呼び出し元の callback は callbacks に移してあるため、ここで再度呼ばない。
    if (typeof encoding === "function") {
      return originalWrite.call(stream, out, (error) => runCallbacks(callbacks, error));
    }
    return originalWrite.call(stream, out, encoding, (error) => runCallbacks(callbacks, error));
  };

  const flushOnExit = () => {
    const out = prefixer.flush();
    const callbacks = pendingCallbacks.splice(0);
    if (out === "") {
      runCallbacks(callbacks);
      return;
    }
    originalWrite.call(stream, out, (error) => runCallbacks(callbacks, error));
  };
  targetProcess.on("exit", flushOnExit);

  return () => {
    stream.write = originalWrite;
    targetProcess.off("exit", flushOnExit);
  };
}
