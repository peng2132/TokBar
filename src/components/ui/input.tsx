import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";

export function Input({ className, ...props }: ComponentProps<"input">) {
  return (
    <input
      data-slot="input"
      className={cn(
        "h-8 min-w-0 rounded-lg border border-input bg-card px-2.5 text-sm text-foreground shadow-[var(--control-shadow)] placeholder:text-muted-foreground focus-visible:outline-offset-0",
        className,
      )}
      {...props}
    />
  );
}
