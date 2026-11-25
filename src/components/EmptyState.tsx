import { ChartNoAxesColumn } from "lucide-react";
import { useI18n } from "@/lib/i18n";
import { cn } from "@/lib/utils";

export function EmptyState({
  message,
  className,
}: {
  message?: string;
  className?: string;
}) {
  const { t } = useI18n();
  return (
    <div
      className={cn(
        "flex min-h-32 flex-col items-center justify-center gap-2 px-4 py-6 text-center text-sm text-muted-foreground",
        className,
      )}
    >
      <ChartNoAxesColumn className="size-5 opacity-70" aria-hidden />
      <p>{message ?? t("common.empty")}</p>
    </div>
  );
}
