// Deterministic mock backend for the browser preview (design QA).
// Loaded lazily by api.ts only when the app runs outside Tauri, so it
// never affects the packaged app. Data is seeded, not random: reloading
// the page renders identical numbers, which keeps screenshot diffs clean.

import type {
  Block,
  DailyRow,
  ModelRow,
  Overview,
  ProjectRow,
  ScanStats,
  SessionRow,
  SourceInfo,
} from "./api";

/** Small LCG; stable across reloads for a given seed. */
function makeRng(seed: number) {
  let s = seed >>> 0;
  return () => {
    s = (s * 1664525 + 1013904223) >>> 0;
    return s / 4294967296;
  };
}

const AGENTS = ["claude-code", "codex", "kimi"] as const;
/** Rough daily spend scale per agent, in USD. */
const AGENT_SCALE: Record<string, number> = {
  "claude-code": 27.4,
  codex: 9.6,
  kimi: 1.7,
};
const AGENT_MODELS: Record<string, [string, number][]> = {
  "claude-code": [
    ["claude-sonnet-4-5-20250929", 0.63],
    ["claude-opus-4-5-20251101", 0.37],
  ],
  codex: [["gpt-5.2-codex", 1]],
  kimi: [["kimi-code/k3", 1]],
};
const PROJECTS = [
  "TokBar",
  "meridian-api",
  "ledger-sync",
  "blog-astro",
  "infra-scripts",
];

const DAY = 86_400_000;

