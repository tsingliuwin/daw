import { createSignal, Show, For, onMount, onCleanup, createEffect, createMemo } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Workspace, QueryTask, ChatMessage, ModelOption } from "./lib/types";
import { modelKeyOf, modelIdOfKey, providerIdOfKey } from "./lib/types";
import { appendDelta, pushToolCall, mergeToolResult, normalizeMessage, newSegmentId } from "./lib/chat";
import { mergeUsage } from "./lib/metrics";
import { logsSignal, logError, installAppLogListener, clearLogsStore } from "./lib/logger";
import TitleBar from "./components/TitleBar";
import LeftNav from "./components/LeftNav";
import ChatView from "./components/ChatView";
import SettingsPage from "./components/SettingsPage";

/**
 * 应用主布局与状态中枢。相比 lakemind 大幅精简：
 *   - 删除所有 SQL 编辑器、结果表、右侧 inspector、文件拖拽、HomePanel、
 *     数据树、数据库连接、import 监听、chart 段处理等。
 *   - 保留 chat 相关的状态管理与流式聚合：workspaces / currentWorkspace /
 *     tasks / activeTaskId / streamingTaskId / chatMessages / 可用模型 / 选择器。
 *
 * 布局：TitleBar 顶部；下方水平 flex = LeftNav（可折叠）+ 主区（ChatView 占满）
 * + 可折叠的日志抽屉（BottomConsole 风格，展示 logsSignal）。
 */
