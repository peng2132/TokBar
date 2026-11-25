import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";

// Adapted from the coss ui Frame (MIT).
export function Frame({ className, ...props }: ComponentProps<"section">) {
  return (
    <section
      data-slot="frame"
      className={cn("min-w-0 rounded-2xl bg-muted/80 p-1", className)}
      {...props}
    />
  );
}

export function FrameHeader({ className, ...props }: ComponentProps<"header">) {
  return (
    <header
      className={cn(
        "flex flex-wrap items-center justify-between gap-2 px-4 py-3",
        className,
      )}
      {...props}
    />
  );
}

export function FramePanel({ className, ...props }: ComponentProps<"div">) {
  return (
    <div
      data-slot="frame-panel"
      className={cn(
        "min-w-0 overflow-hidden rounded-xl border border-border bg-card",
        className,
      )}
      {...props}
    />
  );
}
