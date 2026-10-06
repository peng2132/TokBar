import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react";
import { api } from "./api";

export type Lang = "zh" | "en";

const en = {
  // App shell
  "app.subtitle": "AI Usage Dashboard",
  "nav.overview": "Overview",
  "nav.trends": "Trends",
  "nav.sessions": "Sessions",
  "nav.models": "Models",
  "nav.blocks": "Billing Blocks",
  "nav.settings": "Settings",
  "app.refresh": "Refresh",
  "app.scanning": "Scanning…",
  "app.scanProgress": "Scanning {done}/{total}",
  "app.upToDate": "Up to date",
  "app.refreshed": "Updated {n} entries",
  "app.refreshFailed": "Refresh failed",
  "range.today": "Today",
  "range.7d": "7D",
  "range.30d": "30D",
  "range.90d": "90D",
  "range.all": "All",

  // Overview
  "overview.totalCost": "Total Cost",
  "overview.totalTokens": "Total Tokens",
  "overview.requests": "Requests",
  "overview.sessions": "Sessions",
  "overview.activeDays": "{n} active days",
  "overview.inOut": "in {in} / out {out}",
  "overview.cacheRead": "cache read {n}",
  "overview.agents": "{n} agents",
  "overview.costTrend": "Cost Trend",
  "overview.costByModel": "Cost by Model",
  "overview.costByAgent": "Cost by Agent",
  "overview.topProjects": "Top Projects",
  "overview.projectStats": "{cost} · {tokens} tok · {sessions} sessions",
  "common.empty": "No usage data in this period",
  "common.loadFailed": "Failed to load data",
  "common.retry": "Retry",
  "blocks.remainHM": "{h}h {m}m left",
  "blocks.remainM": "{m}m left",
  "unit.perMin": "/min",
  "unit.perHour": "/h",
  /** Tight slots (notch wing) where "/小时" would not fit. */
  "unit.perHourShort": "/h",
  "unit.tok": "tok",

  // Table heads
  "th.date": "Date",
  "th.time": "Time",
  "th.week": "Week of",
  "th.month": "Month",
  "th.input": "Input",
  "th.output": "Output",
  "th.cacheWrite": "Cache Write",
  "th.cacheRead": "Cache Read",
  "th.requests": "Requests",
  "th.cost": "Cost",
  "th.project": "Project",
  "th.agent": "Agent",
  "th.models": "Models",
  "th.lastActivity": "Last Activity",
  "th.tokens": "Tokens",
  "th.model": "Model",

  // Trends
  "trends.byDay": "Daily",
  "trends.byWeek": "Weekly",
  "trends.byMonth": "Monthly",
  "trends.cost.hour": "Hourly Cost",
  "trends.cost.day": "Daily Cost",
  "trends.cost.week": "Weekly Cost",
  "trends.cost.month": "Monthly Cost",
  "trends.tokens.hour": "Hourly Tokens by Type",
  "trends.tokens.day": "Daily Tokens by Type",
  "trends.tokens.week": "Weekly Tokens by Type",
  "trends.tokens.month": "Monthly Tokens by Type",
  "trends.breakdown.hour": "Hourly Breakdown",
  "trends.breakdown.day": "Daily Breakdown",
  "trends.breakdown.week": "Weekly Breakdown",
  "trends.breakdown.month": "Monthly Breakdown",
  "chart.input": "Input",
  "chart.output": "Output",
  "chart.cacheRead": "Cache Read",
  "chart.cacheWrite": "Cache Write",

  // Sessions
  "sessions.title": "Sessions",
  "sessions.empty": "No sessions in this period",
  "sessions.search": "Search project / session / model…",
  "sessions.allAgents": "All",

  // Models
  "models.costDist": "Cost Distribution",
  "models.tokenDist": "Token Distribution",
  "models.all": "All Models",

  // Blocks
  "blocks.desc":
    "5-hour billing blocks: each agent's usage is grouped into 5-hour windows aligned to the hour, matching Claude's session billing window. Last {days} days.",
  "blocks.empty": "No billing blocks in the last {days} days",
  "blocks.completed": "Completed",
  "blocks.live": "LIVE",
  "blocks.cost": "Cost",
  "blocks.tokens": "Tokens",
  "blocks.requests": "Requests",
  "blocks.burnRate": "Burn rate",
  "blocks.costRate": "Cost rate",

  // Settings
  "settings.language": "Language",
  "settings.languageDesc": "Display language for the interface.",
  "settings.general": "General",
  "settings.autostart": "Launch at login",
  "settings.autostartDesc": "Start TokBar automatically when you log in.",
  "settings.appearance": "Appearance",
  "settings.themeMode": "Theme",
  "settings.theme.dark": "Dark",
  "settings.theme.light": "Light",
  "settings.accentColor": "Accent color",
  "settings.accent.amber": "Amber",
  "settings.accent.blue": "Blue",
  "settings.accent.emerald": "Emerald",
  "settings.accent.violet": "Violet",
  "settings.accent.rose": "Rose",
  "settings.saveFailed": "Couldn't save that setting. Please try again.",
  "settings.trayDisplay": "Always-On Display",
  "settings.trayDisplayDesc":
    "Today's usage, always visible at the top of your screen.",
  "settings.tray.cost": "Today's cost",
  "settings.tray.tokens": "Today's tokens",
  "settings.tray.off": "Icon only",
  "settings.displayPosition": "Position",
  "settings.position.notch": "Notch",
  "settings.position.menubar": "Menu bar",
  "settings.position.notchHint":
    "At rest it looks like the notch itself (a green dot glows while a billing block is active). Hover to reveal the numbers, click to expand details.",
  "settings.trayContent": "Menu bar content",
  "notch.other": "Other",
  "notch.rhythm": "Today's rhythm",
  "notch.peak": "peak",
  "notch.expand": "Show today's usage details",
  "notch.collapse": "Hide usage details",
  "settings.dataSources": "Data Sources",
  "settings.sources.inactive": "Not detected ({n})",
  "settings.files": "{n} files",
  "settings.lastScan":
    "Last scan: {parsed} of {total} files re-parsed, {entries} entries updated in {ms} ms.",
  "settings.costMode": "Cost Mode",
  "settings.mode.auto": "Auto",
  "settings.mode.autoDesc":
    "Use the log's costUSD when present, otherwise calculate from tokens.",
  "settings.mode.calculate": "Calculate",
  "settings.mode.calculateDesc":
    "Always calculate from token counts using LiteLLM pricing.",
  "settings.mode.display": "Display",
  "settings.mode.displayDesc": "Only show pre-computed costUSD from logs.",
  "settings.pricing": "Pricing Data",
  "settings.pricingDesc":
    "Cost is calculated from LiteLLM's community model pricing database. Prices refresh automatically once a day; a successful refresh re-prices all recorded usage. Without a network connection the price table built into the app is used.",
  "pricing.source": "Source",
  "pricing.online": "Online (LiteLLM)",
  "pricing.snapshot": "Offline snapshot",
  "pricing.snapshotDate": "built in, prices as of {date}",
  "pricing.fetchedAt": "fetched {time}",
  "pricing.models": "Priced models",
  "pricing.refresh": "Refresh prices",
  "pricing.refreshing": "Refreshing…",
  "pricing.lastError": "Last refresh failed: {error}",
  "pricing.unpricedTitle": "Unpriced models ({n})",
  "pricing.unpricedDesc":
    "No price is known for these models, so their usage is counted as $0 — the real cost is higher.",
  "pricing.allPriced": "Every model in your usage has a known price.",
  "pricing.unpriced": "unpriced",
  "pricing.unpricedTip":
    "No price is known for this model: its usage is counted as $0, which is not its real cost.",

  // Quick panel (menu bar popover)
  "quick.todayCost": "Today's Cost",
  "quick.todayTokens": "Tokens Today",
  "quick.todayRequests": "Requests Today",
  "quick.monthCost": "This Month",
  "quick.activeBlock": "Active Billing Block",
  "quick.noActiveBlock": "No active billing block",
  "quick.burnRate": "Burn rate",
  "quick.costRate": "Cost rate",
  "quick.blockEnds": "Block ends",
  "quick.openDashboard": "Open Dashboard",

  // Subscription ROI (Overview)
  "roi.title": "Subscription ROI",
  "roi.subtitle": "This month · at API prices",
  "roi.apiValue": "API-priced value",
  "roi.youPay": "you pay",
  "roi.saved": "Saved {amount}",
  "roi.notRecouped": "{amount} to break even",
  "roi.multiple": "{x}× back",
  "roi.noUsage": "no usage",
  "roi.untitled": "Untitled",
  "roi.hint":
    "When plans share an agent, its usage is split across them by fee — never double-counted. Real savings are usually higher, since plans also cover usage TokBar can't see.",

  // Settings — subscriptions
  "settings.subscriptions": "Subscriptions",
  "settings.subscriptionsDesc":
    "Track flat-rate plans (Claude Max, ChatGPT Pro…). TokBar prices the agents they cover at API rates and shows your real ROI on the Overview.",
  "settings.sub.namePlaceholder": "Plan name",
  "settings.sub.agent": "Covers",
  "settings.sub.pickAgents": "Pick agents",
  "settings.sub.addAgent": "Add",
  "settings.sub.perMonth": "/mo",
  "settings.sub.quickAdd": "Quick add",
  "settings.sub.custom": "Custom",
  "settings.sub.remove": "Remove",
  "settings.sub.confirmRemove": "Confirm delete",
  "settings.sub.empty":
    "No subscriptions yet — pick one below to see your ROI on the Overview.",
};

