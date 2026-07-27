import { For, Show } from "solid-js";
import type { Workspace, Task } from "../lib/types";
import { logoSrc, currentTheme } from "../lib/theme";
import BrandFooter from "./BrandFooter";

const isMac = typeof navigator !== "undefined" && navigator.userAgent.includes("Mac");

/**
 * 左侧导航（重写版）。相比 lakemind 删掉了全部文件树 / 数据树 / 数据库连接 / 更新
 * 徽标 / 多种新建动作逻辑，只保留 OA 任务所需的最小集合：
 *   - 工作区折叠分组（多工作区并存，非下拉切换）：每组 header = ▶/▼ 三角 +
 *     工作区名 + 右侧「+」按钮（新建任务）；展开时渲染该工作区下的任务项。
 *   - 任务项：点击选中、可删除，active 高亮。
 * 风格沿用 lakemind 的 CSS 类名（ln-*）与深色变量。
 */
export default function LeftNav(props: {
  workspaces: Workspace[];
  /** 按 workspace.path 分组的任务列表。 */
  tasksByWorkspace: Record<string, Task[]>;
  /** 工作区折叠态（path → true 表示折叠）。 */
  collapsedWs: Record<string, boolean>;
  activeTaskId: string | undefined;
  /** 切换某工作区的折叠态。 */
  onToggleWorkspace: (wsPath: string) => void;
  onSelectTask: (id: string) => void;
  onDeleteTask: (id: string) => void;
  /** 在指定工作区下新建任务。 */
  onNewTask: (wsPath: string) => void;
  busy?: boolean;
  leftOpen: boolean;
  onToggleLeft: () => void;
  /** 打开设置页。底部品牌区右侧的设置按钮触发。 */
  onOpenSettings: () => void;
  /** 快捷切换主题（深色 ↔ 浅色）。底部品牌区主题按钮触发。 */
  onToggleTheme: () => void;
}) {
  return (
    <div class="left-nav">
      {/* 顶部：logo（非 mac）+ 侧边栏折叠按钮 */}
      <div class="ln-top-bar" classList={{ "mac-nav": isMac }}>
        <Show when={!isMac}>
          <div class="ln-logo-box" title="AI OA">
            <img src={logoSrc()} alt="AI OA" style="width: 18px; height: 18px; object-fit: contain;" />
          </div>
        </Show>
        <div class="ln-nav-arrows" data-tauri-drag-region>
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
      </div>

      {/* 新建任务快捷按钮（常驻顶部，不依赖具体工作区展开态） */}
      <div class="ln-quick-actions">
        <button
          class="ln-action-btn"
          title="新建任务"
          onClick={() => {
            // 归属第一个工作区（首启必有 DefaultProject）；后续可让用户选目标工作区。
            const wsPath = props.workspaces[0]?.path;
            if (wsPath) props.onNewTask(wsPath);
          }}
          disabled={props.busy || props.workspaces.length === 0}
        >
          <span class="action-icon">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <line x1="12" y1="5" x2="12" y2="19"></line>
              <line x1="5" y1="12" x2="19" y2="12"></line>
            </svg>
          </span>
          <span class="action-label">新建任务</span>
        </button>
      </div>

      {/* 任务区：按工作区折叠分组 */}
      <div class="ln-section-header">
        <span class="section-title">任务</span>
      </div>
      <div class="ln-task-list">
        <Show
          when={props.workspaces.length > 0}
          fallback={<div class="empty-hint">尚未添加工作区</div>}
        >
          <For each={props.workspaces}>
            {(ws) => {
              const collapsed = () => !!props.collapsedWs[ws.path];
              const tasks = () => props.tasksByWorkspace[ws.path] ?? [];
              return (
                <div class="ln-workspace-group">
                  <div class="ln-ws-header" onClick={() => props.onToggleWorkspace(ws.path)}>
                    <span class="ln-ws-toggle">{collapsed() ? "▶" : "▼"}</span>
                    <span class="ln-ws-name">{ws.name}</span>
                    <button
                      class="ln-ws-new"
                      title="新建任务"
                      onClick={(e) => { e.stopPropagation(); props.onNewTask(ws.path); }}
                      disabled={props.busy}
                    >+</button>
                  </div>
                  <Show when={!collapsed()}>
                    <For each={tasks()} fallback={<div class="empty-hint">暂无任务</div>}>
                      {(task) => (
                        <div
                          class="ln-task-item"
                          classList={{ active: task.id === props.activeTaskId }}
                          onClick={() => props.onSelectTask(task.id)}
                        >
                          <span class="ln-task-name">{task.name}</span>
                          <button
                            class="ln-task-delete"
                            title="删除任务"
                            onClick={(e) => { e.stopPropagation(); props.onDeleteTask(task.id); }}
                          >
                            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 11px; height: 11px;">
                              <polyline points="3 6 5 6 21 6"></polyline>
                              <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"></path>
                            </svg>
                          </button>
                        </div>
                      )}
                    </For>
                  </Show>
                </div>
              );
            }}
          </For>
        </Show>
      </div>

      <BrandFooter
        onToggleTheme={props.onToggleTheme}
        isDarkTheme={currentTheme() !== "light"}
        onOpenSettings={props.onOpenSettings}
      />
    </div>
  );
}
