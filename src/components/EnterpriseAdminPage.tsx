import { Show, For, createSignal, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { logError } from "../lib/logger";
import { persistTheme, currentTheme, type Theme } from "../lib/theme";
import BrandFooter from "./BrandFooter";

type AdminTab = "overview" | "llm" | "search";

interface EnterpriseEnt {
  id: string;
  name: string;
  serverUrl: string;
  token: string;
  username: string;
}

/**
 * 企业管理后台首页——从品牌区"管理"按钮打开。
 *
 * 结构：左侧导航（概览/LLM配置/搜索配置）+ 右侧内容区，复用 settings-page 样式。
 * 数据来源：从客户端 settings.json 读当前企业的 serverUrl + token，
 * 调服务端 GET /enterprise/status + GET /models + POST /enterprise/setup。
 */
export default function EnterpriseAdminPage(props: {
  onClose: () => void;
  onSaved?: () => void;
}) {
  const [tab, setTab] = createSignal<AdminTab>("overview");
  const [ent, setEnt] = createSignal<EnterpriseEnt | null>(null);
  const [status, setStatus] = createSignal<{ configured?: boolean; serverName?: string; hasProviders?: boolean; hasSearch?: boolean }>({});
  const [models, setModels] = createSignal<{ id: string; name: string; apiFormat: string; models: { id: string; contextWindow: number; maxTokens?: number }[] }[]>([]);
  const [busy, setBusy] = createSignal(false);
  const [msg, setMsg] = createSignal("");

  // 搜索配置表单
  const [searchEngine, setSearchEngine] = createSignal("");
  const [searchApiKey, setSearchApiKey] = createSignal("");

  // 获取当前企业信息 + 从服务端拉数据。
  onMount(async () => {
    try {
      const json = await invoke<string>("load_settings_json");
      const parsed = JSON.parse(json);
      const active = parsed.activeSpace as string;
      const ents = (parsed.enterprises || []) as EnterpriseEnt[];
      const current = ents.find((e) => e.id === active);
      if (!current) { setMsg("找不到当前企业配置"); return; }
      setEnt(current);

      // 并行拉取 status + models。
      const [statusResp, modelsResp] = await Promise.all([
        fetch(`${current.serverUrl}/enterprise/status`, { headers: { Authorization: `Bearer ${current.token}` } }),
        fetch(`${current.serverUrl}/models`, { headers: { Authorization: `Bearer ${current.token}` } }),
      ]);
      if (statusResp.ok) setStatus(await statusResp.json());
      if (modelsResp.ok) {
        const data = await modelsResp.json() as { providers: { id: string; name: string; apiFormat: string; models: { id: string; contextWindow: number; maxTokens?: number }[] }[] };
        setModels(data.providers || []);
      }
    } catch (err) {
      logError("ui", "Failed to load enterprise admin", err);
      setMsg(`加载失败：${err}`);
    }
  });

  // 保存搜索配置。
  const handleSaveSearch = async () => {
    const e = ent();
    if (!e) return;
    setBusy(true);
    setMsg("");
    try {
      const resp = await fetch(`${e.serverUrl}/enterprise/setup`, {
        method: "POST",
        headers: { "Content-Type": "application/json", Authorization: `Bearer ${e.token}` },
        body: JSON.stringify({ searchEngine: searchEngine(), searchApiKey: searchApiKey() }),
      });
      if (!resp.ok) { const t = await resp.text(); setMsg(`保存失败：${t}`); return; }
      setMsg("搜索配置已保存");
      props.onSaved?.();
    } catch (err) { setMsg(`保存失败：${err}`); } finally { setBusy(false); }
  };

  return (
    <div class="settings-page">
      <div class="settings-header" data-tauri-drag-region>
        <h2 class="settings-title" data-tauri-drag-region>企业管理</h2>
        <button class="settings-close-btn" title="关闭" onClick={props.onClose}>✕</button>
      </div>
      <div class="settings-body">
        <nav class="settings-nav">
          <div class="settings-nav__items">
            <button class="settings-nav-item" classList={{ active: tab() === "overview" }} onClick={() => setTab("overview")}>概览</button>
            <button class="settings-nav-item" classList={{ active: tab() === "llm" }} onClick={() => setTab("llm")}>LLM 配置</button>
            <button class="settings-nav-item" classList={{ active: tab() === "search" }} onClick={() => setTab("search")}>搜索配置</button>
          </div>
          <BrandFooter
            onToggleTheme={() => { const next: Theme = currentTheme() === "light" ? "geek-dark" : "light"; persistTheme(next); }}
            isDarkTheme={currentTheme() !== "light"}
            onCloseSettings={props.onClose}
          />
        </nav>

        <main class="settings-content">
          {/* 概览 */}
          <Show when={tab() === "overview"}>
            <div class="settings-section">
              <h3 class="settings-section-title">企业概况</h3>
              <p class="settings-section-desc">当前企业的基本配置状态。</p>

              <div class="settings-row">
                <label class="settings-label">企业名称</label>
                <div class="settings-control">
                  <span style="font-size: 13px; color: var(--text-primary);">{status().serverName || ent()?.name || "—"}</span>
                </div>
              </div>
              <div class="settings-row">
                <label class="settings-label">企业 ID</label>
                <div class="settings-control">
                  <span style="font-size: 12px; color: var(--text-dim); font-family: var(--font-mono);">{ent()?.id || "—"}</span>
                </div>
              </div>
              <div class="settings-row">
                <label class="settings-label">服务地址</label>
                <div class="settings-control">
                  <span style="font-size: 12px; color: var(--text-dim);">{ent()?.serverUrl || "—"}</span>
                </div>
              </div>
              <div class="settings-row">
                <label class="settings-label">LLM 配置</label>
                <div class="settings-control">
                  <span style="font-size: 12px;">
                    {status().hasProviders ? `✅ 已配置（${models().reduce((n, p) => n + p.models.length, 0)} 个模型）` : "❌ 未配置"}
                  </span>
                </div>
              </div>
              <div class="settings-row">
                <label class="settings-label">搜索配置</label>
                <div class="settings-control">
                  <span style="font-size: 12px;">{status().hasSearch ? "✅ 已配置" : "❌ 未配置"}</span>
                </div>
              </div>
            </div>
          </Show>

          {/* LLM 配置 */}
          <Show when={tab() === "llm"}>
            <div class="settings-section">
              <h3 class="settings-section-title">LLM 服务配置</h3>
              <p class="settings-section-desc">企业员工使用的 LLM 配置，由服务端统一管理。</p>

              <Show when={models().length > 0} fallback={<div class="empty-hint">尚未配置 LLM 服务商</div>}>
                <For each={models()}>
                  {(prov) => (
                    <div class="provider-card">
                      <div class="provider-card__head">
                        <span class="provider-card__name">{prov.name}</span>
                        <span class="provider-card__format">{prov.apiFormat}</span>
                      </div>
                      <div class="provider-card__body">
                        <div class="provider-field">
                          <label>模型列表</label>
                          <div style="font-size: 12px; color: var(--text-secondary); font-family: var(--font-mono);">
                            {prov.models.map((m) => m.id).join("  ·  ")}
                          </div>
                        </div>
                      </div>
                    </div>
                  )}
                </For>
              </Show>

              <div class="settings-row" style="margin-top: 16px;">
                <label class="settings-label" />
                <div class="settings-control">
                  <span style="font-size: 11.5px; color: var(--text-dim);">
                    LLM 配置变更请通过服务端环境变量或 enterprise-config.json 修改。
                  </span>
                </div>
              </div>
            </div>
          </Show>

          {/* 搜索配置 */}
          <Show when={tab() === "search"}>
            <div class="settings-section">
              <h3 class="settings-section-title">搜索服务配置</h3>
              <p class="settings-section-desc">企业员工使用的搜索服务，由服务端统一管理。</p>

              <div class="settings-row">
                <label class="settings-label">搜索引擎</label>
                <div class="settings-control">
                  <select class="settings-select" value={searchEngine()} onChange={(e) => setSearchEngine(e.currentTarget.value)}>
                    <option value="">未配置</option>
                    <option value="exa">Exa</option>
                    <option value="brave">Brave（待实现）</option>
                  </select>
                </div>
              </div>
              <div class="settings-row">
                <label class="settings-label">API Key</label>
                <div class="settings-control">
                  <input class="settings-input" type="password" value={searchApiKey()} placeholder="搜索服务 API Key" onInput={(e) => setSearchApiKey(e.currentTarget.value)} />
                </div>
              </div>
              <div class="settings-row">
                <label class="settings-label" />
                <div class="settings-control">
                  <button class="settings-primary-btn" disabled={busy()} onClick={() => void handleSaveSearch()}>
                    {busy() ? "保存中…" : "保存"}
                  </button>
                </div>
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
        </main>
      </div>
    </div>
  );
}
