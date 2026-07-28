import { createSignal, Show, onMount, onCleanup } from "solid-js";
import { Portal } from "solid-js/web";
import { invoke } from "@tauri-apps/api/core";
import { logError } from "../lib/logger";

interface AppSettings {
  [key: string]: unknown;
}

interface ServerInfo {
  url?: string;
  token?: string;
  username?: string;
  serverName?: string;
}

/**
 * 共用的底部品牌区：左侧「研途教育 AIOA 工作台」品牌名 + 右侧按钮组。
 *
 * 主界面 LeftNav 和设置页 SettingsPage 都用它，保证品牌区样式一致、单一数据源
 * （品牌名只在这里改一次）。品牌名可点击，弹出企业模式服务端配置浮层（连接 /
 * 登录 / 断开企业服务端）——该逻辑从 SettingsPage 迁来，集中到品牌区后设置页
 * 不再保留「服务端」section。
 *
 * 右侧按钮（从左到右）：
 *  - `onToggleTheme`：太阳/月亮图标，在深色 ↔ 浅色间二态切换（主界面用）。
 *  - `onOpenSettings`：齿轮图标，打开设置页（主界面 LeftNav 用）。
 *  - `onCloseSettings`：返回箭头图标，关闭设置页回到首页（设置页用，等同于右上角
 *    的 ✕ 关闭按钮，但鼠标不用移到右上角）。
 * 各按钮可选，不传则不渲染。
 *
 * `onServerChanged`：登录或断开企业服务端后通知父级刷新模型列表（企业模式与
 * 个人模式模型来源不同，切换时必须重新拉取）。
 */
