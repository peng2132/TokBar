import { useI18n } from "@/lib/i18n";
import { cn } from "@/lib/utils";
import { Badge } from "@/components/ui/badge";

/** Marks a model with no known price, so its $0 cost isn't read as free. */
export function UnpricedBadge({ className }: { className?: string }) {
  const { t } = useI18n();
  const tip = t("pricing.unpricedTip");
  return (
    <Badge
      variant="warning"
      title={tip}
      aria-label={`${t("pricing.unpriced")}: ${tip}`}
      className={cn("shrink-0 cursor-help px-1.5 py-0 text-[10px]", className)}
    >
      {t("pricing.unpriced")}
    </Badge>
  );
}
