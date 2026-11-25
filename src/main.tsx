import React, { useEffect, useState } from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import App, { isPage, type Page } from "./App";
import { LoadError } from "./components/LoadError";
import { Logo } from "./components/Logo";
import { QuickPanel } from "./pages/QuickPanel";
import { NotchBar } from "./pages/NotchBar";
import { I18nProvider, useI18n } from "./lib/i18n";
import { ThemeProvider } from "./lib/theme";
import { SubscriptionsProvider } from "./lib/subscriptions";
import { api, IN_TAURI, onEvent } from "./lib/api";
import "./index.css";

// Outside Tauri (browser preview / design QA) there is no webview label
// and no vibrancy behind the page; `?window=quick` / `?window=notch`
// selects the corresponding surface.
const isMac = navigator.userAgent.includes("Mac");
const windowLabel = IN_TAURI
  ? getCurrentWebviewWindow().label
  : new URLSearchParams(location.search).get("window") ?? "main";
const isQuickPanel = windowLabel === "quick";
const isNotchBar = windowLabel === "notch";
const isMainWindow = !isQuickPanel && !isNotchBar;

// macOS keeps a translucent sidebar and room for the native window buttons.
if (isMac && IN_TAURI) {
  document.documentElement.classList.add("mac");
}

if (isQuickPanel) {
  document.documentElement.classList.add("quick-panel");
  // The quick window is only transparent on macOS (see setup_tray); on
  // Windows/Linux it is opaque, so the page must paint its own background.
  if (!isMac || !IN_TAURI) {
    document.documentElement.classList.add("quick-panel-opaque");
  }
}

// The notch window is a fully transparent stage; the bar paints itself.
if (isNotchBar) {
  document.documentElement.classList.add("notch-window");
}

function WindowContent() {
  const { lang, t } = useI18n();
  const [page, setPage] = useState<Page>("overview");
  const [ready, setReady] = useState(!IN_TAURI || isNotchBar);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    if (!IN_TAURI || !isMainWindow) return;
    let active = true;
    // Listen before revealing the themed shell, including while its data loads.
    const unlisten = onEvent<string>("navigate-page", ({ payload }) => {
      if (active && isPage(payload)) setPage(payload);
    });
    void unlisten
      .catch((error) => console.error("listening for navigation failed:", error))
      .then(() => {
        if (active) return api.showMainWindow();
      })
      .catch((error) => console.error("showing the main window failed:", error));
    return () => {
      active = false;
      void unlisten.then((stop) => stop(), () => {});
    };
  }, []);

  useEffect(() => {
    if (!IN_TAURI || isNotchBar) return;
    let active = true;
    api.initializeApp().then(
      () => { if (active) setReady(true); },
      (error) => { if (active) setError(String(error)); },
    );
    return () => { active = false; };
  }, [attempt]);

  useEffect(() => {
    if (IN_TAURI && isMainWindow && ready) {
      void api.setLanguage(lang).catch((error) => console.error("set_language failed:", error));
    }
  }, [lang, ready]);

  if (!ready) {
    return (
      <main className="flex h-full items-center justify-center bg-background px-5">
        <div className="w-full max-w-sm space-y-4">
          <div className="flex items-center justify-center gap-2.5 font-semibold">
            <Logo size={27} /> TokBar
          </div>
          {error !== null ? (
            <>
              <LoadError onRetry={() => { setError(null); setAttempt((n) => n + 1); }} />
              <p className="text-center text-xs break-words text-muted-foreground">{error}</p>
            </>
          ) : (
            <p role="status" className="text-center text-sm text-muted-foreground">
              {t("app.starting")}
            </p>
          )}
        </div>
      </main>
    );
  }

  if (isNotchBar) return <NotchBar />;
  if (isQuickPanel) return <QuickPanel />;
  return <App page={page} setPage={setPage} />;
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ThemeProvider>
      <I18nProvider>
        <SubscriptionsProvider>
          <WindowContent />
        </SubscriptionsProvider>
      </I18nProvider>
    </ThemeProvider>
  </React.StrictMode>,
);
