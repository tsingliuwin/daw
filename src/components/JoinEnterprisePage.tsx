import { Show, createSignal } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import BrandFooter from "./BrandFooter";

/**
 * 加入企业页面——全屏覆盖层，类似设置页的结构。
 *
 * 引导用户：填服务地址 → 连接验证 → 登录 → 加入企业空间。
 * 未来：全新企业引导配置 LLM/搜索。
 */
export default function JoinEnterprisePage(props: {
  onClose: () => void;
  /** 加入成功后通知 App 刷新空间。 */
  onJoined: () => void;
}) {
  const [serverUrl, setServerUrl] = createSignal("");
  const [username, setUsername] = createSignal("");
  const [password, setPassword] = createSignal("");
  const [step, setStep] = createSignal<"address" | "login">("address");
  const [busy, setBusy] = createSignal(false);
  const [msg, setMsg] = createSignal("");

  const handleConnect = async () => {
    const url = serverUrl().trim().replace(/\/+$/, "");
    if (!url) { setMsg("请输入服务地址"); return; }
    setServerUrl(url);
    setBusy(true);
    setMsg("");
    try {
      const resp = await fetch(`${url}/client-config`);
      if (!resp.ok) {
        const text = await resp.text().catch(() => "");
        setMsg(`无法连接服务端（${resp.status}）${text ? "：" + text : ""}`);
        return;
      }
      const cfg = (await resp.json()) as { serverName?: string; enterpriseId?: string };
      setStep("login");
      setMsg(cfg.serverName ? `已识别服务：${cfg.serverName}` : "服务地址有效，请登录");
    } catch (err) {
      setMsg(`连接失败：${err}`);
    } finally {
      setBusy(false);
    }
  };

  const handleJoin = async () => {
    const url = serverUrl().trim().replace(/\/+$/, "");
    const user = username().trim();
    const pass = password();
    if (!url || !user || !pass) { setMsg("服务地址、用户名、密码均不能为空"); return; }
    setBusy(true);
    setMsg("");
    try {
      const name = await invoke<string>("join_enterprise", {
        serverUrl: url,
        username: user,
        password: pass,
      });
      setMsg(`已加入：${name}`);
      props.onJoined();
    } catch (err) {
      setMsg(`加入失败：${err}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div class="settings-page">
      <div class="settings-header" data-tauri-drag-region>
        <h2 class="settings-title" data-tauri-drag-region>加入企业</h2>
        <button class="settings-close-btn" title="关闭" onClick={props.onClose}>✕</button>
      </div>
      <div class="settings-body">
        <nav class="settings-nav">
          <div class="settings-nav__items">
            <button class="settings-nav-item active">企业连接</button>
          </div>
          <BrandFooter onCloseSettings={props.onClose} />
        </nav>

        <main class="settings-content">
          <div class="settings-section">
            <h3 class="settings-section-title">连接企业服务端</h3>
            <p class="settings-section-desc">
              输入企业部署的 AIOA 服务端地址，连接后用企业账号登录即可使用。
              模型和搜索由服务端统一管理，无需单独配置。
            </p>

            <Show when={step() === "address"}>
              <div class="settings-row">
                <label class="settings-label">服务地址</label>
                <div class="settings-control" style="display: flex; gap: 8px; align-items: center;">
                  <input
                    class="settings-input"
                    style="flex: 1; min-width: 0;"
                    value={serverUrl()}
                    placeholder="http://localhost:3000"
                    onInput={(e) => setServerUrl(e.currentTarget.value)}
                    onKeyDown={(e) => { if (e.key === "Enter") void handleConnect(); }}
                  />
                  <button
                    class="settings-primary-btn"
                    style="flex-shrink: 0;"
                    disabled={busy()}
                    onClick={() => void handleConnect()}
                  >
                    {busy() ? "连接中…" : "连接"}
                  </button>
                </div>
              </div>
            </Show>

            <Show when={step() === "login"}>
              <div class="settings-row">
                <label class="settings-label">服务地址</label>
                <div class="settings-control">
                  <input class="settings-input" value={serverUrl()} disabled style="color: var(--text-secondary);" />
                </div>
              </div>
              <div class="settings-row">
                <label class="settings-label">用户名</label>
                <div class="settings-control">
                  <input
                    class="settings-input"
                    value={username()}
                    placeholder="用户名"
                    onInput={(e) => setUsername(e.currentTarget.value)}
                    onKeyDown={(e) => { if (e.key === "Enter") void handleJoin(); }}
                  />
                </div>
              </div>
              <div class="settings-row">
                <label class="settings-label">密码</label>
                <div class="settings-control" style="display: flex; gap: 8px; align-items: center;">
                  <input
                    class="settings-input"
                    style="flex: 1; min-width: 0;"
                    type="password"
                    value={password()}
                    placeholder="密码"
                    onInput={(e) => setPassword(e.currentTarget.value)}
                    onKeyDown={(e) => { if (e.key === "Enter") void handleJoin(); }}
                  />
                  <button
                    class="settings-primary-btn"
                    style="flex-shrink: 0;"
                    disabled={busy()}
                    onClick={() => void handleJoin()}
                  >
                    {busy() ? "加入中…" : "加入"}
                  </button>
                </div>
              </div>
              <div class="settings-row">
                <label class="settings-label" />
                <div class="settings-control">
                  <button
                    class="settings-secondary-btn"
                    onClick={() => { setStep("address"); setMsg(""); setUsername(""); setPassword(""); }}
                  >
                    ← 返回修改地址
                  </button>
                </div>
              </div>
            </Show>

            <Show when={msg()}>
              <div class="settings-row">
                <label class="settings-label" />
                <div class="settings-control">
                  <span style="color: var(--text-secondary); font-size: 12px;">{msg()}</span>
                </div>
              </div>
            </Show>
          </div>
        </main>
      </div>
    </div>
  );
}