function startOfToday(): number {
  const d = new Date();
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

function fmtDate(ms: number): string {
  const d = new Date(ms);
  const mm = String(d.getMonth() + 1).padStart(2, "0");
  const dd = String(d.getDate()).padStart(2, "0");
  return `${d.getFullYear()}-${mm}-${dd}`;
}

function emptyRow(date: string, agent: string): DailyRow {
  return {
    date,
    agent,
    cost: 0,
    inputTokens: 0,
    outputTokens: 0,
    cacheCreationTokens: 0,
    cacheReadTokens: 0,
    totalTokens: 0,
    requests: 0,
  };
}

/** Fill token/request fields from a cost figure, with seeded jitter. */
function fill(row: DailyRow, cost: number, rnd: () => number): DailyRow {
  row.cost = cost;
  row.inputTokens = Math.round(cost * 41_000 * (0.8 + rnd() * 0.4));
  row.outputTokens = Math.round(cost * 12_500 * (0.8 + rnd() * 0.4));
  row.cacheCreationTokens = Math.round(cost * 96_000 * (0.7 + rnd() * 0.6));
  row.cacheReadTokens = Math.round(cost * 690_000 * (0.7 + rnd() * 0.6));
  row.totalTokens =
    row.inputTokens +
    row.outputTokens +
    row.cacheCreationTokens +
    row.cacheReadTokens;
  row.requests = Math.max(1, Math.round(cost * 11 * (0.7 + rnd() * 0.6)));
  return row;
}

/** Per-(day, agent) usage for the last 120 days, weekday-weighted. */
function allDaily(): DailyRow[] {
  const rnd = makeRng(20260703);
  const today = startOfToday();
  const rows: DailyRow[] = [];
  for (let back = 119; back >= 0; back--) {
    const dayMs = today - back * DAY;
    const weekday = new Date(dayMs).getDay();
    const weekend = weekday === 0 || weekday === 6;
    for (const agent of AGENTS) {
      // Some quiet days; weekends lighter.
      if (rnd() < (weekend ? 0.45 : 0.12)) continue;
      const scale = AGENT_SCALE[agent] * (weekend ? 0.35 : 1);
      const cost = scale * (0.25 + rnd() * 1.5);
      rows.push(fill(emptyRow(fmtDate(dayMs), agent), cost, rnd));
    }
  }
  return rows;
}

const DAILY = allDaily();

function inRange(row: DailyRow, sinceMs?: number, untilMs?: number): boolean {
  const ms = new Date(`${row.date}T12:00:00`).getTime();
  if (sinceMs != null && ms < sinceMs) return false;
  if (untilMs != null && ms >= untilMs) return false;
  return true;
}

function filtered(args?: Record<string, unknown>): DailyRow[] {
  const sinceMs = args?.sinceMs as number | undefined;
  const untilMs = args?.untilMs as number | undefined;
  return DAILY.filter((r) => inRange(r, sinceMs, untilMs));
}

function overview(rows: DailyRow[]): Overview {
  const totals = {
    cost: 0,
    inputTokens: 0,
    outputTokens: 0,
    cacheCreationTokens: 0,
    cacheReadTokens: 0,
    totalTokens: 0,
    requests: 0,
    sessions: 0,
    activeDays: new Set(rows.map((r) => r.date)).size,
  };
  const byAgent = new Map<
    string,
    { agent: string; cost: number; totalTokens: number; requests: number; sessions: number }
  >();
  for (const r of rows) {
    totals.cost += r.cost;
    totals.inputTokens += r.inputTokens;
    totals.outputTokens += r.outputTokens;
    totals.cacheCreationTokens += r.cacheCreationTokens;
    totals.cacheReadTokens += r.cacheReadTokens;
    totals.totalTokens += r.totalTokens;
    totals.requests += r.requests;
    const a = byAgent.get(r.agent) ?? {
      agent: r.agent,
      cost: 0,
      totalTokens: 0,
      requests: 0,
      sessions: 0,
    };
    a.cost += r.cost;
    a.totalTokens += r.totalTokens;
    a.requests += r.requests;
    byAgent.set(r.agent, a);
  }
  for (const a of byAgent.values()) {
    a.sessions = Math.max(1, Math.round(a.requests / 23));
    totals.sessions += a.sessions;
  }
  return {
    totals,
    byAgent: [...byAgent.values()].sort((x, y) => y.cost - x.cost),
  };
}

function hourly(): DailyRow[] {
  const rnd = makeRng(42);
  const rows: DailyRow[] = [];
  for (let h = 8; h <= 23; h++) {
    // Morning ramp, afternoon dip, evening peak.
    const hump =
      h < 12 ? (h - 7) / 5 : h < 15 ? 0.4 : h < 20 ? 0.7 + (h - 15) * 0.12 : 1.1;
    for (const agent of AGENTS) {
      if (rnd() < 0.35) continue;
      const cost = AGENT_SCALE[agent] * 0.09 * hump * (0.5 + rnd());
      rows.push(
        fill(emptyRow(`${String(h).padStart(2, "0")}:00`, agent), cost, rnd),
      );
    }
  }
  return rows;
}

function models(rows: DailyRow[]): ModelRow[] {
  const acc = new Map<string, ModelRow>();
  for (const r of rows) {
    for (const [model, share] of AGENT_MODELS[r.agent] ?? []) {
      const m =
        acc.get(model) ??
        ({
          model,
          cost: 0,
          inputTokens: 0,
          outputTokens: 0,
          cacheCreationTokens: 0,
          cacheReadTokens: 0,
          totalTokens: 0,
          requests: 0,
        } as ModelRow);
      m.cost += r.cost * share;
      m.inputTokens += Math.round(r.inputTokens * share);
      m.outputTokens += Math.round(r.outputTokens * share);
      m.cacheCreationTokens += Math.round(r.cacheCreationTokens * share);
      m.cacheReadTokens += Math.round(r.cacheReadTokens * share);
      m.totalTokens += Math.round(r.totalTokens * share);
      m.requests += Math.round(r.requests * share);
      acc.set(model, m);
    }
  }
  return [...acc.values()].sort((a, b) => b.cost - a.cost);
}

function sessions(args?: Record<string, unknown>): SessionRow[] {
  const rnd = makeRng(777);
  const sinceMs = (args?.sinceMs as number | undefined) ?? 0;
  const now = Date.now();
  const out: SessionRow[] = [];
  for (let i = 0; i < 24; i++) {
    const agent = AGENTS[i % AGENTS.length];
    const lastTs = now - Math.round(rnd() * 28 * DAY);
    if (lastTs < sinceMs) continue;
    const cost = AGENT_SCALE[agent] * (0.1 + rnd() * 0.9);
    out.push({
      sessionId: `${agent.slice(0, 2)}-${(rnd() * 0xffffff) .toString(16).slice(0, 6)}${i}`,
      agent,
      project: PROJECTS[Math.floor(rnd() * PROJECTS.length)],
      firstTs: lastTs - Math.round((0.4 + rnd() * 3) * 3_600_000),
      lastTs,
      cost,
      totalTokens: Math.round(cost * 830_000),
      requests: Math.max(2, Math.round(cost * 12)),
      models: (AGENT_MODELS[agent] ?? []).map(([m]) => m).join(","),
    });
  }
  return out.sort((a, b) => b.lastTs - a.lastTs);
}

function projects(rows: DailyRow[]): ProjectRow[] {
  const rnd = makeRng(99);
  const total = rows.reduce((s, r) => s + r.cost, 0);
  const shares = [0.41, 0.24, 0.17, 0.11, 0.07];
  return PROJECTS.map((project, i) => {
    const cost = total * shares[i] * (0.9 + rnd() * 0.2);
    return {
      project,
      cost,
      totalTokens: Math.round(cost * 810_000),
      requests: Math.max(1, Math.round(cost * 10.7)),
      sessions: Math.max(1, Math.round(cost / 9.3)),
    };
  }).sort((a, b) => b.cost - a.cost);
}

function blocks(): Block[] {
  const rnd = makeRng(5);
  const now = Date.now();
  const hour = 3_600_000;
  const alignedNow = Math.floor(now / hour) * hour;
  const out: Block[] = [];
  // A handful of completed 5h blocks over the last two days + one live.
  for (let i = 5; i >= 1; i--) {
    const start = alignedNow - i * 9 * hour;
    const cost = 4.3 + rnd() * 21;
    out.push({
      id: `blk-${i}`,
      startMs: start,
      endMs: start + 5 * hour,
      actualEndMs: start + Math.round((2.1 + rnd() * 2.6) * hour),
      isActive: false,
      isGap: false,
      cost,
      totalTokens: Math.round(cost * 840_000),
      requests: Math.max(3, Math.round(cost * 11)),
      models: AGENT_MODELS["claude-code"].map(([m]) => m),
      burnRateTpm: null,
      burnRateCostPerHour: null,
    });
  }
  const liveStart = alignedNow - 2 * hour;
  out.push({
    id: "blk-live",
    startMs: liveStart,
    endMs: liveStart + 5 * hour,
    actualEndMs: null,
    isActive: true,
    isGap: false,
    cost: 12.47,
    totalTokens: 9_882_340,
    requests: 141,
    models: [
      "claude-sonnet-4-5-20250929",
      "claude-opus-4-5-20251101",
      "gpt-5.2-codex",
    ],
    burnRateTpm: 68_430,
    burnRateCostPerHour: 5.93,
  });
  return out.reverse();
}

function sources(): SourceInfo[] {
  const home = "~/";
  const active: [string, string[], number][] = [
    ["claude-code", [`${home}.claude/projects`], 412],
    ["codex", [`${home}.codex/sessions`, `${home}.codex/archived_sessions`], 187],
    ["kimi", [`${home}.kimi/sessions`, `${home}.kimi-code/sessions`], 36],
  ];
  const inactive = [
    "gemini",
    "copilot",
    "qwen",
    "opencode",
    "openclaw",
    "amp",
    "goose",
    "droid",
    "pi",
    "codebuff",
    "hermes",
    "kilo",
  ];
  return [
    ...active.map(([agent, dirs, fileCount]) => ({ agent, dirs, fileCount })),
    ...inactive.map((agent) => ({
      agent,
      dirs: [`${home}.${agent}`],
      fileCount: 0,
    })),
  ];
}

let trayMode = "cost";

export async function mockInvoke<T>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<T> {
  // Brief delay so loading states are visible and realistic in preview.
  await new Promise((r) => setTimeout(r, 90));
  switch (cmd) {
    case "refresh_data":
      return {
        filesTotal: 507,
        filesParsed: 3,
        filesRemoved: 0,
        entriesInserted: 42,
        durationMs: 187,
      } satisfies ScanStats as T;
    case "get_overview":
      return overview(filtered(args)) as T;
    case "get_daily":
      return filtered(args) as T;
    case "get_hourly":
      return hourly() as T;
    case "get_models":
      return models(filtered(args)) as T;
    case "get_sessions":
      return sessions(args) as T;
    case "get_projects":
      return projects(filtered(args)) as T;
    case "get_blocks":
      return blocks() as T;
    case "get_sources":
      return sources() as T;
    case "get_session_models":
      return models(
        filtered({ sinceMs: Date.now() - 7 * DAY }).filter(
          (r) => r.agent === (args?.agent as string),
        ),
      ) as T;
    case "get_tray_mode":
      return trayMode as T;
    case "set_tray_mode":
      trayMode = (args?.mode as string) ?? "cost";
      return undefined as T;
    case "show_main_window":
      return undefined as T;
    default:
      throw new Error(`mockInvoke: unhandled command ${cmd}`);
  }
}
