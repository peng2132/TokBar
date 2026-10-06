import { useCallback, useEffect, useRef, useState } from "react";
import { disable, enable, isEnabled } from "@tauri-apps/plugin-autostart";
import {
  AlertTriangle,
  ChevronDown,
  CreditCard,
  Database,
  DollarSign,
  FolderSearch,
  Languages,
  MenuSquare,
  Palette,
  RefreshCw,
  Rocket,
  Trash2,
} from "lucide-react";
import {
  IN_TAURI,
  api,
  onEvent,
  type CostMode,
  type NotchInfo,
  type PricingStatus,
  type ScanStats,
  type SourceInfo,
} from "@/lib/api";
import { agentLabel, formatDateTime, formatNumber } from "@/lib/format";
import { useI18n, type I18nKey, type Lang } from "@/lib/i18n";
import { useSubscriptions } from "@/lib/subscriptions";
import { AgentMultiSelect } from "@/components/AgentMultiSelect";
import { BrandIcon, agentBrand, subscriptionBrand } from "@/components/BrandIcon";
import { ACCENTS, useTheme, type AccentKey, type ThemeMode } from "@/lib/theme";
import { cn } from "@/lib/utils";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";

/** What the menu bar shows when the notch bar is not in use (the backend
 *  hides the menu-bar title by itself while the notch bar is shown). */
type TrayMode = "cost" | "tokens" | "off";

const isTrayMode = (v: unknown): v is TrayMode =>
  v === "cost" || v === "tokens" || v === "off";

const SUB_PRESETS: { name: string; monthlyUsd: number; agents: string[] }[] = [
  { name: "Claude Pro", monthlyUsd: 20, agents: ["claude-code"] },
  { name: "Claude Max 5×", monthlyUsd: 100, agents: ["claude-code"] },
  { name: "Claude Max 20×", monthlyUsd: 200, agents: ["claude-code"] },
  { name: "ChatGPT Plus", monthlyUsd: 20, agents: ["codex"] },
  { name: "ChatGPT Pro", monthlyUsd: 200, agents: ["codex"] },
  { name: "Copilot Pro", monthlyUsd: 10, agents: ["copilot"] },
  { name: "Gemini Advanced", monthlyUsd: 20, agents: ["gemini"] },
];

