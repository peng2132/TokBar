import { useEffect, useMemo, useState } from "react";
import {
  Activity,
  Bot,
  Coins,
  DollarSign,
  FolderGit2,
  MessageSquare,
} from "lucide-react";
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
  formatCost,
  formatNumber,
  formatTokens,
} from "@/lib/format";
import { startOfMonth } from "@/lib/dates";
import { useI18n } from "@/lib/i18n";
import { useSubscriptions } from "@/lib/subscriptions";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { LoadError } from "@/components/LoadError";
import { StatCard } from "@/components/StatCard";
import { RoiCard } from "@/components/RoiCard";
import {
  CostTrendChart,
  DistributionPie,
  modelPie as buildModelPie,
} from "@/components/charts";

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

  // Stable references so DistributionPie's internal memo isn't defeated
  // by a fresh array on every unrelated re-render.
  const modelPie = useMemo(
    () => buildModelPie(current?.models ?? [], "cost", 6),
    [current],
  );
  const agentPie = useMemo(
    () =>
      (current?.overview.byAgent ?? []).map((a) => ({
        key: a.agent,
        name: agentLabel(a.agent),
        value: a.cost,
        color: agentColor(a.agent),
      })),
    [current],
  );

  if (error) {
    return <LoadError onRetry={() => setAttempt((a) => a + 1)} />;
  }
  if (!current) {
    return (
      <div className="grid grid-cols-2 gap-4 lg:grid-cols-4">
        {Array.from({ length: 8 }).map((_, i) => (
          <Skeleton key={i} className="h-28" />
        ))}
      </div>
    );
  }

  const { overview, daily, projects } = current;
  const tot = overview.totals;

  return (
    <div className="space-y-4">
      <RoiCard
        monthByAgent={monthByAgent}
        failed={roiError}
        onRetry={() => setRoiAttempt((a) => a + 1)}
      />

      <div className="grid grid-cols-2 gap-4 lg:grid-cols-4">
        <StatCard
          title={t("overview.totalCost")}
          value={formatCost(tot.cost)}
          sub={t("overview.activeDays", { n: tot.activeDays })}
          icon={DollarSign}
        />
        <StatCard
          title={t("overview.totalTokens")}
          value={formatTokens(tot.totalTokens)}
          sub={t("overview.inOut", {
            in: formatTokens(tot.inputTokens),
            out: formatTokens(tot.outputTokens),
          })}
          icon={Coins}
        />
        <StatCard
          title={t("overview.requests")}
          value={formatNumber(tot.requests)}
          sub={t("overview.cacheRead", {
            n: formatTokens(tot.cacheReadTokens),
          })}
          icon={MessageSquare}
        />
        <StatCard
          title={t("overview.sessions")}
          value={formatNumber(tot.sessions)}
          sub={t("overview.agents", { n: overview.byAgent.length })}
          icon={Activity}
        />
      </div>

      <Card>
        <CardHeader>
          <CardTitle>{t("overview.costTrend")}</CardTitle>
        </CardHeader>
        <CardContent>
          {daily.length > 0 ? (
            <CostTrendChart
              rows={daily}
              granularity={hourly ? "hour" : "day"}
            />
          ) : (
            <EmptyHint />
          )}
        </CardContent>
      </Card>

      <div className="grid gap-4 lg:grid-cols-2">
        <Card>
          <CardHeader className="flex-row items-center justify-between space-y-0">
            <CardTitle>{t("overview.costByModel")}</CardTitle>
            <Bot className="h-4 w-4 text-muted-foreground" />
          </CardHeader>
          <CardContent>
            {modelPie.length > 0 ? (
              <DistributionPie data={modelPie} valueFormatter={formatCost} />
            ) : (
              <EmptyHint />
            )}
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="flex-row items-center justify-between space-y-0">
            <CardTitle>{t("overview.costByAgent")}</CardTitle>
            <Bot className="h-4 w-4 text-muted-foreground" />
          </CardHeader>
          <CardContent>
            {agentPie.length > 0 ? (
              <DistributionPie data={agentPie} valueFormatter={formatCost} />
            ) : (
              <EmptyHint />
            )}
          </CardContent>
        </Card>
      </div>

      <Card>
        <CardHeader className="flex-row items-center justify-between space-y-0">
          <CardTitle>{t("overview.topProjects")}</CardTitle>
          <FolderGit2 className="h-4 w-4 text-muted-foreground" />
        </CardHeader>
        <CardContent className="space-y-3">
          {projects.length === 0 && <EmptyHint />}
          {projects.map((p) => {
            const max = projects[0]?.cost || 1;
            return (
              <div key={p.project} className="space-y-1">
                <div className="flex items-center justify-between text-sm">
                  <span className="truncate font-medium">{p.project}</span>
                  <span className="ml-3 shrink-0 tabular-nums text-muted-foreground">
                    {t("overview.projectStats", {
                      cost: formatCost(p.cost),
                      tokens: formatTokens(p.totalTokens),
                      sessions: p.sessions,
                    })}
                  </span>
                </div>
                <div className="h-1.5 overflow-hidden rounded-full bg-muted">
                  <div
                    className="h-full rounded-full bg-primary"
                    style={{ width: `${Math.max(2, (p.cost / max) * 100)}%` }}
                  />
                </div>
              </div>
            );
          })}
        </CardContent>
      </Card>
    </div>
  );
}

function EmptyHint() {
  const { t } = useI18n();
  return (
    <div className="flex h-40 items-center justify-center text-sm text-muted-foreground">
      {t("common.empty")}
    </div>
  );
}
