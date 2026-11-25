import {
  Suspense,
  lazy,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  AlertTriangle,
  BarChart3,
  Bot,
  Check,
  LayoutDashboard,
  ListTree,
  RefreshCw,
  Settings,
  Timer,
} from "lucide-react";
import {
  api,
  onEvent,
  rangeToSinceMs,
  type QueryParams,
  type RangeKey,
  type ScanStats,
} from "@/lib/api";
import { useCostMode } from "@/lib/costMode";
import { useDayKey } from "@/lib/dates";
import { cn } from "@/lib/utils";
import { useI18n, type I18nKey } from "@/lib/i18n";
import { Button } from "@/components/ui/button";
import { SegmentedControl } from "@/components/ui/segmented-control";
import { Skeleton } from "@/components/ui/skeleton";
import { Logo } from "@/components/Logo";

// Lazy pages: keeps recharts and the page bodies out of the initial
// bundle, which the lightweight quick panel shares.
const OverviewPage = lazy(() =>
  import("@/pages/Overview").then((m) => ({ default: m.OverviewPage })),
);
const TrendsPage = lazy(() =>
  import("@/pages/Trends").then((m) => ({ default: m.TrendsPage })),
);
const SessionsPage = lazy(() =>
  import("@/pages/Sessions").then((m) => ({ default: m.SessionsPage })),
);
const ModelsPage = lazy(() =>
  import("@/pages/Models").then((m) => ({ default: m.ModelsPage })),
);
const BlocksPage = lazy(() =>
  import("@/pages/Blocks").then((m) => ({ default: m.BlocksPage })),
);
const SettingsPage = lazy(() =>
  import("@/pages/Settings").then((m) => ({ default: m.SettingsPage })),
);

export type Page =
  | "overview"
  | "trends"
  | "sessions"
  | "models"
  | "blocks"
  | "settings";

const NAV: { id: Page; labelKey: I18nKey; icon: typeof LayoutDashboard }[] = [
  { id: "overview", labelKey: "nav.overview", icon: LayoutDashboard },
  { id: "trends", labelKey: "nav.trends", icon: BarChart3 },
  { id: "sessions", labelKey: "nav.sessions", icon: ListTree },
  { id: "models", labelKey: "nav.models", icon: Bot },
  { id: "blocks", labelKey: "nav.blocks", icon: Timer },
  { id: "settings", labelKey: "nav.settings", icon: Settings },
];

export function isPage(value: string): value is Page {
  return NAV.some(({ id }) => id === value);
}

const RANGES: { id: RangeKey; labelKey: I18nKey }[] = [
  { id: "today", labelKey: "range.today" },
  { id: "7d", labelKey: "range.7d" },
  { id: "30d", labelKey: "range.30d" },
  { id: "90d", labelKey: "range.90d" },
  { id: "all", labelKey: "range.all" },
];

// Persisted like lang/theme/subscriptions: the selected range survives
// restarts instead of silently resetting. (The cost mode lives in the
// backend; see useCostMode.)
const RANGE_KEY = "tokbar-range";

/** A progress label that stops advancing this long is dropped: a scan
 *  that dies mid-way never sends its final done === total event. */
const PROGRESS_STALE_MS = 10_000;

function loadRange(): RangeKey {
  const v = localStorage.getItem(RANGE_KEY);
  return RANGES.some((r) => r.id === v) ? (v as RangeKey) : "30d";
}

