import { Show, createSignal } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import BrandFooter from "./BrandFooter";

/**
 * 加入企业页面——全屏覆盖层，类似设置页的结构。
 *
 * 流程：粘贴签名链接/地址 → 认证/登录 → （首次）配置企业信息 → 进入企业空间。
 */
export default function JoinEnterprisePage(props: {
  onClose: () => void;
  /** 加入成功后通知 App 刷新空间。 */
  onJoined: () => void;
  /** 模式："join"=加入企业（默认），"admin"=企业管理（直接显示配置表单）。 */
  mode?: "join" | "admin";
}) {
  const [serverUrl, setServerUrl] = createSignal("");
  const [username, setUsername] = createSignal("");
  const [password, setPassword] = createSignal("");
  const isAdmin = props.mode === "admin";
  const [step, setStep] = createSignal<"address" | "login" | "config">(isAdmin ? "config" : "address");
  const [busy, setBusy] = createSignal(false);
  const [msg, setMsg] = createSignal("");

  // ── 企业配置表单状态 ──
  const [entName, setEntName] = createSignal("");
  const [llmEndpoint, setLlmEndpoint] = createSignal("");
  const [llmApiKey, setLlmApiKey] = createSignal("");
  const [llmApiFormat, setLlmApiFormat] = createSignal("openai");
  const [llmModels, setLlmModels] = createSignal("");

  // 管理模式：加载当前企业的配置（从服务端 GET /enterprise/status 拉取）。
  if (isAdmin) {
    void (async () => {
      try {
        const settings = await invoke<Record<string, unknown>>("load_settings_json");
        const parsed = typeof settings === "string" ? JSON.parse(settings) : settings;
        const active = parsed.activeSpace as string;
        const ents = parsed.enterprises as { id: string; name: string; serverUrl: string; token: string }[];
        const ent = ents?.find((e) => e.id === active);
        if (!ent) { setMsg("找不到当前企业配置"); return; }
        // 从服务端拉取当前企业配置。
        const resp = await fetch(`${ent.serverUrl}/enterprise/status`, {
          headers: { Authorization: `Bearer ${ent.token}` },
        });
        if (resp.ok) {
          const cfg = await resp.json() as { serverName?: string; configured?: boolean };
          setEntName(cfg.serverName ?? ent.name);
        }
      } catch (err) {
        setMsg(`加载企业配置失败：${err}`);
      }
    })();
  }

  // 判断输入是否为签名 URL（含 /auth/setup?token=）
  const isSetupUrl = (input: string) => input.includes("/auth/setup?token=");

  const handleConnect = async () => {
    const input = serverUrl().trim();
    if (!input) { setMsg("请输入服务地址或签名链接"); return; }

    // 如果粘贴的是签名 URL，直接走 setup 流程。
    if (isSetupUrl(input)) {
      setBusy(true);
      setMsg("正在通过签名链接认证…");
      try {
        const result = await invoke<{ serverName: string; needsConfig: boolean }>(
          "join_enterprise_via_setup", { setupUrl: input }
        );
        if (result.needsConfig) {
          setEntName(result.serverName || "");
          setStep("config");
          setMsg("认证成功！请完成企业信息配置。");
        } else {
          setMsg(`已加入：${result.serverName}`);
          props.onJoined();
        }
      } catch (err) {
        setMsg(`认证失败：${err}`);
      } finally {
        setBusy(false);
      }
      return;
    }

    // 普通地址：走 client-config 验证 + 登录流程。
    const url = input.replace(/\/+$/, "");
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

  // 保存企业配置（调 setup_enterprise 命令 → POST /enterprise/setup）。
  const handleSetupEnterprise = async () => {
    if (!entName().trim()) { setMsg("请输入企业名称"); return; }
    if (!llmEndpoint().trim() || !llmApiKey().trim()) { setMsg("请填写 LLM 服务地址和 API Key"); return; }
    if (!llmModels().trim()) { setMsg("请填写至少一个模型 ID"); return; }
    setBusy(true);
    setMsg("");
    try {
      const models = llmModels().split("\n").map(s => s.trim()).filter(s => s).map(id => ({
        id,
        contextWindow: 256000,
        maxTokens: 64000,
      }));
      const configJson = JSON.stringify({
        serverName: entName().trim(),
        providers: [{
          id: "primary",
          name: entName().trim(),
          endpoint: llmEndpoint().trim(),
          apiKey: llmApiKey().trim(),
          apiFormat: llmApiFormat(),
          models,
          enabled: true,
        }],
      });
      await invoke("setup_enterprise", { configJson });
      setMsg("企业配置已保存！");
      props.onJoined();
    } catch (err) {
      setMsg(`保存失败：${err}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div class="settings-page">
      <div class="settings-header" data-tauri-drag-region>
        <h2 class="settings-title" data-tauri-drag-region>{isAdmin ? "企业管理" : "加入企业"}</h2>
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
              粘贴服务端首次启动生成的签名链接可一键认证；或输入服务地址后用账号密码登录。
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
                    placeholder="粘贴签名链接或输入 http://localhost:3000"
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

            {/* 步骤 3：企业信息配置（首次认证后） */}
            <Show when={step() === "config"}>
              <div class="settings-row">
                <label class="settings-label">企业名称 <span class="provider-field__hint">必填</span></label>
                <div class="settings-control">
                  <input
                    class="settings-input"
                    value={entName()}
                    placeholder="如：研途教育"
                    onInput={(e) => setEntName(e.currentTarget.value)}
                  />
                </div>
              </div>

              <h4 class="settings-section-title" style="margin-top: 20px;">LLM 服务配置</h4>
              <p class="settings-section-desc">企业员工将使用此配置，无需各自申请 API Key。</p>

              <div class="settings-row">
                <label class="settings-label">Base URL <span class="provider-field__hint">填到 /v1</span></label>
                <div class="settings-control">
                  <input
                    class="settings-input"
                    value={llmEndpoint()}
                    placeholder="https://api.openai.com/v1"
                    onInput={(e) => setLlmEndpoint(e.currentTarget.value)}
                  />
                </div>
              </div>
              <div class="settings-row">
                <label class="settings-label">API Key</label>
                <div class="settings-control">
                  <input
                    class="settings-input"
                    type="password"
                    value={llmApiKey()}
                    placeholder="sk-..."
                    onInput={(e) => setLlmApiKey(e.currentTarget.value)}
                  />
                </div>
              </div>
              <div class="settings-row">
                <label class="settings-label">API 格式</label>
                <div class="settings-control">
                  <select
                    class="settings-select"
                    value={llmApiFormat()}
                    onChange={(e) => setLlmApiFormat(e.currentTarget.value)}
                  >
                    <option value="openai">openai（兼容/OpenAI）</option>
                    <option value="anthropic">anthropic（Claude）</option>
                    <option value="responses">responses（OpenAI Responses）</option>
                  </select>
                </div>
              </div>
              <div class="settings-row">
                <label class="settings-label">模型 ID <span class="provider-field__hint">每行一个</span></label>
                <div class="settings-control">
                  <textarea
                    class="settings-input"
                    style="min-height: 60px; resize: vertical; font-family: var(--font-mono);"
                    value={llmModels()}
                    placeholder={"如：\ndeepseek-v4-pro\ngpt-4o"}
                    onInput={(e) => setLlmModels(e.currentTarget.value)}
                  />
                </div>
              </div>
              <div class="settings-row">
                <label class="settings-label" />
                <div class="settings-control">
                  <button
                    class="settings-primary-btn"
                    disabled={busy()}
                    onClick={() => void handleSetupEnterprise()}
                  >
                    {busy() ? "保存中…" : "保存并进入企业空间"}
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
