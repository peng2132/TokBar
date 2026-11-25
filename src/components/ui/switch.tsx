import { Switch as SwitchPrimitive } from "@base-ui/react/switch";

// Adapted from the coss ui Switch (MIT).
export function Switch(
  props: Omit<SwitchPrimitive.Root.Props, "className" | "children">,
) {
  return (
    <SwitchPrimitive.Root
      data-slot="switch"
      className="inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full bg-input p-0.5 transition-colors data-checked:bg-primary data-disabled:cursor-default data-disabled:opacity-60"
      {...props}
    >
      <SwitchPrimitive.Thumb className="block size-4 rounded-full bg-white shadow-xs transition-transform data-checked:translate-x-4 motion-reduce:transition-none" />
    </SwitchPrimitive.Root>
  );
}