const zh: Record<keyof typeof en, string> = {
  "app.subtitle": "AI 用量仪表盘",
  "nav.overview": "总览",
  "nav.trends": "趋势",
  "nav.sessions": "会话",
  "nav.models": "模型",
  "nav.blocks": "计费块",
  "nav.settings": "设置",
  "app.refresh": "刷新",
  "app.scanning": "扫描中…",
  "app.scanProgress": "扫描中 {done}/{total}",
  "app.upToDate": "已是最新",
  "app.refreshed": "已更新 {n} 条",
  "app.refreshFailed": "刷新失败",
  "range.today": "今日",
  "range.7d": "7天",
  "range.30d": "30天",
  "range.90d": "90天",
  "range.all": "全部",

  "overview.totalCost": "总成本",
  "overview.totalTokens": "总 Token",
  "overview.requests": "请求数",
  "overview.sessions": "会话数",
  "overview.activeDays": "{n} 个活跃天",
  "overview.inOut": "输入 {in} / 输出 {out}",
  "overview.cacheRead": "缓存读取 {n}",
  "overview.agents": "{n} 个 Agent",
  "overview.costTrend": "成本趋势",
  "overview.costByModel": "按模型分布",
  "overview.costByAgent": "按 Agent 分布",
  "overview.topProjects": "项目排行",
  "overview.projectStats": "{cost} · {tokens} tok · {sessions} 个会话",
  "common.empty": "该时间段内暂无使用数据",
  "common.loadFailed": "数据加载失败",
  "common.retry": "重试",
  "blocks.remainHM": "还剩 {h} 小时 {m} 分",
  "blocks.remainM": "还剩 {m} 分钟",
  "unit.perMin": "/分钟",
  "unit.perHour": "/小时",
  "unit.perHourShort": "/时",
  "unit.tok": "tok",

  "th.date": "日期",
  "th.time": "时间段",
  "th.week": "周起始",
  "th.month": "月份",
  "th.input": "输入",
  "th.output": "输出",
  "th.cacheWrite": "缓存写入",
  "th.cacheRead": "缓存读取",
  "th.requests": "请求数",
  "th.cost": "成本",
  "th.project": "项目",
  "th.agent": "Agent",
  "th.models": "模型",
  "th.lastActivity": "最近活动",
  "th.tokens": "Token",
  "th.model": "模型",

  "trends.byDay": "按日",
  "trends.byWeek": "按周",
  "trends.byMonth": "按月",
  "trends.cost.hour": "每小时成本",
  "trends.cost.day": "每日成本",
  "trends.cost.week": "每周成本",
  "trends.cost.month": "每月成本",
  "trends.tokens.hour": "每小时 Token(按类型)",
  "trends.tokens.day": "每日 Token(按类型)",
  "trends.tokens.week": "每周 Token(按类型)",
  "trends.tokens.month": "每月 Token(按类型)",
  "trends.breakdown.hour": "每小时明细",
  "trends.breakdown.day": "每日明细",
  "trends.breakdown.week": "每周明细",
  "trends.breakdown.month": "每月明细",
  "chart.input": "输入",
  "chart.output": "输出",
  "chart.cacheRead": "缓存读取",
  "chart.cacheWrite": "缓存写入",

  "sessions.title": "会话",
  "sessions.empty": "该时间段内暂无会话",
  "sessions.search": "搜索项目 / 会话 / 模型…",
  "sessions.allAgents": "全部",

  "models.costDist": "成本分布",
  "models.tokenDist": "Token 分布",
  "models.all": "全部模型",

  "blocks.desc":
    "5 小时计费块:每个 Agent 的用量按整点对齐的 5 小时窗口分组,对应 Claude 的会话计费窗口。显示最近 {days} 天。",
  "blocks.empty": "最近 {days} 天内没有计费块",
  "blocks.completed": "已结束",
  "blocks.live": "进行中",
  "blocks.cost": "成本",
  "blocks.tokens": "Token",
  "blocks.requests": "请求数",
  "blocks.burnRate": "燃烧率",
  "blocks.costRate": "成本速率",

  "settings.language": "语言",
  "settings.languageDesc": "界面显示语言。",
  "settings.general": "通用",
  "settings.autostart": "开机自启",
  "settings.autostartDesc": "登录系统时自动启动 TokBar。",
  "settings.appearance": "外观",
  "settings.themeMode": "主题",
  "settings.theme.dark": "深色",
  "settings.theme.light": "浅色",
  "settings.accentColor": "主题色",
  "settings.accent.amber": "琥珀",
  "settings.accent.blue": "蓝",
  "settings.accent.emerald": "翠绿",
  "settings.accent.violet": "紫罗兰",
  "settings.accent.rose": "玫瑰",
  "settings.saveFailed": "设置保存失败,请重试。",
  "settings.trayDisplay": "常驻显示",
  "settings.trayDisplayDesc": "屏幕顶部常驻的今日用量。",
  "settings.tray.cost": "今日成本",
  "settings.tray.tokens": "今日 Token",
  "settings.tray.off": "仅图标",
  "settings.displayPosition": "显示位置",
  "settings.position.notch": "刘海",
  "settings.position.menubar": "菜单栏",
  "settings.position.notchHint":
    "平时看起来就是刘海本身(计费活跃时亮起小绿点);鼠标移上去显示数字,点击展开详情。",
  "settings.trayContent": "菜单栏内容",
  "notch.other": "其他",
  "notch.rhythm": "今日节奏",
  "notch.peak": "峰值",
  "notch.expand": "展开今日用量详情",
  "notch.collapse": "收起用量详情",
  "settings.dataSources": "数据源",
  "settings.sources.inactive": "未检测到 ({n})",
  "settings.files": "{n} 个文件",
  "settings.lastScan":
    "上次扫描:重新解析 {parsed}/{total} 个文件,更新 {entries} 条记录,耗时 {ms} 毫秒。",
  "settings.costMode": "成本模式",
  "settings.mode.auto": "自动",
  "settings.mode.autoDesc":
    "日志中有 costUSD 时优先使用,否则按 Token 计算。",
  "settings.mode.calculate": "计算",
  "settings.mode.calculateDesc": "始终按 Token 数 × LiteLLM 价格重新计算。",
  "settings.mode.display": "展示",
  "settings.mode.displayDesc": "只显示日志中预先计算好的 costUSD。",
  "settings.pricing": "定价数据",
  "settings.pricingDesc":
    "成本基于 LiteLLM 社区模型价格库计算。价格每天自动更新一次,更新成功后会按新价格重新计算全部已记录用量。无网络时使用应用内置的价格表。",
  "pricing.source": "来源",
  "pricing.online": "在线(LiteLLM)",
  "pricing.snapshot": "离线快照",
  "pricing.snapshotDate": "内置,价格截至 {date}",
  "pricing.fetchedAt": "获取于 {time}",
  "pricing.models": "已定价模型",
  "pricing.refresh": "刷新价格",
  "pricing.refreshing": "刷新中…",
  "pricing.lastError": "上次刷新失败:{error}",
  "pricing.unpricedTitle": "未定价模型({n})",
  "pricing.unpricedDesc":
    "这些模型没有已知价格,其用量按 $0 计入 —— 实际成本更高。",
  "pricing.allPriced": "用量中的所有模型都有已知价格。",
  "pricing.unpriced": "未定价",
  "pricing.unpricedTip": "该模型没有已知价格:其用量按 $0 计入,并非真实成本。",

  "quick.todayCost": "今日成本",
  "quick.todayTokens": "今日 Token",
  "quick.todayRequests": "今日请求",
  "quick.monthCost": "本月成本",
  "quick.activeBlock": "活跃计费块",
  "quick.noActiveBlock": "当前没有活跃计费块",
  "quick.burnRate": "燃烧率",
  "quick.costRate": "成本速率",
  "quick.blockEnds": "块结束于",
  "quick.openDashboard": "打开主面板",

  // 订阅回本(总览)
  "roi.title": "订阅回本",
  "roi.subtitle": "本月 · 按 API 价",
  "roi.apiValue": "按 API 计价",
  "roi.youPay": "实付",
  "roi.saved": "省了 {amount}",
  "roi.notRecouped": "还差 {amount} 回本",
  "roi.multiple": "{x}× 回本",
  "roi.noUsage": "暂无用量",
  "roi.untitled": "未命名",
  "roi.hint":
    "多个套餐覆盖同一 agent 时,其用量按月费占比分摊,不会重复计价。实际节省通常更多 —— 套餐还覆盖了 TokBar 看不到的用量。",

  // 设置 — 订阅
  "settings.subscriptions": "订阅",
  "settings.subscriptionsDesc":
    "记录你的固定月费套餐(Claude Max、ChatGPT Pro…)。TokBar 会按 API 价给它们覆盖的 agent 计价,在总览展示你真实的回本情况。",
  "settings.sub.namePlaceholder": "套餐名称",
  "settings.sub.agent": "覆盖",
  "settings.sub.pickAgents": "选择 Agent",
  "settings.sub.addAgent": "添加",
  "settings.sub.perMonth": "/月",
  "settings.sub.quickAdd": "快速添加",
  "settings.sub.custom": "自定义",
  "settings.sub.remove": "删除",
  "settings.sub.confirmRemove": "确认删除",
  "settings.sub.empty": "还没有订阅 —— 在下方选一个即可在总览看到回本情况。",
};

