import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import App from "./App";
import { QuickPanel } from "./pages/QuickPanel";
import { NotchBar } from "./pages/NotchBar";
import { I18nProvider } from "./lib/i18n";
import { ThemeProvider } from "./lib/theme";
import { SubscriptionsProvider } from "./lib/subscriptions";
import { IN_TAURI } from "./lib/api";
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

// macOS gets the vibrancy material treatment (transparent windows +
// translucent surface washes, see index.css); other platforms keep the
// solid palette.
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

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ThemeProvider>
      <I18nProvider>
        <SubscriptionsProvider>
          {isNotchBar ? <NotchBar /> : isQuickPanel ? <QuickPanel /> : <App />}
        </SubscriptionsProvider>
      </I18nProvider>
    </ThemeProvider>
  </React.StrictMode>,
);
