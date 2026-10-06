import { useCallback, useEffect, useRef, useState } from "react";
import { emit } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { IN_TAURI, api, onEvent, type NotchInfo } from "@/lib/api";
import {
  agentLabel,
  colorByName,
  formatCost,
  formatNumber,
  formatTokens,
  remainingLabel,
} from "@/lib/format";
import { useI18n } from "@/lib/i18n";
import { useTodayStats } from "@/lib/useTodayStats";

/** At rest the bar IS the notch: just a thin margin so the corners blend.
 *  Mirrors WING in src-tauri/src/notch.rs. */
const IDLE_WING = 14;
/** Expanded panel width — the island grows in BOTH dimensions. */
const EXP_W = 412;
/** Apple-feel ease shared by every notch transition. */
const EASE = "cubic-bezier(0.32, 0.72, 0, 1)";
/** Apple systemGreen (dark variant) — the live indicator. */
const LIVE_GREEN = "#30d158";

// Validated categorical palette for the pure-black surface (dataviz
// reference palette, dark steps 1-5). Colors follow the ENTITY, never the
// cost rank: the five named agents own these slots.
const NOTCH_AGENT_COLORS: Record<string, string> = {
  "claude-code": "#3987e5",
  codex: "#199e70",
  kimi: "#c98500",
  gemini: "#008300",
  opencode: "#9085e9",
};
/** Unknown agents hash (by name) into these hues, disjoint from the named
 *  slots above, so they can never wear a named agent's color. */
const NOTCH_EXTRA_COLORS = ["#e5566f", "#c561c7", "#9fb825", "#e2703a", "#1fb3c9"];
/** Neutral fold bucket ("other"), never a series hue. */
const FOLD_GRAY = "#8e8e93";

function agentTint(agent: string): string {
  return colorByName(agent, NOTCH_AGENT_COLORS, NOTCH_EXTRA_COLORS);
}

/** Rest → hover-peek (numbers in the wings) → click-open (full island). */
type Mode = "idle" | "peek" | "open";