const dicts = { en, zh };

export type I18nKey = keyof typeof en;

interface I18nValue {
  lang: Lang;
  setLang: (lang: Lang) => void;
  t: (key: I18nKey, vars?: Record<string, string | number>) => string;
}

const I18nContext = createContext<I18nValue>({
  lang: "en",
  setLang: () => {},
  t: (k) => k,
});

const STORAGE_KEY = "tokbar-lang";

function detectLang(): Lang {
  const saved = localStorage.getItem(STORAGE_KEY);
  if (saved === "zh" || saved === "en") return saved;
  return navigator.language.toLowerCase().startsWith("zh") ? "zh" : "en";
}

export function I18nProvider({ children }: { children: ReactNode }) {
  const [lang, setLangState] = useState<Lang>(detectLang);

  const setLang = useCallback((l: Lang) => {
    localStorage.setItem(STORAGE_KEY, l);
    setLangState(l);
  }, []);

  // Keep the main window and the menu-bar quick panel in sync:
  // storage events fire in the other window when one changes language.
  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key === STORAGE_KEY && (e.newValue === "zh" || e.newValue === "en")) {
        setLangState(e.newValue);
      }
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);

  // The backend localizes the menu-bar tooltip; tell it on startup and on
  // every change. Idempotent, so every window reporting the same
  // (shared, storage-synced) language is harmless.
  useEffect(() => {
    api.setLanguage(lang).catch((e) => console.error("set_language failed:", e));
  }, [lang]);

  const t = useCallback(
    (key: I18nKey, vars?: Record<string, string | number>) => {
      let s: string = dicts[lang][key] ?? en[key] ?? key;
      if (vars) {
        for (const [k, v] of Object.entries(vars)) {
          s = s.replace(`{${k}}`, String(v));
        }
      }
      return s;
    },
    [lang],
  );

  return (
    <I18nContext.Provider value={{ lang, setLang, t }}>
      {children}
    </I18nContext.Provider>
  );
}

export function useI18n() {
  return useContext(I18nContext);
}