export function SettingsPage({
  costMode,
  onCostModeChange,
  lastScan,
}: {
  costMode: CostMode;
  /** Optimistic; rejects (after rolling back) if the backend refuses. */
  onCostModeChange: (mode: CostMode) => Promise<void>;
  lastScan: ScanStats | null;
}) {
  const { t, lang, setLang } = useI18n();
  const { mode, accent, setMode, setAccent } = useTheme();
  const { subscriptions, add, update, remove } = useSubscriptions();
  const [sources, setSources] = useState<SourceInfo[]>([]);
  const [trayMode, setTrayMode] = useState<TrayMode>("cost");
  const [autostart, setAutostart] = useState(false);
  const [showInactive, setShowInactive] = useState(false);
  // Notch companion bar: the position picker only appears on notched
  // MacBooks. The backend enforces exclusivity (no menu-bar title while
  // the notch bar is shown), so switching position is a single call.
  const [notchSupported, setNotchSupported] = useState(false);
  const [notchEnabled, setNotchEnabled] = useState(false);
  const notchOn = notchSupported && notchEnabled;
  // A setting the backend refused: the control has already been rolled
  // back; this banner says why it jumped back.
  const [saveError, setSaveError] = useState(false);
  const saveErrorTimer = useRef<number | undefined>(undefined);
  const [pricing, setPricing] = useState<PricingStatus | null>(null);
  const [pricingLoadError, setPricingLoadError] = useState(false);
  const [pricingBusy, setPricingBusy] = useState(false);
  // Two-step delete: first click arms the button (turns into a red
  // "confirm" label), second click deletes; auto-disarms after 3s.
  const [confirmRemove, setConfirmRemove] = useState<string | null>(null);

  const askRemove = (id: string) => {
    if (confirmRemove === id) {
      remove(id);
      setConfirmRemove(null);
      return;
    }
    setConfirmRemove(id);
    setTimeout(() => setConfirmRemove((c) => (c === id ? null : c)), 3000);
  };

  const failSave = useCallback((what: string, e: unknown) => {
    console.error(`${what} failed:`, e);
    setSaveError(true);
    window.clearTimeout(saveErrorTimer.current);
    saveErrorTimer.current = window.setTimeout(() => setSaveError(false), 5000);
  }, []);

  useEffect(() => () => window.clearTimeout(saveErrorTimer.current), []);

  useEffect(() => {
    let cancelled = false;
    api
      .getSources()
      .then((s) => !cancelled && setSources(s))
      .catch((e) => console.error("get_sources failed:", e));
    Promise.all([api.getNotchInfo(), api.getNotchEnabled(), api.getTrayMode()])
      .then(([info, enabled, tray]) => {
        if (cancelled) return;
        setNotchSupported(info.hasNotch);
        setNotchEnabled(enabled);
        if (isTrayMode(tray)) setTrayMode(tray);
      })
      .catch((e) => console.error("reading display settings failed:", e));
    if (IN_TAURI) {
      isEnabled()
        .then((on) => !cancelled && setAutostart(on))
        .catch((e) => console.error("autostart query failed:", e));
    }
    // Displays come and go (clamshell, external monitor): follow them.
    const unlistenNotch = onEvent<NotchInfo>("notch-info-changed", (e) =>
      setNotchSupported(e.payload.hasNotch),
    );
    return () => {
      cancelled = true;
      unlistenNotch.then((fn) => fn());
    };
  }, []);

  // Pricing status: read once, then follow the backend's broadcasts
  // (manual or daily auto refresh) and new usage (unpriced models list).
  useEffect(() => {
    let cancelled = false;
    const read = () =>
      api
        .getPricingStatus()
        .then((s) => {
          if (cancelled) return;
          setPricing(s);
          setPricingLoadError(false);
        })
        .catch((e) => {
          console.error("get_pricing_status failed:", e);
          if (!cancelled) setPricingLoadError(true);
        });
    read();
    const unlistenPricing = onEvent<PricingStatus>("pricing-updated", (e) => {
      setPricing(e.payload);
      setPricingLoadError(false);
    });
    const unlistenUsage = onEvent("usage-updated", () => read());
    return () => {
      cancelled = true;
      unlistenPricing.then((fn) => fn());
      unlistenUsage.then((fn) => fn());
    };
  }, []);

  const refreshPrices = async () => {
    setPricingBusy(true);
    try {
      // Resolves with `lastError` set when the fetch fails.
      setPricing(await api.refreshPricing());
      setPricingLoadError(false);
    } catch (e) {
      console.error("refresh_pricing failed:", e);
      setPricing((p) =>
        p ? { ...p, refreshing: false, lastError: String(e) } : p,
      );
      if (!pricing) setPricingLoadError(true);
    } finally {
      setPricingBusy(false);
    }
  };
  const pricingRefreshing = pricingBusy || (pricing?.refreshing ?? false);

  /** Optimistic write: show `next` now, restore `prev` if it fails. */
  const persist = async <T,>(
    set: (v: T) => void,
    prev: T,
    next: T,
    write: () => Promise<void>,
    what: string,
  ) => {
    set(next);
    try {
      await write();
    } catch (e) {
      set(prev);
      failSave(what, e);
    }
  };

  const choosePosition = (notch: boolean) => {
    if (notch === notchEnabled) return;
    void persist(
      setNotchEnabled,
      notchEnabled,
      notch,
      () => api.setNotchEnabled(notch),
      "set_notch_enabled",
    );
  };

  const changeTrayMode = (m: TrayMode) => {
    if (m === trayMode) return;
    void persist(
      setTrayMode,
      trayMode,
      m,
      () => api.setTrayMode(m),
      "set_tray_mode",
    );
  };

  const changeCostMode = (m: CostMode) => {
    if (m === costMode) return;
    onCostModeChange(m).catch((e) => failSave("set_cost_mode", e));
  };

  const toggleAutostart = async () => {
    try {
      if (autostart) {
        await disable();
        setAutostart(false);
      } else {
        await enable();
        setAutostart(true);
      }
    } catch (e) {
      failSave("autostart toggle", e);
    }
  };

  const languages: { value: Lang; label: string }[] = [
    { value: "zh", label: "中文" },
    { value: "en", label: "English" },
  ];

  const modes: { value: CostMode; label: string; desc: string }[] = [
    {
      value: "auto",
      label: t("settings.mode.auto"),
      desc: t("settings.mode.autoDesc"),
    },
    {
      value: "calculate",
      label: t("settings.mode.calculate"),
      desc: t("settings.mode.calculateDesc"),
    },
    {
      value: "display",
      label: t("settings.mode.display"),
      desc: t("settings.mode.displayDesc"),
    },
  ];

  const pillBtn = (selected: boolean) =>
    cn(
      "rounded-lg border px-4 py-2 text-sm font-medium transition-colors",
      selected
        ? "border-primary bg-primary/10 text-foreground"
        : "border-border text-muted-foreground hover:bg-accent hover:text-foreground",
    );

  const active = sources.filter((s) => s.fileCount > 0);
  const inactive = sources.filter((s) => s.fileCount === 0);

  return (
    <div className="max-w-3xl space-y-4">
      {saveError && (
        <div
          role="alert"
          className="sticky top-0 z-10 flex items-center gap-2 rounded-lg border border-red-500/30 bg-card px-3 py-2 text-xs text-red-500 shadow-sm"
        >
          <AlertTriangle className="h-3.5 w-3.5 shrink-0" />
          {t("settings.saveFailed")}
        </div>
      )}

      {/* Language + General: two compact cards side by side */}
      <div className="grid gap-4 sm:grid-cols-2">
        <Card>
          <CardHeader className="flex-row items-center gap-2 space-y-0">
            <Languages className="h-4 w-4 text-muted-foreground" />
            <CardTitle>{t("settings.language")}</CardTitle>
          </CardHeader>
          <CardContent className="space-y-3">
            <p className="text-xs text-muted-foreground">
              {t("settings.languageDesc")}
            </p>
            <div className="flex gap-2">
              {languages.map((l) => (
                <button
                  key={l.value}
                  onClick={() => setLang(l.value)}
                  aria-pressed={lang === l.value}
                  className={pillBtn(lang === l.value)}
                >
                  {l.label}
                </button>
              ))}
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="flex-row items-center gap-2 space-y-0">
            <Rocket className="h-4 w-4 text-muted-foreground" />
            <CardTitle>{t("settings.general")}</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="flex items-center justify-between gap-3 rounded-lg border border-border p-3">
              <div>
                <div id="autostart-label" className="text-sm font-medium">
                  {t("settings.autostart")}
                </div>
                <div
                  id="autostart-desc"
                  className="mt-0.5 text-xs text-muted-foreground"
                >
                  {t("settings.autostartDesc")}
                </div>
              </div>
              <button
                type="button"
                onClick={toggleAutostart}
                className={cn(
                  "relative h-6 w-11 shrink-0 rounded-full transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background",
                  autostart ? "bg-primary" : "bg-muted",
                )}
                role="switch"
                aria-checked={autostart}
                aria-labelledby="autostart-label"
                aria-describedby="autostart-desc"
              >
                <span
                  className={cn(
                    "absolute top-0.5 h-5 w-5 rounded-full bg-white shadow transition-all",
                    autostart ? "left-[22px]" : "left-0.5",
                  )}
                />
              </button>
            </div>
          </CardContent>
        </Card>
      </div>

      <Card>
        <CardHeader className="flex-row items-center gap-2 space-y-0">
          <CreditCard className="h-4 w-4 text-muted-foreground" />
          <CardTitle>{t("settings.subscriptions")}</CardTitle>
        </CardHeader>
        <CardContent className="space-y-3">
          <p className="text-xs text-muted-foreground">
            {t("settings.subscriptionsDesc")}
          </p>

          {subscriptions.map((s) => {
            const brand = subscriptionBrand(s);
            const initial = (s.name.trim()[0] || "·").toUpperCase();
            return (
              <div
                key={s.id}
                className="group rounded-xl border border-border p-3 transition-colors hover:border-primary/30"
              >
                <div className="flex items-center gap-3">
                  <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-muted text-xs font-semibold text-foreground">
                    {brand ? (
                      <BrandIcon brand={brand} className="h-[18px] w-[18px]" />
                    ) : (
                      initial
                    )}
                  </div>
                  <input
                    value={s.name}
                    onChange={(e) => update(s.id, { name: e.target.value })}
                    placeholder={t("settings.sub.namePlaceholder")}
                    list="sub-presets"
                    className="min-w-0 flex-1 rounded-md bg-transparent px-2 py-1 text-sm font-medium outline-none transition-colors placeholder:font-normal placeholder:text-muted-foreground hover:bg-accent/50 focus:bg-accent/50"
                  />
                  <div className="flex shrink-0 items-center gap-0.5 text-sm tabular-nums">
                    <span className="text-muted-foreground">$</span>
                    <input
                      type="number"
                      min={0}
                      value={s.monthlyUsd}
                      onChange={(e) =>
                        update(s.id, {
                          // Clamp: `min` only guards the spinner arrows,
                          // typed negatives would corrupt the ROI math.
                          monthlyUsd: Math.max(0, Number(e.target.value) || 0),
                        })
                      }
                      className="w-12 bg-transparent text-right outline-none [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none"
                    />
                    <span className="text-xs text-muted-foreground">
                      {t("settings.sub.perMonth")}
                    </span>
                  </div>
                  <button
                    onClick={() => askRemove(s.id)}
                    title={t("settings.sub.remove")}
                    className={cn(
                      "flex shrink-0 items-center gap-1 rounded-md p-1.5 transition-colors",
                      confirmRemove === s.id
                        ? "bg-red-500/10 text-red-500"
                        : "text-muted-foreground hover:bg-red-500/10 hover:text-red-500",
                    )}
                  >
                    <Trash2 className="h-4 w-4" />
                    {confirmRemove === s.id && (
                      <span className="text-xs font-medium">
                        {t("settings.sub.confirmRemove")}
                      </span>
                    )}
                  </button>
                </div>
                <div className="mt-2 flex flex-wrap items-center gap-1.5 pl-11">
                  <span className="shrink-0 text-xs text-muted-foreground">
                    {t("settings.sub.agent")}
                  </span>
                  <AgentMultiSelect
                    value={s.agents}
                    onChange={(agents) => update(s.id, { agents })}
                  />
                </div>
              </div>
            );
          })}

          {subscriptions.length === 0 && (
            <p className="text-xs text-muted-foreground">
              {t("settings.sub.empty")}
            </p>
          )}

          <div className="space-y-2 pt-1">
            <div className="text-xs font-medium text-muted-foreground">
              {t("settings.sub.quickAdd")}
            </div>
            <div className="flex flex-wrap gap-2">
              {SUB_PRESETS.map((p) => (
                <button
                  key={p.name}
                  onClick={() =>
                    add({
                      name: p.name,
                      monthlyUsd: p.monthlyUsd,
                      agents: p.agents,
                    })
                  }
                  className="rounded-full border border-border px-3 py-1 text-xs text-muted-foreground transition-colors hover:border-primary hover:text-foreground"
                >
                  + {p.name}
                </button>
              ))}
              <button
                onClick={() => add({ name: "", monthlyUsd: 0, agents: [] })}
                className="rounded-full border border-dashed border-border px-3 py-1 text-xs text-muted-foreground transition-colors hover:text-foreground"
              >
                {t("settings.sub.custom")}
              </button>
            </div>
          </div>

          <datalist id="sub-presets">
            {SUB_PRESETS.map((p) => (
              <option key={p.name} value={p.name} />
            ))}
          </datalist>
        </CardContent>
      </Card>

      <Card>
        <CardHeader className="flex-row items-center gap-2 space-y-0">
          <Palette className="h-4 w-4 text-muted-foreground" />
          <CardTitle>{t("settings.appearance")}</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-wrap gap-x-10 gap-y-4">
          <div>
            <div className="mb-2 text-xs text-muted-foreground">
              {t("settings.themeMode")}
            </div>
            <div className="flex gap-2">
              {(["dark", "light"] as ThemeMode[]).map((m) => (
                <button
                  key={m}
                  onClick={() => setMode(m)}
                  aria-pressed={mode === m}
                  className={pillBtn(mode === m)}
                >
                  {t(m === "dark" ? "settings.theme.dark" : "settings.theme.light")}
                </button>
              ))}
            </div>
          </div>
          <div>
            <div className="mb-2 text-xs text-muted-foreground">
              {t("settings.accentColor")}
            </div>
            <div className="flex h-full items-center gap-3">
              {(Object.keys(ACCENTS) as AccentKey[]).map((key) => (
                <button
                  key={key}
                  onClick={() => setAccent(key)}
                  title={t(`settings.accent.${key}` as const)}
                  aria-label={t(`settings.accent.${key}` as const)}
                  aria-pressed={accent === key}
                  className={cn(
                    "h-8 w-8 rounded-full transition-transform hover:scale-110",
                    accent === key &&
                      "ring-2 ring-foreground ring-offset-2 ring-offset-background",
                  )}
                  style={{ backgroundColor: ACCENTS[key].swatch }}
                />
              ))}
            </div>
          </div>
        </CardContent>
      </Card>

      {/* Menu bar + Cost mode side by side */}
      <div className="grid gap-4 md:grid-cols-2">
        <Card>
          <CardHeader className="flex-row items-center gap-2 space-y-0">
            <MenuSquare className="h-4 w-4 text-muted-foreground" />
            <CardTitle>{t("settings.trayDisplay")}</CardTitle>
          </CardHeader>
          <CardContent className="space-y-3">
            <p className="text-xs text-muted-foreground">
              {t("settings.trayDisplayDesc")}
            </p>
            {notchSupported && (
              <div>
                <div className="mb-2 text-xs text-muted-foreground">
                  {t("settings.displayPosition")}
                </div>
                <div className="flex gap-2">
                  <button
                    onClick={() => choosePosition(true)}
                    aria-pressed={notchOn}
                    className={pillBtn(notchOn)}
                  >
                    {t("settings.position.notch")}
                  </button>
                  <button
                    onClick={() => choosePosition(false)}
                    aria-pressed={!notchOn}
                    className={pillBtn(!notchOn)}
                  >
                    {t("settings.position.menubar")}
                  </button>
                </div>
                {notchOn && (
                  <p className="mt-2 text-xs text-muted-foreground">
                    {t("settings.position.notchHint")}
                  </p>
                )}
              </div>
            )}
            {(!notchSupported || !notchOn) && (
              <div>
                {notchSupported && (
                  <div className="mb-2 text-xs text-muted-foreground">
                    {t("settings.trayContent")}
                  </div>
                )}
                <div className="flex gap-2">
                  {(
                    [
                      ["cost", "settings.tray.cost"],
                      ["tokens", "settings.tray.tokens"],
                      ["off", "settings.tray.off"],
                    ] as [TrayMode, I18nKey][]
                  ).map(([m, key]) => (
                    <button
                      key={m}
                      onClick={() => changeTrayMode(m)}
                      aria-pressed={trayMode === m}
                      className={pillBtn(trayMode === m)}
                    >
                      {t(key)}
                    </button>
                  ))}
                </div>
              </div>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="flex-row items-center gap-2 space-y-0">
            <DollarSign className="h-4 w-4 text-muted-foreground" />
            <CardTitle>{t("settings.costMode")}</CardTitle>
          </CardHeader>
          <CardContent className="flex gap-2">
            {modes.map((m) => (
              <button
                key={m.value}
                onClick={() => changeCostMode(m.value)}
                title={m.desc}
                aria-pressed={costMode === m.value}
                className={cn(
                  "flex-1 rounded-lg border px-2 py-2 text-center text-sm font-medium transition-colors",
                  costMode === m.value
                    ? "border-primary bg-primary/10 text-foreground"
                    : "border-border text-muted-foreground hover:bg-accent hover:text-foreground",
                )}
              >
                {m.label}
              </button>
            ))}
          </CardContent>
        </Card>
      </div>

      <Card>
        <CardHeader className="flex-row items-center gap-2 space-y-0">
          <FolderSearch className="h-4 w-4 text-muted-foreground" />
          <CardTitle>{t("settings.dataSources")}</CardTitle>
        </CardHeader>
        <CardContent className="space-y-3">
          {active.length > 0 && (
            <div className="grid gap-2 sm:grid-cols-2">
              {active.map((s) => {
                const brand = agentBrand(s.agent);
                return (
                  <div
                    key={s.agent}
                    className="rounded-lg border border-border p-3"
                  >
                    <div className="flex items-center justify-between gap-2">
                      <div className="flex min-w-0 items-center gap-2">
                        {brand && (
                          <BrandIcon
                            brand={brand}
                            className="h-4 w-4 shrink-0 text-foreground"
                          />
                        )}
                        <span className="truncate text-sm font-medium">
                          {agentLabel(s.agent)}
                        </span>
                      </div>
                      <Badge variant="success">
                        {t("settings.files", { n: s.fileCount })}
                      </Badge>
                    </div>
                    {s.dirs.map((d) => (
                      <div
                        key={d}
                        className="mt-1 break-all font-mono text-[11px] leading-relaxed text-muted-foreground"
                      >
                        {d}
                      </div>
                    ))}
                  </div>
                );
              })}
            </div>
          )}

          {inactive.length > 0 && (
            <div>
              <button
                onClick={() => setShowInactive((v) => !v)}
                className="flex items-center gap-1 text-xs text-muted-foreground transition-colors hover:text-foreground"
              >
                <ChevronDown
                  className={cn(
                    "h-3.5 w-3.5 transition-transform",
                    showInactive && "rotate-180",
                  )}
                />
                {t("settings.sources.inactive", { n: inactive.length })}
              </button>
              {showInactive && (
                <div className="mt-2 grid grid-cols-2 gap-2 sm:grid-cols-3">
                  {inactive.map((s) => {
                    const brand = agentBrand(s.agent);
                    return (
                      <div
                        key={s.agent}
                        className="flex items-center gap-1.5 rounded-md border border-border/60 px-2 py-1.5 text-xs text-muted-foreground"
                      >
                        {brand && (
                          <BrandIcon
                            brand={brand}
                            className="h-3.5 w-3.5 shrink-0 opacity-60"
                          />
                        )}
                        <span className="truncate">{agentLabel(s.agent)}</span>
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          )}

          {lastScan && (
            <p className="text-xs text-muted-foreground">
              {t("settings.lastScan", {
                parsed: lastScan.filesParsed,
                total: lastScan.filesTotal,
                entries: lastScan.entriesInserted,
                ms: lastScan.durationMs,
              })}
            </p>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader className="flex-row items-center justify-between gap-2 space-y-0">
          <div className="flex items-center gap-2">
            <Database className="h-4 w-4 text-muted-foreground" />
            <CardTitle>{t("settings.pricing")}</CardTitle>
          </div>
          <Button
            variant="outline"
            size="sm"
            onClick={refreshPrices}
            disabled={pricingRefreshing}
            aria-busy={pricingRefreshing}
          >
            <RefreshCw
              className={cn("h-3.5 w-3.5", pricingRefreshing && "animate-spin")}
            />
            {pricingRefreshing ? t("pricing.refreshing") : t("pricing.refresh")}
          </Button>
        </CardHeader>
        <CardContent className="space-y-3">
          <p className="text-xs text-muted-foreground">
            {t("settings.pricingDesc")}
          </p>
          {pricing ? (
            <>
              <div className="grid gap-2 sm:grid-cols-2">
                <div className="rounded-lg border border-border p-3">
                  <div className="text-xs text-muted-foreground">
                    {t("pricing.source")}
                  </div>
                  <div className="mt-0.5 flex items-center gap-2 text-sm font-medium">
                    {pricing.source === "online"
                      ? t("pricing.online")
                      : t("pricing.snapshot")}
                    {pricing.source === "online" && (
                      <span className="h-1.5 w-1.5 rounded-full bg-emerald-500" />
                    )}
                  </div>
                  <div className="mt-0.5 text-xs text-muted-foreground">
                    {pricing.source === "online" && pricing.fetchedAtMs != null
                      ? t("pricing.fetchedAt", {
                          time: formatDateTime(pricing.fetchedAtMs),
                        })
                      : t("pricing.snapshotDate", {
                          date: pricing.snapshotDate,
                        })}
                  </div>
                </div>
                <div className="rounded-lg border border-border p-3">
                  <div className="text-xs text-muted-foreground">
                    {t("pricing.models")}
                  </div>
                  <div className="mt-0.5 text-sm font-medium tabular-nums">
                    {formatNumber(pricing.modelCount)}
                  </div>
                </div>
              </div>

              {pricing.lastError && (
                <p
                  role="alert"
                  className="flex items-start gap-1.5 text-xs text-red-500"
                >
                  <AlertTriangle className="mt-px h-3.5 w-3.5 shrink-0" />
                  <span className="break-words">
                    {t("pricing.lastError", { error: pricing.lastError })}
                  </span>
                </p>
              )}

              {pricing.unpricedModels.length > 0 ? (
                <div className="space-y-2 rounded-lg border border-amber-500/30 bg-amber-500/5 p-3">
                  <div className="text-xs font-medium">
                    {t("pricing.unpricedTitle", {
                      n: pricing.unpricedModels.length,
                    })}
                  </div>
                  <p className="text-xs text-muted-foreground">
                    {t("pricing.unpricedDesc")}
                  </p>
                  <div className="flex flex-wrap gap-1.5">
                    {pricing.unpricedModels.map((m) => (
                      <Badge key={m} variant="outline" className="font-mono">
                        {m}
                      </Badge>
                    ))}
                  </div>
                </div>
              ) : (
                <p className="text-xs text-muted-foreground">
                  {t("pricing.allPriced")}
                </p>
              )}
            </>
          ) : pricingLoadError ? (
            <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
              <AlertTriangle className="h-3.5 w-3.5" />
              {t("common.loadFailed")}
            </p>
          ) : (
            <Skeleton className="h-16" />
          )}
        </CardContent>
      </Card>
    </div>
  );
}
