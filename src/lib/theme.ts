import { createSignal, createEffect } from "solid-js";
import { invoke } from "@tauri-apps/api/core";

export type Theme = "geek-dark" | "classic-dark" | "light";

// 主题的权威数据源是后端 config 表（~/.daw/daw.db，key = ui.theme）。
// localStorage 只作为「启动首帧」的快速镜像：webview 首帧早于任何 invoke，
// 只有同步可读的 localStorage 能让 index.html 的 splash 与首帧直接用对主题，
// 避免每次启动「深色 splash → 浅色首页」的闪屏。权威仍以后端为准——
// loadThemeFromBackend 会校正并把读到的值重新写回镜像。
const THEME_CONFIG_KEY = "ui.theme";
const THEME_CACHE_KEY = "ui.theme";

function readCachedTheme(): Theme | null {
  try {
    const v = localStorage.getItem(THEME_CACHE_KEY);
    if (v === "geek-dark" || v === "classic-dark" || v === "light") return v;
  } catch {
    /* localStorage 不可用时回退默认 */
  }
  return null;
}

// 初始值同步读镜像：让本模块的第一个 signal 值就和 index.html 内联脚本
// 设置到 <html> 上的 class 一致，createEffect 首帧不会把主题覆盖回深色。
export const [currentTheme, setCurrentTheme] = createSignal<Theme>(
  readCachedTheme() ?? "geek-dark",
);

/** 启动时从后端 config 恢复主题。在 App onMount 里调用。 */
export async function loadThemeFromBackend() {
  try {
    const saved = await invoke<string | null>("get_app_config", { key: THEME_CONFIG_KEY });
    if (saved === "geek-dark" || saved === "classic-dark" || saved === "light") {
      setCurrentTheme(saved);
      try {
        localStorage.setItem(THEME_CACHE_KEY, saved);
      } catch {
        /* 忽略镜像写失败 */
      }
    }
  } catch {
    /* 后端不可用时静默用缓存/默认 */
  }
}

/** 切主题 + 写回后端 config 表 + 刷新 localStorage 镜像。 */
export function persistTheme(t: Theme) {
  setCurrentTheme(t);
  try {
    localStorage.setItem(THEME_CACHE_KEY, t);
  } catch {
    /* 忽略镜像写失败 */
  }
  invoke("set_app_config", { key: THEME_CONFIG_KEY, value: t }).catch(() => {
    /* 持久化失败不阻断切换——当前会话仍生效，仅下次启动回默认 */
  });
}

export const [currentZoom, setCurrentZoom] = createSignal<number>(100);

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