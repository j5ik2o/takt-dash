// issue #315 D20：アクティビティの出来事に、日ごとの見出しを入れる（元帥の決定、案 A）。
//
// 設計は docs/design/issue-315.md の D20。ActivityContent をサーバー描画し、jsdom で読む。
// 出来事の一覧は期間で絞らない最新の 100 件なので、1 年以上前の出来事も、時計のずれで now より
// 先の出来事も届く。見出しは JST の日で区切り、年が now と違う日だけ年を書く。
import test from "node:test";
import assert from "node:assert/strict";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { JSDOM } from "jsdom";
import { ActivityKind, type ActivityEvent, type GetActivityResponse } from "../../src/proto/taktdash.ts";
import { createActivityResponse, createDashboardState, NOW } from "../support/fixtures.ts";
import { ActivityContent } from "../../src/components/activity-content.tsx";
import { groupActivityEvents, type ActivityGroup } from "../../src/state/activity.ts";

const MINUTE = 60_000;
const DAY = 24 * 60 * MINUTE;
const JST_OFFSET = 9 * 60 * MINUTE;

const normalize = (text: string | null | undefined): string => (text ?? "").replace(/\s+/g, " ").trim();
const pad = (value: number): string => String(value).padStart(2, "0");

// JST の年月日と時分。端末のタイムゾーンに頼らず +9 時間で数える。
const jst = (date: Date) => {
  const shifted = new Date(date.getTime() + JST_OFFSET);
  return {
    year: shifted.getUTCFullYear(),
    month: shifted.getUTCMonth() + 1,
    day: shifted.getUTCDate(),
    clock: `${pad(shifted.getUTCHours())}:${pad(shifted.getUTCMinutes())}`,
  };
};

const sameJstDay = (a: Date, b: Date): boolean => {
  const x = jst(a);
  const y = jst(b);
  return x.year === y.year && x.month === y.month && x.day === y.day;
};

// 設計の見出しの文：年は now と違うときだけ、（今日）は now と同じ日だけ。
const expectedHeading = (date: Date | undefined, now: Date): string => {
  if (date === undefined) {
    return "日付不明";
  }
  const day = jst(date);
  const year = day.year === jst(now).year ? "" : `${day.year}年`;
  const today = sameJstDay(date, now) ? "（今日）" : "";
  return `${year}${day.month}月${day.day}日${today}`;
};

// 設計の行の時刻の文。
const expectedTime = (group: ActivityGroup): string => {
  const newest = group.newest.occurredAt;
  const oldest = group.oldest.occurredAt;
  if (group.count === 1) {
    return newest === undefined ? "—" : jst(newest).clock;
  }
  if (newest === undefined) {
    return "— 〜 —";
  }
  if (oldest === undefined) {
    return `— 〜 ${jst(newest).clock}`;
  }
  const from = jst(oldest);
  const to = jst(newest);
  if (sameJstDay(oldest, newest)) {
    return `${from.clock} 〜 ${to.clock}`;
  }
  const date = `${pad(from.month)}-${pad(from.day)}`;
  return from.year === to.year
    ? `${date} ${from.clock} 〜 ${to.clock}`
    : `${from.year}-${date} ${from.clock} 〜 ${to.clock}`;
};

const eventTime = (event: ActivityEvent): number => (event.occurredAt ? event.occurredAt.getTime() : 0);
// 画面と同じ新しい順（時刻の無い出来事は最後）。
const newestFirst = (events: readonly ActivityEvent[]): ActivityEvent[] =>
  [...events].sort((a, b) => eventTime(b) - eventTime(a));

const event = (
  occurredAt: Date | undefined,
  body: string,
  overrides: Partial<ActivityEvent> = {},
): ActivityEvent => ({
  occurredAt,
  serverId: "mini",
  project: "esa",
  kind: ActivityKind.ACTIVITY_KIND_INQUIRY,
  body,
  ...overrides,
});

