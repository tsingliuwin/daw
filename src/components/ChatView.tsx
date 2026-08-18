import { Index, Show, Switch, Match, createSignal, createEffect, createMemo, onMount, onCleanup, untrack } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { ChatMessage, Segment, TokenUsage, ModelOption, RetryNotice } from "../lib/types";
import { derivePanelMetrics, fmtCap, fmtPct } from "../lib/metrics";
import ToolSegment from "./ToolSegment";
import ChartSegment from "./ChartSegment";
import MessageText from "./MessageText";
import ComposerActions from "./ComposerActions";

type ReasoningSeg = Extract<Segment, { type: "reasoning" }>;
type TextSeg = Extract<Segment, { type: "text" }>;
type ErrorSeg = Extract<Segment, { type: "error" }>;
const asReasoning = (s: Segment): ReasoningSeg | null => (s.type === "reasoning" ? s : null);
const asText = (s: Segment): TextSeg | null => (s.type === "text" ? s : null);
const asError = (s: Segment): ErrorSeg | null => (s.type === "error" ? s : null);

/**
 * 任务模式主区：消息流（上）+ 段内嵌 + 底部常驻输入框。
 *
 * 相比早期数据湖版本：
 *  - 去掉 onOpenInSqlPanel / onAddFile / onAddFolder 等 prop 及对应 UI。
 *  - 内部渲染把 ChartSegment 段移除（无 chart 段），ToolSegment 换成新版，
 *    MessageText 换成纯文本版（只接收 text）。
 *  - 保留：消息流渲染（按 segment 顺序：reasoning 折叠 → tool 段 → text markdown）、
 *    贴底滚动、token 容量条（用 derivePanelMetrics）、模型选择器、优先级选择器、
 *    确认模式选择器、底部输入框、Stop 按钮、onSend/onStop/onConfirmTool 回调。
 */
