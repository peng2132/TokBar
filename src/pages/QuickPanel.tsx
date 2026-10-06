import { useCallback, useEffect, useRef, useState } from "react";
import { emit } from "@tauri-apps/api/event";
import {
  AlertTriangle,
  Check,
  Flame,
  LayoutDashboard,
  RefreshCw,
} from "lucide-react";
import { IN_TAURI, api } from "@/lib/api";
import {
  agentLabel,
  formatCost,
  formatNumber,
  formatTime,
  formatTokens,
} from "@/lib/format";
import { useI18n } from "@/lib/i18n";
import { useTodayStats } from "@/lib/useTodayStats";
import { cn } from "@/lib/utils";
import { Logo } from "@/components/Logo";

export function QuickPanel() {
  const { t } = useI18n();
  // Reloads when shown, on live usage updates and on a slow poll so the
  // burn rate stays current. The panel is hidden ~99% of the time
  // (hide-on-blur, webview stays alive), so polls and watcher events are
  // skipped while hidden — the reload on show catches up.
  const { stats, error, reload } = useTodayStats({
    month: true,
    pollMs: 30_000,
    pauseWhileHidden: true,
  });
  const [refreshing, setRefreshing] = useState(false);
  const [justRefreshed, setJustRefreshed] = useState(false);
  const [refreshFailed, setRefreshFailed] = useState(false);
  const feedbackTimer = useRef<number | undefined>(undefined);

  useEffect(() => () => window.clearTimeout(feedbackTimer.current), []);

  // Minimum spin + a brief check mark: the file watcher keeps data
  // current, so the scan usually finishes in milliseconds and the click
  // would otherwise look like a no-op.
  const refresh = useCallback(async () => {
    setRefreshing(true);
    setRefreshFailed(false);
    window.clearTimeout(feedbackTimer.current);
    try {
      await Promise.all([
        api.refreshData(),
        new Promise((r) => setTimeout(r, 500)),
      ]);
      await reload();
      setJustRefreshed(true);
      feedbackTimer.current = window.setTimeout(
        () => setJustRefreshed(false),
        1800,
      );
    } catch (e) {
      console.error("refresh failed:", e);
      setRefreshFailed(true);
      feedbackTimer.current = window.setTimeout(
        () => setRefreshFailed(false),
        4000,
      );
    } finally {
      setRefreshing(false);
    }
  }, [reload]);

  // Deep link: open the main window already switched to the right page.
  const openPage = useCallback(async (page: string) => {
    try {
      await api.showMainWindow();
      if (IN_TAURI) {
        await emit("navigate-page", page);
      }
    } catch (e) {
      console.error("opening the dashboard failed:", e);
    }
  }, []);

  const block = stats?.activeBlock ?? null;

  return (
    <div className="flex h-screen flex-col overflow-hidden rounded-2xl border border-border bg-background p-4 text-foreground">
      {/* Header */}
      <div className="mb-3 flex items-center justify-between">
        <div className="flex items-center gap-2">
          <Logo size={24} />
          <span className="text-sm font-semibold">TokBar</span>
        </div>
        <button
          onClick={refresh}
          disabled={refreshing}
          className="rounded-md p-1.5 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
          title={refreshFailed ? t("app.refreshFailed") : t("app.refresh")}
          aria-label={refreshFailed ? t("app.refreshFailed") : t("app.refresh")}
        >
          {refreshFailed && !refreshing ? (
            <AlertTriangle className="h-3.5 w-3.5 text-red-500" />
          ) : justRefreshed && !refreshing ? (
            <Check className="h-3.5 w-3.5 text-primary" />
          ) : (
            <RefreshCw className={cn("h-3.5 w-3.5", refreshing && "animate-spin")} />
          )}
        </button>
      </div>

      {/* Load failure: keep the last good numbers, say they may be stale. */}
      {error && (
        <div
          role="alert"
          className="mb-3 flex items-center justify-between gap-2 rounded-lg bg-red-500/10 px-3 py-1.5 text-xs text-red-500"
        >
          <span className="flex items-center gap-1.5">
            <AlertTriangle className="h-3.5 w-3.5" />
            {t("common.loadFailed")}
          </span>
          <button
            type="button"
            onClick={() => void reload()}
            className="rounded px-1.5 py-0.5 font-medium transition-colors hover:bg-red-500/10"
          >
            {t("common.retry")}
          </button>
        </div>
      )}

      {/* Today cost — every card deep-links into the matching page */}
      <button
        type="button"
        onClick={() => openPage("overview")}
        className="w-full rounded-xl bg-card p-4 text-left transition-colors hover:bg-accent/60"
      >
        <div className="text-xs text-muted-foreground">{t("quick.todayCost")}</div>
        <div className="mt-1 text-3xl font-bold tabular-nums text-primary">
          {stats ? formatCost(stats.todayCost) : "—"}
        </div>
      </button>

      {/* Stats grid */}
      <div className="mt-3 grid grid-cols-3 gap-2">
        <MiniStat
          label={t("quick.todayTokens")}
          value={stats ? formatTokens(stats.todayTokens) : "—"}
          onClick={() => openPage("models")}
        />
        <MiniStat
          label={t("quick.todayRequests")}
          value={stats ? formatNumber(stats.todayRequests) : "—"}
          onClick={() => openPage("sessions")}
        />
        <MiniStat
          label={t("quick.monthCost")}
          value={stats?.monthCost != null ? formatCost(stats.monthCost) : "—"}
          onClick={() => openPage("trends")}
        />
      </div>

      {/* Active block (of the most recently active agent; blocks are per
          agent, so the header names whose it is). */}
      <button
        type="button"
        onClick={() => openPage("blocks")}
        className="mt-3 w-full flex-1 rounded-xl bg-card p-4 text-left transition-colors hover:bg-accent/60"
      >
        <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <Flame
            className={cn("h-3.5 w-3.5 shrink-0", block ? "text-primary" : "")}
          />
          <span className="truncate">
            {t("quick.activeBlock")}
            {block && ` · ${agentLabel(block.agent)}`}
          </span>
        </div>
        {block ? (
          <div className="mt-2 space-y-2">
            <div className="flex items-baseline justify-between">
              <span className="text-xl font-semibold tabular-nums">
                {formatCost(block.cost)}
              </span>
              <span className="text-xs text-muted-foreground">
                {formatTokens(block.totalTokens)} {t("unit.tok")}
              </span>
            </div>
            <div className="space-y-1 text-xs text-muted-foreground">
              {block.burnRateTpm != null && (
                <div className="flex justify-between">
                  <span>{t("quick.burnRate")}</span>
                  <span className="tabular-nums text-foreground">
                    {formatTokens(Math.round(block.burnRateTpm))}
                    {t("unit.perMin")}
                  </span>
                </div>
              )}
              {block.burnRateCostPerHour != null && (
                <div className="flex justify-between">
                  <span>{t("quick.costRate")}</span>
                  <span className="tabular-nums text-foreground">
                    {formatCost(block.burnRateCostPerHour)}
                    {t("unit.perHour")}
                  </span>
                </div>
              )}
              <div className="flex justify-between">
                <span>{t("quick.blockEnds")}</span>
                <span className="tabular-nums text-foreground">
                  {formatTime(block.endMs)}
                </span>
              </div>
            </div>
          </div>
        ) : (
          <div className="mt-3 text-sm text-muted-foreground">
            {t("quick.noActiveBlock")}
          </div>
        )}
      </button>

      {/* Footer */}
      <button
        onClick={() =>
          api
            .showMainWindow()
            .catch((e) => console.error("opening the dashboard failed:", e))
        }
        className="mt-3 flex w-full items-center justify-center gap-2 rounded-lg bg-primary py-2 text-sm font-medium text-primary-foreground transition-opacity hover:opacity-90"
      >
        <LayoutDashboard className="h-4 w-4" />
        {t("quick.openDashboard")}
      </button>
    </div>
  );
}

function MiniStat({
  label,
  value,
  onClick,
}: {
  label: string;
  value: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="min-w-0 rounded-xl bg-card p-3 text-left transition-colors hover:bg-accent/60"
    >
      <div className="truncate text-[10px] text-muted-foreground">{label}</div>
      <div className="mt-0.5 truncate text-sm font-semibold tabular-nums">
        {value}
      </div>
    </button>
  );
}
