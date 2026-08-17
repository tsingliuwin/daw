import { createSignal, Show, For, onMount, onCleanup, createMemo } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { Workspace, Task, ChatMessage, ModelOption, RetryNotice } from "./lib/types";
import { modelKeyOf, modelIdOfKey, providerIdOfKey } from "./lib/types";
import { appendDelta, pushToolCall, mergeToolResult, normalizeMessage, newSegmentId } from "./lib/chat";
import { mergeUsage } from "./lib/metrics";
import { logsSignal, logError, installAppLogListener, clearLogsStore } from "./lib/logger";
import { persistTheme, currentTheme, loadThemeFromBackend, type Theme } from "./lib/theme";
import TitleBar from "./components/TitleBar";
import LeftNav from "./components/LeftNav";
import ChatView from "./components/ChatView";
import HomeView from "./components/HomeView";
import SettingsPage from "./components/SettingsPage";

/**
 * 应用主布局与状态中枢。相比 lakemind 大幅精简：
 *   - 删除所有 SQL 编辑器、结果表、右侧 inspector、文件拖拽、HomePanel、
 *     数据树、数据库连接、import 监听、chart 段处理等。
 *   - 保留 task 相关的状态管理与流式聚合：workspaces / tasksByWorkspace /
 *     activeTaskId / streamingTaskId / chatMessages / 可用模型 / 选择器。
 *
 * 工作区与任务的关系从「单工作区下拉切换 + 平铺任务列表」改为「多工作区折叠
 * 分组 + 懒加载」：tasksByWorkspace 按 workspace.path 分组；collapsedWs 记录
 * 每个工作区是否折叠（localStorage 持久化 key `ws_collapsed`，默认全部展开）；
 * loadedWs 记录已懒加载过的工作区，避免重复 load。
 *
 * 布局：TitleBar 顶部；下方水平 flex = LeftNav（可折叠）+ 主区（ChatView 占满）
 * + 可折叠的日志抽屉（BottomConsole 风格，展示 logsSignal）。
 */
