// stdout 用の --import エントリ（issue #186）。
// 子プロセスの process.stdout に流れる行へ UTC ISO 時刻を前置する。
import { installPrefixOnStream } from "./web-log-prefix.mjs";

installPrefixOnStream(process.stdout);