export default function ChatView(props: {
  taskId: string;
  messages: ChatMessage[];
  workspace: string;
  taskName: string;
  onSend: (prompt: string) => void;
  /** Abort the running stream (stop button). */
  onStop?: () => void;
  /** Token usage from the last LLM response (for context window display). */
  tokenUsage?: TokenUsage | null;
  /** Current model's context window size (from settings.json). */
  contextWindow?: number;
  onDelete?: () => void;
  availableModels: ModelOption[];
  selectedModel: string;
  onSelectModel: (model: string) => void;
  selectedPriority: string;
  onSelectPriority: (priority: string) => void;
  selectedConfirm: string;
  onSelectConfirm: (mode: string) => void;
  /** 用户对 awaiting 状态的工具做出确认/取消决定。 */
  onConfirmTool: (toolCallId: string, approved: boolean) => void;
  /** 速率限制自动重试的瞬时提示（null 表示当前无重试）。 */
  retryNotice?: RetryNotice | null;
  /** 该任务是否正在流式输出（由父级 streamingTaskId 派生）。 */
  streaming: boolean;
}) {
  /** All chart segments across the conversation — a `{{chart:<id>}}` marker in
   *  the current message may reference a chart emitted in a previous assistant
   *  message, so MessageText searches the whole transcript, not just this msg. */
  const allCharts = createMemo<Segment[]>(() =>
    props.messages.flatMap((m) => m.segments).filter(
      (s): s is Extract<Segment, { type: "chart" }> => s.type === "chart",
    )
  );

  const [chatWidth, setChatWidth] = createSignal<number>(
    parseInt(localStorage.getItem("chat_width") || "800")
  );

  const startDraggingChatWidth = (e: MouseEvent) => {
    e.preventDefault();
    document.body.classList.add("dragging-active");
    const stream = scrollEl;
    if (!stream) return;
    const rect = stream.getBoundingClientRect();
    const centerX = rect.left + rect.width / 2;
    const onMouseMove = (moveEvent: MouseEvent) => {
      const deltaFromCenter = Math.abs(moveEvent.clientX - centerX);
      const newWidth = Math.max(400, Math.min(rect.width - 48, deltaFromCenter * 2));
      setChatWidth(newWidth);
      localStorage.setItem("chat_width", String(newWidth));
    };
    const onMouseUp = () => {
      document.body.classList.remove("dragging-active");
      window.removeEventListener("mousemove", onMouseMove);
      window.removeEventListener("mouseup", onMouseUp);
    };
    window.addEventListener("mousemove", onMouseMove);
    window.addEventListener("mouseup", onMouseUp);
  };

  const [input, setInput] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [showConfirm, setShowConfirm] = createSignal(false);
  const [copiedMessageId, setCopiedMessageId] = createSignal<string | null>(null);

  const handleCopyMessage = async (msg: ChatMessage) => {
    const textToCopy = getMessageCopyText(msg);
    try {
      await navigator.clipboard.writeText(textToCopy);
      setCopiedMessageId(msg.id);
      setTimeout(() => {
        if (copiedMessageId() === msg.id) setCopiedMessageId(null);
      }, 1500);
    } catch {}
  };

  const displayTitle = createMemo(() => {
    if (props.taskName && !props.taskName.endsWith("...")) return props.taskName;
    const firstMsg = props.messages.find((m) => m.role === "user");
    if (firstMsg) {
      for (const seg of firstMsg.segments) {
        const ts = asText(seg);
        if (ts) return ts.text.trim().replace(/\n/g, " ");
      }
    }
    return props.taskName || "任务";
  });

  // 流式输出状态：发送瞬间的本地 busy 与父级 streaming 合成。
  const isStreaming = createMemo(() => busy() || props.streaming);

  createEffect(() => {
    props.messages;
    props.taskName;
    setShowConfirm(false);
  });

  // 折叠状态。
  const [openReasoningIds, setOpenReasoningIds] = createSignal<Set<string>>(new Set());
  const [manualReasoningIds, setManualReasoningIds] = createSignal<Set<string>>(new Set());
  const [expandedToolIds, setExpandedToolIds] = createSignal<Set<string>>(new Set());
  const [manualToolIds, setManualToolIds] = createSignal<Set<string>>(new Set());

  function toggleReasoning(segId: string) {
    setManualReasoningIds((prev) => new Set(prev).add(segId));
    setOpenReasoningIds((prev) => {
      const next = new Set(prev);
      if (next.has(segId)) next.delete(segId); else next.add(segId);
      return next;
    });
  }

  function toggleTool(segId: string) {
    setManualToolIds((prev) => new Set(prev).add(segId));
    setExpandedToolIds((prev) => {
      const next = new Set(prev);
      if (next.has(segId)) next.delete(segId); else next.add(segId);
      return next;
    });
  }

  // ── 计时：按墙钟连续走表 ──
  const [now, setNow] = createSignal(Date.now());
  let streamStart = 0;

  createEffect(() => {
    if (isStreaming()) {
      if (streamStart === 0) streamStart = Date.now();
    } else {
      streamStart = 0;
    }
  });

  const metrics = createMemo(() =>
    derivePanelMetrics(props.tokenUsage ?? null, {
      contextWindow: props.contextWindow ?? 128000,
      nowMs: now(),
      runStartMs: streamStart > 0 ? streamStart : undefined,
      streaming: isStreaming(),
    }),
  );
  const capPct = createMemo(() => metrics()?.capacity.pct ?? 0);

  // 事件驱动的时钟补血：每条消息变化（推理/正文 delta）时把时钟刷新到当前时刻。
  // 100ms 泵在窗口被遮挡/切后台时会被浏览器节流甚至停摆——那时 delta 仍然在
  // 到达（文字照常增长），但计时显示需要靠这里每次事件把时间推进，否则两个
  // 计时（思考耗时/已工作）都会钉在旧值上，出现"输出在走、计时不动"。
  createEffect(() => {
    props.messages;
    setNow(Date.now());
  });

  onMount(() => {
    // 时钟刷新泵：前台每 100ms tick 一次。窗口被最小化/完全遮挡/切到后台时，
    // Chromium 会节流 JS 定时器（后台页降频甚至暂停），两个计时（思考耗时/
    // 已工作）都会冻住；回到前台瞬间强制刷新一次时钟，计时立即跳到正确值，
    // 不会一直钉在旧值上。
    const refreshClock = () => setNow(Date.now());
    const handle = setInterval(() => {
      const t = Date.now();
      setNow(t);
      // 思考耗时/已工作已改为纯响应式绑定（单一写入者），不再直改 DOM；
      // 工具运行计时没有响应式绑定（占位符为"…"），保留直改补表——该 span
      // 无竞争写入者，textContent 直改是安全的单写入者路径。
      const lm = props.messages[props.messages.length - 1];
      if (lm && lm.role === "assistant" && isStreaming()) {
        const s = lm.segments[lm.segments.length - 1];
        if (s && s.type === "tool" && s.status === "running" && s.startTime != null) {
          const el = document.getElementById(`tool-timer-${s.id}`);
          if (el) el.textContent = fmtMs(t - s.startTime);
        }
      }
    }, 100);

    // 回到前台的两个信号都补一发刷新：页面可见性变化 + 原生窗口焦点恢复
    // （被别的窗口完全遮住时 Chromium 也会把定时器节流掉，焦点事件兜底）。
    let unlistenFocus: (() => void) | undefined;
    document.addEventListener("visibilitychange", refreshClock);
    void getCurrentWindow()
      .onFocusChanged((ev) => {
        if (ev.payload) refreshClock();
      })
      .then((unlisten) => {
        unlistenFocus = unlisten;
      })
      .catch(() => {});

    onCleanup(() => {
      clearInterval(handle);
      document.removeEventListener("visibilitychange", refreshClock);
      unlistenFocus?.();
    });
  });

  let scrollEl: HTMLDivElement | undefined;

  // ── 贴底滚动 ──
  const [stickToBottom, setStickToBottom] = createSignal(true);
  const [showScrollDown, setShowScrollDown] = createSignal(false);
  // 程序化滚动（贴底吸附 / 「回到底部」平滑滚动）会触发 scroll 事件，必须在滚动
  // 结束前忽略它们，避免把自己的动作误读成用户行为。用户滚轮可随时提前取消抑制。
  let ignoreScrollUntil = 0;
  // 上一条用户 scroll 事件的位置，用于判断滚动方向（拖动滚动条不触发 wheel 事件，
  // 只能靠 scroll 事件检测方向）。
  let lastUserScrollTop = -1;

  const atBottomOf = (el: HTMLDivElement) =>
    el.scrollHeight - el.scrollTop - el.clientHeight <= 30;

  const handleScroll = (e: Event) => {
    if (Date.now() < ignoreScrollUntil) return;
    const el = e.currentTarget as HTMLDivElement;
    const scrolledUp = lastUserScrollTop >= 0 && el.scrollTop < lastUserScrollTop;
    lastUserScrollTop = el.scrollTop;
    if (scrolledUp) {
      // 任意距离的向上滚动都解除贴底（哪怕 30px 以内），尊重用户回看的意图。
      setStickToBottom(false);
    } else if (atBottomOf(el)) {
      // 滚到底部即恢复贴底，流式输出中同样生效。拖滚动条到底不触发 wheel，
      // 只能在这里按滚动后的位置收尾。
      setStickToBottom(true);
    }
    setShowScrollDown(!atBottomOf(el));
  };

  const handleWheel = (e: WheelEvent) => {
    // 滚轮 = 明确的用户意图：立即结束程序化滚动的事件抑制（如打断「回到底部」
    // 的平滑滚动），之后贴底与否交给 handleScroll 按滚动后的位置判定。
    ignoreScrollUntil = 0;
    if (e.deltaY < 0) {
      setStickToBottom(false);
    }
  };

  function stickScrollToBottom() {
    const el = scrollEl;
    if (!el) return;
    ignoreScrollUntil = Date.now() + 100;
    el.scrollTop = el.scrollHeight;
  }

  function smoothStickToBottom() {
    const el = scrollEl;
    if (!el) return;
    ignoreScrollUntil = Date.now() + 600;
    el.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
  }

  createEffect(() => {
    props.messages;
    isStreaming();
    const el = scrollEl;
    if (!el) return;
    if (untrack(stickToBottom)) {
      stickScrollToBottom();
    } else {
      // 内容在视口下方生长不改变 scrollTop、不触发 scroll 事件，按钮状态必须在
      // 此主动刷新——保证解除贴底后永远有"回到底部"这个恢复入口。
      setShowScrollDown(el.scrollHeight - el.scrollTop - el.clientHeight > 30);
    }
  });

  // 切换任务时重置折叠状态。
  let prevTaskId: string | undefined;
  createEffect(() => {
    const currentId = props.taskId;
    if (currentId === prevTaskId) return;
    prevTaskId = currentId;
    setOpenReasoningIds(new Set<string>());
    setExpandedToolIds(new Set<string>());
    setManualReasoningIds(new Set<string>());
    setManualToolIds(new Set<string>());
    setStickToBottom(true);
    stickScrollToBottom();
  });

  function lastAssistantId(): string | undefined {
    const msgs = props.messages;
    if (msgs.length === 0) return undefined;
    const last = msgs[msgs.length - 1];
    return last.role === "assistant" ? last.id : undefined;
  }

  const activeReasoningId = createMemo(() => {
    const id = lastAssistantId();
    if (!id) return undefined;
    const msg = props.messages.find((m) => m.id === id);
    if (!msg) return undefined;
    const segs = msg.segments;
    const last = segs[segs.length - 1];
    return isStreaming() && last && last.type === "reasoning" ? last.id : undefined;
  });

  createEffect(() => {
    const id = lastAssistantId();
    if (!id) return;
    const msg = props.messages.find((m) => m.id === id);
    if (!msg) return;
    const segs = msg.segments;
    const last = segs[segs.length - 1];
    const activeId = isStreaming() && last && last.type === "reasoning" ? last.id : undefined;
    const manual = manualReasoningIds();
    setOpenReasoningIds((prev) => {
      const next = new Set(prev);
      for (const s of segs) {
        if (s.type !== "reasoning") continue;
        if (manual.has(s.id)) continue;
        if (s.id === activeId) next.add(s.id); else next.delete(s.id);
      }
      return next;
    });
  });

  createEffect(() => {
    const id = lastAssistantId();
    if (!id) return;
    const msg = props.messages.find((m) => m.id === id);
    if (!msg) return;
    const running = new Set(
      msg.segments
        .filter((s): s is Extract<Segment, { type: "tool" }> => s.type === "tool")
        .filter((s) => s.status === "running")
        .map((s) => s.id),
    );
    const awaiting = new Set(
      msg.segments
        .filter((s): s is Extract<Segment, { type: "tool" }> => s.type === "tool")
        .filter((s) => s.status === "awaiting")
        .map((s) => s.id),
    );
    const expanded0 = untrack(expandedToolIds);
    let changed = false;
    const next = new Set(expanded0);
    for (const r of running) { if (!next.has(r)) { next.add(r); changed = true; } }
    for (const a of awaiting) { if (!next.has(a)) { next.add(a); changed = true; } }
    if (changed) setExpandedToolIds(next);

    const manual = manualToolIds();
    const expanded = untrack(expandedToolIds);
    const toCollapse: string[] = [];
    for (const s of msg.segments) {
      if (s.type !== "tool" || running.has(s.id) || awaiting.has(s.id)) continue;
      if (!manual.has(s.id) && expanded.has(s.id)) toCollapse.push(s.id);
    }
    if (toCollapse.length > 0) {
      setExpandedToolIds((prev) => {
        const next = new Set(prev);
        for (const c of toCollapse) next.delete(c);
        return next;
      });
    }
  });

  async function send() {
    const text = input().trim();
    if (!text || isStreaming()) return;
    setInput("");
    setBusy(true);
    setStickToBottom(true);
    try {
      await props.onSend(text);
    } finally {
      setBusy(false);
    }
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void send();
    }
  }

  return (
    <div class="chat-view">
      <div class="chat-header">
        <div style="display: flex; align-items: center; gap: 8px; flex: 1; min-width: 0;">
          <span class="chat-header__title">{displayTitle()}</span>
          <span class="chat-header__ws" title={`当前工作区: ${props.workspace}`}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px; flex-shrink: 0; color: var(--text-dim);">
              <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
            </svg>
            <span class="ws-text">{props.workspace}</span>
          </span>
        </div>
        <Show
          when={showConfirm()}
          fallback={
            <button
              class="header-close-btn"
              title="删除该任务"
              onClick={() => {
                if (props.messages.length > 0) setShowConfirm(true);
                else props.onDelete?.();
              }}
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 10px; height: 10px;">
                <polyline points="3 6 5 6 21 6"></polyline>
                <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"></path>
              </svg>
            </button>
          }
        >
          <div style="display: flex; align-items: center; gap: 8px; font-size: 12px; background: var(--bg-hover); padding: 4px 10px; border-radius: 6px; border: 1px solid var(--border-faint);">
            <span style="color: var(--accent-red); font-weight: 500;">确定删除？</span>
            <button
              onClick={() => { setShowConfirm(false); props.onDelete?.(); }}
              style="background: var(--accent-red); color: white; border: none; padding: 2px 8px; border-radius: 4px; cursor: pointer; font-size: 11px; font-weight: 500;"
            >确定</button>
            <button
              onClick={() => setShowConfirm(false)}
              style="background: transparent; color: var(--text-secondary); border: 1px solid var(--border-strong); padding: 2px 8px; border-radius: 4px; cursor: pointer; font-size: 11px; font-weight: 500;"
            >取消</button>
          </div>
        </Show>
      </div>
      <div class="chat-stream" ref={scrollEl} onScroll={handleScroll} onWheel={handleWheel}>
        <div
          class="chat-stream-inner"
          style={{
            width: `${chatWidth()}px`,
            "max-width": "100%",
            margin: "0 auto",
            position: "relative",
            display: "flex",
            "flex-direction": "column",
            gap: "16px",
            height: props.messages.length > 0 ? "auto" : "100%",
            "justify-content": props.messages.length > 0 ? "flex-start" : "center"
          }}
        >
          <div class="chat-resizer-l" onMouseDown={(e) => startDraggingChatWidth(e)} />
          <div class="chat-resizer-r" onMouseDown={(e) => startDraggingChatWidth(e)} />

          <Show
            when={props.messages.length > 0}
            fallback={<div class="chat-empty">开始任务，用自然语言完成请假、报销、审批等办公流程。</div>}
          >
            <Index each={props.messages}>
              {(msg) => (
                <div class={`chat-msg chat-msg--${msg().role}`}>
                  <div class="chat-msg__body">
                    <Index each={msg().segments}>
                      {(seg) => {
                        const rs = () => asReasoning(seg());
                        const ts = () => asText(seg());
                        const es = () => asError(seg());
                        const reasoningMs = createMemo<number | undefined>(() => {
                          if (seg().id === activeReasoningId() && rs()?.startTime != null) {
                            return now() - rs()!.startTime!;
                          }
                          return rs()?.elapsedMs;
                        });
                        return (
                          <Switch>
                            <Match when={seg().type === "error" && es()}>
                              <div class="chat-terminal-error">
                                <span class="chat-terminal-error__icon">
                                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
                                    <path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3Z" />
                                    <line x1="12" y1="9" x2="12" y2="13" />
                                    <line x1="12" y1="17" x2="12.01" y2="17" />
                                  </svg>
                                </span>
                                <span class="chat-terminal-error__text">{es()!.text}</span>
                              </div>
                            </Match>
                            <Match when={seg().type === "reasoning"}>
                              <div class="chat-reasoning">
                                <div class="chat-reasoning__header" onClick={() => toggleReasoning(seg().id)}>
                                  <span class="chat-reasoning__icon">
                                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
                                      <path d="M9.5 2A2.5 2.5 0 0 1 12 4.5v15a2.5 2.5 0 0 1-4.96-.44 2.5 2.5 0 0 1 0-3.12 3 3 0 0 1 0-4.88 2.5 2.5 0 0 1 0-3.12A2.5 2.5 0 0 1 9.5 2Z" />
                                      <path d="M14.5 2A2.5 2.5 0 0 0 12 4.5v15a2.5 2.5 0 0 0 4.96-.44 2.5 2.5 0 0 0 0-3.12 3 3 0 0 0 0-4.88 2.5 2.5 0 0 0 0-3.12A2.5 2.5 0 0 0 14.5 2Z" />
                                    </svg>
                                  </span>
                                  <span class="chat-reasoning__label">思考过程</span>
                                  <Show when={reasoningMs() != null}>
                                    <span style="color: var(--text-dim); margin-left: 2px;">· <span id={`rs-timer-${seg().id}`}>{fmtMs(reasoningMs()!)}</span></span>
                                  </Show>
                                  <span class="chat-reasoning__toggle" classList={{ "chat-reasoning__toggle--open": openReasoningIds().has(seg().id) }}>
                                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" style="width: 10px; height: 10px; transition: transform 0.15s ease;">
                                      <polyline points="9 18 15 12 9 6"></polyline>
                                    </svg>
                                  </span>
                                </div>
                                <Show when={openReasoningIds().has(seg().id) && rs()}>
                                  <ReasoningBody text={rs()!.text} />
                                </Show>
                              </div>
                            </Match>
                            <Match when={seg().type === "tool"}>
                              <Show when={seg()}>
                                {(s) => (
                                  <ToolSegment
                                    seg={s()}
                                    expanded={expandedToolIds().has(s().id)}
                                    onToggle={toggleTool}
                                    onConfirm={(approved) => props.onConfirmTool(s().id, approved)}
                                  />
                                )}
                              </Show>
                            </Match>
                            <Match when={seg().type === "chart"}>
                              <Show when={seg()}>
                                {(s) => (
                                  <ChartSegment seg={s() as Extract<Segment, { type: "chart" }>} />
                                )}
                              </Show>
                            </Match>
                            <Match when={seg().type === "text" && ts()}>
                              <div class="chat-msg__text">
                                <Show when={msg().role === "assistant"} fallback={ts()!.text}>
                                  <MessageText text={ts()!.text} segments={msg().segments} charts={allCharts()} />
                                </Show>
                              </div>
                            </Match>
                          </Switch>
                        );
                      }}
                    </Index>
                    <Show when={!(msg().role === "assistant" && isStreaming() && msg().id === props.messages[props.messages.length - 1]?.id)}>
                      <div class="chat-msg__actions">
                        <span class="chat-msg__time">{formatTime(msg().ts)}</span>
                        <button
                          class="chat-msg__copy-btn"
                          title={copiedMessageId() === msg().id ? "已复制" : "复制"}
                          onClick={() => handleCopyMessage(msg())}
                        >
                          <Show
                            when={copiedMessageId() === msg().id}
                            fallback={
                              <svg xmlns="http://www.w3.org/2000/svg" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                                <rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect>
                                <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path>
                              </svg>
                            }
                          >
                            <svg xmlns="http://www.w3.org/2000/svg" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="var(--accent-green, #10b981)" stroke-width="3" stroke-linecap="round" stroke-linejoin="round">
                              <polyline points="20 6 9 17 4 12"></polyline>
                            </svg>
                          </Show>
                        </button>
                      </div>
                    </Show>
                  </div>
                </div>
              )}
            </Index>

            {/* Busy / streaming indicator */}
            <Show when={isStreaming()}>
              <div class="chat-msg chat-msg--assistant">
                <div class="chat-msg__body">
                  <div class="chat-agent-status">
                    <span class="agent-status__timer">⏱ 已工作 <span id="chat-bottom-timer">{Math.floor((now() - streamStart) / 1000)}</span> 秒</span>
                  </div>
                </div>
              </div>
              {/* 速率限制重试横幅：倒计时 + 尝试次数。内容恢复后由父级清除。 */}
              <Show when={props.retryNotice}>
                {(notice) => (
                  <div class="chat-msg chat-msg--assistant">
                    <div class="chat-msg__body">
                      <RetryBanner notice={notice()} nowMs={now()} />
                    </div>
                  </div>
                )}
              </Show>
            </Show>
          </Show>
        </div>
      </div>

      <Show when={showScrollDown()}>
        <button
          class="chat-view__scroll-down"
          onClick={() => { setStickToBottom(true); smoothStickToBottom(); }}
          title="回到底部"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
            <line x1="12" y1="5" x2="12" y2="19"></line>
            <polyline points="19 12 12 19 5 12"></polyline>
          </svg>
        </button>
      </Show>

      <div class="chat-composer">
        <div
          class="chat-composer-inner"
          style={{
            width: "100%",
            "max-width": `${chatWidth()}px`,
            margin: "0 auto",
            display: "flex",
            "flex-direction": "column"
          }}
        >
          <div class="chat-composer__box">
            <textarea
              class="chat-composer__input"
              placeholder="输入你的需求（Enter 发送 · Shift+Enter 换行）…"
              value={input()}
              onInput={(e) => setInput(e.currentTarget.value)}
              onkeydown={onKeydown}
              disabled={isStreaming()}
              rows={2}
            />
            <div class="chat-composer__toolbar">
              <div style="display: flex; align-items: center; gap: 4px; min-width: 0; flex-shrink: 1; margin-left: auto;">
                {/* Token usage indicator */}
                <div class="token-usage-wrap">
                  <div
                    class="token-usage-pill"
                    classList={{
                      "token-usage-pill--warn": capPct() >= 70 && capPct() < 90,
                      "token-usage-pill--danger": capPct() >= 90,
                    }}
                    title="上下文容量"
                  >
                    <span class="battery-icon-wrapper">
                      <svg class="battery-icon" viewBox="0 0 24 12" fill="none" stroke="currentColor" stroke-width="1.5">
                        <rect x="1" y="1" width="18" height="10" rx="2" />
                        <path d="M20 4v4" stroke-linecap="round" />
                        <Show when={capPct() > 0}>
                          <rect
                            x="2.5"
                            y="2.5"
                            width={15 * (capPct() / 100)}
                            height="7"
                            rx="1"
                            fill="currentColor"
                            stroke="none"
                          />
                        </Show>
                      </svg>
                    </span>
                  </div>
                  <div class="token-usage-panel">
                    <Show
                      when={metrics()}
                      fallback={<div class="token-usage-panel__empty">暂无用量数据</div>}
                    >
                      {(m) => (
                        <>
                          <div class="token-usage-panel__header">
                            <span class="token-usage-panel__title">上下文容量</span>
                            <span class="token-usage-panel__capacity">
                              {fmtCap(m().capacity.peak)}/{fmtCap(m().capacity.ctx)} ({fmtPct(m().capacity.pct)})
                            </span>
                          </div>
                          <div class="token-usage-panel__bar">
                            <div
                              class="token-usage-panel__bar-fill"
                              style={{
                                width: `${Math.max(0, Math.min(100, m().capacity.pct))}%`,
                                background: m().capacity.pct >= 90 ? "var(--accent-red)" : m().capacity.pct >= 70 ? "var(--accent-amber)" : "var(--accent-blue)",
                              }}
                            />
                          </div>
                          <div class="token-usage-panel__list">
                            <div class="token-usage-panel__item">
                              <span class="token-usage-panel__dot token-usage-panel__dot--msg" />
                              <span class="token-usage-panel__label">消息</span>
                              <span class="token-usage-panel__value">{fmtPct(m().composition.messages.pct)}</span>
                            </div>
                            <div class="token-usage-panel__item">
                              <span class="token-usage-panel__dot token-usage-panel__dot--tools" />
                              <span class="token-usage-panel__label">系统工具</span>
                              <span class="token-usage-panel__value">{fmtPct(m().composition.tools.pct)}</span>
                            </div>
                            <div class="token-usage-panel__item">
                              <span class="token-usage-panel__dot token-usage-panel__dot--preamble" />
                              <span class="token-usage-panel__label">系统提示词</span>
                              <span class="token-usage-panel__value">{fmtPct(m().composition.preamble.pct)}</span>
                            </div>
                          </div>
                          <div class="token-usage-panel__spacer" />
                          <div class="token-usage-panel__item token-usage-panel__item--highlight">
                            <span class="token-usage-panel__dot token-usage-panel__dot--hitrate" />
                            <span class="token-usage-panel__label">平均缓存命中率</span>
                            <span class="token-usage-panel__value token-usage-panel__value--green">
                              {fmtPct(m().cumulative.hitRate)}
                            </span>
                          </div>
                        </>
                      )}
                    </Show>
                  </div>
                </div>

                <ComposerActions
                  availableModels={props.availableModels}
                  selectedModel={props.selectedModel}
                  onSelectModel={props.onSelectModel}
                  selectedPriority={props.selectedPriority}
                  onSelectPriority={props.onSelectPriority}
                  selectedConfirm={props.selectedConfirm}
                  onSelectConfirm={props.onSelectConfirm}
                  canSend={input().trim().length > 0}
                  onSend={() => void send()}
                  streaming={isStreaming()}
                  onStop={() => props.onStop?.()}
                />
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

/**
 * 思考过程内容区：自带独立的贴底滚动管理（沿用早期实现）。
 */
function ReasoningBody(props: { text: string }) {
  let bodyRef: HTMLDivElement | undefined;
  const [stick, setStick] = createSignal(true);
  let lastScrollHeight = 0;

  createEffect(() => {
    props.text;
    if (bodyRef && untrack(stick)) bodyRef.scrollTop = bodyRef.scrollHeight;
  });

  const handleScroll = () => {
    if (!bodyRef) return;
    const currentScrollHeight = bodyRef.scrollHeight;
    if (currentScrollHeight !== lastScrollHeight) {
      lastScrollHeight = currentScrollHeight;
      return;
    }
    const diff = bodyRef.scrollHeight - bodyRef.scrollTop - bodyRef.clientHeight;
    setStick(diff <= 15);
  };

  const handleWheel = (e: WheelEvent) => {
    if (!bodyRef) return;
    const el = bodyRef;
    if (e.deltaY < 0) {
      setStick(false);
      if (el.scrollTop > 0) e.stopPropagation();
    } else if (e.deltaY > 0) {
      if (el.scrollHeight - el.scrollTop - el.clientHeight > 1) e.stopPropagation();
    }
  };

  return (
    <div class="chat-reasoning__body" ref={bodyRef} onScroll={handleScroll} onWheel={handleWheel}>
      {props.text}
    </div>
  );
}

function fmtMs(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

/**
 * 速率限制重试横幅：琥珀色提示 + 倒计时（nowMs 由父级 100ms 时钟驱动，
 * 无需额外 interval）。倒计时归零后显示"重试中…"，直到内容恢复被父级清除。
 */
function RetryBanner(props: { notice: RetryNotice; nowMs: number }) {
  const remainingMs = () => {
    const left = props.notice.at + props.notice.delaySecs * 1000 - props.nowMs;
    return Math.max(0, left);
  };
  return (
    <div class="chat-retry">
      <span class="chat-retry__icon">⏳</span>
      <span class="chat-retry__text">
        {remainingMs() > 0
          ? `遇到速率限制，${Math.ceil(remainingMs() / 1000)} 秒后自动重试（第 ${props.notice.attempt}/${props.notice.maxAttempts} 次）`
          : `正在重试…（第 ${props.notice.attempt}/${props.notice.maxAttempts} 次）`}
      </span>
    </div>
  );
}

function formatTime(ts: number): string {
  const d = new Date(ts);
  const h = d.getHours().toString().padStart(2, '0');
  const m = d.getMinutes().toString().padStart(2, '0');
  return `${h}:${m}`;
}

function getMessageCopyText(msg: ChatMessage): string {
  const texts = msg.segments
    .filter((s) => s.type === "text")
    .map((s) => (s as any).text);
  if (texts.length > 0) return texts.join("\n");
  return msg.segments
    .map((s) => {
      if (s.type === "text" || s.type === "reasoning" || s.type === "error") return (s as any).text;
      return "";
    })
    .filter(Boolean)
    .join("\n");
}
