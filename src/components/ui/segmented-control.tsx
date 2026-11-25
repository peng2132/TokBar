import { Radio } from "@base-ui/react/radio";
import { RadioGroup } from "@base-ui/react/radio-group";
import { cn } from "@/lib/utils";

interface Option<Value extends string> {
  id: Value;
  label: string;
  description?: string;
}

// The coss segmented surface with radio semantics for single-choice filters.
export function SegmentedControl<Value extends string>({
  label,
  options,
  value,
  onChange,
  disabled,
  className,
}: {
  label: string;
  options: readonly Option<Value>[];
  value: Value;
  onChange: (value: Value) => void;
  disabled?: boolean;
  className?: string;
}) {
  return (
    <RadioGroup
      aria-label={label}
      value={value}
      onValueChange={onChange}
      disabled={disabled}
      className={cn(
        "inline-flex w-fit max-w-full flex-wrap items-center gap-0.5 rounded-lg bg-muted p-0.5",
        className,
      )}
    >
      {options.map((option) => (
        <Radio.Root
          key={option.id}
          value={option.id}
          title={option.description}
          className="relative inline-flex h-7 cursor-pointer select-none items-center justify-center whitespace-nowrap rounded-md border border-transparent px-2.5 text-xs font-medium text-muted-foreground transition-colors hover:text-foreground data-disabled:cursor-default data-disabled:opacity-60 data-checked:border-border data-checked:bg-card data-checked:text-foreground data-checked:shadow-xs"
        >
          {option.label}
        </Radio.Root>
      ))}
    </RadioGroup>
  );
}
