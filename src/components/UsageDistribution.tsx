import type { ModelRow } from "@/lib/api";
import { CHART_PALETTE, modelDisplayNames } from "@/lib/format";
import { useI18n } from "@/lib/i18n";

interface DistributionRow {
  id: string;
  label: string;
  value: number;
  color: string;
}

export function modelDistribution(
  models: ModelRow[],
  metric: "cost" | "totalTokens",
): DistributionRow[] {
  const labels = modelDisplayNames(models.map((model) => model.model));
  return models.map((model, index) => ({
    id: model.model,
    label: labels.get(model.model) ?? model.model,
    value: model[metric],
    color: CHART_PALETTE[index % CHART_PALETTE.length],
  }));
}

export function UsageDistribution({
  rows,
  formatValue,
}: {
  rows: DistributionRow[];
  formatValue: (value: number) => string;
}) {
  const { t } = useI18n();
  const ranked = rows
    .filter((row) => row.value > 0)
    .sort((a, b) => b.value - a.value);
  if (ranked.length === 0)
    return <p className="text-xs text-muted-foreground">{t("common.empty")}</p>;

  const total = ranked.reduce((sum, row) => sum + row.value, 0);
  const shown = ranked.slice(0, 5);
  if (ranked.length > shown.length) {
    shown.push({
      id: "",
      label: t("distribution.other"),
      value: ranked.slice(shown.length).reduce((sum, row) => sum + row.value, 0),
      color: "var(--muted-foreground)",
    });
  }
  const segments = shown.map((row) => ({
    ...row,
    share: (row.value / total) * 100,
  }));

  return (
    <div className="@container space-y-3">
      <div aria-hidden className="flex h-2 overflow-hidden rounded-full bg-muted">
        {segments.map((row) => (
          <span
            key={row.id}
            className="shrink-0"
            style={{ width: `${row.share}%`, backgroundColor: row.color }}
          />
        ))}
      </div>
      <ul className="flex flex-wrap gap-x-4 gap-y-3">
        {segments.map((row) => (
          <li
            key={row.id}
            className="flex min-w-0 flex-[1_1_8rem] flex-col gap-1 text-xs @min-[520px]:flex-[1_1_11rem] @min-[850px]:flex-[1_1_8rem]"
          >
            <div className="flex min-w-0 items-start gap-1.5">
              <span
                aria-hidden
                className="mt-1 size-2 shrink-0 rounded-full"
                style={{ backgroundColor: row.color }}
              />
              <span
                className="line-clamp-2 min-w-0 font-medium [overflow-wrap:anywhere]"
                title={row.id || row.label}
              >
                {row.label}
              </span>
            </div>
            <div className="mt-auto flex flex-wrap items-baseline gap-x-2 pl-3.5 tabular-nums">
              <span>{formatValue(row.value)}</span>
              <span className="text-muted-foreground">
                {row.share < 1 ? "<1%" : `${Math.round(row.share)}%`}
              </span>
            </div>
          </li>
        ))}
      </ul>
    </div>
  );
}
