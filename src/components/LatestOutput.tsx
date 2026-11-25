import { Activity, ChevronRight } from "lucide-react";
import type { LatestPerformance } from "@/lib/api";
import {
  formatDateTime,
  formatOutputSpeed,
  performanceSourceLabel,
} from "@/lib/format";
import { useI18n } from "@/lib/i18n";
import { cn } from "@/lib/utils";

export function LatestOutput({
  sample,
  loading,
  onClick,
  className,
}: {
  sample?: LatestPerformance | null;
  loading?: boolean;
  onClick: () => void;
  className?: string;
}) {
  const { t } = useI18n();
  const source = sample
    ? `${sample.model} · ${performanceSourceLabel(sample)}`
    : null;

  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "group block w-full rounded-lg border border-border bg-background/75 px-3 py-2.5 text-left transition-colors hover:bg-accent/70",
        className,
      )}
    >
      <div className="flex items-center justify-between gap-2">
        <span className="flex min-w-0 items-center gap-1.5 text-xs font-medium text-muted-foreground">
          <Activity aria-hidden className="size-3.5 shrink-0 text-primary" />
          {t("quick.latestSpeed")}
        </span>
        <span className="flex shrink-0 items-center gap-1 text-sm font-semibold tabular-nums">
          {sample ? formatOutputSpeed(sample.outputTps) : "—"}
          <ChevronRight
            aria-hidden
            className="size-3.5 text-muted-foreground"
          />
        </span>
      </div>
      {sample ? (
        <div className="mt-1 flex justify-between gap-2 text-[11px] text-muted-foreground">
          <span className="min-w-0 truncate" title={source ?? undefined}>
            {source}
          </span>
          <span className="shrink-0 tabular-nums">
            {formatDateTime(sample.timestampMs)}
          </span>
        </div>
      ) : (
        !loading && (
          <p className="mt-1 text-[11px] text-muted-foreground">
            {t("quick.noSpeed")}
          </p>
        )
      )}
    </button>
  );
}
