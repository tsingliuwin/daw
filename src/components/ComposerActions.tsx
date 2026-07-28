import { Show, For, createSignal, createMemo, onMount, onCleanup } from "solid-js";
import type { ModelOption } from "../lib/types";
import { modelKeyOf, modelIdOfKey } from "../lib/types";

/**
 * 输入框工具栏共用组件：模型选择 + 优先级 + 确认模式 + 发送/停止按钮。
 *
 * 以对话页（ChatView）的 pill-btn 自定义下拉为标准实现，首页（HomeView）也用它，
 * 保证两边的四个工具完全一致。发送按钮通过可选 streaming/onStop prop 处理
 * 对话页的"流式中切换为停止"逻辑——HomeView 不传这两个 prop，永远走发送分支。
 *
 * CSS 复用 chat-composer__pill-btn / chat-composer__send（已验证的样式）。
 */
export default function ComposerActions(props: {
  availableModels: ModelOption[];
  selectedModel: string;
  onSelectModel: (model: string) => void;
  selectedPriority: string;
  onSelectPriority: (priority: string) => void;
  selectedConfirm: string;
  onSelectConfirm: (mode: string) => void;
  /** 发送按钮是否可用（true=可点）。 */
  canSend: boolean;
  onSend: () => void;
  /** 流式中显示停止按钮。HomeView 不传 → 永远显示发送。 */
  streaming?: boolean;
  onStop?: () => void;
}) {
  const [modelDropdownOpen, setModelDropdownOpen] = createSignal(false);
  const [priorityDropdownOpen, setPriorityDropdownOpen] = createSignal(false);
  const [confirmDropdownOpen, setConfirmDropdownOpen] = createSignal(false);

  // 把可选模型按 provider 分组。
  const groupedModels = createMemo(() => {
    const map = new Map<string, { providerName: string; models: ModelOption[] }>();
    for (const m of props.availableModels) {
      const g = map.get(m.providerId) ?? { providerName: m.providerName, models: [] };
      g.models.push(m);
      map.set(m.providerId, g);
    }
    return [...map.values()];
  });

  let modelRef: HTMLDivElement | undefined;
  let priorityRef: HTMLDivElement | undefined;
  let confirmRef: HTMLDivElement | undefined;

  const handleClickOutside = (e: MouseEvent) => {
    if (modelRef && !modelRef.contains(e.target as Node)) setModelDropdownOpen(false);
    if (priorityRef && !priorityRef.contains(e.target as Node)) setPriorityDropdownOpen(false);
    if (confirmRef && !confirmRef.contains(e.target as Node)) setConfirmDropdownOpen(false);
  };

  onMount(() => {
    document.addEventListener("mousedown", handleClickOutside);
    onCleanup(() => document.removeEventListener("mousedown", handleClickOutside));
  });

  return (
    <>
      {/* 模型选择 */}
      <div class="dropdown-wrapper" ref={modelRef} style="position: relative;">
        <button
          class="chat-composer__pill-btn select-btn chat-composer__model-btn"
          onClick={() => setModelDropdownOpen(!modelDropdownOpen())}
        >
          <span class="btn-prefix">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px;">
              <path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41" />
              <circle cx="12" cy="12" r="4" />
            </svg>
          </span>
          <span>{props.selectedModel ? modelIdOfKey(props.selectedModel) : "选择模型"}</span>
          <span class="btn-caret">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" style="width: 8px; height: 8px;">
              <polyline points="6 9 12 15 18 9"></polyline>
            </svg>
          </span>
        </button>
        <Show when={modelDropdownOpen()}>
          <div class="custom-dropdown-list">
            <Show
              when={props.availableModels.length > 0}
              fallback={
                <div class="dropdown-item muted" style="font-size: 11px; pointer-events: none; padding: 6px 12px;">
                  无可用模型
                </div>
              }
            >
              <For each={groupedModels()}>
                {(group) => (
                  <>
                    <div class="dropdown-group-label">{group.providerName}</div>
                    <For each={group.models}>
                      {(m) => {
                        const isSelected = () => modelKeyOf(m) === props.selectedModel;
                        return (
                          <button
                            class="dropdown-item"
                            classList={{ selected: isSelected() }}
                            title={`${m.providerName} · ${m.modelId}`}
                            onClick={() => { props.onSelectModel(modelKeyOf(m)); setModelDropdownOpen(false); }}
                          >
                            {m.modelId}
                          </button>
                        );
                      }}
                    </For>
                  </>
                )}
              </For>
            </Show>
          </div>
        </Show>
      </div>

      {/* 优先级选择 */}
      <div class="dropdown-wrapper" ref={priorityRef} style="position: relative;">
        <button
          class="chat-composer__pill-btn select-btn chat-composer__priority-btn"
          onClick={() => setPriorityDropdownOpen(!priorityDropdownOpen())}
        >
          <span class="btn-prefix">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px;">
              <path d="M9.5 2A2.5 2.5 0 0 1 12 4.5v15a2.5 2.5 0 0 1-4.96-.44 2.5 2.5 0 0 1 0-3.12 3 3 0 0 1 0-4.88 2.5 2.5 0 0 1 0-3.12A2.5 2.5 0 0 1 9.5 2Z" />
              <path d="M14.5 2A2.5 2.5 0 0 0 12 4.5v15a2.5 2.5 0 0 0 4.96-.44 2.5 2.5 0 0 0 0-3.12 3 3 0 0 0 0-4.88 2.5 2.5 0 0 0 0-3.12A2.5 2.5 0 0 0 14.5 2Z" />
            </svg>
          </span>
          <span>{props.selectedPriority}</span>
          <span class="btn-caret">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" style="width: 8px; height: 8px;">
              <polyline points="6 9 12 15 18 9"></polyline>
            </svg>
          </span>
        </button>
        <Show when={priorityDropdownOpen()}>
          <div class="custom-dropdown-list fit-trigger">
            <button class="dropdown-item" onClick={() => { props.onSelectPriority("最高"); setPriorityDropdownOpen(false); }}>最高</button>
            <button class="dropdown-item" onClick={() => { props.onSelectPriority("均衡"); setPriorityDropdownOpen(false); }}>均衡</button>
            <button class="dropdown-item" onClick={() => { props.onSelectPriority("最快"); setPriorityDropdownOpen(false); }}>最快</button>
          </div>
        </Show>
      </div>

      {/* 确认模式选择 */}
      <div class="dropdown-wrapper" ref={confirmRef} style="position: relative;">
        <button
          class="chat-composer__pill-btn select-btn chat-composer__confirm-btn"
          onClick={() => setConfirmDropdownOpen(!confirmDropdownOpen())}
        >
          <span class="btn-prefix">
            {props.selectedConfirm === "自动执行" ? (
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px;">
                <polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2"></polygon>
              </svg>
            ) : (
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px;">
                <path d="M9 11V6a2 2 0 0 1 4 0v5"></path>
                <path d="M13 6a2 2 0 0 1 4 0v5"></path>
                <path d="M17 6a2 2 0 0 1 4 0v8a8 8 0 0 1-8 8h-2c-2.8 0-4.5-.86-5.99-2.34l-3.6-3.6a2 2 0 0 1 2.83-2.82L7 15"></path>
              </svg>
            )}
          </span>
          <span>{props.selectedConfirm}</span>
          <span class="btn-caret">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" style="width: 8px; height: 8px;">
              <polyline points="6 9 12 15 18 9"></polyline>
            </svg>
          </span>
        </button>
        <Show when={confirmDropdownOpen()}>
          <div class="custom-dropdown-list fit-trigger">
            <button class="dropdown-item" onClick={() => { props.onSelectConfirm("变更前确认"); setConfirmDropdownOpen(false); }}>
              <span class="btn-prefix">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 13px; height: 13px;">
                  <path d="M9 11V6a2 2 0 0 1 4 0v5"></path>
                  <path d="M13 6a2 2 0 0 1 4 0v5"></path>
                  <path d="M17 6a2 2 0 0 1 4 0v8a8 8 0 0 1-8 8h-2c-2.8 0-4.5-.86-5.99-2.34l-3.6-3.6a2 2 0 0 1 2.83-2.82L7 15"></path>
                </svg>
              </span> 变更前确认
            </button>
            <button class="dropdown-item" onClick={() => { props.onSelectConfirm("自动执行"); setConfirmDropdownOpen(false); }}>
              <span class="btn-prefix">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 13px; height: 13px;">
                  <polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2"></polygon>
                </svg>
              </span> 自动执行
            </button>
          </div>
        </Show>
      </div>

      {/* 发送 / 停止按钮 */}
      <Show
        when={!props.streaming}
        fallback={
          <button
            class="chat-composer__send chat-composer__send--stop"
            onClick={() => props.onStop?.()}
            title="停止生成"
          >
            <svg viewBox="0 0 24 24" fill="currentColor" style="width: 12px; height: 12px;">
              <rect x="6" y="6" width="12" height="12" rx="2" />
            </svg>
          </button>
        }
      >
        <button
          class="chat-composer__send"
          disabled={!props.canSend}
          onClick={() => props.onSend()}
          title="发送"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 16px; height: 16px;">
            <line x1="22" y1="2" x2="11" y2="13"></line>
            <polygon points="22 2 15 22 11 13 2 9 22 2"></polygon>
          </svg>
        </button>
      </Show>
    </>
  );
}
