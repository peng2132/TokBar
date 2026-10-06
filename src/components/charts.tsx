import { useMemo } from "react";
import {
  Area,
  AreaChart,
  Bar,
  BarChart,
  CartesianGrid,
  Cell,
  Legend,
  Pie,
  PieChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import type { DailyRow, ModelRow } from "@/lib/api";
import {
  agentColor,
  agentLabel,
  CHART_PALETTE,
  formatCost,
  formatTokens,
  modelDisplayNames,
} from "@/lib/format";
import { useI18n } from "@/lib/i18n";

const AXIS_STYLE = { fontSize: 11, fill: "var(--muted-foreground)" };
const GRID_STROKE = "var(--border)";
const TOOLTIP_STYLE = {
  backgroundColor: "var(--card)",
  border: "1px solid var(--border)",
  borderRadius: 8,
  fontSize: 12,
};

/** Bucket size of the rows a chart is given; always passed explicitly by
 *  the caller (never guessed from the gaps between dates). */
export type Granularity = "hour" | "day" | "week" | "month";

/** Label shape per granularity: "HH:00", "YYYY-MM-DD" (weeks are keyed by
 *  their Monday), "YYYY-MM". */
const LABEL_FORMAT: Record<Granularity, RegExp> = {
  hour: /^\d{2}:00$/,
  day: /^\d{4}-\d{2}-\d{2}$/,
  week: /^\d{4}-\d{2}-\d{2}$/,
  month: /^\d{4}-\d{2}$/,
};

const pad2 = (n: number) => String(n).padStart(2, "0");

/**
 * Aggregation buckets only exist where there was usage; quiet hours/days
 * are absent, which breaks lines in the chart. Fill the timeline so every
 * bucket between the first and last is present (zeros for quiet periods).
 */
function fillTimeline(labels: string[], granularity: Granularity): string[] {
  if (labels.length === 0) return labels;
  const sorted = [...labels].sort();
  // Labels that don't fit the declared granularity are drawn as they are
  // rather than misread (e.g. dates parsed as hours).
  if (!sorted.every((l) => LABEL_FORMAT[granularity].test(l))) return sorted;
  const first = sorted[0];
  const last = sorted[sorted.length - 1];
  const out: string[] = [];
  if (granularity === "hour") {
    // From midnight to the last active hour.
    const lastHour = parseInt(last, 10);
    for (let h = 0; h <= lastHour; h++) out.push(`${pad2(h)}:00`);
  } else if (granularity === "month") {
    let [y, m] = first.split("-").map(Number);
    while (out.length < 1200) {
      const label = `${y}-${pad2(m)}`;
      out.push(label);
      if (label >= last) break;
      m += 1;
      if (m > 12) {
        m = 1;
        y += 1;
      }
    }
  } else {
    // Day / week: calendar steps (setDate), so DST days don't skew it.
    const step = granularity === "week" ? 7 : 1;
    const d = new Date(`${first}T00:00:00`);
    while (out.length < 5000) {
      const label = `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
      out.push(label);
      if (label >= last) break;
      d.setDate(d.getDate() + step);
    }
  }
  // Never drop a real bucket that falls off the generated grid.
  return [...new Set([...out, ...sorted])].sort();
}

/** Pivot (bucket, agent) rows into one row per bucket with per-agent keys. */
export function pivotDailyByAgent(
  rows: DailyRow[],
  metric: "cost" | "totalTokens" | "requests",
  granularity: Granularity,
): { data: Record<string, number | string>[]; agents: string[] } {
  const agents = [...new Set(rows.map((r) => r.agent))].sort();
  const byDate = new Map<string, Record<string, number | string>>();
  for (const r of rows) {
    const row = byDate.get(r.date) ?? { date: r.date };
    row[r.agent] = ((row[r.agent] as number) ?? 0) + r[metric];
    byDate.set(r.date, row);
  }
  const data = fillTimeline([...byDate.keys()], granularity).map((label) => {
    const row = byDate.get(label) ?? { date: label };
    for (const agent of agents) {
      row[agent] = (row[agent] as number) ?? 0;
    }
    return row;
  });
  return { data, agents };
}

function shortDate(d: string): string {
  // "YYYY-MM-DD" -> "MM-DD"; hourly buckets ("08:00") pass through as-is.
  return /^\d{4}-\d{2}-\d{2}$/.test(d) ? d.slice(5) : d;
}

export function CostTrendChart({
  rows,
  granularity,
}: {
  rows: DailyRow[];
  granularity: Granularity;
}) {
  const { data, agents } = useMemo(
    () => pivotDailyByAgent(rows, "cost", granularity),
    [rows, granularity],
  );
  return (
    <ResponsiveContainer width="100%" height={280}>
      <AreaChart data={data} margin={{ top: 8, right: 8, left: 0, bottom: 0 }}>
        <CartesianGrid strokeDasharray="3 3" stroke={GRID_STROKE} />
        <XAxis dataKey="date" tick={AXIS_STYLE} tickFormatter={shortDate} />
        <YAxis tick={AXIS_STYLE} tickFormatter={(v) => formatCost(v)} width={70} />
        <Tooltip
          content={({ active, payload, label }) => {
            // Zero-cost agents are invisible in the stack; hide their
            // "$0.0000" rows in the tooltip too.
            const items = (payload ?? []).filter((p) => Number(p.value) > 0);
            if (!active || items.length === 0) return null;
            return (
              <div style={{ ...TOOLTIP_STYLE, padding: "8px 12px" }}>
                <div style={{ marginBottom: 4 }}>{label}</div>
                {items.map((p) => (
                  <div
                    key={String(p.dataKey)}
                    style={{ color: agentColor(String(p.dataKey)) }}
                  >
                    {agentLabel(String(p.dataKey))} : {formatCost(Number(p.value))}
                  </div>
                ))}
              </div>
            );
          }}
        />
        <Legend formatter={(v) => agentLabel(String(v))} />
        {agents.map((agent) => (
          // Fill-only stacked bands: an agent's usage is its band
          // thickness. No per-series stroke or hover dot — a zero-usage
          // series would otherwise draw its line and dot on top of the
          // band below it, reading as if it had that band's value.
          <Area
            key={agent}
            type="monotone"
            dataKey={agent}
            stackId="cost"
            stroke="none"
            fill={agentColor(agent)}
            fillOpacity={0.45}
            activeDot={false}
          />
        ))}
      </AreaChart>
    </ResponsiveContainer>
  );
}

export function TokenTrendChart({
  rows,
  granularity,
}: {
  rows: DailyRow[];
  granularity: Granularity;
}) {
  const { t } = useI18n();
  const data = useMemo(() => {
    const byDate = new Map<string, Record<string, number | string>>();
    for (const r of rows) {
      const row =
        byDate.get(r.date) ??
        ({ date: r.date, input: 0, output: 0, cacheRead: 0, cacheCreate: 0 } as Record<
          string,
          number | string
        >);
      row.input = (row.input as number) + r.inputTokens;
      row.output = (row.output as number) + r.outputTokens;
      row.cacheRead = (row.cacheRead as number) + r.cacheReadTokens;
      row.cacheCreate = (row.cacheCreate as number) + r.cacheCreationTokens;
      byDate.set(r.date, row);
    }
    return fillTimeline([...byDate.keys()], granularity).map(
      (label) =>
        byDate.get(label) ?? {
          date: label,
          input: 0,
          output: 0,
          cacheRead: 0,
          cacheCreate: 0,
        },
    );
  }, [rows, granularity]);

  const series: { key: string; label: string; color: string }[] = [
    { key: "input", label: t("chart.input"), color: "var(--chart-2)" },
    { key: "output", label: t("chart.output"), color: "var(--chart-1)" },
    { key: "cacheRead", label: t("chart.cacheRead"), color: "var(--chart-3)" },
    { key: "cacheCreate", label: t("chart.cacheWrite"), color: "var(--chart-4)" },
  ];

  return (
    <ResponsiveContainer width="100%" height={280}>
      <BarChart data={data} margin={{ top: 8, right: 8, left: 0, bottom: 0 }}>
        <CartesianGrid strokeDasharray="3 3" stroke={GRID_STROKE} />
        <XAxis dataKey="date" tick={AXIS_STYLE} tickFormatter={shortDate} />
        <YAxis tick={AXIS_STYLE} tickFormatter={(v) => formatTokens(v)} width={70} />
        <Tooltip
          contentStyle={TOOLTIP_STYLE}
          formatter={(v, name) => [
            formatTokens(Number(v ?? 0)),
            series.find((s) => s.key === name)?.label ?? String(name),
          ]}
        />
        <Legend formatter={(v) => series.find((s) => s.key === v)?.label ?? String(v)} />
        {series.map((s) => (
          <Bar key={s.key} dataKey={s.key} stackId="tokens" fill={s.color} />
        ))}
      </BarChart>
    </ResponsiveContainer>
  );
}

export interface PieDatum {
  /** Stable identity (full model id, agent id): keys slices and colors,
   *  so two entries that happen to share a display name never collide. */
  key: string;
  /** Display label. */
  name: string;
  value: number;
  color?: string;
}

/** Pie slices for the top `limit` models: keyed by the full model id,
 *  labeled with short names that stay distinct when two ids collide. */
export function modelPie(
  models: ModelRow[],
  metric: "cost" | "totalTokens",
  limit: number,
): PieDatum[] {
  const top = models.slice(0, limit);
  const names = modelDisplayNames(top.map((m) => m.model));
  return top.map((m) => ({
    key: m.model,
    name: names.get(m.model) ?? m.model,
    value: m[metric],
  }));
}

export function DistributionPie({
  data,
  valueFormatter,
}: {
  data: PieDatum[];
  valueFormatter: (v: number) => string;
}) {
  // Colors are keyed off the caller's (unsorted) order so the same entry
  // gets the same color in every pie on the page; slices and the legend
  // are then drawn largest-first. The legend payload is explicit because
  // recharts does not follow the sorted data order on its own.
  const { sorted, colorOf } = useMemo(() => {
    const colors = new Map<string, string>();
    data.forEach((d, i) =>
      colors.set(d.key, d.color ?? CHART_PALETTE[i % CHART_PALETTE.length]),
    );
    return {
      sorted: [...data].sort((a, b) => b.value - a.value),
      colorOf: (key: string) => colors.get(key) ?? CHART_PALETTE[0],
    };
  }, [data]);
  return (
    <ResponsiveContainer width="100%" height={260}>
      <PieChart>
        <Pie
          data={sorted}
          dataKey="value"
          nameKey="name"
          innerRadius={55}
          outerRadius={90}
          paddingAngle={2}
          strokeWidth={0}
        >
          {sorted.map((d) => (
            <Cell key={d.key} fill={colorOf(d.key)} />
          ))}
        </Pie>
        <Tooltip
          contentStyle={TOOLTIP_STYLE}
          formatter={(v, name) => [valueFormatter(Number(v ?? 0)), String(name)]}
        />
        <Legend
          layout="vertical"
          align="right"
          verticalAlign="middle"
          wrapperStyle={{ fontSize: 12 }}
          content={() => (
            <ul style={{ listStyle: "none", margin: 0, padding: 0 }}>
              {sorted.map((d) => (
                <li
                  key={d.key}
                  style={{
                    color: colorOf(d.key),
                    display: "flex",
                    alignItems: "center",
                    gap: 6,
                    marginBottom: 4,
                  }}
                >
                  <span
                    style={{
                      width: 10,
                      height: 10,
                      borderRadius: 2,
                      flexShrink: 0,
                      background: colorOf(d.key),
                    }}
                  />
                  {d.name}
                </li>
              ))}
            </ul>
          )}
        />
      </PieChart>
    </ResponsiveContainer>
  );
}
