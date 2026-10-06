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

type Page = "overview" | "trends" | "sessions" | "models" | "blocks" | "settings";

const NAV: { id: Page; labelKey: I18nKey; icon: typeof LayoutDashboard }[] = [
  { id: "overview", labelKey: "nav.overview", icon: LayoutDashboard },
  { id: "trends", labelKey: "nav.trends", icon: BarChart3 },
  { id: "sessions", labelKey: "nav.sessions", icon: ListTree },
  { id: "models", labelKey: "nav.models", icon: Bot },
  { id: "blocks", labelKey: "nav.blocks", icon: Timer },
  { id: "settings", labelKey: "nav.settings", icon: Settings },
];

// With the macOS overlay title bar (transparent, traffic lights floating
// over the content) the sidebar header leaves room for the buttons and the
// top strip doubles as the window drag handle.
const IS_MAC = navigator.userAgent.includes("Mac");

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

function App() {
  const { t } = useI18n();
  const [page, setPage] = useState<Page>("overview");
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
    // Deep links from the quick panel's stat cards.
    const unlistenNav = onEvent<string>("navigate-page", (e) => {
      if (NAV.some((n) => n.id === e.payload)) {
        setPage(e.payload as Page);
      }
    });
    return () => {
      clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisibility);
      unlistenUpdate.then((fn) => fn());
      unlistenProgress.then((fn) => fn());
      unlistenNav.then((fn) => fn());
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

  return (
    <div className="flex h-full">
      {/* Sidebar: on macOS it stays transparent so the window vibrancy
          (desktop blurred through) is strongest here, like Finder /
          System Settings; elsewhere it keeps its solid surface step. */}
      <aside
        className={cn(
          "flex w-52 shrink-0 flex-col border-r border-border",
          IS_MAC ? "bg-transparent" : "bg-card/50",
        )}
      >
        <div
          data-tauri-drag-region
          className={cn(
            "flex items-center gap-2.5 px-5 pb-5",
            IS_MAC ? "pt-11" : "pt-5",
          )}
        >
          <Logo size={32} />
          <div>
            <div className="text-sm font-semibold leading-none">TokBar</div>
            <div className="mt-0.5 text-[10px] text-muted-foreground">
              {t("app.subtitle")}
            </div>
          </div>
        </div>
        <nav className="flex-1 space-y-1 px-3">
          {NAV.map((item) => (
            <button
              key={item.id}
              onClick={() => setPage(item.id)}
              className={cn(
                "flex w-full items-center gap-2.5 rounded-lg px-3 py-2 text-sm transition-[color,background-color,transform] duration-150 active:scale-[0.97]",
                page === item.id
                  ? "bg-primary/10 font-medium text-primary"
                  : "text-muted-foreground hover:bg-accent/50 hover:text-foreground",
              )}
            >
              <item.icon className="h-4 w-4" />
              {t(item.labelKey)}
            </button>
          ))}
        </nav>
      </aside>

      {/* Main: carries the content wash (translucent on macOS, solid
          elsewhere) so data stays legible over the vibrancy. */}
      <div className="flex min-w-0 flex-1 flex-col bg-background">
        <header
          data-tauri-drag-region
          className="flex items-center justify-between border-b border-border px-6 py-3"
        >
          <h1 className="text-base font-semibold">
            {t(NAV.find((n) => n.id === page)!.labelKey)}
          </h1>
          <div className="flex items-center gap-2">
            {showRange && (
              <div className="flex rounded-lg border border-border p-0.5">
                {RANGES.map((r) => (
                  <button
                    key={r.id}
                    onClick={() => setRange(r.id)}
                    aria-pressed={range === r.id}
                    className={cn(
                      "rounded-md px-3 py-1 text-xs font-medium transition-colors",
                      range === r.id
                        ? "bg-primary/10 text-primary"
                        : "text-muted-foreground hover:text-foreground",
                    )}
                  >
                    {t(r.labelKey)}
                  </button>
                ))}
              </div>
            )}
            <Button
              variant="outline"
              size="sm"
              onClick={() => refresh(true)}
              disabled={scanning}
              title={refreshError && !scanning ? refreshError : undefined}
              className="min-w-24"
            >
              {refreshError && !scanning ? (
                <AlertTriangle className="h-3.5 w-3.5 text-red-500" />
              ) : justRefreshed && !scanning ? (
                <Check className="h-3.5 w-3.5 text-primary" />
              ) : (
                <RefreshCw
                  className={cn("h-3.5 w-3.5", scanning && "animate-spin")}
                />
              )}
              <span aria-live="polite">
                {scanProgress
                  ? t("app.scanProgress", {
                      done: scanProgress.done,
                      total: scanProgress.total,
                    })
                  : scanning
                    ? t("app.scanning")
                    : refreshError
                      ? t("app.refreshFailed")
                      : justRefreshed
                        ? justRefreshed.entriesInserted > 0
                          ? t("app.refreshed", {
                              n: justRefreshed.entriesInserted,
                            })
                          : t("app.upToDate")
                        : t("app.refresh")}
              </span>
            </Button>
          </div>
        </header>

        <main className="flex-1 overflow-y-auto p-6">
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
        </main>
      </div>
    </div>
  );
}

export default App;
