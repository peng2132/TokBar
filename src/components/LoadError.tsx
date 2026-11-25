import { AlertTriangle } from "lucide-react";
import { useI18n } from "@/lib/i18n";
import { Button } from "@/components/ui/button";

/** Full-area error state with a retry button — shown when a page's
 *  backend query rejects instead of leaving the skeleton up forever. */
export function LoadError({ onRetry }: { onRetry: () => void }) {
  const { t } = useI18n();
  return (
    <div
      role="alert"
      className="flex flex-col items-center justify-center gap-3 rounded-xl border border-border bg-card py-12 text-muted-foreground"
    >
      <AlertTriangle className="size-5 text-danger" aria-hidden />
      <span className="text-sm">{t("common.loadFailed")}</span>
      <Button variant="outline" size="sm" onClick={onRetry}>
        {t("common.retry")}
      </Button>
    </div>
  );
}
