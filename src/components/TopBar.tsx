import { Show } from "solid-js";

/**
 * 备用顶部栏组件。相比 lakemind 瘦身：去掉 inspector 切换（aioa 无右侧检查器），
 * 只保留品牌、日志抽屉折叠、设置按钮。主布局实际使用 TitleBar，TopBar 作为
 * 轻量替代保留，按需引用。
 */
export default function TopBar(props: {
  consoleOpen: boolean;
  onToggleConsole: () => void;
  onOpenSettings?: () => void;
}) {
  return (
    <header class="topbar">
      <span class="brand">AI OA</span>
      <span class="brand-sub">对话驱动办公</span>
      <span class="spacer" />
      <div class="toggle-group">
        <Show
          when={props.consoleOpen}
          fallback={
            <button
              class="icon-btn"
              title="显示日志"
              onClick={() => props.onToggleConsole()}
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
                <rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
                <line x1="3" y1="15" x2="21" y2="15"></line>
              </svg>
            </button>
          }
        >
          <button
            class="icon-btn active"
            title="隐藏日志"
            onClick={() => props.onToggleConsole()}
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
              <rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
              <line x1="3" y1="15" x2="21" y2="15"></line>
            </svg>
          </button>
        </Show>
        <Show when={props.onOpenSettings}>
          <button
            class="icon-btn"
            title="设置"
            onClick={() => props.onOpenSettings?.()}
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
              <circle cx="12" cy="12" r="3"></circle>
              <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
            </svg>
          </button>
        </Show>
      </div>
    </header>
  );
}
