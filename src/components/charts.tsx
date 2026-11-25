import { useId, useMemo } from "react";
import {
  Area,
  AreaChart,
  Bar,
  BarChart,
  CartesianGrid,
  Legend,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import type { DailyRow } from "@/lib/api";
import { agentColor, agentLabel, formatCost, formatTokens } from "@/lib/format";
import { useI18n } from "@/lib/i18n";

const AXIS_STYLE = { fontSize: 10, fill: "var(--muted-foreground)" };
const GRID_STROKE = "var(--border)";
const TOOLTIP_STYLE = {
  backgroundColor: "var(--card)",
  border: "1px solid var(--border)",
  borderRadius: 10,
  color: "var(--foreground)",
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
  aggregate = false,
  height = 280,
}: {
  rows: DailyRow[];
  granularity: Granularity;
  aggregate?: boolean;
  height?: number;
}) {
  const fillId = useId();
  const { data, agents } = useMemo(() => {
    const pivot = pivotDailyByAgent(rows, "cost", granularity);
    if (!aggregate) return pivot;
    return {
      agents: pivot.agents,
      data: pivot.data.map((row) => ({
        ...row,
        total: pivot.agents.reduce((sum, agent) => sum + Number(row[agent]), 0),
      })),
    };
  }, [rows, granularity, aggregate]);
  return (
    <ResponsiveContainer width="100%" height={height} minWidth={0}>
      <AreaChart data={data} margin={{ top: 5, right: 10, left: 0, bottom: 0 }}>
        {aggregate && (
          <defs>
            <linearGradient id={fillId} x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor="var(--primary)" stopOpacity={0.26} />
              <stop
                offset="100%"
                stopColor="var(--primary)"
                stopOpacity={0.015}
              />
            </linearGradient>
          </defs>
        )}
        <CartesianGrid
          vertical={false}
          strokeDasharray="3 4"
          stroke={GRID_STROKE}
        />
        <XAxis
          dataKey="date"
          axisLine={false}
          tickLine={false}
          minTickGap={20}
          tickMargin={8}
          tick={AXIS_STYLE}
          tickFormatter={shortDate}
        />
        <YAxis
          axisLine={false}
          tickLine={false}
          tick={AXIS_STYLE}
          tickFormatter={formatCost}
          width={58}
        />
        <Tooltip
          cursor={{ stroke: "var(--input)", strokeDasharray: "3 3" }}
          content={({ active, payload, label }) => {
            const point = payload?.[0]?.payload as
              | Record<string, number | string>
              | undefined;
            if (!active || !point) return null;
            const items = agents.filter((agent) => Number(point[agent]) > 0);
            return (
              <div
                style={TOOLTIP_STYLE}
                className="flex flex-col gap-1.5 px-3 py-2 shadow-md"
              >
                <div className="flex justify-between gap-4 font-medium">
                  <span className="text-muted-foreground">{label}</span>
                  {aggregate && <span>{formatCost(Number(point.total))}</span>}
                </div>
                {items.map((agent) => (
                  <div
                    key={agent}
                    className="flex items-baseline justify-between gap-4"
                  >
                    <span className="flex items-center gap-1.5">
                      <span
                        aria-hidden
                        className="size-2 rounded-full"
                        style={{ backgroundColor: agentColor(agent) }}
                      />
                      {agentLabel(agent)}
                    </span>
                    <span className="tabular-nums">
                      {formatCost(Number(point[agent]))}
                    </span>
                  </div>
                ))}
              </div>
            );
          }}
        />
        {!aggregate && (
          <Legend
            iconType="circle"
            iconSize={7}
            wrapperStyle={{ fontSize: 11, paddingTop: 10 }}
            formatter={(value) => (
              <span className="text-muted-foreground">
                {agentLabel(String(value))}
              </span>
            )}
          />
        )}
        {aggregate ? (
          <Area
            type="monotone"
            dataKey="total"
            stroke="var(--primary)"
            strokeWidth={2.25}
            fill={`url(#${fillId})`}
            isAnimationActive={false}
            activeDot={{
              r: 4,
              fill: "var(--primary)",
              stroke: "var(--card)",
              strokeWidth: 2,
            }}
          />
        ) : (
          agents.map((agent) => (
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
              isAnimationActive={false}
            />
          ))
        )}
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
        ({
          date: r.date,
          input: 0,
          output: 0,
          cacheRead: 0,
          cacheCreate: 0,
        } as Record<string, number | string>);
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
    {
      key: "cacheCreate",
      label: t("chart.cacheWrite"),
      color: "var(--chart-4)",
    },
  ];

  return (
    <ResponsiveContainer width="100%" height={280}>
      <BarChart data={data} margin={{ top: 5, right: 10, left: 0, bottom: 0 }}>
        <CartesianGrid
          vertical={false}
          strokeDasharray="3 4"
          stroke={GRID_STROKE}
        />
        <XAxis
          dataKey="date"
          axisLine={false}
          tickLine={false}
          minTickGap={20}
          tickMargin={8}
          tick={AXIS_STYLE}
          tickFormatter={shortDate}
        />
        <YAxis
          axisLine={false}
          tickLine={false}
          tick={AXIS_STYLE}
          tickFormatter={formatTokens}
          width={58}
        />
        <Tooltip
          contentStyle={TOOLTIP_STYLE}
          itemStyle={{ color: "var(--foreground)" }}
          cursor={{ fill: "var(--muted)", opacity: 0.65 }}
          formatter={(v, name) => [
            formatTokens(Number(v ?? 0)),
            series.find((s) => s.key === name)?.label ?? String(name),
          ]}
        />
        <Legend
          iconType="circle"
          iconSize={7}
          wrapperStyle={{ fontSize: 11, paddingTop: 10 }}
          formatter={(value) => (
            <span className="text-muted-foreground">
              {series.find((item) => item.key === value)?.label ??
                String(value)}
            </span>
          )}
        />
        {series.map((s) => (
          <Bar
            key={s.key}
            dataKey={s.key}
            stackId="tokens"
            fill={s.color}
            isAnimationActive={false}
          />
        ))}
      </BarChart>
    </ResponsiveContainer>
  );
}
