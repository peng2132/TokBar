import { invoke } from "@tauri-apps/api/core";
import {
  listen,
  type EventCallback,
  type UnlistenFn,
} from "@tauri-apps/api/event";
import { startOfDay } from "./dates";

/** True inside the Tauri app; false in a plain browser (dev preview /
 *  design QA), where deterministic mock data is served instead. */
export const IN_TAURI = "__TAURI_INTERNALS__" in window;

const call = <T,>(cmd: string, args?: Record<string, unknown>): Promise<T> =>
  IN_TAURI
    ? invoke<T>(cmd, args)
    : import("./mockApi").then((m) => m.mockInvoke<T>(cmd, args));

/** `listen()` that no-ops in the browser preview. */
export function onEvent<T>(
  event: string,
  handler: EventCallback<T>,
): Promise<UnlistenFn> {
  return IN_TAURI ? listen(event, handler) : Promise.resolve(() => {});
}

export type CostMode = "auto" | "calculate" | "display";
export type RangeKey = "today" | "7d" | "30d" | "90d" | "all";

export interface Totals {
  cost: number;
  inputTokens: number;
  outputTokens: number;
  cacheCreationTokens: number;
  cacheReadTokens: number;
  totalTokens: number;
  requests: number;
  sessions: number;
  activeDays: number;
}

export interface AgentBreakdown {
  agent: string;
  cost: number;
  totalTokens: number;
  requests: number;
  sessions: number;
}

export interface Overview {
  totals: Totals;
  byAgent: AgentBreakdown[];
}

export interface DailyRow {
  date: string;
  agent: string;
  cost: number;
  inputTokens: number;
  outputTokens: number;
  cacheCreationTokens: number;
  cacheReadTokens: number;
  totalTokens: number;
  requests: number;
}

export interface ModelRow {
  model: string;
  cost: number;
  inputTokens: number;
  outputTokens: number;
  cacheCreationTokens: number;
  cacheReadTokens: number;
  totalTokens: number;
  requests: number;
  /** False when no price is known for the model: its cost is $0 and
   *  unreliable, not free. */
  priced: boolean;
}

export interface SessionRow {
  sessionId: string;
  agent: string;
  project: string;
  firstTs: number;
  lastTs: number;
  cost: number;
  totalTokens: number;
  requests: number;
  models: string;
}

export interface ProjectRow {
  project: string;
  cost: number;
  totalTokens: number;
  requests: number;
  sessions: number;
}

/** A 5-hour billing block. Blocks are computed independently per agent;
 *  gap blocks carry the agent too. */
export interface Block {
  id: string;
  agent: string;
  startMs: number;
  endMs: number;
  actualEndMs: number | null;
  isActive: boolean;
  isGap: boolean;
  cost: number;
  totalTokens: number;
  requests: number;
  models: string[];
  burnRateTpm: number | null;
  burnRateCostPerHour: number | null;
}

export interface ScanStats {
  filesTotal: number;
  filesParsed: number;
  filesRemoved: number;
  entriesInserted: number;
  durationMs: number;
}

export interface SourceInfo {
  agent: string;
  dirs: string[];
  fileCount: number;
}

/** Geometry of the MacBook notch, logical px; all zeros when absent. */
export interface NotchInfo {
  hasNotch: boolean;
  notchWidth: number;
  barHeight: number;
  screenWidth: number;
}

export interface PricingStatus {
  /** "snapshot" = the offline price table embedded in the build. */
  source: "snapshot" | "online";
  /** YYYY-MM-DD of the embedded offline price table. */
  snapshotDate: string;
  /** When online prices were last fetched; null if never. */
  fetchedAtMs: number | null;
  modelCount: number;
  lastError: string | null;
  refreshing: boolean;
  /** Models with recorded usage but no known price (counted as $0). */
  unpricedModels: string[];
}

export type UiLang = "en" | "zh";

export interface QueryParams {
  sinceMs?: number;
  untilMs?: number;
  costMode?: CostMode;
}

export function isCostMode(v: unknown): v is CostMode {
  return v === "auto" || v === "calculate" || v === "display";
}

/** Local midnight `days - 1` calendar days ago (so "7d" covers today + 6
 *  prior days). Calendar arithmetic keeps it on midnight across DST.
 *  Call it at fetch time: the bound moves every midnight. */
export function rangeToSinceMs(
  range: RangeKey,
  now = new Date(),
): number | undefined {
  if (range === "all") return undefined;
  const days = { today: 1, "7d": 7, "30d": 30, "90d": 90 }[range];
  return startOfDay(days - 1, now);
}

export const api = {
  refreshData: () => call<ScanStats>("refresh_data"),
  getOverview: (p: QueryParams) => call<Overview>("get_overview", { ...p }),
  getDaily: (p: QueryParams) => call<DailyRow[]>("get_daily", { ...p }),
  /** Same row shape as getDaily, bucketed by local hour ("HH:00"). */
  getHourly: (p: QueryParams) => call<DailyRow[]>("get_hourly", { ...p }),
  getModels: (p: QueryParams) => call<ModelRow[]>("get_models", { ...p }),
  getSessions: (p: QueryParams & { limit?: number }) =>
    call<SessionRow[]>("get_sessions", { ...p }),
  getProjects: (p: QueryParams & { limit?: number }) =>
    call<ProjectRow[]>("get_projects", { ...p }),
  /** Without `agent`, every agent's blocks, most recent first. */
  getBlocks: (p: { sinceMs?: number; costMode?: CostMode; agent?: string }) =>
    call<Block[]>("get_blocks", { ...p }),
  getSources: () => call<SourceInfo[]>("get_sources"),
  getSessionModels: (agent: string, sessionId: string, costMode?: CostMode) =>
    call<ModelRow[]>("get_session_models", { agent, sessionId, costMode }),
  getTrayMode: () => call<string>("get_tray_mode"),
  setTrayMode: (mode: string) => call<void>("set_tray_mode", { mode }),
  showMainWindow: () => call<void>("show_main_window"),
  getNotchInfo: () => call<NotchInfo>("get_notch_info"),
  getNotchEnabled: () => call<boolean>("get_notch_enabled"),
  setNotchEnabled: (enabled: boolean) =>
    call<void>("set_notch_enabled", { enabled }),
  notchResize: (width: number, height: number) =>
    call<void>("notch_resize", { width, height }),
  /** The backend owns the cost mode (it also drives the menu-bar title);
   *  changes broadcast `cost-mode-changed` to every window. */
  getCostMode: () => call<CostMode>("get_cost_mode"),
  setCostMode: (mode: CostMode) => call<void>("set_cost_mode", { mode }),
  getPricingStatus: () => call<PricingStatus>("get_pricing_status"),
  /** Slow (network). Resolves with `lastError` set instead of rejecting
   *  when the fetch fails. */
  refreshPricing: () => call<PricingStatus>("refresh_pricing"),
  /** Localizes the menu-bar tooltip. */
  setLanguage: (lang: UiLang) => call<void>("set_language", { lang }),
};
