import { createSignal, createEffect } from "solid-js";
import { invoke } from "@tauri-apps/api/core";

export type Theme = "geek-dark" | "classic-dark" | "light";

// 主题存在后端 config 表（~/.aioa/aioa.db），key = ui.theme。
// 不用 localStorage——webview 的 localStorage 跨窗口不共享、清缓存会丢；
// 后端 config 表是单一数据源，重启/跨窗口一致。
const THEME_CONFIG_KEY = "ui.theme";

export const [currentTheme, setCurrentTheme] = createSignal<Theme>("geek-dark");

/** 启动时从后端 config 恢复主题。在 App onMount 里调用。 */
export async function loadThemeFromBackend() {
  try {
    const saved = await invoke<string | null>("get_app_config", { key: THEME_CONFIG_KEY });
    if (saved === "geek-dark" || saved === "classic-dark" || saved === "light") {
      setCurrentTheme(saved);
    }
  } catch { /* 后端不可用时静默用默认 */ }
}

/** 切主题 + 写回后端 config 表。 */
export function persistTheme(t: Theme) {
  setCurrentTheme(t);
  invoke("set_app_config", { key: THEME_CONFIG_KEY, value: t }).catch(() => {
    /* 持久化失败不阻断切换——当前会话仍生效，仅下次启动回默认 */
  });
}
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
