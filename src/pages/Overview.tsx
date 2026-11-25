import { useEffect, useState } from "react";
import {
  api,
  type AgentBreakdown,
  type DailyRow,
  type ModelRow,
  type Overview as OverviewData,
  type ProjectRow,
  type QueryParams,
} from "@/lib/api";
import {
  agentColor,
  agentLabel,
  formatCacheHitRate,
  formatCost,
  formatNumber,
  formatTokens,
} from "@/lib/format";
import { startOfMonth } from "@/lib/dates";
import { useI18n } from "@/lib/i18n";
import { useSubscriptions } from "@/lib/subscriptions";
import { Skeleton } from "@/components/ui/skeleton";
import { LoadError } from "@/components/LoadError";
import { RoiCard } from "@/components/RoiCard";
import { CostTrendChart } from "@/components/charts";
import { EmptyState } from "@/components/EmptyState";
import {
  UsageDistribution,
  modelDistribution,
} from "@/components/UsageDistribution";
import { Frame, FrameHeader, FramePanel } from "@/components/ui/frame";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

interface OverviewLoad {
  /** Which query these rows answer (see `queryKey`). */
  key: string;
  overview: OverviewData;
  daily: DailyRow[];
  models: ModelRow[];
  projects: ProjectRow[];
}

export function OverviewPage({
  params,
  refreshKey,
  hourly,
}: {
  params: QueryParams;
  refreshKey: number;
  hourly: boolean;
}) {
  const { t } = useI18n();
  const { subscriptions } = useSubscriptions();
  const hasSubs = subscriptions.length > 0;
  const [loaded, setLoaded] = useState<OverviewLoad | null>(null);
  const [error, setError] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [monthByAgent, setMonthByAgent] = useState<AgentBreakdown[] | null>(
    null,
  );
  const [roiError, setRoiError] = useState(false);
  const [roiAttempt, setRoiAttempt] = useState(0);

  // ROI always reflects this calendar month at API prices, independent of
  // the page's range/cost-mode selector. Only fetched when there are
  // subscriptions to price against. The month bound is computed per fetch.
  useEffect(() => {
    if (!hasSubs) return;
    let cancelled = false;
    setRoiError(false);
    api
      .getOverview({ sinceMs: startOfMonth(), costMode: "calculate" })
      .then((o) => !cancelled && setMonthByAgent(o.byAgent))
      .catch((e) => {
        console.error("ROI query failed:", e);
        if (!cancelled) setRoiError(true);
      });
    return () => {
      cancelled = true;
    };
  }, [hasSubs, refreshKey, roiAttempt]);

  // Results are tagged with the query they answer: after a range switch
  // the previous range's numbers are never shown under the new button.
  const queryKey = `${hourly}|${params.sinceMs}|${params.untilMs}|${params.costMode}`;

  useEffect(() => {
    let cancelled = false;
    setError(false);
    Promise.all([
      api.getOverview(params),
      hourly ? api.getHourly(params) : api.getDaily(params),
      api.getModels(params),
      api.getProjects({ ...params, limit: 6 }),
    ])
      .then(([overview, daily, models, projects]) => {
        if (cancelled) return;
        setLoaded({ key: queryKey, overview, daily, models, projects });
      })
      .catch((e) => {
        console.error("overview query failed:", e);
        if (!cancelled) setError(true);
      });
    return () => {
      cancelled = true;
    };
  }, [queryKey, refreshKey, attempt]);

  const current = loaded?.key === queryKey ? loaded : null;
  if (error)
    return <LoadError onRetry={() => setAttempt((value) => value + 1)} />;
  if (!current) return <Skeleton className="h-[360px]" />;

  const { overview, daily, models, projects } = current;
  const totals = overview.totals;
  const metrics = [
    {
      title: t("overview.totalTokens"),
      value: formatTokens(totals.totalTokens),
      detail: t("overview.inOut", {
        in: formatTokens(totals.inputTokens),
        out: formatTokens(totals.outputTokens),
      }),
    },
    { title: t("overview.requests"), value: formatNumber(totals.requests) },
    {
      title: t("overview.sessions"),
      value: formatNumber(totals.sessions),
      detail: t("overview.agents", { n: overview.byAgent.length }),
    },
    {
      title: t("overview.cacheHitRate"),
      value: formatCacheHitRate(totals) ?? "—",
      detail: t("overview.cacheRead", {
        n: formatTokens(totals.cacheReadTokens),
      }),
    },
  ];

  return (
    <div className="flex flex-col gap-5">
      <Frame aria-labelledby="overview-cost-trend">
        <FrameHeader>
          <h2 id="overview-cost-trend" className="text-sm font-semibold">
            {t("overview.costTrend")}
          </h2>
          <span className="text-xs tabular-nums text-muted-foreground">
            {t("overview.activeDays", { n: totals.activeDays })}
          </span>
        </FrameHeader>
        <FramePanel className="grid min-[900px]:grid-cols-[minmax(0,1fr)_230px]">
          <div className="min-w-0 px-4 pt-5 pb-3 sm:px-6">
            <div className="text-xs font-medium text-muted-foreground">
              {t(
                params.costMode === "display"
                  ? "overview.reportedCost"
                  : params.costMode === "calculate"
                    ? "overview.apiCostEstimate"
                    : "overview.totalCost",
              )}
            </div>
            <div className="mt-1 text-[clamp(2rem,4vw,2.75rem)] leading-none font-semibold tracking-[-0.045em] tabular-nums">
              {formatCost(totals.cost)}
            </div>
            {params.costMode !== "display" &&
              overview.byAgent.some((row) => row.agent === "codex") && (
                <p className="mt-2 text-xs leading-relaxed text-muted-foreground">
                  {t("overview.codexPricing", {
                    returned: formatNumber(overview.codexPricing.returned),
                    sent: formatNumber(overview.codexPricing.sent),
                    estimated: formatNumber(overview.codexPricing.estimated),
                  })}
                </p>
              )}
            <div className="mt-5">
              {daily.length > 0 ? (
                <CostTrendChart
                  rows={daily}
                  granularity={hourly ? "hour" : "day"}
                  aggregate
                  height={214}
                />
              ) : (
                <EmptyState className="min-h-[214px]" />
              )}
            </div>
          </div>
          <aside
            aria-label={t("overview.usageSummary")}
            className="border-t border-border bg-background/75 px-4 py-4 min-[900px]:border-t-0 min-[900px]:border-l min-[900px]:px-5"
          >
            <h3 className="mb-2 text-xs font-semibold text-muted-foreground">
              {t("overview.usageSummary")}
            </h3>
            <dl className="grid grid-cols-2 gap-x-4 divide-border min-[900px]:grid-cols-1 min-[900px]:divide-y">
              {metrics.map(({ title, value, detail }) => (
                <div key={title} className="min-w-0 py-2.5">
                  <dt className="text-xs text-muted-foreground">{title}</dt>
                  <dd className="mt-0.5 text-xl leading-tight font-semibold tracking-tight tabular-nums">
                    {value}
                  </dd>
                  {detail && (
                    <dd className="mt-0.5 text-[11px] text-muted-foreground">
                      {detail}
                    </dd>
                  )}
                </div>
              ))}
            </dl>
          </aside>
        </FramePanel>
      </Frame>

      <Frame aria-labelledby="overview-cost-distribution">
        <FrameHeader>
          <h2 id="overview-cost-distribution" className="text-sm font-semibold">
            {t("overview.costDistribution")}
          </h2>
        </FrameHeader>
        <FramePanel className="divide-y divide-border">
          {totals.cost > 0 ? (
            <>
              <div className="space-y-2.5 px-4 py-3.5">
                <h3 className="text-xs font-medium text-muted-foreground">
                  {t("overview.costByAgent")}
                </h3>
                <UsageDistribution
                  rows={overview.byAgent.map((row) => ({
                    id: row.agent,
                    label: agentLabel(row.agent),
                    value: row.cost,
                    color: agentColor(row.agent),
                  }))}
                  formatValue={formatCost}
                />
              </div>
              <div className="space-y-2.5 px-4 py-3.5">
                <h3 className="text-xs font-medium text-muted-foreground">
                  {t("overview.costByModel")}
                </h3>
                <UsageDistribution
                  rows={modelDistribution(models, "cost")}
                  formatValue={formatCost}
                />
              </div>
            </>
          ) : (
            <EmptyState />
          )}
        </FramePanel>
      </Frame>

      <RoiCard
        monthByAgent={monthByAgent}
        failed={roiError}
        onRetry={() => setRoiAttempt((value) => value + 1)}
      />

      <Frame aria-labelledby="overview-projects">
        <FrameHeader>
          <h2 id="overview-projects" className="text-sm font-semibold">
            {t("overview.topProjects")}
          </h2>
        </FrameHeader>
        <FramePanel>
          {projects.length === 0 ? (
            <EmptyState />
          ) : (
            <Table className="min-w-[360px]">
              <TableHeader>
                <TableRow>
                  <TableHead>{t("th.project")}</TableHead>
                  <TableHead className="text-right">{t("th.tokens")}</TableHead>
                  <TableHead className="text-right">
                    {t("overview.sessions")}
                  </TableHead>
                  <TableHead className="text-right">{t("th.cost")}</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {projects.map((project) => (
                  <TableRow key={project.project}>
                    <TableCell
                      className="max-w-56 truncate font-medium"
                      title={project.project}
                    >
                      {project.project}
                    </TableCell>
                    <TableCell className="text-right tabular-nums">
                      {formatTokens(project.totalTokens)}
                    </TableCell>
                    <TableCell className="text-right tabular-nums">
                      {formatNumber(project.sessions)}
                    </TableCell>
                    <TableCell className="text-right font-medium tabular-nums">
                      {formatCost(project.cost)}
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
