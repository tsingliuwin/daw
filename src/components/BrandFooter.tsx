import { createSignal, Show, onMount, onCleanup, For } from "solid-js";
import { Portal } from "solid-js/web";
import { invoke } from "@tauri-apps/api/core";
import { logError } from "../lib/logger";

/** 后端 get_enterprises 返回的已加入企业项（与 commands::EnterpriseInfo 对应，
 * serde 把 server_url 序列化成 serverUrl；token 故意不下发）。 */
interface EnterpriseInfo {
  id: string;
  name: string;
  serverUrl: string;
  username: string;
}

/**
 * 共用的底部品牌区：左侧「研途教育 AIOA 工作台」品牌名 + 右侧按钮组。
 *
 * 主界面 LeftNav 和设置页 SettingsPage 都用它，保证品牌区样式一致、单一数据源
 * （品牌名只在这里改一次）。品牌名可点击，弹出「多企业空间」面板：展示当前
 * 空间、已加入的企业列表（可点击切换、可断开），以及「加入企业」向导（服务地址
 * 连接 → 用户名/密码登录 → join_enterprise）。
 *
 * 右侧按钮（从左到右）：
 *  - `onToggleTheme`：太阳/月亮图标，在深色 ↔ 浅色间二态切换（主界面用）。
 *  - `onOpenSettings`：齿轮图标，打开设置页（主界面 LeftNav 用）。
 *  - `onCloseSettings`：返回箭头图标，关闭设置页回到首页（设置页用，等同于右上角
 *    的 ✕ 关闭按钮，但鼠标不用移到右上角）。
 * 各按钮可选，不传则不渲染。
 *
 * `activeSpace`：父级当前激活空间，用作面板打开前的初始显示；面板打开时仍会从
 * 后端 get_active_space 拉取最新值。`onSpaceChanged`：加入/断开/切换空间后
 * 通知父级重新加载工作区与任务（并刷新模型列表——企业空间与个人空间模型来源
 * 不同）。
 *
 * `onServerChanged`：兼容 SettingsPage 旧用法保留（空间面板不触发）。
 */
