import { Tabs } from "@base-ui/react/tabs";
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { disable, enable, isEnabled } from "@tauri-apps/plugin-autostart";
import {
  AlertTriangle,
  Check,
  ChevronDown,
  Database,
  FolderSearch,
  RefreshCw,
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
import { useI18n } from "@/lib/i18n";
import { useSubscriptions } from "@/lib/subscriptions";
import { AgentMultiSelect } from "@/components/AgentMultiSelect";
import {
  BrandIcon,
  agentBrand,
  subscriptionBrand,
} from "@/components/BrandIcon";
import { ACCENTS, useTheme, type AccentKey } from "@/lib/theme";
import { cn } from "@/lib/utils";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Frame, FrameHeader, FramePanel } from "@/components/ui/frame";
import { SegmentedControl } from "@/components/ui/segmented-control";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
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
  /** Rejects if the backend cannot save the selection. */
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
  const [savingPreference, setSavingPreference] = useState(false);
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

  const persist = async (write: () => Promise<void>, what: string) => {
    if (savingPreference) return;
    setSavingPreference(true);
    setSaveError(false);
    try {
      await write();
    } catch (e) {
      failSave(what, e);
    } finally {
      setSavingPreference(false);
    }
  };

  const choosePosition = (notch: boolean) => {
    if (notch === notchEnabled) return;
    void persist(async () => {
      await api.setNotchEnabled(notch);
      setNotchEnabled(notch);
    }, "set_notch_enabled");
  };

  const changeTrayMode = (m: TrayMode) => {
    if (m === trayMode) return;
    void persist(async () => {
      await api.setTrayMode(m);
      setTrayMode(m);
    }, "set_tray_mode");
  };

  const changeCostMode = (m: CostMode) => {
    if (m === costMode) return;
    void persist(() => onCostModeChange(m), "set_cost_mode");
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

  const modes = (["auto", "calculate", "display"] as const).map((id) => ({
    id,
    label: t(`settings.mode.${id}`),
    description: t(`settings.mode.${id}Desc`),
  }));

  const active = sources.filter((s) => s.fileCount > 0);
  const inactive = sources.filter((s) => s.fileCount === 0);

  return (
    <Tabs.Root
      defaultValue="general"
      className="flex max-w-[940px] flex-col gap-5"
    >
      <Tabs.List
        aria-label={t("nav.settings")}
        className="flex gap-4 overflow-x-auto border-b border-border"
      >
        {[
          { id: "general", label: t("settings.general") },
          { id: "data", label: t("settings.dataAndPricing") },
          { id: "subscriptions", label: t("settings.subscriptions") },
        ].map(({ id, label }) => (
          <Tabs.Tab
            key={id}
            value={id}
            className="shrink-0 cursor-pointer border-b-2 border-transparent px-1.5 pt-1 pb-2.5 text-sm font-medium whitespace-nowrap text-muted-foreground transition-colors hover:text-foreground focus-visible:-outline-offset-2 data-active:border-primary data-active:text-foreground"
          >
            {label}
          </Tabs.Tab>
        ))}
      </Tabs.List>
      {saveError && (
        <div
          role="alert"
          className="flex items-center gap-2 rounded-lg border border-danger/20 bg-danger-surface px-3 py-2 text-sm text-danger"
        >
          <AlertTriangle className="size-4 shrink-0" aria-hidden />
          {t("settings.saveFailed")}
        </div>
      )}

      <Tabs.Panel value="general" className="flex flex-col gap-4 outline-none">
        <Frame>
          <FrameHeader>
            <h2 className="text-sm font-semibold">{t("settings.general")}</h2>
          </FrameHeader>
          <FramePanel className="divide-y divide-border">
            <SettingsRow label={t("settings.language")}>
              <SegmentedControl
                label={t("settings.language")}
                options={[
                  { id: "zh", label: "中文" },
                  { id: "en", label: "English" },
                ]}
                value={lang}
                onChange={setLang}
              />
            </SettingsRow>
            <SettingsRow
              label={t("settings.autostart")}
              description={t("settings.autostartDesc")}
              id="settings-autostart"
            >
              <Switch
                checked={autostart}
                onCheckedChange={() => void toggleAutostart()}
                aria-labelledby="settings-autostart-label"
                aria-describedby="settings-autostart-description"
              />
            </SettingsRow>
          </FramePanel>
        </Frame>

        <Frame>
          <FrameHeader>
            <h2 className="text-sm font-semibold">
              {t("settings.appearance")}
            </h2>
          </FrameHeader>
          <FramePanel className="divide-y divide-border">
            <SettingsRow label={t("settings.themeMode")}>
              <SegmentedControl
                label={t("settings.themeMode")}
                options={[
                  { id: "light", label: t("settings.theme.light") },
                  { id: "dark", label: t("settings.theme.dark") },
                ]}
                value={mode}
                onChange={setMode}
              />
            </SettingsRow>
            <SettingsRow label={t("settings.accentColor")}>
              <div
                role="group"
                aria-label={t("settings.accentColor")}
                className="flex flex-wrap items-center gap-2.5"
              >
                {(Object.keys(ACCENTS) as AccentKey[]).map((key) => (
                  <button
                    key={key}
                    type="button"
                    onClick={() => setAccent(key)}
                    title={t(`settings.accent.${key}`)}
                    aria-label={t(`settings.accent.${key}`)}
                    aria-pressed={accent === key}
                    className="grid size-7 place-items-center rounded-full border border-black/10 text-white aria-pressed:ring-1 aria-pressed:ring-foreground aria-pressed:ring-offset-2 aria-pressed:ring-offset-card"
                    style={{ backgroundColor: ACCENTS[key].swatch }}
                  >
                    {accent === key && <Check className="size-4" aria-hidden />}
                  </button>
                ))}
              </div>
            </SettingsRow>
          </FramePanel>
        </Frame>

        <Frame>
          <FrameHeader>
            <h2 className="text-sm font-semibold">
              {t("settings.trayDisplay")}
            </h2>
          </FrameHeader>
          <FramePanel className="divide-y divide-border">
            {notchSupported && (
              <SettingsRow
                label={t("settings.displayPosition")}
                description={
                  notchOn
                    ? t("settings.position.notchHint")
                    : t("settings.trayDisplayDesc")
                }
              >
                <SegmentedControl
                  label={t("settings.displayPosition")}
                  options={[
                    { id: "notch", label: t("settings.position.notch") },
                    { id: "menu", label: t("settings.position.menubar") },
                  ]}
                  value={notchOn ? "notch" : "menu"}
                  disabled={savingPreference}
                  onChange={(position) => choosePosition(position === "notch")}
                />
              </SettingsRow>
            )}
            {!notchOn && (
              <SettingsRow
                label={t("settings.trayContent")}
                description={
                  notchSupported ? undefined : t("settings.trayDisplayDesc")
                }
              >
                <SegmentedControl
                  label={t("settings.trayContent")}
                  options={[
                    { id: "cost", label: t("settings.tray.cost") },
                    { id: "tokens", label: t("settings.tray.tokens") },
                    { id: "off", label: t("settings.tray.off") },
                  ]}
                  value={trayMode}
                  disabled={savingPreference}
                  onChange={changeTrayMode}
                />
              </SettingsRow>
            )}
          </FramePanel>
        </Frame>
      </Tabs.Panel>

      <Tabs.Panel value="data" className="grid gap-4 outline-none">
        <Frame>
          <FrameHeader>
            <h2 className="text-sm font-semibold">{t("settings.costMode")}</h2>
          </FrameHeader>
          <FramePanel>
            <SettingsRow
              label={t(`settings.mode.${costMode}`)}
              description={
                modes.find((item) => item.id === costMode)?.description
              }
            >
              <SegmentedControl
                label={t("settings.costMode")}
                options={modes}
                value={costMode}
                disabled={savingPreference}
                onChange={changeCostMode}
              />
            </SettingsRow>
          </FramePanel>
        </Frame>
        <Card>
          <CardHeader className="flex-row items-center gap-2 space-y-0">
            <FolderSearch className="h-4 w-4 text-muted-foreground" />
            <CardTitle>{t("settings.dataSources")}</CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-3">
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
                          {s.agent === "magpie"
                            ? t("settings.performanceSource")
                            : t("settings.files", { n: s.fileCount })}
                        </Badge>
                      </div>
                      {s.dirs.map((d) => (
                        <div
                          key={d}
                          className="mt-1 break-all font-mono select-text text-[11px] leading-relaxed text-muted-foreground"
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
                  type="button"
                  aria-expanded={showInactive}
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
                          <span className="truncate">
                            {agentLabel(s.agent)}
                          </span>
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
                className={cn(
                  "h-3.5 w-3.5",
                  pricingRefreshing && "motion-safe:animate-spin",
                )}
              />
              {pricingRefreshing
                ? t("pricing.refreshing")
                : t("pricing.refresh")}
            </Button>
          </CardHeader>
          <CardContent className="flex flex-col gap-3">
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
                      {pricing.source === "online" &&
                      pricing.fetchedAtMs != null
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
                    className="flex items-start gap-1.5 text-xs text-danger"
                  >
                    <AlertTriangle className="mt-px h-3.5 w-3.5 shrink-0" />
                    <span className="break-words">
                      {t("pricing.lastError", { error: pricing.lastError })}
                    </span>
                  </p>
                )}

                {pricing.unpricedModels.length > 0 ? (
                  <div className="space-y-2 rounded-lg border border-warning/20 bg-warning-surface p-3">
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
      </Tabs.Panel>

      <Tabs.Panel value="subscriptions" className="outline-none">
        <Card>
          <CardHeader className="flex-row items-center gap-2 space-y-0">
            <CardTitle>{t("settings.subscriptions")}</CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-3">
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
                  <div className="flex flex-wrap items-center gap-2.5">
                    <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-muted text-xs font-semibold text-foreground">
                      {brand ? (
                        <BrandIcon
                          brand={brand}
                          className="h-[18px] w-[18px]"
                        />
                      ) : (
                        initial
                      )}
                    </div>
                    <Input
                      aria-label={t("settings.sub.namePlaceholder")}
                      value={s.name}
                      onChange={(e) => update(s.id, { name: e.target.value })}
                      placeholder={t("settings.sub.namePlaceholder")}
                      list="sub-presets"
                      className="min-w-20 flex-1"
                    />
                    <div className="flex shrink-0 items-center gap-0.5 text-sm tabular-nums">
                      <span className="text-muted-foreground">$</span>
                      <Input
                        type="number"
                        aria-label={t("settings.sub.monthlyCost")}
                        min={0}
                        value={s.monthlyUsd}
                        onChange={(e) =>
                          update(s.id, {
                            // Clamp: `min` only guards the spinner arrows,
                            // typed negatives would corrupt the ROI math.
                            monthlyUsd: Math.max(
                              0,
                              Number(e.target.value) || 0,
                            ),
                          })
                        }
                        className="w-16 px-1.5 text-right [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none"
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
                          ? "bg-danger-surface text-danger"
                          : "text-muted-foreground hover:bg-danger-surface hover:text-danger",
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
                  <Button
                    key={p.name}
                    size="sm"
                    variant="outline"
                    onClick={() =>
                      add({
                        name: p.name,
                        monthlyUsd: p.monthlyUsd,
                        agents: p.agents,
                      })
                    }
                  >
                    + {p.name}
                  </Button>
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
      </Tabs.Panel>
    </Tabs.Root>
  );
}

function SettingsRow({
  label,
  description,
  id,
  children,
}: {
  label: string;
  description?: string;
  id?: string;
  children: ReactNode;
}) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-3 px-4 py-3.5 sm:px-5">
      <div className="min-w-0">
        <div id={id && `${id}-label`} className="text-sm font-medium">
          {label}
        </div>
        {description && (
          <p
            id={id && `${id}-description`}
            className="mt-1 text-xs text-muted-foreground"
          >
            {description}
          </p>
        )}
      </div>
      {children}
    </div>
  );
}
