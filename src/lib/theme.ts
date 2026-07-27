import { createSignal, createEffect } from "solid-js";

export type Theme = "geek-dark" | "classic-dark" | "light";

const THEME_KEY = "aioa_theme";

// 启动时从 localStorage 恢复主题，默认 geek-dark。
function loadTheme(): Theme {
  try {
    const saved = localStorage.getItem(THEME_KEY);
    if (saved === "geek-dark" || saved === "classic-dark" || saved === "light") return saved;
  } catch { /* localStorage 不可用时静默回退默认 */ }
  return "geek-dark";
}

export const [currentTheme, setCurrentTheme] = createSignal<Theme>(loadTheme());

// 持久化包装：切主题时写回 localStorage，重启后保持。
export const persistTheme = (t: Theme) => {
  setCurrentTheme(t);
  try { localStorage.setItem(THEME_KEY, t); } catch { /* ignore */ }
};
export const [currentZoom, setCurrentZoom] = createSignal<number>(100);

/** Logo path for the current theme: white logo on dark themes, dark logo on light. */
export const logoSrc = () => (currentTheme() === "light" ? "/logo.png" : "/logo_white.png");

// Set theme class on the document root when it changes
createEffect(() => {
  const t = currentTheme();
  document.documentElement.className = t;
});

// Set zoom style on the document root when it changes
createEffect(() => {
  const z = currentZoom();
  // Using zoom property for Chromium webview engine
  document.documentElement.style.zoom = (z / 100).toString();
});
