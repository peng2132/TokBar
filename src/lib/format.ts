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

export const AGENT_LABELS: Record<string, string> = {
  "claude-code": "Claude Code",
  codex: "Codex CLI",
  kimi: "Kimi Code",
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
// accent chosen in Settings (SVG fill/stroke accept var()). Agents beyond
// the named ones fall back to CHART_PALETTE by index.
export const AGENT_COLORS: Record<string, string> = {
  "claude-code": "var(--chart-1)",
  codex: "var(--chart-2)",
  kimi: "var(--chart-3)",
  gemini: "var(--chart-4)",
  opencode: "var(--chart-5)",
};

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

export function agentColor(agent: string, index = 0): string {
  return AGENT_COLORS[agent] ?? CHART_PALETTE[index % CHART_PALETTE.length];
}
