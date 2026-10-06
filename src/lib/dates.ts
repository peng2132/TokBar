import { useEffect, useState } from "react";

// Local-calendar date bounds. Everything here uses calendar arithmetic
// (setDate / setHours) rather than multiples of 86_400_000, so the bounds
// stay on local midnight across DST changes (days of 23 or 25 hours).

/** Local midnight `daysAgo` calendar days before today (0 = today). */
export function startOfDay(daysAgo = 0, now = new Date()): number {
  const d = new Date(now);
  d.setHours(0, 0, 0, 0);
  d.setDate(d.getDate() - daysAgo);
  return d.getTime();
}

export function startOfToday(now = new Date()): number {
  return startOfDay(0, now);
}

/** Start of the current calendar month (local time). */
export function startOfMonth(now = new Date()): number {
  const d = new Date(now);
  d.setHours(0, 0, 0, 0);
  d.setDate(1);
  return d.getTime();
}

/** "YYYY-MM-DD" of the local calendar day. */
export function localDayKey(now = new Date()): string {
  const mm = String(now.getMonth() + 1).padStart(2, "0");
  const dd = String(now.getDate()).padStart(2, "0");
  return `${now.getFullYear()}-${mm}-${dd}`;
}

/**
 * The current local day as a key that changes at midnight. Re-checked on
 * a timer aimed just past the next midnight, and whenever the window is
 * shown or focused (timers drift or pause while the Mac sleeps), so
 * anything derived from "today" can recompute its bounds.
 */
export function useDayKey(): string {
  const [key, setKey] = useState(() => localDayKey());
  useEffect(() => {
    let timer: number | undefined;
    // Same string → React bails out, so redundant checks are free.
    const check = () => setKey(localDayKey());
    const schedule = () => {
      const next = new Date();
      next.setHours(24, 0, 1, 0); // 00:00:01 tomorrow (overflow rolls the date)
      timer = window.setTimeout(() => {
        check();
        schedule();
      }, next.getTime() - Date.now());
    };
    const onShow = () => {
      if (!document.hidden) check();
    };
    schedule();
    document.addEventListener("visibilitychange", onShow);
    window.addEventListener("focus", check);
    return () => {
      window.clearTimeout(timer);
      document.removeEventListener("visibilitychange", onShow);
      window.removeEventListener("focus", check);
    };
  }, []);
  return key;
}
