import { useEffect, useMemo, useState } from "react";
import { api, type DailyRow, type QueryParams } from "@/lib/api";
import { formatCost, formatNumber, formatTokens } from "@/lib/format";
import { useI18n, type I18nKey } from "@/lib/i18n";
import { EmptyState } from "@/components/EmptyState";
import { Frame, FrameHeader, FramePanel } from "@/components/ui/frame";
import { SegmentedControl } from "@/components/ui/segmented-control";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Skeleton } from "@/components/ui/skeleton";
import { LoadError } from "@/components/LoadError";
import {
  CostTrendChart,
  TokenTrendChart,
  type Granularity,
} from "@/components/charts";

/** User-selectable re-bucketing; "hour" comes from the Today range. */
type Bucket = Exclude<Granularity, "hour">;

const DATE_HEAD: Record<Granularity, I18nKey> = {
  hour: "th.time",
  day: "th.date",
  week: "th.week",
  month: "th.month",
};

/** Monday of the week containing `date` (YYYY-MM-DD). */
function weekStart(date: string): string {
  const d = new Date(`${date}T00:00:00`);
  const day = (d.getDay() + 6) % 7; // Monday = 0
  d.setDate(d.getDate() - day);
  const mm = String(d.getMonth() + 1).padStart(2, "0");
  const dd = String(d.getDate()).padStart(2, "0");
  return `${d.getFullYear()}-${mm}-${dd}`;
}

/** Re-bucket per-(date, agent) rows into week or month buckets. */
function rebucket(rows: DailyRow[], granularity: Bucket): DailyRow[] {
  if (granularity === "day") return rows;
  const acc = new Map<string, DailyRow>();
  for (const r of rows) {
    const bucket =
      granularity === "month" ? r.date.slice(0, 7) : weekStart(r.date);
    const key = `${bucket}:${r.agent}`;
    const row = acc.get(key);
    if (!row) {
      acc.set(key, { ...r, date: bucket });
    } else {
      row.cost += r.cost;
      row.inputTokens += r.inputTokens;
      row.outputTokens += r.outputTokens;
      row.cacheCreationTokens += r.cacheCreationTokens;
      row.cacheReadTokens += r.cacheReadTokens;
      row.totalTokens += r.totalTokens;
      row.requests += r.requests;
    }
  }
  return [...acc.values()].sort((a, b) => a.date.localeCompare(b.date));
}