const connectionLost = (occurredAt: Date | undefined, body: string): ActivityEvent =>
  event(occurredAt, body, { kind: ActivityKind.ACTIVITY_KIND_CONNECTION_LOST, serverId: "down", project: "takt" });

const render = (events: ActivityEvent[], now: Date = NOW): Document => {
  const response: GetActivityResponse = { ...createActivityResponse(), events };
  const html = renderToStaticMarkup(
    createElement(ActivityContent, { state: createDashboardState(), response, now }),
  );
  return new JSDOM(html).window.document;
};

type RenderedRow = { time: string; dateTime: string | null; message: string };
type RenderedDay = { heading: string; rows: RenderedRow[] };

const eventsSection = (document: Document): Element => {
  const section = document.querySelector('section[aria-label="出来事の一覧"]');
  assert.ok(section, "出来事の一覧の節がある");
  return section;
};

// 出来事の一覧の、日ごとのまとまり（見出しと行）。
const daysOf = (document: Document): RenderedDay[] =>
  Array.from(eventsSection(document).querySelectorAll(".events-day"), (day) => ({
    heading: normalize(day.querySelector("h2.day-head")?.textContent),
    rows: Array.from(day.querySelectorAll("ul.events > li.ev"), (row) => ({
      time: normalize(row.querySelector("time")?.textContent),
      dateTime: row.querySelector("time")?.getAttribute("datetime") ?? null,
      message: normalize(row.querySelector(".msg")?.textContent),
    })),
  }));

const jstDate = (iso: string): Date => new Date(`${iso}+09:00`);

test("D20: 3 日にまたがる出来事は、新しい日から日ごとの見出しの下に並び、行の時刻は HH:MM のまま", () => {
  const events = [
    event(jstDate("2026-11-13T16:25:00"), "今日の出来事です。"),
    event(jstDate("2026-11-13T09:05:00"), "今日の朝の出来事です。"),
    event(jstDate("2026-11-12T22:40:00"), "昨日の出来事です。"),
    event(jstDate("2026-11-07T08:00:00"), "6 日前の出来事です。"),
  ];
  const days = daysOf(render(events));
  assert.deepEqual(
    days.map((day) => day.heading),
    ["11月13日（今日）", "11月12日", "11月7日"],
  );
  assert.deepEqual(
    days.map((day) => day.rows.map((row) => row.time)),
    [["16:25", "09:05"], ["22:40"], ["08:00"]],
  );
  assert.match(days[0]!.rows[0]!.message, /今日の出来事です。/);
  assert.match(days[1]!.rows[0]!.message, /昨日の出来事です。/);
  assert.match(days[2]!.rows[0]!.message, /6 日前の出来事です。/);
  assert.equal(days[0]!.rows[0]!.dateTime, events[0]!.occurredAt!.toISOString(), "行の time は dateTime を持つ");
});

test("D20: 日のまとまりはランドマークにしない div で、見出しは h2.day-head", () => {
  const document = render([
    event(jstDate("2026-11-13T16:25:00"), "今日の出来事です。"),
    event(jstDate("2026-11-12T22:40:00"), "昨日の出来事です。"),
  ]);
  const days = Array.from(eventsSection(document).querySelectorAll(".events-day"));
  assert.equal(days.length, 2);
  for (const day of days) {
    assert.equal(day.tagName, "DIV", "日のまとまりは div");
    assert.equal(day.hasAttribute("aria-labelledby"), false, "aria-labelledby を付けない");
    assert.equal(day.hasAttribute("aria-label"), false, "aria-label を付けない");
    assert.equal(day.hasAttribute("role"), false, "role を付けない");
    assert.ok(day.querySelector(":scope > h2.day-head"), "見出しは h2.day-head");
    assert.ok(day.querySelector(":scope > ul.events"), "行は ul.events に入る");
  }
  // 出来事の一覧の中に、日の数だけ region が並ばない。
  assert.equal(eventsSection(document).querySelectorAll("section").length, 0);
});