export function NotchBar() {
  const { t } = useI18n();
  const [info, setInfo] = useState<NotchInfo | null>(null);
  // The bar is always on screen: poll slowly and lean on the file
  // watcher's usage-updated events for immediacy.
  const { stats, error, reload } = useTodayStats({
    hourly: true,
    pollMs: 60_000,
  });
  const [mode, setMode] = useState<Mode>("idle");
  // Open-island height, measured from the rendered panel so the window
  // always fits the content (locale / active-block variations).
  const [boxH, setBoxH] = useState(0);
  // Peek wing width, content-hugged to the widest of the two readouts.
  const [wing, setWing] = useState(90);
  const panelRef = useRef<HTMLDivElement>(null);
  const leftRef = useRef<HTMLSpanElement>(null);
  const rightRef = useRef<HTMLDivElement>(null);
  const shrinkTimer = useRef<number | undefined>(undefined);
  const hoverTimer = useRef<number | undefined>(undefined);
  const modeRef = useRef<Mode>("idle");
  modeRef.current = mode;
  // Hover state that doesn't wait for React: whether the pointer is over
  // the island right now, and a counter bumped by every transition so an
  // expand whose (async) window resize resolves after a newer transition
  // — pointer left, Esc, blur — is abandoned instead of applied.
  const pointerInside = useRef(false);
  const transitionSeq = useRef(0);

  const setModeNow = useCallback((m: Mode) => {
    modeRef.current = m;
    setMode(m);
  }, []);

  // Geometry comes from the backend at startup and again whenever the
  // display configuration changes (it moves/recreates this window).
  useEffect(() => {
    let cancelled = false;
    api
      .getNotchInfo()
      .then((i) => !cancelled && setInfo(i))
      .catch((e) => console.error("get_notch_info failed:", e));
    const unlisten = onEvent<NotchInfo>("notch-info-changed", (e) =>
      setInfo(e.payload),
    );
    return () => {
      cancelled = true;
      unlisten.then((fn) => fn());
    };
  }, []);

  const notchW = info ? Math.round(info.notchWidth) : 0;
  const colH = info ? Math.ceil(info.barHeight) : 0;
  const idleW = notchW + IDLE_WING * 2;
  const peekW = notchW + wing * 2;
  const expW = Math.max(peekW, EXP_W);

  // Content-hug the peek wings: measured widest readout + 12px notch-side
  // padding + 12px outer breathing room.
  useEffect(() => {
    if (!stats) return;
    const lw = leftRef.current?.offsetWidth ?? 0;
    const rw = rightRef.current?.offsetWidth ?? 0;
    const needed = Math.ceil(Math.max(lw, rw)) + 24;
    const next = Math.min(110, Math.max(56, needed));
    if (next !== wing) setWing(next);
  }, [stats, wing]);

  const resizeWindow = useCallback((w: number, h: number) => {
    if (!IN_TAURI) return Promise.resolve();
    return api
      .notchResize(w, h)
      .catch((e) => console.error("notch_resize failed:", e));
  }, []);

  /** Back to notch-width; shrink the native window after the CSS retract.
   *  Supersedes any in-flight expand. */
  const toIdle = useCallback(() => {
    transitionSeq.current++;
    window.clearTimeout(hoverTimer.current);
    setModeNow("idle");
    window.clearTimeout(shrinkTimer.current);
    shrinkTimer.current = window.setTimeout(() => {
      void resizeWindow(idleW, colH);
    }, 400);
  }, [idleW, colH, resizeWindow, setModeNow]);

  const toPeek = useCallback(async () => {
    window.clearTimeout(shrinkTimer.current);
    const seq = ++transitionSeq.current;
    await resizeWindow(peekW, colH);
    if (seq !== transitionSeq.current) return; // superseded meanwhile
    // The pointer left while the window was growing: don't strand the
    // wings open (onLeave saw "idle" and had nothing to retract yet).
    if (!pointerInside.current) {
      toIdle();
      return;
    }
    setModeNow("peek");
  }, [peekW, colH, resizeWindow, toIdle, setModeNow]);

  const open = useCallback(async () => {
    window.clearTimeout(shrinkTimer.current);
    window.clearTimeout(hoverTimer.current);
    const seq = ++transitionSeq.current;
    const h = colH + (panelRef.current?.offsetHeight ?? 300);
    await resizeWindow(expW, h);
    if (seq !== transitionSeq.current) return;
    setBoxH(h);
    setModeNow("open");
  }, [expW, colH, resizeWindow, setModeNow]);

  // Hover intent: peek after a short pause (so a cursor merely passing
  // through the notch row doesn't pop the wings), retract shortly after
  // the pointer leaves — whatever state an in-flight resize is in.
  const onEnter = useCallback(() => {
    pointerInside.current = true;
    window.clearTimeout(hoverTimer.current);
    if (modeRef.current !== "idle") return;
    hoverTimer.current = window.setTimeout(() => {
      if (pointerInside.current && modeRef.current === "idle") void toPeek();
    }, 140);
  }, [toPeek]);

  const onLeave = useCallback(() => {
    pointerInside.current = false;
    window.clearTimeout(hoverTimer.current);
    // The open island stays until click / Esc / blur.
    if (modeRef.current === "open") return;
    hoverTimer.current = window.setTimeout(() => {
      if (!pointerInside.current && modeRef.current !== "open") toIdle();
    }, 600);
  }, [toIdle]);

  // Display configuration changed (backend re-placed the window): fold
  // up and refit the window to the new notch geometry.
  const geometry = info ? `${info.notchWidth}x${info.barHeight}` : "";
  const lastGeometry = useRef(geometry);
  useEffect(() => {
    if (geometry === lastGeometry.current) return;
    const initial = lastGeometry.current === "";
    lastGeometry.current = geometry;
    if (!initial) toIdle();
  }, [geometry, toIdle]);

  // Panel reflow while open (legend wrap, block card appearing) — keep
  // the native window fitted.
  useEffect(() => {
    if (mode !== "open") return;
    const h = colH + (panelRef.current?.offsetHeight ?? 300);
    if (h !== boxH) {
      setBoxH(h);
      resizeWindow(expW, h);
    }
  }, [stats, mode, expW, colH, boxH, resizeWindow]);

  // Click elsewhere on the desktop → the window loses focus → fold up.
  useEffect(() => {
    if (!IN_TAURI) return;
    const un = getCurrentWebviewWindow().onFocusChanged(({ payload }) => {
      if (!payload && modeRef.current === "open") toIdle();
    });
    return () => {
      un.then((fn) => fn());
    };
  }, [toIdle]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && modeRef.current !== "idle") toIdle();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [toIdle]);

  // Deep link: open the main window already switched to the right page.
  const openPage = useCallback(
    async (page?: string) => {
      toIdle();
      try {
        await api.showMainWindow();
        if (page && IN_TAURI) await emit("navigate-page", page);
      } catch (e) {
        console.error("opening the dashboard failed:", e);
      }
    },
    [toIdle],
  );

  const openDashboard = useCallback(() => openPage(), [openPage]);

  // No notch on the current display: the backend closes this window.
  if (!info || !info.hasNotch) return null;

  const block = stats?.activeBlock ?? null;
  const blockPct = block
    ? Math.min(
        100,
        Math.max(
          0,
          ((Date.now() - block.startMs) / (block.endMs - block.startMs)) * 100,
        ),
      )
    : 0;

  const hourly = stats?.hourly ?? [];
  const hourlyMax = hourly.length ? Math.max(...hourly) : 0;
  const nowHour = new Date().getHours();

  // Top 3 agents + a neutral "other" fold (identity via legend, never rank).
  const agents = stats?.byAgent ?? [];
  const topAgents = agents.slice(0, 3);
  const foldCost = agents.slice(3).reduce((s, a) => s + a.cost, 0);
  const agentTotal = agents.reduce((s, a) => s + a.cost, 0);
  const segments: { key: string; label: string; cost: number; color: string }[] =
    [
      ...topAgents.map((a) => ({
        key: a.agent,
        label: agentLabel(a.agent),
        cost: a.cost,
        color: agentTint(a.agent),
      })),
      ...(foldCost > 0
        ? [
            {
              key: "__other",
              label: t("notch.other"),
              cost: foldCost,
              color: FOLD_GRAY,
            },
          ]
        : []),
    ];

  const isOpen = mode === "open";
  const barW = isOpen ? expW : mode === "peek" ? peekW : idleW;

  return (
    <>
      {!IN_TAURI && (
        <PreviewBackdrop barHeight={colH} notchWidth={notchW} onCover={toIdle} />
      )}
      <div
        className="fixed left-1/2 top-0 z-10 -translate-x-1/2 select-none"
        style={{ width: expW }}
      >
        {/* The island. At rest it is visually just the notch — no wings,
            no shadow (hardware has no drop shadow). Numbers spring out on
            hover; the full panel opens on click, growing in BOTH
            dimensions like the iOS Dynamic Island. */}
        <div
          className="mx-auto overflow-hidden bg-black text-white"
          onMouseEnter={onEnter}
          onMouseLeave={onLeave}
          style={{
            width: barW,
            height: isOpen ? boxH : colH,
            borderBottomLeftRadius: isOpen ? 28 : 11,
            borderBottomRightRadius: isOpen ? 28 : 11,
            boxShadow: isOpen ? "0 24px 60px rgba(0,0,0,0.55)" : "none",
            transition: `width 320ms ${EASE}, height 360ms ${EASE}, border-radius 360ms ${EASE}, box-shadow 360ms ${EASE}`,
          }}
        >
          {/* Bar row: two wings flanking the physical notch. */}
          <button
            type="button"
            onClick={isOpen ? toIdle : open}
            aria-expanded={isOpen}
            aria-controls="notch-panel"
            aria-label={isOpen ? t("notch.collapse") : t("notch.expand")}
            className="relative flex w-full cursor-default items-center outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-white/60"
            style={{ height: colH }}
          >
            {/* Idle live indicator: a quiet camera-style dot beside the
                notch while a billing block is burning. Outer span owns
                the show/hide fade — animate-pulse animates opacity and
                would override an inline value on the same element. */}
            <span
              className="absolute right-[9px] top-1/2 -translate-y-1/2"
              style={{
                opacity: mode === "idle" && block ? 1 : 0,
                transition: "opacity 200ms ease",
              }}
            >
              {/* Steady, not blinking: it's a state light (recording-dot
                  style), not an alert. */}
              <span
                className="block h-[5px] w-[5px] rounded-full"
                style={{ backgroundColor: LIVE_GREEN }}
              />
            </span>
            {/* Wings only carry content while peeking; the open panel
                repeats these numbers right below (morph, don't stack). */}
            <div
              className="flex flex-1 items-center justify-end pr-3"
              style={{
                opacity: mode === "peek" ? 1 : 0,
                transition: "opacity 180ms ease",
              }}
            >
              {/* 13px medium ≈ the menu bar's own type, so the number
                  reads as system furniture, not a widget. */}
              <span
                ref={leftRef}
                className="whitespace-nowrap text-[13px] font-medium tabular-nums tracking-tight text-white/90"
              >
                {stats ? formatCost(stats.todayCost) : "–"}
              </span>
            </div>
            <div className="shrink-0" style={{ width: notchW }} />
            <div
              className="flex flex-1 items-center justify-start pl-3"
              style={{
                opacity: mode === "peek" ? 1 : 0,
                transition: "opacity 180ms ease",
              }}
            >
              <div
                ref={rightRef}
                className="flex items-center gap-1.5 whitespace-nowrap"
              >
                {block ? (
                  <>
                    <span
                      className="h-[5px] w-[5px] shrink-0 rounded-full"
                      style={{ backgroundColor: LIVE_GREEN }}
                    />
                    {/* Short unit here: the wing is a tight slot and
                        "/小时" would widen it; the panel uses the full one. */}
                    <span className="text-[12px] font-medium tabular-nums text-white/75">
                      {block.burnRateCostPerHour != null
                        ? `${formatCost(block.burnRateCostPerHour)}${t("unit.perHourShort")}`
                        : formatTokens(block.totalTokens)}
                    </span>
                  </>
                ) : (
                  <>
                    <span className="h-[5px] w-[5px] shrink-0 rounded-full bg-white/25" />
                    <span className="text-[12px] font-medium tabular-nums text-white/55">
                      {stats ? formatTokens(stats.todayTokens) : "–"}
                    </span>
                  </>
                )}
              </div>
            </div>
          </button>

          {/* Open detail panel (height measured for window sizing). */}
          <div
            ref={panelRef}
            id="notch-panel"
            // Folded away: out of the tab order and the a11y tree.
            inert={!isOpen}
            className="px-5 pb-4 pt-1"
            style={{
              width: expW,
              opacity: isOpen ? 1 : 0,
              transform: isOpen
                ? "translateY(0) scale(1)"
                : "translateY(-10px) scale(0.97)",
              transition: isOpen
                ? `opacity 240ms ease 80ms, transform 360ms ${EASE} 60ms`
                : `opacity 140ms ease, transform 240ms ${EASE}`,
              pointerEvents: isOpen ? "auto" : "none",
            }}
          >
            {/* Hero: today's numbers, each deep-linking into the app. */}
            <div className="flex items-end justify-between px-0.5 pt-1.5">
              <button
                type="button"
                onClick={() => openPage("overview")}
                className="-m-1.5 rounded-xl p-1.5 text-left transition-colors hover:bg-white/5"
              >
                <div className="text-[11px] font-medium text-white/45">
                  {t("quick.todayCost")}
                </div>
                <div className="mt-1.5 text-[30px] font-semibold leading-none tracking-tight tabular-nums">
                  {stats ? formatCost(stats.todayCost) : "—"}
                </div>
              </button>
              <div className="flex gap-4 pb-0.5 text-right">
                <button
                  type="button"
                  onClick={() => openPage("models")}
                  className="-my-1 rounded-lg px-1.5 py-1 transition-colors hover:bg-white/5"
                >
                  <div className="text-[10px] font-medium text-white/45">
                    {t("quick.todayTokens")}
                  </div>
                  <div className="mt-1 text-[14px] font-semibold tabular-nums text-white/90">
                    {stats ? formatTokens(stats.todayTokens) : "—"}
                  </div>
                </button>
                <button
                  type="button"
                  onClick={() => openPage("sessions")}
                  className="-my-1 rounded-lg px-1.5 py-1 transition-colors hover:bg-white/5"
                >
                  <div className="text-[10px] font-medium text-white/45">
                    {t("quick.todayRequests")}
                  </div>
                  <div className="mt-1 text-[14px] font-semibold tabular-nums text-white/90">
                    {stats ? formatNumber(stats.todayRequests) : "—"}
                  </div>
                </button>
              </div>
            </div>

            {/* Load failure: the last good numbers stay, flagged. */}
            {error && (
              <div
                role="alert"
                className="mt-3 flex items-center justify-between rounded-xl bg-white/[0.06] px-3 py-1.5 text-[11px] text-white/60"
              >
                <span>{t("common.loadFailed")}</span>
                <button
                  type="button"
                  onClick={() => void reload()}
                  className="rounded-md px-2 py-0.5 font-medium text-white/85 transition-colors hover:bg-white/10"
                >
                  {t("common.retry")}
                </button>
              </div>
            )}

            {/* One overview card: today's rhythm + agent split, divided
                by a hairline — grouped, not floating on bare black. */}
            <div className="mt-4 rounded-2xl bg-white/[0.06] p-3.5">
              <div className="flex items-center justify-between text-[11px] font-medium text-white/45">
                <span>{t("notch.rhythm")}</span>
                {hourlyMax > 0 && (
                  <span className="tabular-nums text-white/35">
                    {t("notch.peak")} {formatCost(hourlyMax)}
                  </span>
                )}
              </div>
              <div className="mt-2.5 flex h-9 items-end gap-[2px]">
                {Array.from({ length: 24 }, (_, h) => {
                  const c = hourly[h] ?? 0;
                  return (
                    <div
                      key={h}
                      title={`${String(h).padStart(2, "0")}:00 · ${formatCost(c)}`}
                      className="flex-1 rounded-[2px]"
                      style={{
                        height:
                          hourlyMax > 0
                            ? `${Math.max(6, (c / hourlyMax) * 100)}%`
                            : "6%",
                        backgroundColor:
                          h === nowHour
                            ? "rgba(255,255,255,0.92)"
                            : c > 0
                              ? "rgba(255,255,255,0.34)"
                              : "rgba(255,255,255,0.10)",
                      }}
                    />
                  );
                })}
              </div>
              <div className="mt-1.5 flex justify-between text-[9px] font-medium tabular-nums text-white/25">
                <span>0</span>
                <span>6</span>
                <span>12</span>
                <span>18</span>
                <span>24</span>
              </div>

              {segments.length > 0 && agentTotal > 0 && (
                <>
                  <div className="my-3 h-px bg-white/[0.08]" />
                  <div className="flex h-[5px] gap-[2px] overflow-hidden rounded-full">
                    {segments.map((s) => (
                      <div
                        key={s.key}
                        title={`${s.label} · ${formatCost(s.cost)}`}
                        style={{
                          width: `${(s.cost / agentTotal) * 100}%`,
                          backgroundColor: s.color,
                        }}
                      />
                    ))}
                  </div>
                  <div className="mt-2.5 flex flex-wrap gap-x-3.5 gap-y-1.5">
                    {segments.map((s) => (
                      <span
                        key={s.key}
                        className="flex items-center gap-1.5 text-[11px]"
                      >
                        <span
                          className="h-[6px] w-[6px] rounded-full"
                          style={{ backgroundColor: s.color }}
                        />
                        <span className="text-white/55">{s.label}</span>
                        <span className="font-medium tabular-nums text-white/85">
                          {formatCost(s.cost)}
                        </span>
                      </span>
                    ))}
                  </div>
                </>
              )}
            </div>

            {/* Active billing block. */}
            <button
              type="button"
              onClick={() => openPage("blocks")}
              className="mt-2.5 w-full rounded-2xl bg-white/[0.06] p-3.5 text-left transition-colors hover:bg-white/[0.09]"
            >
              {block ? (
                <>
                  <div className="flex items-center justify-between">
                    <div className="flex min-w-0 items-center gap-1.5 text-[11px] font-medium text-white/55">
                      <span
                        className="h-[6px] w-[6px] shrink-0 rounded-full"
                        style={{ backgroundColor: LIVE_GREEN }}
                      />
                      {/* Blocks are per agent: say whose is burning. */}
                      <span className="truncate">
                        {t("quick.activeBlock")} · {agentLabel(block.agent)}
                      </span>
                    </div>
                    {block.burnRateCostPerHour != null && (
                      <span className="text-[11px] tabular-nums text-white/55">
                        {formatCost(block.burnRateCostPerHour)}
                        {t("unit.perHour")}
                      </span>
                    )}
                  </div>
                  <div className="mt-1.5 flex items-baseline justify-between">
                    <span className="text-[17px] font-semibold tabular-nums">
                      {formatCost(block.cost)}
                    </span>
                    <span className="text-[11px] tabular-nums text-white/55">
                      {remainingLabel(block.endMs, t)}
                    </span>
                  </div>
                  <div className="mt-2.5 h-1 overflow-hidden rounded-full bg-white/10">
                    <div
                      className="h-full rounded-full bg-white/85"
                      style={{
                        width: `${blockPct}%`,
                        transition: "width 600ms ease",
                      }}
                    />
                  </div>
                </>
              ) : (
                <div className="flex h-[52px] items-center justify-center text-[12px] text-white/40">
                  {t("quick.noActiveBlock")}
                </div>
              )}
            </button>

            <button
              type="button"
              onClick={openDashboard}
              className="mt-3.5 w-full rounded-full bg-white/10 py-[9px] text-[12px] font-medium text-white/90 transition-[background-color,transform] duration-150 hover:bg-white/[0.16] active:scale-[0.98]"
            >
              {t("quick.openDashboard")}
            </button>
          </div>
        </div>
      </div>
    </>
  );
}

