import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useState,
  type ReactNode,
} from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { IN_TAURI } from "@/lib/api";

export type ThemeMode = "dark" | "light";
export type AccentKey = "amber" | "blue" | "emerald" | "violet" | "rose";

export const ACCENTS: Record<
  AccentKey,
  {
    label: string;
    swatch: string;
    light: string;
    dark: string;
    /// Chart series palette led by the accent — charts re-color with the theme.
    charts: [string, string, string, string, string];
  }
> = {
  amber: {
    label: "Amber",
    swatch: "#be672a",
    light: "#9c4b1b",
    dark: "#efae7a",
    charts: ["#be672a", "#5a83c1", "#45907d", "#9676c4", "#b59b50"],
  },
  blue: {
    label: "Blue",
    swatch: "#4c6ddd",
    light: "#3256cf",
    dark: "#9aadff",
    charts: ["#4c6ddd", "#4b9da0", "#b5834e", "#9878c4", "#ca7594"],
  },
  emerald: {
    label: "Emerald",
    swatch: "#268d6c",
    light: "#0e7355",
    dark: "#76d0ae",
    charts: ["#268d6c", "#5f87d0", "#b3834b", "#9878c4", "#aa8191"],
  },
  violet: {
    label: "Violet",
    swatch: "#8863c5",
    light: "#7046b5",
    dark: "#c9aff8",
    charts: ["#8863c5", "#bd728d", "#4988b5", "#51947e", "#b38755"],
  },
  rose: {
    label: "Rose",
    swatch: "#c95e7f",
    light: "#ab3d60",
    dark: "#f4a0b9",
    charts: ["#c95e7f", "#b5824f", "#8875c1", "#4f93a0", "#6086c2"],
  },
};

const MODE_KEY = "tokbar-theme-mode";
const ACCENT_KEY = "tokbar-accent";

function applyTheme(mode: ThemeMode, accent: AccentKey) {
  const root = document.documentElement;
  root.classList.toggle("dark", mode === "dark");
  // Keep the native window chrome (macOS traffic-light strip, Windows
  // title bar) in the same color world as the page.
  if (IN_TAURI) {
    getCurrentWindow().setTheme(mode).catch(() => {});
  }
  const a = ACCENTS[accent];
  root.style.setProperty("--primary", a[mode]);
  root.style.setProperty("--ring", a[mode]);
  a.charts.forEach((color, i) => {
    root.style.setProperty(`--chart-${i + 1}`, color);
  });
}

function detectMode(): ThemeMode {
  const saved = localStorage.getItem(MODE_KEY);
  if (saved === "light" || saved === "dark") return saved;
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

function detectAccent(): AccentKey {
  const saved = localStorage.getItem(ACCENT_KEY);
  return saved && saved in ACCENTS ? (saved as AccentKey) : "blue";
}

interface ThemeValue {
  mode: ThemeMode;
  accent: AccentKey;
  setMode: (mode: ThemeMode) => void;
  setAccent: (accent: AccentKey) => void;
}

const ThemeContext = createContext<ThemeValue>({
  mode: "light",
  accent: "blue",
  setMode: () => {},
  setAccent: () => {},
});

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [mode, setModeState] = useState<ThemeMode>(detectMode);
  const [accent, setAccentState] = useState<AccentKey>(detectAccent);

  useLayoutEffect(() => {
    applyTheme(mode, accent);
  }, [mode, accent]);

  // Sync between the main window and the menu-bar quick panel.
  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key === MODE_KEY && (e.newValue === "dark" || e.newValue === "light")) {
        setModeState(e.newValue);
      }
      if (e.key === ACCENT_KEY && e.newValue && e.newValue in ACCENTS) {
        setAccentState(e.newValue as AccentKey);
      }
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);

  const setMode = useCallback((m: ThemeMode) => {
    localStorage.setItem(MODE_KEY, m);
    setModeState(m);
  }, []);

  const setAccent = useCallback((a: AccentKey) => {
    localStorage.setItem(ACCENT_KEY, a);
    setAccentState(a);
  }, []);

  return (
    <ThemeContext.Provider value={{ mode, accent, setMode, setAccent }}>
      {children}
    </ThemeContext.Provider>
  );
}

export function useTheme() {
  return useContext(ThemeContext);
}
