import { useEffect, useMemo, useState } from "react";
import { Flame, Hourglass } from "lucide-react";
import { api, type Block, type CostMode } from "@/lib/api";
import {
  BLOCK_LOOKBACK_DAYS,
  blockAgents,
  blockLookbackSinceMs,
  defaultBlockAgent,
} from "@/lib/blocks";
import {
  agentLabel,
  formatCost,
  formatDateTime,
  formatNumber,
  formatTime,
  formatTokens,
  remainingLabel,
  shortModelName,
} from "@/lib/format";
import { cn } from "@/lib/utils";
import { Card, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import { LoadError } from "@/components/LoadError";
import { useI18n } from "@/lib/i18n";

export function BlocksPage({
  costMode,
  refreshKey,
}: {
  costMode: CostMode;
  refreshKey: number;
}) {
  const { t } = useI18n();
  const [blocks, setBlocks] = useState<Block[] | null>(null);
  const [error, setError] = useState(false);
  const [attempt, setAttempt] = useState(0);
  // The user's pick; falls back to the default while it has no blocks.
  const [picked, setPicked] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setError(false);
    // Every agent's blocks over the shared lookback (the quick panel and
    // notch bar use the same window, so they agree on the active block).
    api
      .getBlocks({ sinceMs: blockLookbackSinceMs(), costMode })
      .then((b) => !cancelled && setBlocks(b))
      .catch(() => !cancelled && setError(true));
    return () => {
      cancelled = true;
    };
  }, [costMode, refreshKey, attempt]);

  const agents = useMemo(() => blockAgents(blocks ?? []), [blocks]);
  const agent =
    picked && agents.includes(picked) ? picked : defaultBlockAgent(agents);

  if (error) {
    return <LoadError onRetry={() => setAttempt((a) => a + 1)} />;
  }
  if (!blocks) {
    return <Skeleton className="h-80" />;
  }

  const usage = blocks.filter((b) => !b.isGap && b.agent === agent);

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="max-w-2xl text-xs text-muted-foreground">
          {t("blocks.desc", { days: BLOCK_LOOKBACK_DAYS })}
        </p>
        {/* Shown even for a single agent: it says whose blocks these are. */}
        {agents.length > 0 && (
          <div
            role="group"
            aria-label={t("th.agent")}
            className="flex rounded-lg border border-border p-0.5"
          >
            {agents.map((a) => (
              <button
                key={a}
                onClick={() => setPicked(a)}
                aria-pressed={agent === a}
                className={cn(
                  "rounded-md px-3 py-1 text-xs font-medium transition-colors",
                  agent === a
                    ? "bg-primary/10 text-primary"
                    : "text-muted-foreground hover:text-foreground",
                )}
              >
                {agentLabel(a)}
              </button>
            ))}
          </div>
        )}
      </div>
      {usage.length === 0 && (
        <Card>
          <CardContent className="flex h-32 items-center justify-center text-sm text-muted-foreground">
            {t("blocks.empty", { days: BLOCK_LOOKBACK_DAYS })}
          </CardContent>
        </Card>
      )}
      {usage.map((b) => (
        <Card
          key={b.id}
          className={b.isActive ? "border-primary/50" : undefined}
        >
          <CardContent className="flex flex-wrap items-center gap-x-6 gap-y-2 p-4">
            <div className="flex min-w-44 items-center gap-2">
              {b.isActive ? (
                <Flame className="h-4 w-4 text-primary" />
              ) : (
                <Hourglass className="h-4 w-4 text-muted-foreground" />
              )}
              <div>
                <div className="text-sm font-medium">
                  {formatDateTime(b.startMs)} – {formatTime(b.endMs)}
                </div>
                <div className="text-xs text-muted-foreground">
                  {/* The live badge already says "active"; the subtitle
                      carries the useful fact: time left in the window. */}
                  {b.isActive ? remainingLabel(b.endMs, t) : t("blocks.completed")}
                </div>
              </div>
            </div>
            {b.isActive && <Badge variant="success">{t("blocks.live")}</Badge>}
            <Metric label={t("blocks.cost")} value={formatCost(b.cost)} />
            <Metric label={t("blocks.tokens")} value={formatTokens(b.totalTokens)} />
            <Metric label={t("blocks.requests")} value={formatNumber(b.requests)} />
            {b.burnRateTpm != null && (
              <Metric
                label={t("blocks.burnRate")}
                value={`${formatTokens(Math.round(b.burnRateTpm))}${t("unit.perMin")}`}
              />
            )}
            {b.burnRateCostPerHour != null && (
              <Metric
                label={t("blocks.costRate")}
                value={`${formatCost(b.burnRateCostPerHour)}${t("unit.perHour")}`}
              />
            )}
            <div className="ml-auto flex flex-wrap gap-1">
              {b.models.slice(0, 4).map((m) => (
                <Badge key={m} variant="outline">
                  {shortModelName(m)}
                </Badge>
              ))}
            </div>
          </CardContent>
        </Card>
      ))}
    </div>
  );
}

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <div className="text-xs text-muted-foreground">{label}</div>
      <div className="text-sm font-medium tabular-nums">{value}</div>
    </div>
  );
}