export default function App() {
  // ── 工作区与任务 ──
  const [workspaces, setWorkspaces] = createSignal<Workspace[]>([]);
  const [currentWorkspace, setCurrentWorkspace] = createSignal<Workspace>({ name: "", path: "" });
  const [tasks, setTasks] = createSignal<QueryTask[]>([]);
  const [activeTaskId, setActiveTaskId] = createSignal<string | null>(null);
  // 当前正在流式输出的对话任务 id。start_agent_chat 是 fire-and-forget
  // （tokio::spawn 后立即返回），真正的流式通过 agent-event 异步回来，
  // streamingTaskId 用于在流式期间锁定输入、显示 Stop 按钮。
  const [streamingTaskId, setStreamingTaskId] = createSignal<string | null>(null);

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

  const activeTask = createMemo(() => {
    const id = activeTaskId();
    if (!id) return null;
    return tasks().find((t) => t.id === id) ?? null;
  });

  // 加载 settings.json → 收集所有 enabled provider 下的模型为可选项。
  async function loadModelsFromSettings() {
    try {
      const json = await invoke<string>("load_settings_json");
      if (json && json !== "{}") {
        const loaded = JSON.parse(json);
        const models: ModelOption[] = [];
        const ctxMap: Record<string, number> = {};
        if (loaded.providers) {
          for (const prov of loaded.providers) {
            if (prov.enabled && prov.models) {
              for (const m of prov.models) {
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
      }
    } catch (err) {
      logError("ui", "Failed to load models from settings", err);
    }
  }

  // 持久化一个 chat task 到后端（messages + modelId + tokenUsage）。
  async function saveChatTaskBackend(taskId: string, name: string, messages: ChatMessage[]) {
    try {
      const task = tasks().find((t) => t.id === taskId);
      const modelId = task?.modelId || null;
      const tokenUsage = task?.tokenUsage ?? null;
      await invoke("save_chat_task", {
        workspacePath: currentWorkspace().path,
        taskId,
        name,
        messages,
        modelId,
        tokenUsage,
      });
    } catch (err) {
      logError("agent", "Failed to save chat task to backend", err);
    }
  }

  onMount(async () => {
    // 安装 agent-event 监听器：聚合 reasoning/text/tool_call/tool_result/usage/
    // done/error 流进当前 assistant 消息的 segments。
    const unlistenAgent = await listen<any>("agent-event", (event) => {
      const payload = event.payload;
      const targetId = payload.taskId;

      setTasks((prev) =>
        prev.map((t) => {
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
          } else if (kind === "usage" && payload.text) {
            // 通过 mergeUsage 把 usage 事件折叠进 task 的持久化 TokenUsage。
            try {
              const evt = JSON.parse(payload.text);
              t = { ...t, tokenUsage: mergeUsage(t.tokenUsage ?? null, evt) };
            } catch { /* ignore parse error */ }
          } else if (kind === "error") {
            segments = [
              ...segments,
              { type: "error", id: newSegmentId("t"), text: payload.text ?? "未知错误" },
            ];
          }

          messages[messages.length - 1] = { ...lastMsg, segments };

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
            void saveChatTaskBackend(targetId, t.name, messages);
          }

          return { ...t, messages };
        }),
      );

      // 流式结束（成功或出错）：清除执行状态，解除输入锁定。
      if (payload.kind === "done" || payload.kind === "error") {
        setStreamingTaskId(null);
      }
    });

    // 加载工作区列表，优先恢复上次使用的工作区。
    try {
      const list = await invoke<Workspace[]>("load_workspaces");
      if (list && list.length > 0) {
        setWorkspaces(list);
        let last: string | null = null;
        try {
          last = await invoke<string | null>("get_app_config", { key: "workspace.last" });
        } catch { /* best-effort */ }
        const defaultWS =
          (last && list.find((w) => w.path === last)) ||
          list.find((w) => w.path === "DefaultProject") ||
          list[0];
        if (currentWorkspace().path !== defaultWS.path) {
          changeWorkspace(defaultWS);
        }
      }
    } catch (err) {
      logError("ui", "Failed to load workspaces", err);
    }

    await loadModelsFromSettings();

    // 后端 tracing 事件 → 统一日志 signal（前端日志与后端日志在同一控制台混合展示）。
    const unlistenAppLog = await installAppLogListener();
    onCleanup(() => {
      unlistenAppLog();
      unlistenAgent();
    });
  });

  // 跟踪 workspace 切换：加载该 workspace 的任务列表。
  createEffect(async () => {
    const ws = currentWorkspace();
    if (!ws || !ws.path) return;
    setBusy(true);
    try {
      const loadedTasks = await invoke<QueryTask[]>("load_workspace_tasks", { workspacePath: ws.path });
      if (currentWorkspace().path !== ws.path) return;
      // 归一化历史消息（兼容简单的 content 形态）。
      const migrated = loadedTasks
        .map((t) =>
          Array.isArray(t.messages)
            ? { ...t, messages: t.messages.map((m) => normalizeMessage(m)) }
            : t,
        )
        .sort((a, b) => b.createdAt - a.createdAt);
      setTasks(migrated);

      if (migrated.length > 0) {
        const activeId = activeTaskId();
        if (activeId && migrated.some((t) => t.id === activeId)) {
          // 保留当前选中
        } else {
          setActiveTaskId(migrated[0].id);
        }
      } else {
        // 该工作区还没有任何对话——直接新建一个，让用户一进来就能看到对话框，
        // 而不是停在"点击新建对话"的空状态。
        newChat();
      }
    } catch (err) {
      logError("ui", "Failed to load workspace tasks", err);
    } finally {
      setBusy(false);
    }
  });

  function changeWorkspace(ws: Workspace) {
    setCurrentWorkspace(ws);
    setTasks([]);
    setActiveTaskId(null);
    setStreamingTaskId(null);
    invoke("set_app_config", { key: "workspace.last", value: ws.path }).catch((err: unknown) => {
      logError("ui", "Failed to persist workspace.last", err);
    });
  }

  function selectTask(id: string) {
    setActiveTaskId(id);
  }

  // 新建对话：生成 uuid，加入 tasks 列表，选中。
  function newChat() {
    const id = `task-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`;
    const newTask: QueryTask = {
      id,
      name: "新对话",
      createdAt: Date.now(),
      messages: [],
      saved: false,
      modelId: selectedModel() || undefined,
    };
    setTasks((prev) => [newTask, ...prev]);
    setActiveTaskId(id);
  }

  async function deleteTask(id: string) {
    const remaining = tasks().filter((t) => t.id !== id);
    setTasks(remaining);
    if (activeTaskId() === id) {
      if (remaining.length > 0) {
        // 选中相邻的下一个（或上一个）对话，优先有内容的。
        const visible = remaining.filter((t) => (t.messages?.length ?? 0) > 0).sort((a, b) => b.createdAt - a.createdAt);
        setActiveTaskId(visible.length > 0 ? visible[0].id : remaining[0].id);
      } else {
        // 最后一个对话也被删了——新建一个，避免落回空状态。
        newChat();
      }
    }
    try {
      await invoke("delete_task", { taskId: id });
    } catch (err) {
      logError("ui", "Failed to delete task", err);
    }
  }

  // ChatView 发送消息：追加 user 消息 → 触发后端 agent 循环。
  async function sendChatMessage(prompt: string) {
    const id = activeTaskId();
    if (!id) return;
    const task = tasks().find((t) => t.id === id);
    if (!task) return;

    if (availableModels().length === 0) {
      alert("请先在设置中配置并启用大模型服务商及模型。");
      setSettingsOpen(true);
      return;
    }

    // 第一条用户消息 → 作为对话标题。
    const newName = (!task.name || task.name === "新对话") && prompt.trim()
      ? prompt.trim().replace(/\n/g, " ").slice(0, 40)
      : task.name;

    const userMsg: ChatMessage = {
      id: `msg-${Date.now()}`,
      role: "user",
      segments: [{ type: "text", id: newSegmentId("txt"), text: prompt }],
      ts: Date.now(),
    };

    const updatedMessages = [...(task.messages ?? []), userMsg];
    setTasks((prev) =>
      prev.map((t) => (t.id === id ? { ...t, name: newName, messages: updatedMessages } : t)),
    );
    await saveChatTaskBackend(id, newName, updatedMessages);

    try {
      setStreamingTaskId(id);
      const activeModel = task.modelId || selectedModel();
      const historyToSend = task.messages ?? [];
      const historyJson = JSON.stringify(historyToSend);

      await invoke("start_agent_chat", {
        taskId: id,
        modelId: modelIdOfKey(activeModel),
        providerId: providerIdOfKey(activeModel),
        prompt,
        historyJson,
        priority: selectedPriority(),
        confirmMode: selectedConfirm(),
      });
    } catch (err) {
      logError("agent", "Failed to start agent chat", err);
      setStreamingTaskId(null);
      const errorMsg: ChatMessage = {
        id: `msg-err-${Date.now()}`,
        role: "assistant",
        segments: [{ type: "text", id: newSegmentId("txt"), text: `⚠️ **无法启动对话**: ${err}` }],
        ts: Date.now(),
      };
      setTasks((prev) =>
        prev.map((t) => (t.id === id ? { ...t, messages: [...updatedMessages, errorMsg] } : t)),
      );
    }
  }

  // 中止当前流式输出。
  async function stopChat(taskId: string) {
    try {
      await invoke("abort_chat", { taskId });
      setStreamingTaskId(null);
    } catch (err) {
      logError("agent", "Failed to abort chat", err);
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
      setTasks((prev) => prev.map((t) => (t.id === id ? { ...t, modelId: key } : t)));
    }
  }

  const activeMessages = () => activeTask()?.messages ?? [];
  const activeStreaming = () => streamingTaskId() != null && streamingTaskId() === activeTaskId();

  return (
    <div class="app-root">
      <TitleBar
        consoleOpen={consoleOpen()}
        onToggleConsole={() => setConsoleOpen(!consoleOpen())}
        leftOpen={leftOpen()}
        onToggleLeft={() => setLeftOpen(!leftOpen())}
        busy={busy()}
        onOpenSettings={() => setSettingsOpen(true)}
      />

      <div class="app-main">
        <Show when={leftOpen()}>
          <LeftNav
            workspaces={workspaces()}
            currentWorkspacePath={currentWorkspace().path}
            onSelectWorkspace={(path) => {
              const ws = workspaces().find((w) => w.path === path);
              if (ws) changeWorkspace(ws);
            }}
            tasks={tasks()}
            selectedTaskId={activeTaskId() ?? undefined}
            onSelectTask={selectTask}
            onDeleteTask={deleteTask}
            onNewChat={newChat}
            busy={busy()}
            leftOpen={leftOpen()}
            onToggleLeft={() => setLeftOpen(!leftOpen())}
          />
        </Show>

        <main class="app-content">
          <Show
            when={activeTask()}
            fallback={
              <div class="app-empty">
                <div class="app-empty__hint">
                  <p>选择左侧的对话，或点击「新建对话」开始。</p>
                  <button class="app-empty__btn" onClick={newChat}>+ 新建对话</button>
                </div>
              </div>
            }
          >
            {(task) => (
              <ChatView
                taskId={task().id}
                messages={activeMessages()}
                workspace={currentWorkspace().name || currentWorkspace().path}
                taskName={task().name}
                onSend={sendChatMessage}
                onStop={() => stopChat(task().id)}
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
          />
        </div>
      </Show>
    </div>
  );
}