export default function BrandFooter(props: {
  onToggleTheme?: () => void;
  /** 当前是否为深色主题——决定显示月亮（深色）还是太阳（浅色）图标。 */
  isDarkTheme?: boolean;
  onOpenSettings?: () => void;
  onCloseSettings?: () => void;
  /** 父级当前激活空间，用作面板初始显示。 */
  activeSpace?: string;
  /** 空间切换/加入/断开后通知父级刷新。 */
  onSpaceChanged?: () => void;
  /** 兼容 SettingsPage 旧用法（空间面板不触发）。 */
  onServerChanged?: () => void;
}) {
  // ── 浮层开关 ──
  const [popoverOpen, setPopoverOpen] = createSignal(false);

  // ── 空间状态（面板打开时从后端刷新） ──
  const [enterprises, setEnterprises] = createSignal<EnterpriseInfo[]>([]);
  const [activeSpace, setActiveSpace] = createSignal<string>(props.activeSpace ?? "personal");

  // ── 加入企业向导状态 ──
  const [wizardOpen, setWizardOpen] = createSignal(false);
  const [serverUrlInput, setServerUrlInput] = createSignal("");
  const [usernameInput, setUsernameInput] = createSignal("");
  const [passwordInput, setPasswordInput] = createSignal("");
  const [connectStatus, setConnectStatus] = createSignal<"idle" | "connecting" | "connected" | "error">("idle");
  const [joinBusy, setJoinBusy] = createSignal(false);
  const [wizardMsg, setWizardMsg] = createSignal("");

  // 浮层根节点引用：用于点击外部判定关闭。
  let popoverRef: HTMLDivElement | undefined;

  // 当前空间展示名：personal → 「个人空间」；否则查企业名。
  const currentSpaceName = () => {
    const sp = activeSpace();
    if (!sp || sp === "personal") return "个人空间";
    const ent = enterprises().find((e) => e.id === sp);
    return ent ? ent.name : sp;
  };

  // 从后端拉取企业列表与当前空间（面板打开时刷新）。
  const refreshSpaces = async () => {
    try {
      const [ents, sp] = await Promise.all([
        invoke<EnterpriseInfo[]>("get_enterprises").catch(() => [] as EnterpriseInfo[]),
        invoke<string>("get_active_space").catch(() => "personal"),
      ]);
      setEnterprises(ents);
      setActiveSpace(sp);
    } catch (err) {
      logError("ui", "Failed to load spaces", err);
    }
  };

  // 切到指定空间（personal 或企业 id）：后端 set_active_space 后通知父级刷新。
  const switchTo = async (spaceId: string) => {
    try {
      await invoke("set_active_space", { spaceId });
      setActiveSpace(spaceId);
      setPopoverOpen(false);
      props.onSpaceChanged?.();
    } catch (err) {
      logError("ui", "Failed to switch space", err);
    }
  };

  // 断开某企业：leave_enterprise 后刷新列表并通知父级（active space 可能被
  // 后端改回 personal）。
  const disconnect = async (enterpriseId: string) => {
    try {
      await invoke("leave_enterprise", { enterpriseId });
      await refreshSpaces();
      props.onSpaceChanged?.();
    } catch (err) {
      logError("ui", "Failed to leave enterprise", err);
    }
  };

  // 加入向导 - 连接服务地址（fetch {url}/client-config 拿 serverName 验证有效性）。
  const handleConnect = async () => {
    const url = serverUrlInput().trim().replace(/\/+$/, "");
    if (!url) {
      setWizardMsg("请输入服务地址");
      setConnectStatus("error");
      return;
    }
    setServerUrlInput(url);
    setConnectStatus("connecting");
    setWizardMsg("");
    try {
      const resp = await fetch(`${url}/client-config`);
      if (!resp.ok) {
        const text = await resp.text().catch(() => "");
        setConnectStatus("error");
        setWizardMsg(`无法连接服务端（${resp.status}）${text ? "：" + text : ""}`);
        return;
      }
      const cfg = (await resp.json()) as { serverName?: string };
      setConnectStatus("connected");
      setWizardMsg(cfg.serverName ? `已识别服务：${cfg.serverName}` : "服务地址有效，请登录");
    } catch (err) {
      setConnectStatus("error");
      setWizardMsg(`连接失败：${err}`);
    }
  };

  // 加入向导 - 登录（join_enterprise：参数 serverUrl/username/password，返回 serverName）。
  const handleJoin = async () => {
    const url = serverUrlInput().trim().replace(/\/+$/, "");
    const user = usernameInput().trim();
    const pass = passwordInput();
    if (!url || !user || !pass) {
      setWizardMsg("服务地址、用户名、密码均不能为空");
      return;
    }
    setJoinBusy(true);
    setWizardMsg("");
    try {
      const serverName = await invoke<string>("join_enterprise", {
        serverUrl: url,
        username: user,
        password: pass,
      });
      // 成功：关闭向导与面板，通知父级刷新（App 会重载工作区/任务/模型）。
      setWizardOpen(false);
      setPopoverOpen(false);
      setServerUrlInput("");
      setUsernameInput("");
      setPasswordInput("");
      setConnectStatus("idle");
      setWizardMsg(serverName ? `已加入：${serverName}` : "");
      props.onSpaceChanged?.();
    } catch (err) {
      setWizardMsg(`加入失败：${err}`);
    } finally {
      setJoinBusy(false);
    }
  };

  onMount(() => {
    // 点击浮层外 + 品牌名外则关闭（与 Select 组件的 handleClickOutside 同思路）。
    const onDocClick = (e: MouseEvent) => {
      if (!popoverOpen()) return;
      const target = e.target as Node;
      if (
        popoverRef &&
        !popoverRef.contains(target) &&
        !(target as Element).closest?.(".ln-brand-name")
      ) {
        setPopoverOpen(false);
      }
    };
    document.addEventListener("mousedown", onDocClick);
    onCleanup(() => document.removeEventListener("mousedown", onDocClick));
  });

  return (
    <div class="ln-footer">
      <span
        class="ln-brand-name"
        title="点击切换空间 / 加入企业"
        onClick={(e) => {
          // 阻止冒泡，避免触发父容器（如 settings-nav）可能的点击逻辑。
          e.stopPropagation();
          const willOpen = !popoverOpen();
          setPopoverOpen(willOpen);
          if (willOpen) void refreshSpaces();
        }}
      >研途教育 AIOA 工作台</span>
      <div class="ln-footer-actions">
        {props.onToggleTheme && (
          <button
            class="ln-footer-settings"
            title={props.isDarkTheme ? "切换到浅色" : "切换到深色"}
            onClick={() => props.onToggleTheme?.()}
          >
            {/* 深色时显示太阳（点一下变浅色）；浅色时显示月亮（点一下变深色） */}
            <Show when={props.isDarkTheme} fallback={
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 16px; height: 16px;">
                <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"></path>
              </svg>
            }>
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 16px; height: 16px;">
                <circle cx="12" cy="12" r="5"></circle>
                <line x1="12" y1="1" x2="12" y2="3"></line>
                <line x1="12" y1="21" x2="12" y2="23"></line>
                <line x1="4.22" y1="4.22" x2="5.64" y2="5.64"></line>
                <line x1="18.36" y1="18.36" x2="19.78" y2="19.78"></line>
                <line x1="1" y1="12" x2="3" y2="12"></line>
                <line x1="21" y1="12" x2="23" y2="12"></line>
                <line x1="4.22" y1="19.78" x2="5.64" y2="18.36"></line>
                <line x1="18.36" y1="5.64" x2="19.78" y2="4.22"></line>
              </svg>
            </Show>
          </button>
        )}
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
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 16px; height: 16px;">
              <line x1="19" y1="12" x2="5" y2="12"></line>
              <polyline points="12 19 5 12 12 5"></polyline>
            </svg>
          </button>
        )}
      </div>

      {/* 浮层通过 Portal 渲染到 document.body，脱离父级 overflow 容器 */}
      <Portal>
        <Show when={popoverOpen()}>
          <div class="brand-popover" ref={popoverRef}>
            <div class="brand-popover__header">
              <div class="brand-popover__title">空间</div>
              <div class="brand-popover__mode">{currentSpaceName()}</div>
            </div>
            <div class="brand-popover__body">
              <Show
                when={!wizardOpen()}
                fallback={
                  <Show
                    when={connectStatus() === "connected"}
                    fallback={
                      // 步骤 1：服务地址输入 + 连接
                      <div class="brand-popover__row">
                        <input
                          class="brand-popover__input"
                          style="flex: 1; min-width: 0;"
                          value={serverUrlInput()}
                          placeholder="http://localhost:3000"
                          onInput={(e) => setServerUrlInput(e.currentTarget.value)}
                          onKeyDown={(e) => { if (e.key === "Enter") void handleConnect(); }}
                        />
                        <button
                          class="brand-popover__btn"
                          disabled={connectStatus() === "connecting"}
                          onClick={() => void handleConnect()}
                        >
                          {connectStatus() === "connecting" ? "连接中…" : "连接"}
                        </button>
                      </div>
                    }
                  >
                    {/* 步骤 2：用户名/密码 + 登录（加入企业） */}
                    <input class="brand-popover__input" value={serverUrlInput()} disabled />
                    <input
                      class="brand-popover__input"
                      value={usernameInput()}
                      placeholder="用户名"
                      onInput={(e) => setUsernameInput(e.currentTarget.value)}
                      onKeyDown={(e) => { if (e.key === "Enter") void handleJoin(); }}
                    />
                    <div class="brand-popover__row">
                      <input
                        class="brand-popover__input"
                        style="flex: 1; min-width: 0;"
                        type="password"
                        value={passwordInput()}
                        placeholder="密码"
                        onInput={(e) => setPasswordInput(e.currentTarget.value)}
                        onKeyDown={(e) => { if (e.key === "Enter") void handleJoin(); }}
                      />
                      <button
                        class="brand-popover__btn"
                        disabled={joinBusy()}
                        onClick={() => void handleJoin()}
                      >
                        {joinBusy() ? "加入中…" : "登录"}
                      </button>
                    </div>
                  </Show>
                }
              >
                {/* 空间列表：个人空间 + 已加入企业（可切换/断开） */}
                <div class="brand-popover__space-list">
                  <div
                    class="brand-popover__space-item"
                    classList={{ "brand-popover__space-item--active": activeSpace() === "personal" || !activeSpace() }}
                    onClick={() => void switchTo("personal")}
                  >
                    <span class="brand-popover__space-name">个人空间</span>
                  </div>
                  <Show
                    when={enterprises().length > 0}
                    fallback={<div class="brand-popover__hint">尚未加入企业</div>}
                  >
                    <For each={enterprises()}>
                      {(ent) => (
                        <div
                          class="brand-popover__space-item"
                          classList={{ "brand-popover__space-item--active": activeSpace() === ent.id }}
                        >
                          <span
                            class="brand-popover__space-name"
                            onClick={() => void switchTo(ent.id)}
                          >{ent.name}</span>
                          <button
                            class="brand-popover__disconnect"
                            title="断开企业"
                            onClick={(e) => { e.stopPropagation(); void disconnect(ent.id); }}
                          >断开</button>
                        </div>
                      )}
                    </For>
                  </Show>
                </div>
                <button class="brand-popover__btn" onClick={() => setWizardOpen(true)}>加入企业</button>
              </Show>

              <Show when={wizardOpen()}>
                <button class="brand-popover__link" onClick={() => setWizardOpen(false)}>返回空间列表</button>
              </Show>

              <Show when={wizardMsg()}>
                <div class="brand-popover__msg">{wizardMsg()}</div>
              </Show>
            </div>
          </div>
        </Show>
      </Portal>
    </div>
  );
}
