import { useEffect, useMemo, useState } from "react";
import { AlertTriangle, ChevronRight, Search } from "lucide-react";
import {
  api,
  type ModelRow,
  type QueryParams,
  type SessionRow,
} from "@/lib/api";
import {
  agentLabel,
  formatCost,
  formatDateTime,
  formatNumber,
  formatTokens,
  shortModelName,
} from "@/lib/format";
import { useI18n } from "@/lib/i18n";
import { cn } from "@/lib/utils";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Skeleton } from "@/components/ui/skeleton";
import { Button } from "@/components/ui/button";
import { LoadError } from "@/components/LoadError";
import { UnpricedBadge } from "@/components/UnpricedBadge";

/** Per-session model rows, or "error" when that query failed. */
type Detail = ModelRow[] | "error";

const sessionKey = (s: SessionRow) => `${s.agent}:${s.sessionId}`;

const AGENT_BADGE: Record<string, "warning" | "success" | "info"> = {
  "claude-code": "warning",
  codex: "success",
  kimi: "info",
};

export function SessionsPage({
  params,
  refreshKey,
}: {
  params: QueryParams;
  refreshKey: number;
}) {
  const { t } = useI18n();
  const [sessions, setSessions] = useState<SessionRow[] | null>(null);
  const [error, setError] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [search, setSearch] = useState("");
  const [agentFilter, setAgentFilter] = useState<string>("all");
  const [expanded, setExpanded] = useState<string | null>(null);
  const [detail, setDetail] = useState<Record<string, Detail>>({});

  // Cached per-session model rows go stale when the data changes or the
  // cost mode re-prices them; drop them (the open row refetches below).
  useEffect(() => {
    setDetail({});
  }, [refreshKey, params.costMode]);

  // Load the expanded session's model rows unless cached.
  useEffect(() => {
    if (!expanded || detail[expanded] !== undefined) return;
    // Agent ids never contain ":", so the first one splits the key.
    const sep = expanded.indexOf(":");
    const agent = expanded.slice(0, sep);
    const sessionId = expanded.slice(sep + 1);
    let cancelled = false;
    api
      .getSessionModels(agent, sessionId, params.costMode)
      .then((rows) => {
        if (!cancelled) setDetail((d) => ({ ...d, [expanded]: rows }));
      })
      .catch((e) => {
        console.error("session detail failed:", e);
        if (!cancelled) setDetail((d) => ({ ...d, [expanded]: "error" }));
      });
    return () => {
      cancelled = true;
    };
  }, [expanded, detail, params.costMode]);

  const retryDetail = (key: string) =>
    setDetail((d) => {
      const next = { ...d };
      delete next[key];
      return next;
    });

  useEffect(() => {
    let cancelled = false;
    setError(false);
    api
      .getSessions({ ...params, limit: 300 })
      .then((s) => !cancelled && setSessions(s))
      .catch(() => !cancelled && setError(true));
    return () => {
      cancelled = true;
    };
  }, [params.sinceMs, params.untilMs, params.costMode, refreshKey, attempt]);

  const agents = useMemo(
    () => [...new Set((sessions ?? []).map((s) => s.agent))].sort(),
    [sessions],
  );

  const filtered = useMemo(() => {
    if (!sessions) return [];
    const q = search.trim().toLowerCase();
    return sessions.filter((s) => {
      if (agentFilter !== "all" && s.agent !== agentFilter) return false;
      if (!q) return true;
      return (
        s.project.toLowerCase().includes(q) ||
        s.sessionId.toLowerCase().includes(q) ||
        s.models.toLowerCase().includes(q)
      );
    });
  }, [sessions, search, agentFilter]);

  const toggleExpand = (s: SessionRow) => {
    const key = sessionKey(s);
    setExpanded((cur) => (cur === key ? null : key));
  };

  if (error) {
    return <LoadError onRetry={() => setAttempt((a) => a + 1)} />;
  }
  if (!sessions) {
    return <Skeleton className="h-80" />;
  }

  return (
    <Card>
      <CardHeader className="gap-3">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <CardTitle>
            {t("sessions.title")}{" "}
            <span className="text-xs font-normal">({filtered.length})</span>
          </CardTitle>
          <div className="flex flex-wrap items-center gap-2">
            {/* Agent filter */}
            <div className="flex rounded-lg border border-border p-0.5">
              <button
                onClick={() => setAgentFilter("all")}
                className={cn(
                  "rounded-md px-3 py-1 text-xs font-medium transition-colors",
                  agentFilter === "all"
                    ? "bg-primary/10 text-primary"
                    : "text-muted-foreground hover:text-foreground",
                )}
              >
                {t("sessions.allAgents")}
              </button>
              {agents.map((a) => (
                <button
                  key={a}
                  onClick={() => setAgentFilter(a)}
                  className={cn(
                    "rounded-md px-3 py-1 text-xs font-medium transition-colors",
                    agentFilter === a
                      ? "bg-primary/10 text-primary"
                      : "text-muted-foreground hover:text-foreground",
                  )}
                >
                  {agentLabel(a)}
                </button>
              ))}
            </div>
            {/* Search */}
            <div className="relative">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
              <input
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder={t("sessions.search")}
                className="h-8 w-56 rounded-lg border border-border bg-transparent pl-8 pr-3 text-xs outline-none transition-colors focus:border-primary"
              />
            </div>
          </div>
        </div>
      </CardHeader>
      <CardContent className="p-0">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead className="w-8" />
              <TableHead>{t("th.project")}</TableHead>
              <TableHead>{t("th.agent")}</TableHead>
              <TableHead>{t("th.models")}</TableHead>
              <TableHead>{t("th.lastActivity")}</TableHead>
              <TableHead className="text-right">{t("th.requests")}</TableHead>
              <TableHead className="text-right">{t("th.tokens")}</TableHead>
              <TableHead className="text-right">{t("th.cost")}</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {filtered.map((s) => {
              const key = sessionKey(s);
              const isOpen = expanded === key;
              return (
                <SessionRowGroup
                  key={key}
                  session={s}
                  isOpen={isOpen}
                  detail={detail[key]}
                  onToggle={() => toggleExpand(s)}
                  onRetry={() => retryDetail(key)}
                />
              );
            })}
          </TableBody>
        </Table>
        {filtered.length === 0 && (
          <div className="flex h-32 items-center justify-center text-sm text-muted-foreground">
            {t("sessions.empty")}
          </div>
        )}
      </CardContent>
    </Card>
  );
}

