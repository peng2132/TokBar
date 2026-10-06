import { useCallback, useEffect, useRef, useState } from "react";
import {
  api,
  onEvent,
  type AgentBreakdown,
  type Block,
  type CostMode,
} from "./api";
import { blockLookbackSinceMs, pickActiveBlock } from "./blocks";
import { useCostMode } from "./costMode";
import { startOfMonth, startOfToday, useDayKey } from "./dates";

/** Today's glanceable numbers, shared by the quick panel and notch bar. */
export interface TodayStats {
  todayCost: number;
  todayTokens: number;
  todayRequests: number;
  /** Today's per-agent split, most expensive first. */
  byAgent: AgentBreakdown[];
  /** This calendar month's cost (null unless `month` was requested). */
  monthCost: number | null;
  /** Today's cost per local hour, 24 slots (null unless `hourly`). */
  hourly: number[] | null;
  /** Active block of the most recently active agent (blocks are per agent). */
  activeBlock: Block | null;
}

interface Options {
  month?: boolean;
  hourly?: boolean;
  /** Poll interval in ms (keeps the burn rate current). */
  pollMs: number;
  /** For surfaces that are hidden most of the time (the quick panel):
   *  skip polls and live updates while hidden, reload whenever shown. */
  pauseWhileHidden?: boolean;
}

async function fetchTodayStats(
  costMode: CostMode,
  month: boolean,
  hourly: boolean,
): Promise<TodayStats> {
  // Bounds are computed per fetch, so a load after midnight is "today".
  const now = new Date();
  const today = startOfToday(now);
  const [overview, monthOverview, blocks, hourlyRows] = await Promise.all([
    api.getOverview({ sinceMs: today, costMode }),
    month ? api.getOverview({ sinceMs: startOfMonth(now), costMode }) : null,
    api.getBlocks({ sinceMs: blockLookbackSinceMs(now.getTime()), costMode }),
    hourly ? api.getHourly({ sinceMs: today, costMode }) : null,
  ]);
  let hourlyCost: number[] | null = null;
  if (hourlyRows) {
    hourlyCost = Array<number>(24).fill(0);
    for (const r of hourlyRows) {
      const h = parseInt(r.date, 10);
      if (h >= 0 && h < 24) hourlyCost[h] += r.cost;
    }
  }
  return {
    todayCost: overview.totals.cost,
    todayTokens: overview.totals.totalTokens,
    todayRequests: overview.totals.requests,
    byAgent: overview.byAgent,
    monthCost: monthOverview ? monthOverview.totals.cost : null,
    hourly: hourlyCost,
    activeBlock: pickActiveBlock(blocks),
  };
}

/**
 * Loads today's stats with the backend cost mode, reloading on live
 * usage updates, a poll, cost-mode changes and day rollover. Failures
 * set `error` (the last good stats stay visible) instead of rejecting.
 */
export function useTodayStats({
  month = false,
  hourly = false,
  pollMs,
  pauseWhileHidden = false,
}: Options) {
  const { mode: costMode } = useCostMode();
  const dayKey = useDayKey();
  const [stats, setStats] = useState<TodayStats | null>(null);
  const [error, setError] = useState(false);
  // Latest-wins: an older response never overwrites a newer one.
  const gen = useRef(0);
  const debounce = useRef<number | undefined>(undefined);

  const load = useCallback(async () => {
    if (!costMode) return;
    const mine = ++gen.current;
    try {
      const next = await fetchTodayStats(costMode, month, hourly);
      if (mine !== gen.current) return;
      setStats(next);
      setError(false);
    } catch (e) {
      if (mine !== gen.current) return;
      console.error("loading today's stats failed:", e);
      setError(true);
    }
  }, [costMode, month, hourly]);

  // Coalesce triggers that fire together (opening the panel fires both
  // window focus and visibilitychange) into a single fetch.
  const request = useCallback(() => {
    window.clearTimeout(debounce.current);
    debounce.current = window.setTimeout(() => void load(), 80);
  }, [load]);

  // First load once the cost mode is known; again when it or the day changes.
  useEffect(() => {
    request();
  }, [request, dayKey]);

  useEffect(() => {
    const visible = () => !document.hidden;
    const tick = () => {
      if (!pauseWhileHidden || visible()) request();
    };
    const onShow = () => {
      if (visible()) request();
    };
    const timer = window.setInterval(tick, pollMs);
    const unlisten = onEvent("usage-updated", tick);
    if (pauseWhileHidden) {
      window.addEventListener("focus", onShow);
      document.addEventListener("visibilitychange", onShow);
    }
    return () => {
      window.clearInterval(timer);
      unlisten.then((fn) => fn());
      window.removeEventListener("focus", onShow);
      document.removeEventListener("visibilitychange", onShow);
    };
  }, [request, pollMs, pauseWhileHidden]);

  useEffect(() => () => window.clearTimeout(debounce.current), []);

  return { stats, error, reload: load, costMode };
}
