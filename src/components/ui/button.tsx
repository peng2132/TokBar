import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

const buttonVariants = cva(
  "inline-flex shrink-0 cursor-pointer items-center justify-center gap-1.5 whitespace-nowrap rounded-lg border border-transparent text-sm font-medium transition-[color,background-color,box-shadow] disabled:pointer-events-none disabled:opacity-60 [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  {
    variants: {
      variant: {
        default:
          "border-primary bg-primary text-primary-foreground shadow-[inset_0_1px_0_rgb(255_255_255/18%)] hover:bg-primary/90 active:shadow-none",
        secondary: "bg-muted text-foreground hover:bg-muted/70",
        ghost: "hover:bg-accent text-muted-foreground hover:text-foreground",
        outline:
          "border-input bg-card text-foreground shadow-[var(--control-shadow)] hover:bg-accent/50 active:shadow-none",
      },
      size: {
        default: "h-9 px-3",
        sm: "h-8 px-3 text-xs",
        icon: "size-8",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  },
);

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {}

const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, size, ...props }, ref) => (
    <button
      type="button"
      data-slot="button"
      className={cn(buttonVariants({ variant, size, className }))}
      ref={ref}
      {...props}
    />
  ),
);
Button.displayName = "Button";

export { Button, buttonVariants };
