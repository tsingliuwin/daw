/**
 * 共用的底部品牌区：左侧「研途教育 AI OA」品牌名 + 右侧按钮。
 *
 * 主界面 LeftNav 和设置页 SettingsPage 都用它，保证品牌区样式一致、单一数据源
 * （品牌名只在这里改一次）。右侧按钮二选一：
 *  - `onOpenSettings`：齿轮图标，打开设置页（主界面 LeftNav 用）。
 *  - `onCloseSettings`：返回箭头图标，关闭设置页回到首页（设置页用，等同于右上角
 *    的 ✕ 关闭按钮，但鼠标不用移到右上角）。
 * 两者都不传时不渲染按钮。
 */
export default function BrandFooter(props: {
  onOpenSettings?: () => void;
  onCloseSettings?: () => void;
}) {
  return (
    <div class="ln-footer">
      <span class="ln-brand-name">研途教育 AI OA</span>
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
          {/* 左箭头：表示「返回/关闭设置」方向，和主界面打开设置的齿轮图标区分 */}
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 16px; height: 16px;">
            <line x1="19" y1="12" x2="5" y2="12"></line>
            <polyline points="12 19 5 12 12 5"></polyline>
          </svg>
        </button>
      )}
    </div>
  );
}
