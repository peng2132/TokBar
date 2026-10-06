import { useCallback, useEffect, useRef, useState } from "react";
import { api, isCostMode, onEvent, type CostMode } from "./api";

/** Where the main window used to persist the cost mode before the
 *  backend became its single source of truth. */
const LEGACY_KEY = "tokbar-cost-mode";

/** One-time migration: push a locally saved cost mode to the backend,
 *  then drop the key. On failure the key stays for the next launch. */
async function migrateLegacyCostMode(): Promise<void> {
  let legacy: string | null = null;
  try {
    legacy = localStorage.getItem(LEGACY_KEY);
  } catch {
    return;
  }
  if (legacy === null) return;
  try {
    if (isCostMode(legacy)) await api.setCostMode(legacy);
    localStorage.removeItem(LEGACY_KEY);
  } catch (e) {
    console.error("cost mode migration failed:", e);
  }
}

/**
 * The backend's cost mode, kept live via `cost-mode-changed` (emitted to
 * every window). `mode` is null until the first read resolves; if the
 * backend can't be reached it falls back to "auto" so data still loads.
 * `setMode` is optimistic and rolls back (rejecting) if the backend
 * refuses the change.
 */
export function useCostMode({ migrate = false }: { migrate?: boolean } = {}) {
  const [mode, setModeState] = useState<CostMode | null>(null);
  const modeRef = useRef<CostMode | null>(null);
  modeRef.current = mode;
  const changeSeq = useRef(0);

  useEffect(() => {
    let cancelled = false;
    // An event that lands before the initial read is newer than it.
    let heardEvent = false;
    const unlisten = onEvent<string>("cost-mode-changed", (e) => {
      if (!isCostMode(e.payload)) return;
      heardEvent = true;
      setModeState(e.payload);
    });
    (migrate ? migrateLegacyCostMode() : Promise.resolve())
      .then(() => api.getCostMode())
      .then((m) => {
        if (!cancelled && !heardEvent) setModeState(isCostMode(m) ? m : "auto");
      })
      .catch((e) => {
        console.error("get_cost_mode failed:", e);
        if (!cancelled) setModeState((prev) => prev ?? "auto");
      });
    return () => {
      cancelled = true;
      unlisten.then((fn) => fn());
    };
  }, [migrate]);

  const setMode = useCallback(async (next: CostMode) => {
    const prev = modeRef.current;
    const seq = ++changeSeq.current;
    setModeState(next);
    try {
      await api.setCostMode(next);
    } catch (e) {
      // Only roll back if no newer change has superseded this one.
      if (seq === changeSeq.current) setModeState(prev);
      throw e;
    }
  }, []);

  return { mode, setMode };
}
