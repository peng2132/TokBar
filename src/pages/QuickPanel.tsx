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
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { LatestOutput } from "@/components/LatestOutput";

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
    <div className="flex h-full flex-col overflow-hidden rounded-[16px] border border-border bg-card text-foreground">
      <header className="flex h-13 shrink-0 items-center justify-between border-b border-border px-4">
        <div className="flex items-center gap-2">
          <Logo size={23} />
          <span className="text-sm font-semibold tracking-tight">TokBar</span>
        </div>
        <Button
          variant="ghost"
          size="icon"
          onClick={() => void refresh()}
          disabled={refreshing}
          title={refreshFailed ? t("app.refreshFailed") : t("app.refresh")}
          aria-label={refreshFailed ? t("app.refreshFailed") : t("app.refresh")}
        >
          {refreshFailed && !refreshing ? (
            <AlertTriangle className="size-4 text-danger" />
          ) : justRefreshed && !refreshing ? (
            <Check className="size-4 text-success" />
          ) : (
            <RefreshCw
              className={cn("size-4", refreshing && "motion-safe:animate-spin")}
            />
          )}
        </Button>
      </header>

      <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto px-4 pt-3.5 pb-3">
        {error && (
          <div
            role="alert"
            className="flex items-center justify-between gap-2 rounded-lg bg-danger-surface px-3 py-1.5 text-xs text-danger"
          >
            <span className="flex items-center gap-1.5">
              <AlertTriangle className="size-3.5" />
              {t("common.loadFailed")}
            </span>
            <Button variant="ghost" size="sm" onClick={() => void reload()}>
              {t("common.retry")}
            </Button>
          </div>
        )}
        <button
          type="button"
          onClick={() => void openPage("overview")}
          className="w-full rounded-lg text-left"
        >
          <div className="text-xs font-medium text-muted-foreground">
            {t("quick.todayCost")}
          </div>
          <div className="mt-1 text-[34px] leading-none font-semibold tracking-[-0.045em] tabular-nums">
            {stats ? formatCost(stats.todayCost) : "—"}
          </div>
        </button>
        <div className="grid grid-cols-3 divide-x divide-border rounded-lg border border-border bg-background/65">
          <MiniStat
            label={t("quick.todayTokens")}
            value={stats ? formatTokens(stats.todayTokens) : "—"}
            onClick={() => void openPage("models")}
          />
          <MiniStat
            label={t("quick.todayRequests")}
            value={stats ? formatNumber(stats.todayRequests) : "—"}
            onClick={() => void openPage("sessions")}
          />
          <MiniStat
            label={t("quick.monthCost")}
            value={stats?.monthCost != null ? formatCost(stats.monthCost) : "—"}
            onClick={() => void openPage("trends")}
          />
        </div>

        <LatestOutput
          sample={stats?.latestPerformance}
          loading={!stats}
          onClick={() => void openPage("models")}
        />

        <button
          type="button"
          onClick={() => void openPage("blocks")}
          className="w-full rounded-lg border border-border px-3 py-2.5 text-left transition-colors hover:bg-accent/35"
        >
          <div className="flex items-center justify-between gap-2">
            <span className="flex items-center gap-1.5 text-xs font-medium">
              <Flame className="size-3.5 text-muted-foreground" aria-hidden />
              {t("quick.activeBlock")}
            </span>
            {block && (
              <Badge variant="success" className="text-[10px]">
                {agentLabel(block.agent)}
              </Badge>
            )}
          </div>
          {block ? (
            <>
              <div className="mt-2 flex items-baseline justify-between gap-3">
                <span className="text-lg font-semibold tabular-nums">
                  {formatCost(block.cost)}
                </span>
                <span className="text-xs text-muted-foreground">
                  {formatTokens(block.totalTokens)} {t("unit.tok")}
                </span>
              </div>
              <dl className="mt-2 flex flex-col gap-1 border-t border-border pt-2 text-[11px]">
                {block.burnRateTpm != null && (
                  <div className="flex justify-between gap-2">
                    <dt className="text-muted-foreground">
                      {t("quick.burnRate")}
                    </dt>
                    <dd className="tabular-nums">
                      {formatTokens(Math.round(block.burnRateTpm))}
                      {t("unit.perMin")}
                    </dd>
                  </div>
                )}
                {block.burnRateCostPerHour != null && (
                  <div className="flex justify-between gap-2">
                    <dt className="text-muted-foreground">
                      {t("quick.costRate")}
                    </dt>
                    <dd className="tabular-nums">
                      {formatCost(block.burnRateCostPerHour)}
                      {t("unit.perHour")}
                    </dd>
                  </div>
                )}
                <div className="flex justify-between gap-2">
                  <dt className="text-muted-foreground">
                    {t("quick.blockEnds")}
                  </dt>
                  <dd className="tabular-nums">{formatTime(block.endMs)}</dd>
                </div>
              </dl>
            </>
          ) : (
            <p className="mt-1.5 text-xs text-muted-foreground">
              {t("quick.noActiveBlock")}
            </p>
          )}
        </button>
      </div>

      <footer className="shrink-0 border-t border-border px-4 py-3">
        <Button
          variant="outline"
          className="w-full"
          onClick={() =>
            api
              .showMainWindow()
              .catch((error) =>
                console.error("opening the dashboard failed:", error),
              )
          }
        >
          <LayoutDashboard aria-hidden className="size-4" />
          {t("quick.openDashboard")}
        </Button>
      </footer>
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
      className="min-w-0 px-2.5 py-2 text-left transition-colors first:rounded-l-lg last:rounded-r-lg hover:bg-accent/65"
    >
      <div className="truncate text-[10px] text-muted-foreground" title={label}>
        {label}
      </div>
      <div
        className="mt-0.5 truncate text-sm font-semibold tabular-nums"
        title={value}
      >
        {value}
      </div>
    </button>
  );
}
