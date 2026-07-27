import { For, Show, createMemo } from "solid-js";
import type { Workspace, QueryTask } from "../lib/types";
import { logoSrc } from "../lib/theme";

const isMac = typeof navigator !== "undefined" && navigator.userAgent.includes("Mac");

/**
 * 左侧导航（重写版）。相比 lakemind 删掉了全部文件树 / 数据树 / 数据库连接 / 更新
 * 徽标 / 多种新建动作逻辑，只保留 OA 对话所需的最小集合：
 *   - 工作区选择（下拉 + 切换）
 *   - 当前工作区下的对话任务列表（来自 load_workspace_tasks），点击选中、可删除
 *   - 「新建对话」按钮
 * 风格沿用 lakemind 的 CSS 类名（ln-*）与深色变量。
 */
export default function LeftNav(props: {
  workspaces: Workspace[];
  currentWorkspacePath: string;
  onSelectWorkspace: (path: string) => void;
  tasks: QueryTask[];
  selectedTaskId: string | undefined;
  onSelectTask: (id: string) => void;
  onDeleteTask: (id: string) => void;
  onNewChat: () => void;
  busy?: boolean;
  leftOpen: boolean;
  onToggleLeft: () => void;
}) {
  const currentWorkspace = createMemo(
    () => props.workspaces.find((w) => w.path === props.currentWorkspacePath) ?? props.workspaces[0],
  );

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

      {/* 新建对话快捷动作 */}
      <div class="ln-quick-actions">
        <button class="ln-action-btn" title="新建对话" onClick={() => props.onNewChat()} disabled={props.busy}>
          <span class="action-icon">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M21 11.5a8.38 8.38 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.38 8.38 0 0 1-3.8-.9L3 21l1.9-5.7a8.38 8.38 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.38 8.38 0 0 1 3.8-.9h.5a8.48 8.48 0 0 1 8 8v.5z" />
            </svg>
          </span>
          <span class="action-label">新建对话</span>
        </button>
      </div>

      {/* 工作区选择 */}
      <div class="ln-section-header">
        <span class="section-title">工作区</span>
      </div>
      <div class="ln-workspace-select">
        <Show
          when={props.workspaces.length > 0}
          fallback={<div class="empty-hint">尚未添加工作区</div>}
        >
          <select
            class="ln-workspace-dropdown"
            value={currentWorkspace()?.path ?? ""}
            onChange={(e) => props.onSelectWorkspace(e.currentTarget.value)}
          >
            <For each={props.workspaces}>{(w) => <option value={w.path}>{w.name}</option>}</For>
          </select>
        </Show>
      </div>

      {/* 对话任务列表 */}
      <div class="ln-section-header">
        <span class="section-title">对话</span>
      </div>
      <div class="ln-task-list">
        <For each={props.tasks} fallback={<div class="empty-hint">暂无对话</div>}>
          {(task) => (
            <div
              class="ln-task-item"
              classList={{ active: task.id === props.selectedTaskId }}
              onClick={() => props.onSelectTask(task.id)}
            >
              <span class="ln-task-name">{task.name}</span>
              <button
                class="ln-task-delete"
                title="删除对话"
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
      </div>

      <div class="ln-footer">
        <span class="ln-brand-name">AI OA</span>
      </div>
    </div>
  );
}
