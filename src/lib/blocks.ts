import type { Block } from "./api";

/** One lookback for every billing-block query (Blocks page, quick panel,
 *  notch bar), so all surfaces agree on which block is active. */
export const BLOCK_LOOKBACK_DAYS = 14;

/** Rolling lookback start; computed per call so every fetch is fresh. */
export function blockLookbackSinceMs(now = Date.now()): number {
  return now - BLOCK_LOOKBACK_DAYS * 86_400_000;
}

/** Time of the block's last recorded activity. */
export function lastActivityMs(b: Block): number {
  return b.actualEndMs ?? b.startMs;
}

/** The active block of the most recently active agent, or null. Blocks
 *  are per agent, so several agents can each have a live block. */
export function pickActiveBlock(blocks: Block[]): Block | null {
  let best: Block | null = null;
  for (const b of blocks) {
    if (!b.isActive || b.isGap) continue;
    if (!best || lastActivityMs(b) > lastActivityMs(best)) best = b;
  }
  return best;
}

/** Agents with usage blocks, most recently active first. */
export function blockAgents(blocks: Block[]): string[] {
  const latest = new Map<string, number>();
  for (const b of blocks) {
    if (b.isGap) continue;
    const t = lastActivityMs(b);
    if (t > (latest.get(b.agent) ?? -Infinity)) latest.set(b.agent, t);
  }
  return [...latest.entries()].sort((a, b) => b[1] - a[1]).map(([a]) => a);
}

/** Blocks page default: Claude Code (whose billing the 5-hour window
 *  models) when it has blocks, else the most recently active agent. */
export function defaultBlockAgent(agents: string[]): string | null {
  if (agents.includes("claude-code")) return "claude-code";
  return agents[0] ?? null;
}
