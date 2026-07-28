import { Show, createMemo, createSignal } from "solid-js";
import type { ModelOption, Workspace } from "../lib/types";
import { logoSrc } from "../lib/theme";
import Select from "./Select";

/** 工作区下拉末尾「选择新文件夹...」项的占位 value（不会是合法路径）。 */
const NEW_FOLDER_VALUE = "__new_folder__";

/**
 * 首页/欢迎页：居中的大输入框。
 *
 * 与 ChatView 的区别：ChatView 是已进入某个任务后的「消息流 + 底部常驻输入框」；
 * HomeView 是用户还没发起任何任务时的着陆页——一个居中的、占视觉重心的输入框，
 * 用户在这里敲下第一条消息后才正式进入任务流（App 据此新建 task 并切到 ChatView）。
 *
 * 复用 App 已有的模型/优先级/确认模式选择器状态：首页就能选好模型，发出去的就是
 * 选中的那个，避免进了任务页才发现模型不对。
 *
 * 工具栏新增「工作区」下拉（与模型/优先级/确认模式并列）：列出历史工作区 +
 * 末尾「选择新文件夹...」入口（调后端 select_directory）。新任务归属 = 此处选中的
 * 工作区路径（由 App 通过 selectedWorkspacePath/onSelectWorkspace/onSelectNewFolder
 * 注入）。
 */
export default function HomeView(props: {
  /** 历史工作区列表（App 注入）。 */
  workspaces: Workspace[];
  /** 当前首页选中的工作区 path。 */
  selectedWorkspacePath: string;
  /** 选择某个历史工作区时触发。 */
  onSelectWorkspace: (path: string) => void;
  /** 点击「选择新文件夹...」时触发（App 调 select_directory + add_workspace）。 */
  onSelectNewFolder: () => void;
  availableModels: ModelOption[];
  selectedModel: string;
  onSelectModel: (model: string) => void;
  selectedPriority: string;
  onSelectPriority: (priority: string) => void;
  selectedConfirm: string;
  onSelectConfirm: (mode: string) => void;
  /** 用户在首页输入并发送第一条消息。App 负责新建 task + 触发 agent。 */
  onSubmit: (prompt: string) => void;
  /** 打开设置页（模型未配置时的引导按钮）。 */
  onOpenSettings: () => void;
}) {
  const [input, setInput] = createSignal("");

  const hasModels = createMemo(() => props.availableModels.length > 0);
  const canSend = createMemo(() => input().trim().length > 0 && hasModels());

  // 模型选项转成 Select 需要的 {value,label}；value 用 providerId:modelId 复合键。
  const modelOptions = createMemo(() =>
    props.availableModels.map((m) => ({
      value: `${m.providerId}:${m.modelId}`,
      label: m.modelId,
    })),
  );

  // 工作区选项 = 历史工作区 + 末尾「选择新文件夹...」特殊项。
  const workspaceOptions = createMemo(() => [
    ...props.workspaces.map((ws) => ({ value: ws.path, label: ws.name })),
    { value: NEW_FOLDER_VALUE, label: "选择新文件夹..." },
  ]);

  // 工作区下拉变化：特殊项 → 打开目录选择器；普通项 → 切换首页选中工作区。
  function onWorkspaceChange(value: string) {
    if (value === NEW_FOLDER_VALUE) {
      props.onSelectNewFolder();
    } else {
      props.onSelectWorkspace(value);
    }
  }

  const PRIORITY_OPTIONS = [
    { value: "均衡", label: "均衡" },
    { value: "最高", label: "最高" },
    { value: "最快", label: "最快" },
  ] as const;
  const CONFIRM_OPTIONS = [
    { value: "变更前确认", label: "变更前确认" },
    { value: "自动执行", label: "自动执行" },
  ] as const;

  function submit() {
    const text = input().trim();
    if (!text || !hasModels()) return;
    setInput("");
    props.onSubmit(text);
  }

  function onKeydown(e: KeyboardEvent) {
    // Enter 发送，Shift+Enter 换行（与 ChatView 一致）。
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      submit();
    }
  }

  return (
    <div class="home-view">
      <div class="home-view__inner">
        {/* 品牌 */}
        <div class="home-view__brand">
          <img src={logoSrc()} alt="AI OA" class="home-view__logo" />
          <h1 class="home-view__title">AI OA</h1>
          <p class="home-view__subtitle">
            用任务完成请假、报销、审批——不必再翻系统、填表单。
          </p>
        </div>

        {/* 输入框 */}
        <div class="home-composer">
          <textarea
            class="home-composer__input"
            placeholder={
              hasModels()
                ? "试试：「帮我查一下年假余额」或「我下周三请两天假，家里有事」"
                : "请先在设置中配置大模型服务商…"
            }
            value={input()}
            rows={3}
            onInput={(e) => setInput(e.currentTarget.value)}
            onKeyDown={onKeydown}
          />
          <div class="home-composer__toolbar">
            {/* 工作区选择器：靠左 */}
            <Select
              width="150px"
              value={props.selectedWorkspacePath}
              options={workspaceOptions()}
              onChange={onWorkspaceChange}
            />
            {/* 模型/优先级/确认 + 发送：靠右 */}
            <div class="home-composer__selectors">
              <Select
                width="160px"
                value={props.selectedModel}
                options={modelOptions()}
                onChange={props.onSelectModel}
                disabled={!hasModels()}
              />
              <Select
                width="70px"
                value={props.selectedPriority}
                options={PRIORITY_OPTIONS}
                onChange={props.onSelectPriority}
              />
              <Select
                width="110px"
                value={props.selectedConfirm}
                options={CONFIRM_OPTIONS}
                onChange={props.onSelectConfirm}
              />
              <button
                class="home-composer__send"
                title="发送（Enter）"
                disabled={!canSend()}
                onClick={() => submit()}
              >
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 16px; height: 16px;">
                  <line x1="22" y1="2" x2="11" y2="13"></line>
                  <polygon points="22 2 15 22 11 13 2 9 22 2"></polygon>
                </svg>
              </button>
            </div>
          </div>
        </div>

        {/* 模型未配置引导 */}
        <div class="home-view__examples">
          <Show when={!hasModels()}>
            <button class="home-view__config-btn" onClick={() => props.onOpenSettings()}>
              ⚙ 打开设置配置模型
            </button>
          </Show>
        </div>
      </div>
    </div>
  );
}
