import { For, Show, createMemo, createSignal, onCleanup } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ModelOption, Workspace } from "../lib/types";
import { logoSrc, brand } from "../lib/brand";
import Select from "./Select";
import ComposerActions from "./ComposerActions";

/** 工作区下拉末尾「选择新文件夹...」项的占位 value（不会是合法路径）。 */
const NEW_FOLDER_VALUE = "__new_folder__";

/**
 * 场景配置。结构固定（task / data_analysis 两个 id，决定 agent 工具集与
 * preamble），文案来自 `~/.daw/brand.json`（见 lib/brand.ts），可整站定制。
 */
const scenarios = () => [
  { id: "task" as const, ...brand().home.task },
  { id: "data_analysis" as const, ...brand().home.data_analysis },
];

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
  /** 当前选中的场景（决定 agent 工具集和 preamble）。 */
  selectedScenario: "task" | "data_analysis";
  /** 切换场景。 */
  onSelectScenario: (s: "task" | "data_analysis") => void;
  /** 数据分析环境是否就绪（DuckLake 扩展已安装）。 */
  dataAnalysisReady: boolean;
  /** 数据分析环境就绪状态变化时通知父级。 */
  onDataAnalysisReadyChange: () => void;
  /** 用户在首页输入并发送第一条消息。App 负责新建 task + 触发 agent。 */
  onSubmit: (prompt: string) => void;
  /** 打开设置页（模型未配置时的引导按钮）。 */
  onOpenSettings: () => void;
}) {
  const [input, setInput] = createSignal("");
  // 数据分析环境安装状态。
  const [installStep, setInstallStep] = createSignal<{ step: string; message: string } | null>(null);

  const hasModels = createMemo(() => props.availableModels.length > 0);
  const canSend = createMemo(() => {
    if (!input().trim() || !hasModels()) return false;
    // 数据分析场景需要环境就绪。
    if (props.selectedScenario === "data_analysis" && !props.dataAnalysisReady) return false;
    return true;
  });

  // 启用数据分析环境。
  const handleEnableDataAnalysis = async () => {
    setInstallStep({ step: "starting", message: "正在启动安装…" });
    const unlisten = await listen<{ step: string; message: string }>("ducklake-install", (event) => {
      const payload = event.payload;
      setInstallStep({ step: payload.step, message: payload.message });
      if (payload.step === "done") {
        props.onDataAnalysisReadyChange();
        setInstallStep(null);
      }
    });
    try {
      await invoke("install_data_analysis_env");
    } catch (err) {
      setInstallStep({ step: "error", message: String(err) });
    }
    // 安装完成后或出错时清理 listener（done/error 已在上面处理）。
    onCleanup(() => { unlisten(); });
  };

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
          <img src={logoSrc()} alt={brand().app_name} class="home-view__logo" />
          <h1 class="home-view__title">{brand().home.welcome_title || brand().app_name}</h1>
          <p class="home-view__welcome-sub">{brand().home.welcome_subtitle || brand().tagline}</p>
          <div class="home-view__scenario-group">
            <For each={scenarios()}>
              {(s) => (
                <button
                  class="home-view__scenario-btn"
                  classList={{ active: props.selectedScenario === s.id }}
                  onClick={() => props.onSelectScenario(s.id)}
                >
                  {s.label}
                </button>
              )}
            </For>
          </div>
          <p class="home-view__subtitle">
            {scenarios().find((s) => s.id === props.selectedScenario)?.subtitle ?? scenarios()[0].subtitle}
          </p>
        </div>

        {/* 数据分析环境未就绪时的启用面板 */}
        <Show when={props.selectedScenario === "data_analysis" && !props.dataAnalysisReady}>
          <div class="home-view__enable-panel">
            <Show when={!installStep()}>
              <p class="home-view__enable-hint">数据分析需要安装 DuckLake 扩展，点击下方按钮启用。</p>
              <button class="home-view__enable-btn" onClick={() => void handleEnableDataAnalysis()}>
                启用数据分析
              </button>
            </Show>
            <Show when={installStep() && installStep()!.step !== "done" && installStep()!.step !== "error"}>
              <div class="home-view__install-progress">
                <span class="home-view__install-spinner" />
                <span class="home-view__install-msg">{installStep()!.message}</span>
              </div>
            </Show>
            <Show when={installStep()?.step === "error"}>
              <div class="home-view__install-error">
                <span>✕ 安装失败：{installStep()!.message}</span>
                <button class="home-view__enable-btn" onClick={() => void handleEnableDataAnalysis()}>
                  重试
                </button>
              </div>
            </Show>
          </div>
        </Show>

        {/* 输入框 */}
        <div class="home-composer">
          <textarea
            class="home-composer__input"
            placeholder={
              hasModels()
                ? (scenarios().find((s) => s.id === props.selectedScenario)?.placeholder ?? scenarios()[0].placeholder)
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
              <ComposerActions
                availableModels={props.availableModels}
                selectedModel={props.selectedModel}
                onSelectModel={props.onSelectModel}
                selectedPriority={props.selectedPriority}
                onSelectPriority={props.onSelectPriority}
                selectedConfirm={props.selectedConfirm}
                onSelectConfirm={props.onSelectConfirm}
                canSend={canSend()}
                onSend={() => submit()}
              />
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