export default function App() {
  // ── 工作区与任务（按工作区分组） ──
  const [workspaces, setWorkspaces] = createSignal<Workspace[]>([]);
  const [tasksByWorkspace, setTasksByWorkspace] = createSignal<Record<string, Task[]>>({});
  const [collapsedWs, setCollapsedWs] = createSignal<Record<string, boolean>>({});
  // 已懒加载过的工作区集合（普通 Set，不需要响应式）：首次 onMount 时全部加载，
  // 后续 toggleWorkspace 触发的懒加载据此去重。
  const loadedWs = new Set<string>();
  const [activeTaskId, setActiveTaskId] = createSignal<string | null>(null);
  // 首页选中的工作区 path：新建任务的归属工作区由此决定（onMount 时设为
  // workspace.last / DefaultProject / 第一个工作区）。Select 组件的 value。
  const [homeWorkspacePath, setHomeWorkspacePath] = createSignal<string>("");
  // 当前正在流式输出的任务 id。start_agent_task 是 fire-and-forget
  // （tokio::spawn 后立即返回），真正的流式通过 agent-event 异步回来，
  // streamingTaskId 用于在流式期间锁定输入、显示 Stop 按钮。
  const [streamingTaskId, setStreamingTaskId] = createSignal<string | null>(null);
  // 速率限制重试提示（瞬时状态，按任务 id 键控）：kind="retry" 事件写入，
  // 后续任何内容事件（text/reasoning/tool/done 等）到达时清除。
  const [retryNotices, setRetryNotices] = createSignal<Record<string, RetryNotice>>({});
  // ── 模型与选择器 ──
  const [availableModels, setAvailableModels] = createSignal<ModelOption[]>([]);
  const [modelCtxWindows, setModelCtxWindows] = createSignal<Record<string, number>>({});
  const [selectedModel, setSelectedModel] = createSignal<string>("");
  const [selectedPriority, setSelectedPriority] = createSignal<string>("均衡");
  const [selectedConfirm, setSelectedConfirm] = createSignal<string>("变更前确认");

  // ── 布局状态 ──
  const [leftOpen, setLeftOpen] = createSignal<boolean>(true);
  const [consoleOpen, setConsoleOpen] = createSignal<boolean>(false);
  const [settingsOpen, setSettingsOpen] = createSignal<boolean>(false);
  const [busy, setBusy] = createSignal<boolean>(false);
  /** 暂存"新建任务"时选择的场景（由 LeftNav 按钮设置，submitTaskFromHome 消费）。 */
  const [pendingScenario, setPendingScenario] = createSignal<"task" | "data_analysis">("task");
  /** 数据分析环境是否就绪（DuckLake 扩展已安装）。 */
  const [dataAnalysisReady, setDataAnalysisReady] = createSignal(false);

  // 当前活跃工作区路径（供 SettingsPage 使用，避免在 prop 表达式里做响应式读取）。
  const activeWorkspacePath = createMemo(() => {
    const id = activeTaskId();
    if (id) {
      const ws = findTaskWorkspace(id);
      if (ws) return ws;
    }
    return homeWorkspacePath();
  });

  // 在 tasksByWorkspace 中查找 taskId 所属的工作区路径（不存在返回 null）。
  function findTaskWorkspace(taskId: string): string | null {
    const all = tasksByWorkspace();
    for (const [wsPath, arr] of Object.entries(all)) {
      if (arr.some((t) => t.id === taskId)) return wsPath;
    }
    return null;
  }

  const activeTask = createMemo(() => {
    const id = activeTaskId();
    if (!id) return null;
    for (const arr of Object.values(tasksByWorkspace())) {
      const found = arr.find((t) => t.id === id);
      if (found) return found;
    }
    return null;
  });

  // 「已进入任务流」的判定：选中的 task 必须存在且有至少一条消息。
  // 满足时主区渲染 ChatView；否则渲染 HomeView 欢迎页（首次进入、点了「新建任务」
  // 但还没发消息、删光所有任务等情况都会落到 HomeView）。
  const chatTask = createMemo(() => {
    const t = activeTask();
    if (!t) return null;
    return (t.messages?.length ?? 0) > 0 ? t : null;
  });

  // 当前任务所在工作区的展示名（用于 HomeView / ChatView 的 workspace prop）。
  // activeTask 为空时退回第一个工作区名。
  const activeWorkspaceName = createMemo(() => {
    const t = activeTask();
    if (t) {
      const wsPath = findTaskWorkspace(t.id);
      if (wsPath) {
        const ws = workspaces().find((w) => w.path === wsPath);
        if (ws) return ws.name || ws.path;
      }
    }
    const first = workspaces()[0];
    return first ? first.name || first.path : "";
  });

  // 加载 settings.json → 收集可用模型为可选项。
  async function loadModelsFromSettings() {
    try {
      const json = await invoke<string>("load_settings_json");
      if (!json || json === "{}") return;
      const loaded = JSON.parse(json);
      const models: ModelOption[] = [];
      const ctxMap: Record<string, number> = {};

      if (loaded.providers) {
        for (const prov of loaded.providers) {
          if (prov.enabled && prov.models) {
            for (const m of prov.models) {
              if (!m.id || !m.id.trim()) continue; // 跳过空 id 的模型行（未填完）
              const opt: ModelOption = {
                providerId: prov.id,
                providerName: prov.name || prov.id,
                modelId: m.id,
                contextWindow: m.contextWindow,
              };
              models.push(opt);
              if (m.contextWindow) ctxMap[modelKeyOf(opt)] = m.contextWindow;
            }
          }
        }
      }

      setAvailableModels(models);
      setModelCtxWindows(ctxMap);

      const keys = models.map(modelKeyOf);
      const savedDefault = localStorage.getItem("default_model");
      if (models.length > 0) {
        if (savedDefault && keys.includes(savedDefault)) {
          setSelectedModel(savedDefault);
        } else if (!selectedModel() || !keys.includes(selectedModel())) {
          setSelectedModel(keys[0]);
        }
      } else {
        setSelectedModel("");
      }
    } catch (err) {
      logError("ui", "Failed to load models from settings", err);
    }
  }

  // 懒加载某个工作区的任务列表（已加载过则跳过）。结果按 createdAt 倒序。
  async function loadWorkspaceTasks(wsPath: string) {
    if (loadedWs.has(wsPath)) return;
    loadedWs.add(wsPath);
    try {
      const loadedTasks = await invoke<Task[]>("load_workspace_tasks", { workspacePath: wsPath, spaceId: "personal", userId: "default" });
      // 归一化历史消息（兼容简单的 content 形态）。
      const migrated = loadedTasks
        .map((t) =>
          Array.isArray(t.messages)
            ? { ...t, messages: t.messages.map((m) => normalizeMessage(m)) }
            : t,
        )
        .sort((a, b) => b.createdAt - a.createdAt);
      setTasksByWorkspace((prev) => ({ ...prev, [wsPath]: migrated }));
    } catch (err) {
      logError("ui", "Failed to load workspace tasks", err);
    }
  }

  // 持久化一个 task 到后端（messages + modelId + tokenUsage）。
  // workspacePath 由调用方传入（task 所在工作区）。
  async function saveTaskBackend(task: Task, workspacePath: string) {
    try {
      await invoke("save_task", {
        workspacePath,
        taskId: task.id,
        name: task.name,
        messages: task.messages ?? [],
        modelId: task.modelId || null,
        tokenUsage: task.tokenUsage ?? null,
        spaceId: "personal",
        userId: "default",
        kind: task.kind ?? "task",
      });
    } catch (err) {
      logError("agent", "Failed to save task to backend", err);
    }
  }

  // 追加一行到 .jsonl（流式输出时实时落盘，防崩溃丢数据）。
  async function appendChatLine(taskId: string, line: unknown) {
    try {
      await invoke("append_chat_line", { taskId, line, spaceId: "personal", userId: "default" });
    } catch (err) {
      logError("agent", "Failed to append chat line", err);
    }
  }

  // 加载工作区列表并为每个工作区加载任务，恢复 workspace.last 决定初始
  // activeTaskId 与首页工作区下拉选中项。onMount 首次加载走此逻辑。
  async function reloadWorkspacesAndTasks() {
    setBusy(true);
    try {
      const list = await invoke<Workspace[]>("load_workspaces");
      if (list && list.length > 0) {
        setWorkspaces(list);
        await Promise.all(list.map((w) => loadWorkspaceTasks(w.path)));

        // 恢复 workspace.last：决定初始 activeTaskId。
        let last: string | null = null;
        try {
          last = await invoke<string | null>("get_app_config", { key: "workspace.last" });
        } catch { /* best-effort */ }
        const lastWs =
          (last && list.find((w) => w.path === last)) ||
          list.find((w) => w.path === "DefaultProject") ||
          list[0];

        setHomeWorkspacePath(lastWs.path);

        // 从 signal 读最新任务列表（Promise.all 已完成，signal 已更新）。
        const lastArr = tasksByWorkspace()[lastWs.path] ?? [];
        if (lastArr.length > 0) {
          setActiveTaskId(lastArr[0].id);
        } else {
          for (const w of list) {
            const arr = tasksByWorkspace()[w.path] ?? [];
            if (arr.length > 0) {
              setActiveTaskId(arr[0].id);
              break;
            }
          }
        }
      } else {
        setWorkspaces([]);
      }
    } catch (err) {
      logError("ui", "Failed to load workspaces", err);
    } finally {
      setBusy(false);
    }
  }

  onMount(async () => {
    // 检查数据分析环境是否就绪。
    try {
      const ready = await invoke<boolean>("check_data_analysis_env");
      setDataAnalysisReady(ready);
    } catch { /* best-effort */ }

    // 恢复工作区折叠态（localStorage）。
    try {
      const saved = localStorage.getItem("ws_collapsed");
      if (saved) setCollapsedWs(JSON.parse(saved));
    } catch { /* best-effort */ }

    // 从后端 config 表恢复主题（ui.theme）。
    void loadThemeFromBackend();

    // 安装 agent-event 监听器：聚合 reasoning/text/tool_call/tool_result/usage/
    // done/error 流进当前 assistant 消息的 segments。
    const unlistenAgent = await listen<any>("agent-event", (event) => {
      const payload = event.payload;
      const targetId = payload.taskId;

      // 速率限制重试提示：瞬时横幅，不进入消息段（避免把提示文字混进答复与
      // 历史回放）。后续任何实质内容事件到达即清除，表示重试已结束。
      if (payload.kind === "retry") {
        setRetryNotices((prev) => ({
          ...prev,
          [targetId]: {
            attempt: payload.attempt ?? 1,
            maxAttempts: payload.maxAttempts ?? 4,
            delaySecs: payload.delaySecs ?? 5,
            at: Date.now(),
          },
        }));
      } else if (payload.kind !== "usage") {
        setRetryNotices((prev) => {
          if (!(targetId in prev)) return prev;
          const next = { ...prev };
          delete next[targetId];
          return next;
        });
      }

      setTasksByWorkspace((prev) => {
        // 找到 targetId 所在的工作区，找不到则 no-op。
        let targetWs: string | null = null;
        for (const [p, arr] of Object.entries(prev)) {
          if (arr.some((t) => t.id === targetId)) {
            targetWs = p;
            break;
          }
        }
        if (targetWs === null) return prev;
        const wsPath = targetWs;
        const oldArr = prev[wsPath] ?? [];
        const newArr = oldArr.map((t) => {
          if (t.id !== targetId) return t;

          let messages = [...(t.messages ?? [])];
          // 懒创建一条 assistant 消息作为最后一条。
          let lastMsg = messages[messages.length - 1];
          if (!lastMsg || lastMsg.role !== "assistant") {
            lastMsg = {
              id: `msg-assistant-${Date.now()}`,
              role: "assistant",
              segments: [],
              ts: Date.now(),
            };
            messages = [...messages, lastMsg];
          }

          let segments = lastMsg.segments ? [...lastMsg.segments] : [];
          const kind = payload.kind as string;
          let updated: Task = t;

          if (kind === "text") {
            segments = appendDelta(segments, "text", payload.text ?? "");
          } else if (kind === "reasoning") {
            segments = appendDelta(segments, "reasoning", payload.text ?? "");
          } else if (kind === "tool_call" && payload.segment) {
            const s = payload.segment;
            segments = pushToolCall(segments, { id: s.id, tool: s.tool, args: s.args });
          } else if (kind === "tool_result" && payload.segment) {
            const s = payload.segment;
            segments = mergeToolResult(segments, {
              id: s.id,
              status: s.status,
              summary: s.summary,
              detail: s.detail,
              payload: s.payload,
              elapsedMs: s.elapsedMs,
              result: s.result,
            });
          } else if (kind === "chart" && payload.segment) {
            // chart segment：push 进消息 segments 列表，ChatView 渲染 ChartSegment。
            const s = payload.segment;
            segments = [...segments, s];
          } else if (kind === "usage" && payload.text) {
            // 通过 mergeUsage 把 usage 事件折叠进 task 的持久化 TokenUsage。
            try {
              const evt = JSON.parse(payload.text);
              updated = { ...updated, tokenUsage: mergeUsage(updated.tokenUsage ?? null, evt) };
            } catch { /* ignore parse error */ }
          } else if (kind === "error") {
            segments = [
              ...segments,
              { type: "error", id: newSegmentId("t"), text: payload.text ?? "未知错误" },
            ];
          }

          messages[messages.length - 1] = { ...lastMsg, segments };

          // tool_result/chart/error 时追加完整 assistant 消息到 .jsonl（实时落盘）。
          // 同一 msg_id 多行，load 时去重取最后一条。
          // text/reasoning delta 太频繁不追加，done 时全量写覆盖。
          if (kind === "tool_result" || kind === "chart" || kind === "error") {
            const assistantMsg = messages[messages.length - 1];
            void appendChatLine(targetId, assistantMsg);
          }

          // done/error：关闭最后一段 reasoning 的 elapsedMs，并落库。
          if (kind === "done" || kind === "error") {
            const lastIdx = segments.length - 1;
            if (lastIdx >= 0) {
              const lastSeg = segments[lastIdx];
              if (lastSeg.type === "reasoning" && lastSeg.startTime && !lastSeg.elapsedMs) {
                segments[lastIdx] = { ...lastSeg, elapsedMs: Date.now() - lastSeg.startTime };
                messages[messages.length - 1] = { ...lastMsg, segments };
              }
            }
          }

          const finalTask: Task = { ...updated, messages };
          if (kind === "done" || kind === "error") {
            void saveTaskBackend(finalTask, wsPath);
          }
          return finalTask;
        });
        return { ...prev, [wsPath]: newArr };
      });

      // 流式结束（成功或出错）：清除执行状态，解除输入锁定。
      if (payload.kind === "done" || payload.kind === "error") {
        setStreamingTaskId(null);
      }
    });

    // 加载工作区列表，并为每个工作区加载任务（首次全部加载，简单可靠）。
    await reloadWorkspacesAndTasks();

    await loadModelsFromSettings();

    // 后端 tracing 事件 → 统一日志 signal（前端日志与后端日志在同一控制台混合展示）。
    const unlistenAppLog = await installAppLogListener();

    // 关闭窗口时若有 streaming 任务，先把内存消息 flush 到 .jsonl 再退出，
    // 防止"对话过程中关窗/重启"丢失用户消息与已流式片段（兜底）。
    const win = getCurrentWindow();
    const unlistenClose = await win.onCloseRequested(async (event) => {
      const sid = streamingTaskId();
      if (!sid) return; // 无流式任务，放行正常关闭
      event.preventDefault();
      const wsPath = findTaskWorkspace(sid);
      const task = wsPath ? tasksByWorkspace()[wsPath]?.find((t) => t.id === sid) : undefined;
      if (task && wsPath) {
        try { await saveTaskBackend(task, wsPath); } catch (err) { logError("agent", "flush on close failed", err); }
      }
      await win.destroy(); // destroy 不再二次触发 close-requested，避免递归
    });
    onCleanup(() => {
      unlistenAppLog();
      unlistenAgent();
      unlistenClose();
    });
  });

  // 切换某工作区折叠态 + 持久化到 localStorage。
  function toggleWorkspace(wsPath: string) {
    setCollapsedWs((prev) => {
      const next = { ...prev, [wsPath]: !prev[wsPath] };
      try {
        localStorage.setItem("ws_collapsed", JSON.stringify(next));
      } catch { /* best-effort */ }
      return next;
    });
  }

  function selectTask(id: string) {
    setActiveTaskId(id);
    // 持久化工作区到 workspace.last，下次启动恢复。
    const ws = findTaskWorkspace(id);
    if (ws) {
      void invoke("set_app_config", { key: "workspace.last", value: ws }).catch(() => {});
    }
  }

  // 「新建任务」按钮：跳回首页（HomeView），不创建空任务。场景选择在首页
  // 通过圆角按钮完成。进入首页时重置场景为"日常办公"，避免上次选择残留。
  // 同时同步当前任务的工作区到首页下拉，保持一致。
  function goToHome() {
    const curWs = activeTaskId() ? findTaskWorkspace(activeTaskId()!) : null;
    if (curWs) setHomeWorkspacePath(curWs);
    setPendingScenario("task");
    setActiveTaskId(null);
  }

  // 切换首页选中的工作区（首页工作区下拉 onChange 触发）。
  function selectWorkspace(path: string) {
    setHomeWorkspacePath(path);
  }

  // 首页「选择新文件夹...」：打开原生目录选择器 → 用文件夹名注册工作区 →
  // 刷新 workspaces → 设为首页选中工作区，并懒加载该工作区的任务列表。
  async function selectNewFolder() {
    try {
      const path = await invoke<string | null>("select_directory");
      if (!path) return; // 用户取消
      // 从路径提取末段作为工作区名（兼容 Windows 反斜杠与 POSIX 斜杠）。
      const segs = path.replace(/\\/g, "/").split("/").filter(Boolean);
      const name = segs.length > 0 ? segs[segs.length - 1] : path;
      await invoke("add_workspace", { name, path });
      const list = await invoke<Workspace[]>("load_workspaces");
      if (list && list.length > 0) {
        setWorkspaces(list);
        const newWs = list.find((w) => w.path === path);
        if (newWs) {
          setHomeWorkspacePath(newWs.path);
          await loadWorkspaceTasks(newWs.path);
        }
      }
    } catch (err) {
      logError("ui", "Failed to select new folder as workspace", err);
    }
  }

  async function deleteTask(id: string) {
    const wsPath = findTaskWorkspace(id);
    if (!wsPath) return;
    const arr = tasksByWorkspace()[wsPath] ?? [];
    const remaining = arr.filter((t) => t.id !== id);
    setTasksByWorkspace((prev) => ({
      ...prev,
      [wsPath]: remaining,
    }));
    if (activeTaskId() === id) {
      if (remaining.length > 0) {
        // 选中相邻的下一个任务，优先有内容的。
        const visible = remaining
          .filter((t) => (t.messages?.length ?? 0) > 0)
          .sort((a, b) => b.createdAt - a.createdAt);
        setActiveTaskId(visible.length > 0 ? visible[0].id : remaining[0].id);
      } else {
        // 该工作区最后一个任务也被删了：回到首页（HomeView），让用户重新开始。
        setActiveTaskId(null);
      }
    }
    try {
      await invoke("delete_task", { taskId: id, spaceId: "personal", userId: "default" });
    } catch (err) {
      logError("ui", "Failed to delete task", err);
    }
  }

  // 首页发送：用首页选中的工作区（homeWorkspacePath，回退到第一个工作区）作为
  // 任务归属，新建 task + 立即发送第一条消息。「新建任务」按钮不再预先建空任务，
  // 所以这里没有「复用空任务」分支了。
  function submitTaskFromHome(prompt: string) {
    if (availableModels().length === 0) {
      alert("请先在设置中配置并启用大模型服务商及模型。");
      setSettingsOpen(true);
      return;
    }
    const wsPath = homeWorkspacePath() || workspaces()[0]?.path;
    if (!wsPath) return;
    const id = `task-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`;
    const name = prompt.trim().replace(/\n/g, " ").slice(0, 40) || "新任务";
    const scenario = pendingScenario();
    const newT: Task = {
      id,
      name,
      createdAt: Date.now(),
      messages: [],
      saved: false,
      modelId: selectedModel() || undefined,
      kind: scenario,
    };
    setTasksByWorkspace((prev) => ({
      ...prev,
      [wsPath]: [newT, ...(prev[wsPath] ?? [])],
    }));
    setCollapsedWs((prev) => (prev[wsPath] ? { ...prev, [wsPath]: false } : prev));
    setActiveTaskId(id);
    // 切到 ChatView 后再发送（sendTaskMessage 依赖 activeTaskId 已就绪）。
    void sendTaskMessage(prompt);
  }

  // ChatView 发送消息：追加 user 消息 → 触发后端 agent 循环。
  async function sendTaskMessage(prompt: string) {
    const id = activeTaskId();
    if (!id) return;
    const wsPath = findTaskWorkspace(id);
    if (!wsPath) return;
    const task = tasksByWorkspace()[wsPath]?.find((t) => t.id === id);
    if (!task) return;

    if (availableModels().length === 0) {
      alert("请先在设置中配置并启用大模型服务商及模型。");
      setSettingsOpen(true);
      return;
    }

    // 第一条用户消息 → 作为任务标题。
    const newName = (!task.name || task.name === "新任务") && prompt.trim()
      ? prompt.trim().replace(/\n/g, " ").slice(0, 40)
      : task.name;

    const userMsg: ChatMessage = {
      id: `msg-${Date.now()}`,
      role: "user",
      segments: [{ type: "text", id: newSegmentId("txt"), text: prompt }],
      ts: Date.now(),
    };

    const updatedMessages = [...(task.messages ?? []), userMsg];
    const updatedTask = { ...task, name: newName, messages: updatedMessages };
    setTasksByWorkspace((prev) => ({
      ...prev,
      [wsPath]: (prev[wsPath] ?? []).map((t) => (t.id === id ? updatedTask : t)),
    }));
    // 追加用户消息到 .jsonl（实时落盘）+ 更新 SQLite 元数据（不触碰内容文件）。
    void appendChatLine(id, userMsg);
    await invoke("update_task_meta", {
      workspacePath: wsPath, taskId: id, name: newName,
      modelId: task.modelId || null, tokenUsage: task.tokenUsage ?? null,
      spaceId: "personal", userId: "default", kind: task.kind ?? "task",
    }).catch((err) => logError("agent", "Failed to update task meta", err));

    try {
      setStreamingTaskId(id);
      const activeModel = task.modelId || selectedModel();
      const historyToSend = task.messages ?? [];
      const historyJson = JSON.stringify(historyToSend);

      await invoke("start_agent_task", {
        taskId: id,
        modelId: modelIdOfKey(activeModel),
        providerId: providerIdOfKey(activeModel),
        prompt,
        historyJson,
        priority: selectedPriority(),
        confirmMode: selectedConfirm(),
        kind: task.kind ?? "task",
      });
    } catch (err) {
      logError("agent", "Failed to start agent task", err);
      setStreamingTaskId(null);
      const errorMsg: ChatMessage = {
        id: `msg-err-${Date.now()}`,
        role: "assistant",
        segments: [{ type: "text", id: newSegmentId("txt"), text: `⚠️ **无法启动任务**: ${err}` }],
        ts: Date.now(),
      };
      setTasksByWorkspace((prev) => ({
        ...prev,
        [wsPath]: (prev[wsPath] ?? []).map((t) =>
          t.id === id ? { ...t, messages: [...updatedMessages, errorMsg] } : t,
        ),
      }));
    }
  }

  // 中止当前流式输出。
  async function stopTask(taskId: string) {
    try {
      await invoke("abort_task", { taskId });
      setStreamingTaskId(null);
    } catch (err) {
      logError("agent", "Failed to abort task", err);
    }
  }

  // awaiting 工具的确认/取消：把用户决定回传给阻塞等待中的工具。
  async function resolveToolConfirmation(taskId: string, toolCallId: string, approved: boolean) {
    try {
      await invoke("resolve_tool_confirmation", { taskId, toolCallId, approved });
    } catch (err) {
      logError("agent", "Failed to resolve tool confirmation", err);
    }
  }

  // 模型选择器变化时，把所选模型记到当前任务 + localStorage 默认。
  function handleSelectModel(key: string) {
    setSelectedModel(key);
    localStorage.setItem("default_model", key);
    const id = activeTaskId();
    if (id) {
      const wsPath = findTaskWorkspace(id);
      if (wsPath) {
        setTasksByWorkspace((prev) => ({
          ...prev,
          [wsPath]: (prev[wsPath] ?? []).map((t) => (t.id === id ? { ...t, modelId: key } : t)),
        }));
      }
    }
  }

  const activeMessages = () => activeTask()?.messages ?? [];
  const activeStreaming = () => streamingTaskId() != null && streamingTaskId() === activeTaskId();

  return (
    <div class="app-shell">
      {/* 左侧栏：通顶（高度 = 整个窗口），顶部的 logo/折叠按钮与右侧窗口按钮在同一水平线 */}
      <Show when={leftOpen()}>
        <LeftNav
          workspaces={workspaces()}
          tasksByWorkspace={tasksByWorkspace()}
          collapsedWs={collapsedWs()}
          activeTaskId={activeTaskId() ?? undefined}
          onToggleWorkspace={toggleWorkspace}
          onSelectTask={selectTask}
          onDeleteTask={deleteTask}
          onNewTask={() => goToHome()}
          busy={busy()}
          leftOpen={leftOpen()}
          onToggleLeft={() => setLeftOpen(!leftOpen())}
          onOpenSettings={() => setSettingsOpen(true)}
          onToggleTheme={() => {
            // 极客深色 ↔ 浅色 二态切换。classic-dark 也归到深色侧。
            const next: Theme = currentTheme() === "light" ? "geek-dark" : "light";
            persistTheme(next);
          }}
        />
      </Show>

      {/* 右侧容器：标题栏（只占右侧宽度）+ 主区 + 日志抽屉 */}
      <div class="app-right">
        <TitleBar
          consoleOpen={consoleOpen()}
          onToggleConsole={() => setConsoleOpen(!consoleOpen())}
          leftOpen={leftOpen()}
          onToggleLeft={() => setLeftOpen(!leftOpen())}
          busy={busy()}
        />

        <main class="app-content">
          <Show
            when={chatTask()}
            fallback={
              <HomeView
                workspaces={workspaces()}
                selectedWorkspacePath={homeWorkspacePath()}
                onSelectWorkspace={selectWorkspace}
                onSelectNewFolder={selectNewFolder}
                availableModels={availableModels()}
                selectedModel={selectedModel()}
                onSelectModel={handleSelectModel}
                selectedPriority={selectedPriority()}
                onSelectPriority={setSelectedPriority}
                selectedConfirm={selectedConfirm()}
                onSelectConfirm={setSelectedConfirm}
                selectedScenario={pendingScenario()}
                onSelectScenario={setPendingScenario}
                dataAnalysisReady={dataAnalysisReady()}
                onDataAnalysisReadyChange={async () => { const r = await invoke<boolean>("check_data_analysis_env"); setDataAnalysisReady(r); }}
                onSubmit={submitTaskFromHome}
                onOpenSettings={() => setSettingsOpen(true)}
              />
            }
          >
            {(task) => (
              <ChatView
                taskId={task().id}
                messages={activeMessages()}
                workspace={activeWorkspaceName()}
                taskName={task().name}
                onSend={sendTaskMessage}
                onStop={() => stopTask(task().id)}
                tokenUsage={task().tokenUsage ?? null}
                contextWindow={modelCtxWindows()[task().modelId ?? selectedModel()] ?? 128000}
                onDelete={() => deleteTask(task().id)}
                availableModels={availableModels()}
                selectedModel={task().modelId ?? selectedModel()}
                onSelectModel={handleSelectModel}
                selectedPriority={selectedPriority()}
                onSelectPriority={setSelectedPriority}
                selectedConfirm={selectedConfirm()}
                onSelectConfirm={setSelectedConfirm}
                onConfirmTool={(toolCallId, approved) => resolveToolConfirmation(task().id, toolCallId, approved)}
                retryNotice={retryNotices()[task().id] ?? null}
                streaming={activeStreaming()}
              />
            )}
          </Show>
        </main>

        {/* 日志抽屉（可折叠） */}
        <Show when={consoleOpen()}>
          <div class="app-console">
            <div class="app-console__head">
              <span class="app-console__title">日志</span>
              <div class="app-console__actions">
                <button class="app-console__btn" title="清空日志" onClick={() => clearLogsStore()}>清空</button>
                <button class="app-console__btn" title="收起" onClick={() => setConsoleOpen(false)}>✕</button>
              </div>
            </div>
            <div class="app-console__body">
              <For each={logsSignal()} fallback={<div class="app-console__empty">暂无日志</div>}>
                {(log) => (
                  <div class={`app-console__row app-console__row--${log.level}`}>
                    <span class="app-console__ts">{new Date(log.ts).toLocaleTimeString()}</span>
                    <span class="app-console__cat">[{log.category}]</span>
                    <span class="app-console__msg">{log.message}</span>
                  </div>
                )}
              </For>
            </div>
          </div>
        </Show>
      </div>

      {/* 设置页（覆盖层） */}
      <Show when={settingsOpen()}>
        <div class="app-overlay">
          <SettingsPage
            onClose={() => setSettingsOpen(false)}
            onProvidersChanged={() => { void loadModelsFromSettings(); }}
            workspacePath={activeWorkspacePath()}
          />
        </div>
      </Show>

    </div>
  );
}
