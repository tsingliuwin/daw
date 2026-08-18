import { Show } from "solid-js";
import { brand } from "../lib/brand";

/**
 * 共用的底部品牌区：左侧品牌名（来自 ~/.daw/brand.json，见 lib/brand.ts）+ 右侧按钮组。
 *
 * 主界面 LeftNav 和设置页 SettingsPage 都用它，保证品牌区样式一致、单一数据源。
 *
 * 右侧按钮（从左到右）：
 *  - `onToggleTheme`：太阳/月亮图标，在深色 ↔ 浅色间二态切换（主界面用）。
 *  - `onOpenSettings`：齿轮图标，打开设置页（主界面 LeftNav 用）。
 *  - `onCloseSettings`：返回箭头图标，关闭设置页回到首页（设置页用，等同于右上角
 *    的 ✕ 关闭按钮，但鼠标不用移到右上角）。
 * 各按钮可选，不传则不渲染。
 */
export default function BrandFooter(props: {
  onToggleTheme?: () => void;
  /** 当前是否为深色主题--决定显示月亮（深色）还是太阳（浅色）图标。 */
  isDarkTheme?: boolean;
  onOpenSettings?: () => void;
  onCloseSettings?: () => void;
}) {
  return (
    <div class="ln-footer">
      <span class="ln-brand-name">{brand().app_name}</span>
      <div class="ln-footer-actions">
        {props.onToggleTheme && (
          <button
            class="ln-footer-settings"
            title={props.isDarkTheme ? "切换到浅色" : "切换到深色"}
            onClick={() => props.onToggleTheme?.()}
          >
            {/* 深色时显示太阳（点一下变浅色）；浅色时显示月亮（点一下变深色） */}
            <Show when={props.isDarkTheme} fallback={
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 16px; height: 16px;">
                <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"></path>
              </svg>
            }>
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 16px; height: 16px;">
                <circle cx="12" cy="12" r="5"></circle>
                <line x1="12" y1="1" x2="12" y2="3"></line>
                <line x1="12" y1="21" x2="12" y2="23"></line>
                <line x1="4.22" y1="4.22" x2="5.64" y2="5.64"></line>
                <line x1="18.36" y1="18.36" x2="19.78" y2="19.78"></line>
                <line x1="1" y1="12" x2="3" y2="12"></line>
                <line x1="21" y1="12" x2="23" y2="12"></line>
                <line x1="4.22" y1="19.78" x2="5.64" y2="18.36"></line>
                <line x1="18.36" y1="5.64" x2="19.78" y2="4.22"></line>
              </svg>
            </Show>
          </button>
        )}
        {props.onOpenSettings && (
          <button
            class="ln-footer-settings"
            title="设置"
            onClick={() => props.onOpenSettings?.()}
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 15px; height: 15px;">
              <circle cx="12" cy="12" r="3"></circle>
              <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
            </svg>
          </button>
        )}
        {props.onCloseSettings && (
          <button
            class="ln-footer-settings"
            title="返回首页"
            onClick={() => props.onCloseSettings?.()}
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 16px; height: 16px;">
              <line x1="19" y1="12" x2="5" y2="12"></line>
              <polyline points="12 19 5 12 12 5"></polyline>
            </svg>
          </button>
        )}
      </div>
    </div>
  );
}
