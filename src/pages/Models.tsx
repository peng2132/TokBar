import { useEffect, useMemo, useState } from "react";
import { api, type ModelRow, type QueryParams } from "@/lib/api";
import { formatCost, formatNumber, formatTokens } from "@/lib/format";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Skeleton } from "@/components/ui/skeleton";
import { DistributionPie, modelPie } from "@/components/charts";
import { LoadError } from "@/components/LoadError";
import { UnpricedBadge } from "@/components/UnpricedBadge";
import { useI18n } from "@/lib/i18n";

export function ModelsPage({
  params,
  refreshKey,
}: {
  params: QueryParams;
  refreshKey: number;
}) {
  const { t } = useI18n();
  const [models, setModels] = useState<ModelRow[] | null>(null);
  const [error, setError] = useState(false);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let cancelled = false;
    setError(false);
    api
      .getModels(params)
      .then((m) => !cancelled && setModels(m))
      .catch(() => !cancelled && setError(true));
    return () => {
      cancelled = true;
    };
  }, [params.sinceMs, params.untilMs, params.costMode, refreshKey, attempt]);

  // Both pies take the same top-8 (cost order) so a model keeps one
  // color across them; slices are keyed by full model id.
  const costPie = useMemo(() => modelPie(models ?? [], "cost", 8), [models]);
  const tokenPie = useMemo(
    () => modelPie(models ?? [], "totalTokens", 8),
    [models],
  );

  if (error) {
    return <LoadError onRetry={() => setAttempt((a) => a + 1)} />;
  }
  if (!models) {
    return <Skeleton className="h-80" />;
  }

  return (
    <div className="space-y-4">
      <div className="grid gap-4 lg:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle>{t("models.costDist")}</CardTitle>
          </CardHeader>
          <CardContent>
            <DistributionPie data={costPie} valueFormatter={formatCost} />
          </CardContent>
        </Card>
        <Card>
          <CardHeader>
            <CardTitle>{t("models.tokenDist")}</CardTitle>
          </CardHeader>
          <CardContent>
            <DistributionPie data={tokenPie} valueFormatter={formatTokens} />
          </CardContent>
        </Card>
      </div>
      <Card>
        <CardHeader>
          <CardTitle>{t("models.all")}</CardTitle>
        </CardHeader>
        <CardContent className="p-0">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{t("th.model")}</TableHead>
                <TableHead className="text-right">{t("th.input")}</TableHead>
                <TableHead className="text-right">{t("th.output")}</TableHead>
                <TableHead className="text-right">{t("th.cacheWrite")}</TableHead>
                <TableHead className="text-right">{t("th.cacheRead")}</TableHead>
                <TableHead className="text-right">{t("th.requests")}</TableHead>
                <TableHead className="text-right">{t("th.cost")}</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {models.map((m) => (
                <TableRow key={m.model}>
                  <TableCell className="font-medium">
                    <span className="flex max-w-80 items-center gap-1.5">
                      <span className="truncate" title={m.model}>
                        {m.model}
                      </span>
                      {m.priced === false && <UnpricedBadge />}
                    </span>
                  </TableCell>
                  <TableCell className="text-right tabular-nums">
                    {formatTokens(m.inputTokens)}
                  </TableCell>
                  <TableCell className="text-right tabular-nums">
                    {formatTokens(m.outputTokens)}
                  </TableCell>
                  <TableCell className="text-right tabular-nums">
                    {formatTokens(m.cacheCreationTokens)}
                  </TableCell>
                  <TableCell className="text-right tabular-nums">
                    {formatTokens(m.cacheReadTokens)}
                  </TableCell>
                  <TableCell className="text-right tabular-nums">
                    {formatNumber(m.requests)}
                  </TableCell>
                  <TableCell className="text-right font-medium tabular-nums">
                    {formatCost(m.cost)}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </CardContent>
      </Card>
    </div>
  );
}
