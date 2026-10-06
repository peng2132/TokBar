import type { I18nKey } from "./i18n";

export function formatCost(value: number): string {
  if (value >= 1000) {
    return `$${(value / 1000).toFixed(2)}k`;
  }
  // Sub-cent values keep 4 decimals so tiny per-request prices stay
  // visible; everything else uses the conventional 2, so table columns
  // and axis ticks don't mix "$0.3001" with "$8.55".
  if (value > 0 && value < 0.01) {
    return `$${value.toFixed(4)}`;
  }
  return `$${value.toFixed(2)}`;
}

export function formatTokens(value: number): string {
  if (value >= 1e9) return `${(value / 1e9).toFixed(2)}B`;
  if (value >= 1e6) return `${(value / 1e6).toFixed(2)}M`;
  if (value >= 1e3) return `${(value / 1e3).toFixed(1)}K`;
  return String(value);
}

export function formatNumber(value: number): string {
  return new Intl.NumberFormat().format(value);
}

/** Date/time rendering follows the in-app language toggle, not the OS
 *  locale — mirrors detectLang() in i18n.tsx (kept dependency-free). */
function uiLocale(): string {
  const saved = localStorage.getItem("tokbar-lang");
  const lang =
    saved === "zh" || saved === "en"
      ? saved
      : navigator.language.toLowerCase().startsWith("zh")
        ? "zh"
        : "en";
  return lang === "zh" ? "zh-CN" : "en-US";
}

export function formatDateTime(ms: number): string {
  return new Date(ms).toLocaleString(uiLocale(), {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function formatTime(ms: number): string {
  return new Date(ms).toLocaleTimeString(uiLocale(), {
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** "还剩 2 小时 5 分" — time left in a billing block. */
export function remainingLabel(
  endMs: number,
  t: (key: I18nKey, vars?: Record<string, string | number>) => string,
  now = Date.now(),
): string {
  const mins = Math.max(0, Math.round((endMs - now) / 60_000));
  const h = Math.floor(mins / 60);
  const m = mins % 60;
  return h > 0 ? t("blocks.remainHM", { h, m }) : t("blocks.remainM", { m });
}

/** "claude-opus-4-20250101" -> "opus-4" style short model names (ccusage-like). */
export function shortModelName(model: string): string {
  let name = model.replace(/^anthropic\//, "").replace(/^claude-/, "");
  const parts = name.split("-");
  if (parts.length > 1 && /^\d{8}$/.test(parts[parts.length - 1])) {
    parts.pop();
    name = parts.join("-");
  }
  return name;
}

/** Display name per model id: the short name, unless two distinct ids
 *  shorten to the same thing (e.g. `anthropic/claude-sonnet-4-5` vs
 *  `claude-sonnet-4-5-20250929`) — those keep their full id so they stay
 *  distinguishable in legends and tooltips. */
export function modelDisplayNames(models: string[]): Map<string, string> {
  const ids = [...new Set(models)];
  const count = new Map<string, number>();
  for (const id of ids) {
    const s = shortModelName(id);
    count.set(s, (count.get(s) ?? 0) + 1);
  }
  return new Map(
    ids.map((id) => {
      const s = shortModelName(id);
      return [id, (count.get(s) ?? 0) > 1 ? id : s];
    }),
  );
}

export const AGENT_LABELS: Record<string, string> = {
  "claude-code": "Claude Code",
  codex: "Codex CLI",
  kimi: "Kimi CLI",
  gemini: "Gemini CLI",
  opencode: "OpenCode",
  openclaw: "OpenClaw",
  copilot: "Copilot CLI",
  qwen: "Qwen Code",
  amp: "Amp",
  droid: "Droid",
  goose: "Goose",
  kilo: "Kilo",
  codebuff: "Codebuff",
  hermes: "Hermes",
  pi: "pi-agent",
};

// Chart colors reference theme variables so every chart re-colors with the
// accent chosen in Settings (SVG fill/stroke accept var()). The five
// --chart-N slots are reserved for these named agents.
export const AGENT_COLORS: Record<string, string> = {
  "claude-code": "var(--chart-1)",
  codex: "var(--chart-2)",
  kimi: "var(--chart-3)",
  gemini: "var(--chart-4)",
  opencode: "var(--chart-5)",
};

// Every other agent hashes (by name, never by position) into this
// palette, which is disjoint from the --chart-N accent sets in theme.tsx,
// so an unknown agent can never borrow a named agent's color and keeps
// the same color on every chart and every page.
const EXTRA_AGENT_COLORS = [
  "#64748b", // slate
  "#d946ef", // fuchsia
  "#84cc16", // lime
  "#06b6d4", // cyan
  "#ef4444", // red
  "#6366f1", // indigo
  "#eab308", // yellow
  "#14b8a6", // teal
];

/** FNV-1a: small, stable string hash for deterministic color picks. */
export function hashString(s: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h;
}

/** Fixed color for named entries, hashed pick from `extra` otherwise. */
export function colorByName(
  name: string,
  named: Record<string, string>,
  extra: string[],
): string {
  return named[name] ?? extra[hashString(name) % extra.length];
}

/** Palette for non-agent series (e.g. models), assigned by the caller's order. */
export const CHART_PALETTE = [
  "var(--chart-1)",
  "var(--chart-2)",
  "var(--chart-3)",
  "var(--chart-4)",
  "var(--chart-5)",
  "#94a3b8",
  "#64748b",
  "#475569",
];

export function agentLabel(agent: string): string {
  return AGENT_LABELS[agent] ?? agent;
}

/** Stable per-agent color, identical on every chart regardless of order. */
export function agentColor(agent: string): string {
  return colorByName(agent, AGENT_COLORS, EXTRA_AGENT_COLORS);
}