/** Browser-preview stand-in for the desktop: wallpaper gradient, frosted
 *  menu bar and a fake physical notch, so the fusion illusion is visible
 *  in design QA screenshots. Never rendered inside Tauri. */
function PreviewBackdrop({
  barHeight,
  notchWidth,
  onCover,
}: {
  barHeight: number;
  notchWidth: number;
  onCover: () => void;
}) {
  return (
    <div
      className="fixed inset-0"
      onClick={onCover}
      style={{
        background:
          "linear-gradient(165deg, #4a6d94 0%, #33506e 42%, #1a2a3c 100%)",
      }}
    >
      <div
        className="absolute inset-x-0 top-0 flex items-center justify-between px-4 text-[12px] font-medium text-white/85"
        style={{
          height: barHeight,
          background: "rgba(255,255,255,0.08)",
          backdropFilter: "blur(24px)",
        }}
      >
        <div className="flex items-center gap-4">
          <span className="font-semibold"></span>
          <span className="font-semibold">Finder</span>
          <span>文件</span>
          <span>编辑</span>
          <span>显示</span>
          <span>前往</span>
          <span>窗口</span>
          <span>帮助</span>
        </div>
        <div className="flex items-center gap-3">
          <span>100%</span>
          <span>Wi-Fi</span>
          <span>7月4日 周六 10:24</span>
        </div>
      </div>
      <div
        className="absolute left-1/2 top-0 -translate-x-1/2 rounded-b-[12px] bg-black"
        style={{ width: notchWidth, height: barHeight }}
      />
    </div>
  );
}
