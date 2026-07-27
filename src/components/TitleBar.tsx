import { createSignal, onCleanup, onMount, Show } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getVersion } from "@tauri-apps/api/app";
import { logError } from "../lib/logger";
import { logoSrc } from "../lib/theme";

const isMac = typeof navigator !== "undefined" && navigator.userAgent.includes("Mac");

/**
 * 应用顶部标题栏。相比 lakemind：
 *  - 去掉 selectedTable / 数据相关的 prop 与中部 active source 展示。
 *  - 去掉 i18n (t) 与 updater 依赖（aioa 暂未迁移这两块），文案改为中文常量，
 *    更新检查菜单项暂不暴露。
 *  - 保留：品牌 logo/名称、左侧栏折叠按钮、日志抽屉折叠按钮、关于弹窗、
 *    原生窗口最小化/最大化/关闭按钮（Windows/Linux）。
 */
export default function TitleBar(props: {
  consoleOpen: boolean;
  onToggleConsole: () => void;
  busy?: boolean;
  leftOpen: boolean;
  onToggleLeft: () => void;
  /** 是否隐藏布局开关（如设置页全屏）。 */
  hideLayoutToggles?: boolean;
  /** 设置按钮回调。 */
  onOpenSettings?: () => void;
}) {
  const [menuOpen, setMenuOpen] = createSignal(false);
  const [aboutOpen, setAboutOpen] = createSignal(false);
  const [appVersion, setAppVersion] = createSignal("v0.1.0");
  const appWindow = typeof window !== "undefined" && (window as any).__TAURI_INTERNALS__ ? getCurrentWindow() : null;

  let menuRef!: HTMLDivElement;

  // Click outside to close menu
  const handleClickOutside = (e: MouseEvent) => {
    if (menuRef && !menuRef.contains(e.target as Node)) {
      setMenuOpen(false);
    }
  };

  onMount(() => {
    document.addEventListener("mousedown", handleClickOutside);
    if (typeof window !== "undefined" && (window as any).__TAURI_INTERNALS__) {
      getVersion().then((v) => setAppVersion(`v${v}`)).catch((e) => logError("ui", "get version failed", e));
    }
    onCleanup(() => {
      document.removeEventListener("mousedown", handleClickOutside);
    });
  });

  return (
    <div class="titlebar" data-tauri-drag-region>
      {/* Titlebar Left: Logo, Name, and sidebar toggle */}
      <div class="titlebar-left" classList={{ "mac-padding": isMac && !props.leftOpen }} data-tauri-drag-region>
        <Show when={!props.leftOpen}>
          <Show when={!isMac} fallback={
            <div class="ln-nav-arrows" style="display: flex; align-items: center; gap: 6px;" data-tauri-drag-region>
              {/* Sidebar toggle button (macOS) */}
              <button
                class="ln-arrow-btn"
                classList={{ active: props.leftOpen }}
                title={props.leftOpen ? "隐藏侧边栏" : "显示侧边栏"}
                onClick={() => props.onToggleLeft()}
              >
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                  <rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
                  <line x1="9" y1="3" x2="9" y2="21"></line>
                </svg>
              </button>
            </div>
          }>
            {/* 侧边栏折叠后只保留展开按钮（logo 和品牌名跟随左侧栏一起隐藏，
                避免顶部重复出现 "AI OA"）。 */}
            <button
              class="ln-arrow-btn"
              classList={{ active: props.leftOpen }}
              title={props.leftOpen ? "隐藏侧边栏" : "显示侧边栏"}
              onClick={() => props.onToggleLeft()}
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
                <line x1="9" y1="3" x2="9" y2="21"></line>
              </svg>
            </button>
          </Show>
        </Show>
      </div>

      {/* Titlebar Middle: drag region */}
      <div class="titlebar-middle" data-tauri-drag-region />

      {/* Titlebar Right: layout toggles + menu + native window actions */}
      <div class="titlebar-right" ref={menuRef} style="display: flex; align-items: center; gap: 4px; padding-right: 6px;">
        {/* Toggle Bottom Console Button */}
        <Show when={!props.hideLayoutToggles}>
          <button
            class="ln-arrow-btn"
            classList={{ active: props.consoleOpen }}
            title={props.consoleOpen ? "隐藏日志" : "显示日志"}
            onClick={() => props.onToggleConsole()}
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
              <line x1="3" y1="15" x2="21" y2="15"></line>
            </svg>
          </button>
        </Show>

        {/* Settings Button */}
        <Show when={!props.hideLayoutToggles && props.onOpenSettings}>
          <button
            class="ln-arrow-btn"
            title="设置"
            onClick={() => props.onOpenSettings?.()}
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
              <circle cx="12" cy="12" r="3"></circle>
              <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
            </svg>
          </button>
        </Show>

        <div class="tb-menu-wrap">
          <button
            class="tb-win-btn tb-menu-trigger-btn"
            classList={{ active: menuOpen() }}
            title="菜单"
            onClick={() => setMenuOpen(!menuOpen())}
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px;">
              <polyline points="6 9 12 15 18 9"></polyline>
            </svg>
          </button>

          {/* Dropdown Menu */}
          <Show when={menuOpen()}>
            <div class="tb-dropdown-menu right-aligned">
              <button
                class="menu-item"
                onClick={() => { setMenuOpen(false); setAboutOpen(true); }}
              >
                <span class="menu-label">关于应用</span>
                <span class="menu-shortcut"></span>
              </button>

              <div class="menu-divider" />

              <button
                class="menu-item close-item"
                onClick={() => { setMenuOpen(false); void appWindow?.close(); }}
              >
                <span class="menu-label">关闭窗口</span>
                <span class="menu-shortcut"></span>
              </button>
            </div>
          </Show>
        </div>

        <Show when={!isMac}>
          <button
            class="tb-win-btn"
            title="最小化"
            onClick={() => void appWindow?.minimize()}
          >
            <svg viewBox="0 0 10.2 1" style="width: 10px; height: 1px;">
              <rect x="0" y="0" width="10.2" height="1" fill="currentColor" />
            </svg>
          </button>
          <button
            class="tb-win-btn"
            title="最大化"
            onClick={() => void appWindow?.toggleMaximize()}
          >
            <svg viewBox="0 0 10 10" style="width: 10px; height: 10px;">
              <path d="M0,0v10h10V0H0z M9,9H1V1h8V9z" fill="currentColor" />
            </svg>
          </button>
          <button
            class="tb-win-btn close-btn"
            title="关闭"
            onClick={() => void appWindow?.close()}
          >
            <svg viewBox="0 0 10 10" style="width: 10px; height: 10px;">
              <polygon points="10,0.7 9.3,0 5,4.3 0.7,0 0,0.7 4.3,5 0,9.3 0.7,10 5,5.7 9.3,10 10,9.3 5.7,5" fill="currentColor" />
            </svg>
          </button>
        </Show>
      </div>

      {/* About Modal Dialog */}
      <Show when={aboutOpen()}>
        <div class="modal-overlay" onClick={() => setAboutOpen(false)}>
          <div class="modal-card" onClick={(e) => e.stopPropagation()}>
            <div class="modal-header">
              <h3>关于</h3>
              <button class="modal-close" onClick={() => setAboutOpen(false)}>✕</button>
            </div>
            <div class="modal-body">
              <div class="about-logo"><img src={logoSrc()} alt="AI OA" style="width: 48px; height: 48px; object-fit: contain;" /></div>
              <h4>AI OA</h4>
              <p class="about-desc">用对话完成请假、报销、审批等企业办公流程。</p>
              <div class="about-specs">
                <div class="spec-row"><span>版本</span><strong>{appVersion()}</strong></div>
                <div class="spec-row"><span>环境</span><strong>Tauri Webview Backend</strong></div>
                <div class="spec-row"><span>架构</span><strong>SolidJS Chat Layout</strong></div>
              </div>
            </div>
          </div>
        </div>
      </Show>
    </div>
  );
}
