import { Menu } from "@base-ui/react/menu";
import { Check, Plus, X } from "lucide-react";
import { AGENT_LABELS, agentLabel } from "@/lib/format";
import { useI18n } from "@/lib/i18n";
import { cn } from "@/lib/utils";
import { BrandIcon, agentBrand } from "@/components/BrandIcon";

/** Agents a subscription covers, shown as removable chips with a "+ Add"
 *  popover (checkbox list, closes on outside click). */
export function AgentMultiSelect({
  value,
  onChange,
}: {
  value: string[];
  onChange: (agents: string[]) => void;
}) {
  const { t } = useI18n();
  const toggle = (key: string) =>
    onChange(
      value.includes(key) ? value.filter((k) => k !== key) : [...value, key],
    );

  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {value.map((k) => {
        const brand = agentBrand(k);
        return (
          <span
            key={k}
            className="inline-flex items-center gap-1 rounded-full bg-primary/10 py-0.5 pl-2 pr-1 text-xs font-medium text-primary"
          >
            {brand && <BrandIcon brand={brand} className="h-3 w-3" />}
            {agentLabel(k)}
            <button
              type="button"
              onClick={() => toggle(k)}
              title={t("settings.sub.remove")}
              className="rounded-full p-0.5 opacity-60 transition-opacity hover:bg-primary/20 hover:opacity-100"
            >
              <X className="h-3 w-3" />
            </button>
          </span>
        );
      })}
      <Menu.Root modal={false}>
        <Menu.Trigger
          className="inline-flex items-center gap-1 rounded-full border border-dashed border-border px-2 py-0.5 text-xs text-muted-foreground transition-colors hover:border-primary/50 hover:text-foreground"
        >
          <Plus className="h-3 w-3" aria-hidden />
          {value.length === 0
            ? t("settings.sub.pickAgents")
            : t("settings.sub.addAgent")}
        </Menu.Trigger>
        <Menu.Portal>
          <Menu.Positioner align="start" sideOffset={4} className="z-20">
            <Menu.Popup className="max-h-[min(15rem,var(--available-height))] w-52 overflow-auto rounded-xl border border-border bg-card p-1 shadow-lg">
              {Object.entries(AGENT_LABELS).map(([key, label]) => {
                const checked = value.includes(key);
                const brand = agentBrand(key);
                return (
                  <Menu.CheckboxItem
                    key={key}
                    checked={checked}
                    closeOnClick={false}
                    onCheckedChange={() => toggle(key)}
                    className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm transition-colors data-highlighted:bg-accent focus-visible:-outline-offset-2"
                  >
                    <span
                      aria-hidden
                      className={cn(
                        "flex h-4 w-4 shrink-0 items-center justify-center rounded border",
                        checked
                          ? "border-primary bg-primary text-primary-foreground"
                          : "border-border",
                      )}
                    >
                      <Menu.CheckboxItemIndicator>
                        <Check className="h-3 w-3" />
                      </Menu.CheckboxItemIndicator>
                    </span>
                    {brand ? (
                      <BrandIcon brand={brand} className="h-4 w-4 shrink-0" />
                    ) : (
                      <span className="h-4 w-4 shrink-0" />
                    )}
                    {label}
                  </Menu.CheckboxItem>
                );
              })}
            </Menu.Popup>
          </Menu.Positioner>
        </Menu.Portal>
      </Menu.Root>
    </div>
  );
}