function SessionRowGroup({
  session: s,
  isOpen,
  detail,
  onToggle,
  onRetry,
}: {
  session: SessionRow;
  isOpen: boolean;
  detail?: Detail;
  onToggle: () => void;
  onRetry: () => void;
}) {
  const { t } = useI18n();
  return (
    <>
      <TableRow
        onClick={onToggle}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            onToggle();
          }
        }}
        tabIndex={0}
        aria-expanded={isOpen}
        className="cursor-pointer focus-visible:bg-accent/50 focus-visible:outline-none"
      >
        <TableCell className="pr-0">
          <ChevronRight
            className={cn(
              "h-3.5 w-3.5 text-muted-foreground transition-transform",
              isOpen && "rotate-90",
            )}
          />
        </TableCell>
        <TableCell>
          <div className="max-w-56">
            <div className="truncate font-medium">{s.project}</div>
            <div className="truncate font-mono text-xs text-muted-foreground">
              {s.sessionId}
            </div>
          </div>
        </TableCell>
        <TableCell>
          <Badge variant={AGENT_BADGE[s.agent] ?? "default"}>
            {agentLabel(s.agent)}
          </Badge>
        </TableCell>
        <TableCell className="max-w-44">
          <div className="flex flex-wrap gap-1">
            {s.models
              .split(",")
              .filter(Boolean)
              .map((m) => (
                <Badge key={m} variant="outline">
                  {shortModelName(m)}
                </Badge>
              ))}
          </div>
        </TableCell>
        <TableCell className="whitespace-nowrap text-muted-foreground">
          {formatDateTime(s.lastTs)}
        </TableCell>
        <TableCell className="text-right tabular-nums">
          {formatNumber(s.requests)}
        </TableCell>
        <TableCell className="text-right tabular-nums">
          {formatTokens(s.totalTokens)}
        </TableCell>
        <TableCell className="text-right font-medium tabular-nums">
          {formatCost(s.cost)}
        </TableCell>
      </TableRow>
      {isOpen && (
        <TableRow className="hover:bg-transparent">
          <TableCell colSpan={8} className="bg-muted/30 p-0">
            {detail === "error" ? (
              <div className="flex items-center gap-3 px-10 py-3 text-xs text-muted-foreground">
                <AlertTriangle className="h-3.5 w-3.5" />
                {t("common.loadFailed")}
                <Button
                  variant="outline"
                  size="sm"
                  onClick={(e) => {
                    e.stopPropagation();
                    onRetry();
                  }}
                >
                  {t("common.retry")}
                </Button>
              </div>
            ) : detail ? (
              <table className="w-full text-xs">
                <thead>
                  <tr className="text-muted-foreground">
                    <th className="px-10 py-2 text-left font-medium uppercase tracking-wide">
                      {t("th.model")}
                    </th>
                    <th className="px-3 py-2 text-right font-medium uppercase tracking-wide">
                      {t("th.input")}
                    </th>
                    <th className="px-3 py-2 text-right font-medium uppercase tracking-wide">
                      {t("th.output")}
                    </th>
                    <th className="px-3 py-2 text-right font-medium uppercase tracking-wide">
                      {t("th.cacheWrite")}
                    </th>
                    <th className="px-3 py-2 text-right font-medium uppercase tracking-wide">
                      {t("th.cacheRead")}
                    </th>
                    <th className="px-3 py-2 text-right font-medium uppercase tracking-wide">
                      {t("th.requests")}
                    </th>
                    <th className="px-3 py-2 text-right font-medium uppercase tracking-wide">
                      {t("th.cost")}
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {detail.map((m) => (
                    <tr key={m.model} className="border-t border-border/50">
                      <td className="px-10 py-2 font-medium">
                        <span className="flex items-center gap-1.5">
                          <span title={m.model}>{shortModelName(m.model)}</span>
                          {m.priced === false && <UnpricedBadge />}
                        </span>
                      </td>
                      <td className="px-3 py-2 text-right tabular-nums">
                        {formatTokens(m.inputTokens)}
                      </td>
                      <td className="px-3 py-2 text-right tabular-nums">
                        {formatTokens(m.outputTokens)}
                      </td>
                      <td className="px-3 py-2 text-right tabular-nums">
                        {formatTokens(m.cacheCreationTokens)}
                      </td>
                      <td className="px-3 py-2 text-right tabular-nums">
                        {formatTokens(m.cacheReadTokens)}
                      </td>
                      <td className="px-3 py-2 text-right tabular-nums">
                        {formatNumber(m.requests)}
                      </td>
                      <td className="px-3 py-2 text-right font-medium tabular-nums">
                        {formatCost(m.cost)}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            ) : (
              <div className="px-10 py-3">
                <Skeleton className="h-8" />
              </div>
            )}
          </TableCell>
        </TableRow>
      )}
    </>
  );
}