function App({
  page,
  setPage,
}: {
  page: Page;
  setPage: (page: Page) => void;
}) {
  const { t } = useI18n();
  const [range, setRange] = useState<RangeKey>(loadRange);
  // Backend-owned (it also drives the menu-bar title); the main window
  // migrates the old localStorage copy once. Null until first read.
  const { mode: costMode, setMode: setCostMode } = useCostMode({
    migrate: true,
  });
  const dayKey = useDayKey();
  const [refreshKey, setRefreshKey] = useState(0);
  const [scanning, setScanning] = useState(false);
  const [lastScan, setLastScan] = useState<ScanStats | null>(null);
  const [scanProgress, setScanProgress] = useState<{
    done: number;
    total: number;
  } | null>(null);

  const [justRefreshed, setJustRefreshed] = useState<ScanStats | null>(null);
  const [refreshError, setRefreshError] = useState<string | null>(null);
  const feedbackTimer = useRef<number | undefined>(undefined);
  const progressTimer = useRef<number | undefined>(undefined);
  const contentRef = useRef<HTMLElement>(null);

  const clearProgress = useCallback(() => {
    window.clearTimeout(progressTimer.current);
    setScanProgress(null);
  }, []);

  // `feedback` is true only for the manual button: the file watcher keeps
  // data current, so a manual scan usually finds nothing and finishes in
  // milliseconds — without a minimum spin and a result label the click
  // looks like a no-op.
  const refresh = useCallback(
    async (feedback = false) => {
      setScanning(true);
      setRefreshError(null);
      try {
        const [stats] = await Promise.all([
          api.refreshData(),
          feedback ? new Promise((r) => setTimeout(r, 500)) : null,
        ]);
        setLastScan(stats);
        setRefreshKey((k) => k + 1);
        if (feedback) {
          setJustRefreshed(stats);
          window.clearTimeout(feedbackTimer.current);
          feedbackTimer.current = window.setTimeout(
            () => setJustRefreshed(null),
            2500,
          );
        }
      } catch (e) {
        console.error("refresh failed:", e);
        // Surfaced on the refresh button (label + tooltip with the cause).
        setJustRefreshed(null);
        setRefreshError(String(e));
        window.clearTimeout(feedbackTimer.current);
        feedbackTimer.current = window.setTimeout(
          () => setRefreshError(null),
          6000,
        );
      } finally {
        setScanning(false);
        // Success or failure, this scan is over: never leave "Scanning
        // 32/100" behind.
        clearProgress();
      }
    },
    [clearProgress],
  );

  useEffect(() => localStorage.setItem(RANGE_KEY, range), [range]);
  useEffect(() => {
    contentRef.current?.scrollTo(0, 0);
  }, [page]);

  // Initial scan on launch. Live updates arrive via the backend file
  // watcher ("usage-updated"); a 5-minute rescan remains as a fallback,
  // skipped while the window is hidden in the tray (closing only hides
  // it) — no point burning disk scans nobody is looking at.
  useEffect(() => {
    refresh();
    const timer = setInterval(() => {
      if (!document.hidden) refresh();
    }, 300_000);
    // While hidden, live updates only mark the data dirty; one refetch
    // happens when the window is shown again.
    let dirty = false;
    const onVisibility = () => {
      if (!document.hidden && dirty) {
        dirty = false;
        setRefreshKey((k) => k + 1);
      }
    };
    document.addEventListener("visibilitychange", onVisibility);
    const unlistenUpdate = onEvent("usage-updated", () => {
      // Emitted after a scan finished (or on re-pricing / midnight).
      clearProgress();
      if (document.hidden) dirty = true;
      else setRefreshKey((k) => k + 1);
    });
    const unlistenProgress = onEvent<{ done: number; total: number }>(
      "scan-progress",
      (e) => {
        window.clearTimeout(progressTimer.current);
        if (e.payload.done >= e.payload.total) {
          setScanProgress(null);
          return;
        }
        setScanProgress(e.payload);
        progressTimer.current = window.setTimeout(
          () => setScanProgress(null),
          PROGRESS_STALE_MS,
        );
      },
    );
    return () => {
      clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisibility);
      unlistenUpdate.then((fn) => fn());
      unlistenProgress.then((fn) => fn());
      window.clearTimeout(feedbackTimer.current);
      window.clearTimeout(progressTimer.current);
    };
  }, [refresh, clearProgress]);

  // Range bounds are recomputed on every refetch (refreshKey) and when
  // the local day rolls over (dayKey), never frozen at selection time:
  // after midnight "Today" must not include yesterday, and 7D/30D/90D
  // must not grow a day. Pages key their fetches on the resulting
  // primitive values, so an unchanged bound costs nothing.
  const params: QueryParams | null = useMemo(
    () => (costMode ? { sinceMs: rangeToSinceMs(range), costMode } : null),
    [range, costMode, dayKey, refreshKey],
  );

  const showRange = page !== "blocks" && page !== "settings";
  const title = t(NAV.find((item) => item.id === page)!.labelKey);
  const refreshLabel = scanProgress
    ? t("app.scanProgress", scanProgress)
    : scanning
      ? t("app.scanning")
      : refreshError
        ? t("app.refreshFailed")
        : justRefreshed
          ? justRefreshed.entriesInserted > 0
            ? t("app.refreshed", { n: justRefreshed.entriesInserted })
            : t("app.upToDate")
          : t("app.refresh");

  return (
    <div className="app-shell">
      <aside className="workspace-sidebar flex min-w-0 flex-col border-border bg-sidebar max-[981px]:border-b min-[981px]:border-r">
        <div
          data-tauri-drag-region
          className="flex h-19 shrink-0 items-center gap-2.5 px-5 max-[981px]:h-11"
        >
          <Logo size={27} />
          <span className="text-[16px] font-semibold tracking-tight">
            TokBar
          </span>
        </div>
        <nav
          aria-label={t("app.navigation")}
          className="flex min-h-0 flex-1 gap-1 px-2.5 pb-2 max-[981px]:grid max-[981px]:grid-cols-6 min-[981px]:flex-col min-[981px]:px-3 min-[981px]:pb-4"
        >
          {NAV.map((item) => (
            <button
              type="button"
              key={item.id}
              onClick={() => setPage(item.id)}
              aria-current={page === item.id ? "page" : undefined}
              className={cn(
                "flex h-9 min-w-0 shrink-0 items-center gap-2 whitespace-nowrap rounded-lg border border-transparent px-2.5 text-left text-sm font-medium text-muted-foreground transition-colors hover:bg-accent/60 hover:text-foreground aria-[current=page]:border-border aria-[current=page]:bg-card aria-[current=page]:text-foreground aria-[current=page]:shadow-xs",
                "max-[981px]:justify-center max-[680px]:h-11 max-[680px]:flex-col max-[680px]:gap-0.5 max-[680px]:px-1 max-[680px]:text-[11px]",
                item.id === "settings" && "min-[981px]:mt-auto",
              )}
            >
              <item.icon
                aria-hidden
                className={cn(
                  "size-4 shrink-0",
                  page === item.id && "text-primary",
                )}
              />
              <span className="max-w-full truncate">{t(item.labelKey)}</span>
            </button>
          ))}
        </nav>
      </aside>

      <div className="flex min-h-0 min-w-0 flex-col bg-background">
        <header
          data-tauri-drag-region
          className="workspace-header shrink-0 border-b border-border"
        >
          <div
            data-tauri-drag-region
            className="mx-auto flex min-h-19 max-w-[1440px] flex-wrap items-center justify-between gap-2.5 px-4 py-3 sm:px-6 lg:px-8"
          >
            <h1 className="text-[25px] leading-tight font-semibold tracking-tight">
              {title}
            </h1>
            <div className="flex max-w-full flex-wrap items-center gap-2">
              {showRange && (
                <SegmentedControl
                  label={t("app.dateRange")}
                  options={RANGES.map((item) => ({
                    id: item.id,
                    label: t(item.labelKey),
                  }))}
                  value={range}
                  onChange={setRange}
                />
              )}
              <Button
                variant="outline"
                size="sm"
                onClick={() => void refresh(true)}
                disabled={scanning}
                title={refreshError && !scanning ? refreshError : undefined}
              >
                {refreshError && !scanning ? (
                  <AlertTriangle className="size-3.5 text-danger" />
                ) : justRefreshed && !scanning ? (
                  <Check className="size-3.5 text-success" />
                ) : (
                  <RefreshCw
                    className={cn(
                      "size-3.5",
                      scanning && "motion-safe:animate-spin",
                    )}
                  />
                )}
                <span aria-live="polite">{refreshLabel}</span>
              </Button>
            </div>
          </div>
        </header>

        <main
          ref={contentRef}
          aria-label={title}
          className="min-h-0 flex-1 overflow-y-auto"
        >
          <div className="mx-auto max-w-[1440px] px-4 py-5 sm:px-6 lg:px-8">
            <Suspense fallback={<Skeleton className="h-80" />}>
              {/* Every page prices with the backend cost mode: wait for it
              (one quick IPC read) rather than fetch twice. */}
              {!params || !costMode ? (
                <Skeleton className="h-80" />
              ) : (
                <>
                  {page === "overview" && (
                    <OverviewPage
                      params={params}
                      refreshKey={refreshKey}
                      hourly={range === "today"}
                    />
                  )}
                  {page === "trends" && (
                    <TrendsPage
                      params={params}
                      refreshKey={refreshKey}
                      hourly={range === "today"}
                    />
                  )}
                  {page === "sessions" && (
                    <SessionsPage params={params} refreshKey={refreshKey} />
                  )}
                  {page === "models" && (
                    <ModelsPage params={params} refreshKey={refreshKey} />
                  )}
                  {page === "blocks" && (
                    <BlocksPage costMode={costMode} refreshKey={refreshKey} />
                  )}
                  {page === "settings" && (
                    <SettingsPage
                      costMode={costMode}
                      onCostModeChange={setCostMode}
                      lastScan={lastScan}
                    />
                  )}
                </>
              )}
            </Suspense>
          </div>
        </main>
      </div>
    </div>
  );
}

export default App;
