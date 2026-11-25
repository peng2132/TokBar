import { useEffect, useMemo, useState } from "react";
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
import { Card } from "@/components/ui/card";
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

  const usage = blocks.filter((block) => !block.isGap && block.agent === agent);
  const active = usage.filter((block) => block.isActive);
  const history = usage.filter((block) => !block.isActive);

  return (
    <div className="flex flex-col gap-5">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="max-w-xl text-sm text-muted-foreground">
          {t("blocks.desc", { days: BLOCK_LOOKBACK_DAYS })}
        </p>
        {agent && (
          <SegmentedControl
            label={t("th.agent")}
            options={agents.map((id) => ({ id, label: agentLabel(id) }))}
            value={agent}
            onChange={setPicked}
          />
        )}
      </div>

      {usage.length === 0 && (
        <Card>
          <EmptyState
            message={t("blocks.empty", { days: BLOCK_LOOKBACK_DAYS })}
          />
        </Card>
      )}
      {active.map((block) => (
        <Frame key={block.id}>
          <FrameHeader>
            <h2 className="text-sm font-semibold">{t("quick.activeBlock")}</h2>
            <Badge variant="success" className="gap-1.5">
              <span className="size-1.5 rounded-full bg-current" aria-hidden />
              {t("blocks.live")}
            </Badge>
          </FrameHeader>
          <FramePanel className="grid min-[940px]:grid-cols-[minmax(0,1fr)_minmax(0,2fr)]">
            <div className="flex flex-col gap-3 bg-background/75 p-5">
              <div>
                <div className="text-sm font-semibold">
                  {formatDateTime(block.startMs)} – {formatTime(block.endMs)}
                </div>
                <p className="mt-1 text-xs text-muted-foreground">
                  {remainingLabel(block.endMs, t)}
                </p>
              </div>
              <div className="flex flex-wrap gap-1">
                <ModelTags models={block.models} />
              </div>
            </div>
            <dl className="grid grid-cols-2 items-start gap-4 border-t border-border p-5 sm:grid-cols-3 min-[940px]:border-t-0 min-[940px]:border-l">
              <Metric label={t("blocks.cost")} value={formatCost(block.cost)} />
              <Metric
                label={t("blocks.tokens")}
                value={formatTokens(block.totalTokens)}
              />
              <Metric
                label={t("blocks.requests")}
                value={formatNumber(block.requests)}
              />
              {block.burnRateTpm != null && (
                <Metric
                  label={t("blocks.burnRate")}
                  value={`${formatTokens(Math.round(block.burnRateTpm))}${t("unit.perMin")}`}
                />
              )}
              {block.burnRateCostPerHour != null && (
                <Metric
                  label={t("blocks.costRate")}
                  value={`${formatCost(block.burnRateCostPerHour)}${t("unit.perHour")}`}
                />
              )}
            </dl>
          </FramePanel>
        </Frame>
      ))}
      {history.length > 0 && (
        <Frame aria-labelledby="billing-history">
          <FrameHeader>
            <h2 id="billing-history" className="text-sm font-semibold">
              {t("blocks.history")}
            </h2>
          </FrameHeader>
          <FramePanel>
            <Table className="min-w-[560px]">
              <TableHeader>
                <TableRow>
                  <TableHead>{t("th.time")}</TableHead>
                  <TableHead>{t("th.models")}</TableHead>
                  <TableHead className="text-right">
                    {t("blocks.tokens")}
                  </TableHead>
                  <TableHead className="text-right">
                    {t("blocks.requests")}
                  </TableHead>
                  <TableHead className="text-right">
                    {t("blocks.cost")}
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {history.map((block) => (
                  <TableRow key={block.id}>
                    <TableCell className="font-medium whitespace-nowrap">
                      {formatDateTime(block.startMs)} –{" "}
                      {formatTime(block.endMs)}
                    </TableCell>
                    <TableCell>
                      <div className="flex flex-wrap gap-1">
                        <ModelTags models={block.models} />
                      </div>
                    </TableCell>
                    <TableCell className="text-right tabular-nums">
                      {formatTokens(block.totalTokens)}
                    </TableCell>
                    <TableCell className="text-right tabular-nums">
                      {formatNumber(block.requests)}
                    </TableCell>
                    <TableCell className="text-right font-semibold tabular-nums">
                      {formatCost(block.cost)}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </FramePanel>
        </Frame>
      )}
    </div>
  );
}

function ModelTags({ models }: { models: string[] }) {
  return models.slice(0, 4).map((model) => (
    <Badge key={model} variant="outline" title={model}>
      {shortModelName(model)}
    </Badge>
  ));
}

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className="mt-1 text-base font-semibold tabular-nums">{value}</dd>
    </div>
  );
}