test("D20: JST の 0:00 で日が変わる（UTC の 15:00）", () => {
  const now = new Date("2026-11-13T15:30:00Z"); // JST 11月14日 0:30
  const days = daysOf(
    render(
      [
        event(new Date("2026-11-13T15:00:00Z"), "JST 0:00 の出来事です。"),
        event(new Date("2026-11-13T14:59:00Z"), "JST 23:59 の出来事です。"),
      ],
      now,
    ),
  );
  assert.deepEqual(days, [
    {
      heading: "11月14日（今日）",
      rows: [{ time: "00:00", dateTime: "2026-11-13T15:00:00.000Z", message: "esa：JST 0:00 の出来事です。" }],
    },
    {
      heading: "11月13日",
      rows: [{ time: "23:59", dateTime: "2026-11-13T14:59:00.000Z", message: "esa：JST 23:59 の出来事です。" }],
    },
  ]);
});

test("D20: JST の年が now と違う日だけ見出しに年を書き、前の年の同じ月日と見出しが重ならない", () => {
  const days = daysOf(
    render([
      event(jstDate("2026-11-13T10:00:00"), "今年の出来事です。"),
      event(jstDate("2025-11-13T10:00:00"), "去年の同じ日の出来事です。"),
    ]),
  );
  assert.deepEqual(
    days.map((day) => day.heading),
    ["11月13日（今日）", "2025年11月13日"],
  );
});

test("D20: 時計のずれで now より先の出来事は、その日の見出しの下で先頭に並び、（今日）を付けない", () => {
  const days = daysOf(
    render([
      event(jstDate("2026-11-14T10:00:00"), "先の日付の出来事です。"),
      event(jstDate("2026-11-13T10:00:00"), "今日の出来事です。"),
    ]),
  );
  assert.deepEqual(
    days.map((day) => day.heading),
    ["11月14日", "11月13日（今日）"],
  );
  assert.match(days[0]!.rows[0]!.message, /先の日付の出来事です。/);
});

test("D20: 日をまたぐ接続のまとまりは、新しい方の日の下の 1 行で、古い方を MM-DD HH:MM で書く", () => {
  const days = daysOf(
    render([
      connectionLost(jstDate("2026-11-13T00:10:00"), "接続が切れました（2 回目）。"),
      connectionLost(jstDate("2026-11-12T23:50:00"), "接続が切れました（1 回目）。"),
    ]),
  );
  assert.equal(days.length, 1);
  assert.equal(days[0]!.heading, "11月13日（今日）");
  assert.equal(days[0]!.rows.length, 1);
  assert.equal(days[0]!.rows[0]!.time, "11-12 23:50 〜 00:10");
});

test("D20: 年をまたぐ接続のまとまりは、古い方を YYYY-MM-DD HH:MM で書く", () => {
  const now = jstDate("2026-01-01T00:30:00");
  const days = daysOf(
    render(
      [
        connectionLost(jstDate("2026-01-01T00:10:00"), "接続が切れました（2 回目）。"),
        connectionLost(jstDate("2025-12-31T23:50:00"), "接続が切れました（1 回目）。"),
      ],
      now,
    ),
  );
  assert.deepEqual(days.map((day) => day.heading), ["1月1日（今日）"]);
  assert.equal(days[0]!.rows[0]!.time, "2025-12-31 23:50 〜 00:10");
});

test("D20: 古い方に時刻の無い接続のまとまりは、新しい方の日の下で「— 〜 HH:MM」と書く", () => {
  const days = daysOf(
    render([
      connectionLost(jstDate("2026-11-12T08:15:00"), "接続が切れました（時刻あり）。"),
      connectionLost(undefined, "接続が切れました（時刻なし）。"),
    ]),
  );
  assert.deepEqual(days.map((day) => day.heading), ["11月12日"]);
  assert.equal(days[0]!.rows.length, 1);
  assert.equal(days[0]!.rows[0]!.time, "— 〜 08:15");
});

