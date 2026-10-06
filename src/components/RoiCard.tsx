import { AlertTriangle, TrendingUp } from "lucide-react";
import type { AgentBreakdown } from "@/lib/api";
import { agentLabel, formatCost } from "@/lib/format";
import { useI18n } from "@/lib/i18n";
import { useSubscriptions } from "@/lib/subscriptions";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { BrandIcon, subscriptionBrand } from "@/components/BrandIcon";
import { cn } from "@/lib/utils";

/** "Your flat-rate plans would cost $X at API prices" — the ROI hero card.
 *  `monthByAgent` must be this calendar month's usage priced in `calculate`
 *  mode (API rates), so each agent's `.cost` is its API-priced value; null
 *  while loading. `failed` shows an error instead of a fake $0. */
export function RoiCard({
  monthByAgent,
  failed = false,
  onRetry,
}: {
  monthByAgent: AgentBreakdown[] | null;
  failed?: boolean;
  onRetry?: () => void;
}) {
  const { t, lang } = useI18n();
  const { subscriptions } = useSubscriptions();
  if (subscriptions.length === 0) return null;

  if (failed || !monthByAgent) {
    return (
      <Card>
        <CardHeader className="flex-row items-center justify-between space-y-0">
          <CardTitle className="flex items-center gap-2">
            <TrendingUp className="h-4 w-4 text-primary" />
            {t("roi.title")}
          </CardTitle>
          <span className="text-xs text-muted-foreground">
            {t("roi.subtitle")}
          </span>
        </CardHeader>
        <CardContent>
          {failed ? (
            <div className="flex items-center justify-between gap-3 text-sm text-muted-foreground">
              <span className="flex items-center gap-2">
                <AlertTriangle className="h-4 w-4" />
                {t("common.loadFailed")}
              </span>
              {onRetry && (
                <Button variant="outline" size="sm" onClick={onRetry}>
                  {t("common.retry")}
                </Button>
              )}
            </div>
          ) : (
            <div className="space-y-3">
              <Skeleton className="h-9 w-40" />
              <Skeleton className="h-2" />
            </div>
          )}
        </CardContent>
      </Card>
    );
  }

  const apiByAgent = new Map(monthByAgent.map((a) => [a.agent, a.cost]));
  // When several plans cover the same agent (e.g. two ChatGPT Pro seats
  // sharing one Codex quota), split that agent's API value across them by
  // monthly fee, so the per-plan rows always sum to the de-duplicated total.
  const feeByAgent = new Map<string, number>();
  const countByAgent = new Map<string, number>();
  for (const s of subscriptions) {
    for (const a of s.agents) {
      feeByAgent.set(a, (feeByAgent.get(a) ?? 0) + s.monthlyUsd);
      countByAgent.set(a, (countByAgent.get(a) ?? 0) + 1);
    }
  }
  const rows = subscriptions.map((s) => {
    const apiValue = s.agents.reduce((sum, a) => {
      const fee = feeByAgent.get(a) ?? 0;
      const weight = fee > 0 ? s.monthlyUsd / fee : 1 / (countByAgent.get(a) ?? 1);
      return sum + (apiByAgent.get(a) ?? 0) * weight;
    }, 0);
    return { ...s, apiValue };
  });
  // The total counts each covered agent once, even if several plans list it.
  const coveredAgents = [...new Set(subscriptions.flatMap((s) => s.agents))];
  const totalApi = coveredAgents.reduce(
    (sum, a) => sum + (apiByAgent.get(a) ?? 0),
    0,
  );
  const totalFee = subscriptions.reduce((sum, s) => sum + s.monthlyUsd, 0);
  const saved = totalApi - totalFee;
  const multiple = totalFee > 0 ? totalApi / totalFee : 0;
  const recouped = saved >= 0;
  const progress = totalFee > 0 ? Math.min(100, (totalApi / totalFee) * 100) : 0;

  return (
    <Card>
      <CardHeader className="flex-row items-center justify-between space-y-0">
        <CardTitle className="flex items-center gap-2">
          <TrendingUp className="h-4 w-4 text-primary" />
          {t("roi.title")}
        </CardTitle>
        <span className="text-xs text-muted-foreground">
          {t("roi.subtitle")}
        </span>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="flex flex-wrap items-end justify-between gap-3">
          <div>
            <div className="text-3xl font-bold tabular-nums text-primary">
              {formatCost(totalApi)}
            </div>
            <div className="mt-0.5 text-xs text-muted-foreground">
              {t("roi.apiValue")} · {t("roi.youPay")} {formatCost(totalFee)}
            </div>
          </div>
          <div className="text-right">
            <div
              className={cn(
                "text-lg font-semibold tabular-nums",
                recouped ? "text-primary" : "text-muted-foreground",
              )}
            >
              {recouped
                ? t("roi.saved", { amount: formatCost(saved) })
                : t("roi.notRecouped", { amount: formatCost(-saved) })}
            </div>
            {totalFee > 0 && (
              <div className="text-xs tabular-nums text-muted-foreground">
                {t("roi.multiple", { x: multiple.toFixed(1) })}
              </div>
            )}
          </div>
        </div>

        <div className="h-2 overflow-hidden rounded-full bg-muted">
          <div
            className={cn(
              "h-full rounded-full transition-all",
              recouped ? "bg-primary" : "bg-primary/60",
            )}
            style={{ width: `${Math.max(2, progress)}%` }}
          />
        </div>

        <div className="space-y-2">
          {rows.map((r) => {
            const m = r.monthlyUsd > 0 ? r.apiValue / r.monthlyUsd : 0;
            const brand = subscriptionBrand(r);
            return (
              <div
                key={r.id}
                className="flex items-center justify-between gap-3 text-sm"
              >
                <span className="flex min-w-0 items-center gap-1.5 truncate">
                  {brand && (
                    <BrandIcon
                      brand={brand}
                      className="h-3.5 w-3.5 shrink-0 text-foreground"
                    />
                  )}
                  <span className="truncate font-medium">
                    {r.name || t("roi.untitled")}
                  </span>
                  {r.agents.length > 0 && (
                    <span className="shrink-0 text-xs text-muted-foreground">
                      {r.agents
                        .map((a) => agentLabel(a))
                        .join(lang === "zh" ? "、" : ", ")}
                    </span>
                  )}
                </span>
                <span className="shrink-0 tabular-nums text-muted-foreground">
                  {r.apiValue > 0 ? formatCost(r.apiValue) : t("roi.noUsage")}
                  {" / "}
                  {formatCost(r.monthlyUsd)}
                  <span
                    className={cn(
                      "ml-2 font-medium",
                      m >= 1 ? "text-primary" : "text-muted-foreground",
                    )}
                  >
                    {m > 0 ? `${m.toFixed(1)}×` : "—"}
                  </span>
                </span>
              </div>
            );
          })}
        </div>

        <p className="text-xs text-muted-foreground">{t("roi.hint")}</p>
      </CardContent>
    </Card>
  );
}