export function TrendsPage({
  params,
  refreshKey,
  hourly,
}: {
  params: QueryParams;
  refreshKey: number;
  hourly: boolean;
}) {
  const { t } = useI18n();
  // Rows remember which query they answer, so a range switch shows a
  // skeleton instead of the previous range's (or hourly vs daily) data.
  const [loaded, setLoaded] = useState<{
    key: string;
    rows: DailyRow[];
  } | null>(null);
  const [error, setError] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [bucket, setBucket] = useState<Bucket>("day");
  const queryKey = `${hourly}|${params.sinceMs}|${params.untilMs}|${params.costMode}`;

  useEffect(() => {
    let cancelled = false;
    setError(false);
    const fetch = hourly ? api.getHourly : api.getDaily;
    fetch(params)
      .then((rows) => !cancelled && setLoaded({ key: queryKey, rows }))
      .catch(() => !cancelled && setError(true));
    return () => {
      cancelled = true;
    };
  }, [queryKey, refreshKey, attempt]);

  const rawDaily = loaded?.key === queryKey ? loaded.rows : null;
  const granularity: Granularity = hourly ? "hour" : bucket;
  const daily = useMemo(
    () => (rawDaily && !hourly ? rebucket(rawDaily, bucket) : rawDaily),
    [rawDaily, bucket, hourly],
  );

  if (error) {
    return <LoadError onRetry={() => setAttempt((a) => a + 1)} />;
  }
  if (!daily) {
    return <Skeleton className="h-80" />;
  }

  // Collapse (date, agent) rows into one row per date for the table.
  const byDate = new Map<string, DailyRow>();
  for (const r of daily) {
    const acc = byDate.get(r.date);
    if (!acc) {
      byDate.set(r.date, { ...r, agent: "" });
    } else {
      acc.cost += r.cost;
      acc.inputTokens += r.inputTokens;
      acc.outputTokens += r.outputTokens;
      acc.cacheCreationTokens += r.cacheCreationTokens;
      acc.cacheReadTokens += r.cacheReadTokens;
      acc.totalTokens += r.totalTokens;
      acc.requests += r.requests;
    }
  }
  const tableRows = [...byDate.values()].sort((a, b) =>
    b.date.localeCompare(a.date),
  );

  const buckets: { id: Bucket; labelKey: I18nKey }[] = [
    { id: "day", labelKey: "trends.byDay" },
    { id: "week", labelKey: "trends.byWeek" },
    { id: "month", labelKey: "trends.byMonth" },
  ];

  return (
    <div className="flex flex-col gap-5">
      <Frame aria-labelledby="trends-charts">
        <FrameHeader>
          <h2 id="trends-charts" className="text-sm font-semibold">
            {t("trends.usage")}
          </h2>
          {!hourly && (
            <SegmentedControl
              label={t("trends.granularity")}
              options={buckets.map((item) => ({
                id: item.id,
                label: t(item.labelKey),
              }))}
              value={bucket}
              onChange={setBucket}
            />
          )}
        </FrameHeader>
        <FramePanel>
          {daily.length === 0 ? (
            <EmptyState />
          ) : (
            <div className="grid divide-y divide-border min-[1220px]:grid-cols-2 min-[1220px]:divide-x min-[1220px]:divide-y-0">
              <div className="min-w-0 p-4 sm:p-5">
                <h3 className="mb-3 text-sm font-medium">
                  {t(`trends.cost.${granularity}` as const)}
                </h3>
                <CostTrendChart rows={daily} granularity={granularity} />
              </div>
              <div className="min-w-0 p-4 sm:p-5">
                <h3 className="mb-3 text-sm font-medium">
                  {t(`trends.tokens.${granularity}` as const)}
                </h3>
                <TokenTrendChart rows={daily} granularity={granularity} />
              </div>
            </div>
          )}
        </FramePanel>
      </Frame>
      <Frame aria-labelledby="trends-breakdown">
        <FrameHeader>
          <h2 id="trends-breakdown" className="text-sm font-semibold">
            {t(`trends.breakdown.${granularity}` as const)}
          </h2>
        </FrameHeader>
        <FramePanel>
          {tableRows.length === 0 ? (
            <EmptyState />
          ) : (
            <Table className="min-w-[565px]">
              <TableHeader>
                <TableRow>
                  <TableHead>{t(DATE_HEAD[granularity])}</TableHead>
                  <TableHead className="text-right">{t("th.input")}</TableHead>
                  <TableHead className="text-right">{t("th.output")}</TableHead>
                  <TableHead className="text-right">
                    {t("th.cacheWrite")}
                  </TableHead>
                  <TableHead className="text-right">
                    {t("th.cacheRead")}
                  </TableHead>
                  <TableHead className="text-right">
                    {t("th.requests")}
                  </TableHead>
                  <TableHead className="text-right">{t("th.cost")}</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {tableRows.map((r) => (
                  <TableRow key={r.date}>
                    <TableCell className="font-medium">{r.date}</TableCell>
                    <TableCell className="text-right tabular-nums">
                      {formatTokens(r.inputTokens)}
                    </TableCell>
                    <TableCell className="text-right tabular-nums">
                      {formatTokens(r.outputTokens)}
                    </TableCell>
                    <TableCell className="text-right tabular-nums">
                      {formatTokens(r.cacheCreationTokens)}
                    </TableCell>
                    <TableCell className="text-right tabular-nums">
                      {formatTokens(r.cacheReadTokens)}
                    </TableCell>
                    <TableCell className="text-right tabular-nums">
                      {formatNumber(r.requests)}
                    </TableCell>
                    <TableCell className="text-right font-medium tabular-nums">
                      {formatCost(r.cost)}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          )}
        </FramePanel>
      </Frame>
    </div>
  );
}