export default function BrandFooter(props: {
  onToggleTheme?: () => void;
  /** 当前是否为深色主题——决定显示月亮（深色）还是太阳（浅色）图标。 */
  isDarkTheme?: boolean;
  onOpenSettings?: () => void;
  onCloseSettings?: () => void;
  onServerChanged?: () => void;
}) {
  // ── 浮层开关 ──
  const [popoverOpen, setPopoverOpen] = createSignal(false);

  // ── 服务端（企业模式）状态 ──
  // 已连接态由 settings().server 派生（有 token 即已连接）；下面的信号只
  // 驱动地址输入、登录表单与异步状态展示。
  const [settings, setSettings] = createSignal<AppSettings>({});
  const serverInfo = () => settings().server as ServerInfo | undefined;
  const [serverUrlInput, setServerUrlInput] = createSignal("");
  const [usernameInput, setUsernameInput] = createSignal("");
  const [passwordInput, setPasswordInput] = createSignal("");
  const [connectStatus, setConnectStatus] = createSignal<"idle" | "connecting" | "connected" | "error">("idle");
  const [loginBusy, setLoginBusy] = createSignal(false);
  const [serverMsg, setServerMsg] = createSignal("");

  // 浮层根节点引用：用于点击外部判定关闭。
  let popoverRef: HTMLDivElement | undefined;

  const connected = () => !!serverInfo()?.token;

  // 读取 settings.json 刷新本地状态（onMount 和登录/断开后回填都用它）。
  const loadSettings = async () => {
    try {
      const json = await invoke<string>("load_settings_json");
      const loaded = json && json !== "{}" ? (JSON.parse(json) as AppSettings) : {};
      setSettings(loaded);
      const svr = loaded.server as { url?: string } | undefined;
      if (svr?.url) setServerUrlInput(svr.url);
    } catch (err) {
      logError("ui", "Failed to load settings", err);
    }
  };

  onMount(() => {
    void loadSettings();
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

  // ── 服务端：连接（调 client-config 验证地址有效性，用 fetch） ──
  const handleServerConnect = async () => {
    const url = serverUrlInput().trim().replace(/\/+$/, "");
    if (!url) {
      setServerMsg("请输入服务地址");
      setConnectStatus("error");
      return;
    }
    setServerUrlInput(url);
    setConnectStatus("connecting");
    setServerMsg("");
    try {
      const resp = await fetch(`${url}/client-config`);
      if (!resp.ok) {
        const text = await resp.text().catch(() => "");
        setConnectStatus("error");
        setServerMsg(`无法连接服务端（${resp.status}）${text ? "：" + text : ""}`);
        return;
      }
      const cfg = (await resp.json()) as { serverName?: string };
      setConnectStatus("connected");
      setServerMsg(cfg.serverName ? `已识别服务：${cfg.serverName}` : "服务地址有效，请登录");
    } catch (err) {
      setConnectStatus("error");
      setServerMsg(`连接失败：${err}`);
    }
  };

  // ── 服务端：登录（调 login_to_server 拿 token 并写回 settings.json） ──
  const handleServerLogin = async () => {
    const url = serverUrlInput().trim().replace(/\/+$/, "");
    const user = usernameInput().trim();
    const pass = passwordInput();
    if (!url || !user || !pass) {
      setServerMsg("服务地址、用户名、密码均不能为空");
      return;
    }
    setLoginBusy(true);
    setServerMsg("");
    try {
      const serverName = await invoke<string>("login_to_server", {
        serverUrl: url,
        username: user,
        password: pass,
      });
      // 后端已把 server 字段写入 settings.json，重新拉取以刷新本地状态。
      await loadSettings();
      setServerMsg(`已连接${serverName ? "：" + serverName : ""}`);
      setUsernameInput("");
      setPasswordInput("");
      // 切换到企业模式后模型列表来源变了，通知父级刷新。
      props.onServerChanged?.();
    } catch (err) {
      setServerMsg(`登录失败：${err}`);
    } finally {
      setLoginBusy(false);
    }
  };

  // ── 服务端：断开（清除 settings.json 的 server 字段） ──
  const handleServerDisconnect = () => {
    const updated = { ...settings() };
    delete updated.server;
    setSettings(updated);
    invoke("save_settings_json", { json: JSON.stringify(updated, null, 2) }).catch((err: unknown) => {
      logError("ui", "Failed to save settings", err);
    });
    setConnectStatus("idle");
    setServerMsg("已断开服务端");
    setUsernameInput("");
    setPasswordInput("");
    // 退回个人模式，模型列表需重新从本地 providers 加载。
    props.onServerChanged?.();
  };

  return (
    <div class="ln-footer">
      <span
        class="ln-brand-name"
        title="点击切换企业模式 / 服务端配置"
        onClick={(e) => {
          // 阻止冒泡，避免触发父容器（如 settings-nav）可能的点击逻辑。
          e.stopPropagation();
          setPopoverOpen((v) => !v);
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
              <div class="brand-popover__title">模式</div>
              <div class="brand-popover__mode">{connected() ? "企业版" : "个人版"}</div>
            </div>
            <div class="brand-popover__body">
              <Show
                when={connected()}
                fallback={
                  <Show
                    when={connectStatus() === "connected"}
                    fallback={
                      // 未连接 + 地址未验证：服务地址输入 + 连接按钮
                      <div class="brand-popover__row">
                        <input
                          class="brand-popover__input"
                          style="flex: 1; min-width: 0;"
                          value={serverUrlInput()}
                          placeholder="http://localhost:3000"
                          onInput={(e) => setServerUrlInput(e.currentTarget.value)}
                          onKeyDown={(e) => { if (e.key === "Enter") handleServerConnect(); }}
                        />
                        <button
                          class="brand-popover__btn"
                          disabled={connectStatus() === "connecting"}
                          onClick={handleServerConnect}
                        >
                          {connectStatus() === "connecting" ? "连接中…" : "连接"}
                        </button>
                      </div>
                    }
                  >
                    {/* 地址已验证，显示登录表单 */}
                    <input
                      class="brand-popover__input"
                      value={serverUrlInput()}
                      disabled
                    />
                    <input
                      class="brand-popover__input"
                      value={usernameInput()}
                      placeholder="用户名"
                      onInput={(e) => setUsernameInput(e.currentTarget.value)}
                      onKeyDown={(e) => { if (e.key === "Enter") handleServerLogin(); }}
                    />
                    <div class="brand-popover__row">
                      <input
                        class="brand-popover__input"
                        style="flex: 1; min-width: 0;"
                        type="password"
                        value={passwordInput()}
                        placeholder="密码"
                        onInput={(e) => setPasswordInput(e.currentTarget.value)}
                        onKeyDown={(e) => { if (e.key === "Enter") handleServerLogin(); }}
                      />
                      <button
                        class="brand-popover__btn"
                        disabled={loginBusy()}
                        onClick={handleServerLogin}
                      >
                        {loginBusy() ? "登录中…" : "登录"}
                      </button>
                    </div>
                  </Show>
                }
              >
                {/* 已连接：显示服务名 + 用户名 + 断开按钮 */}
                <div class="brand-popover__connected">
                  <span class="brand-popover__conn-info">
                    {serverInfo()?.serverName
                      ? `${serverInfo()?.serverName}${serverInfo()?.username ? "（" + serverInfo()?.username + "）" : ""}`
                      : serverInfo()?.url}
                  </span>
                  <button class="settings-danger-btn" onClick={handleServerDisconnect}>
                    断开
                  </button>
                </div>
              </Show>

              <Show when={serverMsg()}>
                <div class="brand-popover__msg">{serverMsg()}</div>
              </Show>
            </div>
          </div>
        </Show>
      </Portal>
    </div>
  );
}