test("D20: 時刻の無い出来事は最後の「日付不明」の下にあり、時刻は — で dateTime を持たない", () => {
  const days = daysOf(
    render([event(jstDate("2026-11-13T10:00:00"), "時刻のある出来事です。"), event(undefined, "時刻の無い出来事です。")]),
  );
  assert.deepEqual(days.map((day) => day.heading), ["11月13日（今日）", "日付不明"]);
  assert.deepEqual(days[1]!.rows.map((row) => [row.time, row.dateTime]), [["—", null]]);
  assert.match(days[1]!.rows[0]!.message, /時刻の無い出来事です。/);
});

test("D20: 出来事が 0 件なら、見出しを出さず「出来事はありません。」だけを出す", () => {
  const document = render([]);
  assert.equal(document.querySelectorAll(".day-head").length, 0);
  assert.equal(document.querySelectorAll(".events-day").length, 0);
  assert.match(normalize(eventsSection(document).textContent), /出来事はありません。/);
});

// ---- 性質のテスト ----

// 種を決めた疑似乱数（mulberry32）。失敗したときに、種から同じ出来事の列を作り直せる。
const random = (seed: number): (() => number) => {
  let value = seed >>> 0;
  return () => {
    value = (value + 0x6d2b79f5) >>> 0;
    let t = value;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
};

// JST の 0:00（その日の始まり）の UTC の時刻。
const jstMidnight = (date: Date): number => {
  const shifted = date.getTime() + JST_OFFSET;
  return shifted - (shifted % DAY) - JST_OFFSET;
};

const jstYearStart = (date: Date): number => Date.UTC(jst(date).year, 0, 1) - JST_OFFSET;

// 年の境・日の境の近くを含む now の候補。
const NOWS = [
  NOW,
  new Date("2025-12-31T15:30:00Z"), // JST 2026年1月1日 0:30
  new Date("2026-11-13T15:30:00Z"), // JST 11月14日 0:30
  new Date("2026-03-01T14:59:00Z"), // JST 3月1日 23:59
];

const KINDS = [
  ActivityKind.ACTIVITY_KIND_CONNECTION_LOST,
  ActivityKind.ACTIVITY_KIND_CONNECTION_LOST,
  ActivityKind.ACTIVITY_KIND_CONNECTION_RESTORED,
  ActivityKind.ACTIVITY_KIND_INQUIRY,
  ActivityKind.ACTIVITY_KIND_RUN_STARTED,
];

const generateTime = (next: () => number, now: Date): Date | undefined => {
  const pick = next();
  if (pick < 0.1) {
    return undefined;
  }
  if (pick < 0.3) {
    // JST の日の境の前後 1 分。境は now の前後 400 日のどこか。
    const boundary = jstMidnight(now) - Math.floor(next() * 400) * DAY + (next() < 0.1 ? DAY : 0);
    return new Date(boundary + (next() < 0.5 ? -MINUTE : 0));
  }
  if (pick < 0.4) {
    // JST の年の境（12月31日 23:59・1月1日 0:00）。今年と去年。
    const boundary = jstYearStart(now) - (next() < 0.5 ? 0 : 365 * DAY);
    return new Date(jstYearStart(new Date(boundary + DAY)) + (next() < 0.5 ? -MINUTE : 0));
  }
  if (pick < 0.6) {
    // now と同じ JST の日（0:00 から now まで）。
    const start = jstMidnight(now);
    return new Date(start + Math.floor(next() * (now.getTime() - start + 1)));
  }
  // now の 400 日前から 2 日後（時計のずれ）まで。
  return new Date(now.getTime() - 400 * DAY + Math.floor(next() * 402 * DAY / MINUTE) * MINUTE);
};

test("D20（性質）: どんな出来事の列でも、見出しは重ならず新しい日から並び、各行はその日の見出しの下にあり、行の並びと時刻の文は変わらない", () => {
  for (let seed = 1; seed <= 80; seed += 1) {
    const next = random(seed);
    const now = NOWS[Math.floor(next() * NOWS.length)]!;
    const count = Math.floor(next() * 31);
    const events = newestFirst(
      Array.from({ length: count }, (_, index) =>
        event(generateTime(next, now), `出来事 ${index} です。`, {
          kind: KINDS[Math.floor(next() * KINDS.length)]!,
          serverId: next() < 0.5 ? "mini" : "down",
          project: next() < 0.1 ? undefined : next() < 0.5 ? "esa" : "takt",
        }),
      ),
    );
    const where = `種 ${seed}（now ${now.toISOString()}、${count} 件）`;
    const groups = groupActivityEvents(events);
    const days = daysOf(render(events, now));

    if (groups.length === 0) {
      assert.equal(days.length, 0, `${where}: 0 件なら見出しを出さない`);
      continue;
    }

    // 見出しは重ならない。
    const headings = days.map((day) => day.heading);
    assert.equal(new Set(headings).size, headings.length, `${where}: 見出しが重ならない ${JSON.stringify(headings)}`);

    // 行を見出しの順につなぐと、まとまりの並び（新しい順）と同じ。各行の見出し・時刻の文・dateTime・本文が合う。
    const flattened = days.flatMap((day) => day.rows.map((row) => ({ heading: day.heading, ...row })));
    assert.equal(flattened.length, groups.length, `${where}: 行の数はまとまりの数と同じ`);
    groups.forEach((group, index) => {
      const row = flattened[index]!;
      const newest = group.newest.occurredAt;
      const at = `${where} の ${index} 行目`;
      assert.equal(row.heading, expectedHeading(newest, now), `${at}: 見出し`);
      assert.equal(row.time, expectedTime(group), `${at}: 時刻の文`);
      assert.equal(row.dateTime, newest === undefined ? null : newest.toISOString(), `${at}: dateTime`);
      assert.ok(row.message.includes(group.newest.body), `${at}: 本文は新しい方の出来事`);
      if (newest !== undefined) {
        const heading = jst(new Date(row.dateTime!));
        const expectedYear = /^(\d+)年/.exec(row.heading)?.[1];
        assert.equal(Number(expectedYear ?? jst(now).year), heading.year, `${at}: 見出しの年と dateTime の JST の年`);
        assert.ok(row.heading.includes(`${heading.month}月${heading.day}日`), `${at}: 見出しの月日と dateTime の JST の月日`);
      }
    });

    // 見出しは日付の新しい順で、日付不明は最後。
    const dated = days.filter((day) => day.heading !== "日付不明");
    for (let index = 1; index < dated.length; index += 1) {
      const previous = Date.parse(dated[index - 1]!.rows[0]!.dateTime!);
      const current = Date.parse(dated[index]!.rows[0]!.dateTime!);
      assert.ok(jstMidnight(new Date(previous)) > jstMidnight(new Date(current)), `${where}: 見出しは新しい日から並ぶ`);
    }
    const unknown = headings.indexOf("日付不明");
    assert.ok(unknown === -1 || unknown === headings.length - 1, `${where}: 日付不明は最後`);

    // （今日）は now の日の見出しだけで、多くて 1 つ。年は now と違う年の見出しだけ。
    const todays = headings.filter((heading) => heading.endsWith("（今日）"));
    assert.ok(todays.length <= 1, `${where}: （今日）は多くて 1 つ`);
    for (const day of dated) {
      const date = new Date(day.rows[0]!.dateTime!);
      assert.equal(day.heading.endsWith("（今日）"), sameJstDay(date, now), `${where}: ${day.heading} の（今日）`);
      assert.equal(/^\d+年/.test(day.heading), jst(date).year !== jst(now).year, `${where}: ${day.heading} の年`);
    }
  }
});
