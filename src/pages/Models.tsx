import { useEffect, useState } from "react";
import { ChevronDown } from "lucide-react";
import {
  api,
  type ModelPerformance,
  type ModelRow,
  type QueryParams,
} from "@/lib/api";
import {
  formatCacheHitRate,
  formatCost,
  formatFirstToken,
  formatNumber,
  formatOutputSpeed,
  formatTokens,
  performanceSourceLabel,
} from "@/lib/format";
import { Button } from "@/components/ui/button";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Skeleton } from "@/components/ui/skeleton";
import { EmptyState } from "@/components/EmptyState";
import {
  UsageDistribution,
  modelDistribution,
} from "@/components/UsageDistribution";
import { Frame, FrameHeader, FramePanel } from "@/components/ui/frame";
import { SegmentedControl } from "@/components/ui/segmented-control";
import { LoadError } from "@/components/LoadError";
import { UnpricedBadge } from "@/components/UnpricedBadge";
import { useI18n } from "@/lib/i18n";

type DistributionMetric = "cost" | "totalTokens";

export function ModelsPage({
  params,
  refreshKey,
}: {
  params: QueryParams;
  refreshKey: number;
}) {
  const [data, setData] = useState<{
    key: string;
    models: ModelRow[];
    performance: ModelPerformance[];
  } | null>(null);
  const [error, setError] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [metric, setMetric] = useState<DistributionMetric>("cost");
  const queryKey = `${params.sinceMs}|${params.untilMs}|${params.costMode}`;

  useEffect(() => {
    let cancelled = false;
    setError(false);
    Promise.all([api.getModels(params), api.getModelPerformance(params)])
      .then(([models, performance]) => {
        if (!cancelled) setData({ key: queryKey, models, performance });
      })
      .catch(() => !cancelled && setError(true));
    return () => {
      cancelled = true;
    };
  }, [queryKey, refreshKey, attempt]);

  const current = data?.key === queryKey ? data : null;
  if (error)
    return <LoadError onRetry={() => setAttempt((value) => value + 1)} />;
  if (!current) return <Skeleton className="h-80" />;

  return (
    <div className="flex flex-col gap-5">
      <PerformanceTable rows={current.performance} />
      <ModelUsageTable
        rows={current.models}
        metric={metric}
        onMetricChange={setMetric}
      />
    </div>
  );
}

const PERFORMANCE_PREVIEW_ROWS = 5;

function PerformanceTable({ rows }: { rows: ModelPerformance[] }) {
  const { t } = useI18n();
  const [expanded, setExpanded] = useState(false);
  const shown = expanded ? rows : rows.slice(0, PERFORMANCE_PREVIEW_ROWS);

  return (
    <Frame aria-labelledby="model-performance-heading">
      <FrameHeader className="items-start gap-x-4">
        <div className="min-w-0 flex flex-[1_1_25rem] flex-col gap-1">
          <h2 id="model-performance-heading" className="text-sm font-semibold">
            {t("performance.title")}
          </h2>
          <p className="text-xs leading-relaxed text-muted-foreground">
            {t("performance.description")}
          </p>
        </div>
        {rows.length > PERFORMANCE_PREVIEW_ROWS && (
          <Button
            variant="outline"
            size="sm"
            aria-expanded={expanded}
            aria-controls="model-performance-results"
            onClick={() => setExpanded((value) => !value)}
          >
            {expanded
              ? t("performance.collapse")
              : t("performance.expand", { n: formatNumber(rows.length) })}
            <ChevronDown
              aria-hidden
              className={expanded ? "size-3.5 rotate-180" : "size-3.5"}
            />
          </Button>
        )}
      </FrameHeader>
      <FramePanel id="model-performance-results">
        {rows.length === 0 ? (
          <EmptyState message={t("performance.empty")} />
        ) : (
          <Table className="min-w-[465px]">
            <TableHeader>
              <TableRow>
                <TableHead>{t("th.model")}</TableHead>
                <TableHead>{t("performance.source")}</TableHead>
                <TableHead className="text-right">
                  {t("performance.output")}
                </TableHead>
                <TableHead className="text-right">
                  {t("performance.firstToken")}
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {shown.map((row) => {
                const source = performanceSourceLabel(row);
                return (
                  <TableRow
                    key={JSON.stringify([row.source, row.provider, row.model])}
                  >
                    <TableCell
                      className="max-w-60 truncate py-2 font-medium"
                      title={row.model}
                    >
                      {row.model}
                    </TableCell>
                    <TableCell
                      className="max-w-44 truncate py-2 text-xs text-muted-foreground"
                      title={source}
                    >
                      {source}
                    </TableCell>
                    <TableCell className="py-2 text-right whitespace-nowrap tabular-nums">
                      <span
                        className={
                          row.outputTps == null
                            ? "text-muted-foreground"
                            : "font-semibold text-primary"
                        }
                      >
                        {row.outputTps == null
                          ? "—"
                          : formatOutputSpeed(row.outputTps)}
                      </span>
                      {row.outputSamples > 0 && (
                        <div className="text-[11px] text-muted-foreground">
                          {t("performance.samples", {
                            n: formatNumber(row.outputSamples),
                          })}
                        </div>
                      )}
                    </TableCell>
                    <TableCell className="py-2 text-right whitespace-nowrap tabular-nums">
                      {row.firstTokenMs == null
                        ? "—"
                        : formatFirstToken(row.firstTokenMs)}
                      {row.firstTokenSamples > 0 && (
                        <div className="text-[11px] text-muted-foreground">
                          {t("performance.samples", {
                            n: formatNumber(row.firstTokenSamples),
                          })}
                        </div>
                      )}
                    </TableCell>
                  </TableRow>
                );
              })}
            </TableBody>
          </Table>
        )}
      </FramePanel>
    </Frame>
  );
}

function ModelUsageTable({
  rows,
  metric,
  onMetricChange,
}: {
  rows: ModelRow[];
  metric: DistributionMetric;
  onMetricChange: (metric: DistributionMetric) => void;
}) {
  const { t } = useI18n();

  return (
    <Frame aria-labelledby="model-usage-heading">
      <FrameHeader>
        <h2 id="model-usage-heading" className="text-sm font-semibold">
          {t("models.all")}
        </h2>
        {rows.length > 0 && (
          <SegmentedControl
            label={t("models.distribution")}
            options={[
              { id: "cost", label: t("th.cost") },
              { id: "totalTokens", label: t("th.tokens") },
            ]}
            value={metric}
            onChange={onMetricChange}
          />
        )}
      </FrameHeader>
      <FramePanel>
        {rows.length === 0 ? (
          <EmptyState />
        ) : (
          <>
            <div className="border-b border-border px-4 py-4">
              <UsageDistribution
                rows={modelDistribution(rows, metric)}
                formatValue={metric === "cost" ? formatCost : formatTokens}
              />
            </div>
            <Table className="min-w-[630px]">
              <TableHeader>
                <TableRow>
                  <TableHead>{t("th.model")}</TableHead>
                  <TableHead className="text-right">{t("th.input")}</TableHead>
                  <TableHead className="text-right">{t("th.output")}</TableHead>
                  <TableHead className="text-right">
                    {t("th.cacheWrite")}
                  </TableHead>
                  <TableHead
                    className="text-right"
                    title={t("models.cacheHitTip")}
                  >
                    {t("th.cacheReadHit")}
                  </TableHead>
                  <TableHead className="text-right">{t("th.requests")}</TableHead>
                  <TableHead className="text-right">{t("th.cost")}</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {rows.map((model) => (
                  <TableRow key={model.model}>
                    <TableCell className="font-medium">
                      <span className="flex max-w-64 items-center gap-1.5">
                        <span className="truncate" title={model.model}>
                          {model.model}
                        </span>
                        {model.priced === false && <UnpricedBadge />}
                      </span>
                    </TableCell>
                    <TableCell className="text-right tabular-nums">
                      {formatTokens(model.inputTokens)}
                    </TableCell>
                    <TableCell className="text-right tabular-nums">
                      {formatTokens(model.outputTokens)}
                    </TableCell>
                    <TableCell className="text-right tabular-nums">
                      {formatTokens(model.cacheCreationTokens)}
                    </TableCell>
                    <TableCell className="text-right tabular-nums">
                      {formatTokens(model.cacheReadTokens)}
                      <div className="text-[11px] text-muted-foreground">
                        {formatCacheHitRate(model) ?? "—"}
                      </div>
                    </TableCell>
                    <TableCell className="text-right tabular-nums">
                      {formatNumber(model.requests)}
                    </TableCell>
                    <TableCell className="text-right font-semibold tabular-nums">
                      {formatCost(model.cost)}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </>
        )}
      </FramePanel>
    </Frame>
  );
}
