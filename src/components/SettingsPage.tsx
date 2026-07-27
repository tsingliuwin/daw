import { For, Show, createSignal, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { logError } from "../lib/logger";
import { currentTheme, setCurrentTheme, currentZoom, setCurrentZoom, type Theme } from "../lib/theme";
import BrandFooter from "./BrandFooter";

// ---------------------------------------------------------------------------
// Provider / settings model. 镜像后端 settings.json 结构
// （agent/config.rs 的 ModelProvider，camelCase）。改动需同步两侧。
// ---------------------------------------------------------------------------
export interface ModelItem {
  id: string;
  contextWindow: number;
  maxTokens?: number;
}

export interface ModelProvider {
  id: string;
  name: string;
  endpoint: string;
  apiKey: string;
  apiFormat: "openai" | "anthropic" | "responses";
  models: ModelItem[];
  enabled: boolean;
  isPredefined?: boolean;
}

interface AppSettings {
  theme?: string;
  language?: string;
  zoom?: string;
  providers?: ModelProvider[];
  [key: string]: unknown;
}

// 单个模型的连通性测试状态。
interface ModelTestEntry {
  status: "idle" | "testing" | "success" | "error";
  msg?: string;
}

type SettingsTab = "general" | "modelSettings";

const API_FORMATS: ModelProvider["apiFormat"][] = ["openai", "anthropic", "responses"];

/**
 * 设置页。相比 lakemind 大幅瘦身：
 *  - 只保留 general（主题/缩放）与 modelSettings（LLM provider 配置）两个 tab。
 *  - 删除 databases / systemPrompt / tenets / sampling 等 tab 及其全部逻辑。
 *  - provider 配置的 load/save 用 load_settings_json / save_settings_json，
 *    结构是 {providers: [...]}。
 *  - 「批量测试连通性」调 test_llm_connection，「全绿才允许启用」门禁保留。
 */
export default function SettingsPage(props: {
  /** 关闭设置页（返回主界面）。 */
  onClose: () => void;
  /** 初始 tab。 */
  initialTab?: SettingsTab;
  /** providers 变更后通知父级刷新可用模型列表。 */
  onProvidersChanged?: (providers: ModelProvider[]) => void;
}) {
  const [activeTab, setActiveTab] = createSignal<SettingsTab>(props.initialTab ?? "general");
  const [settings, setSettings] = createSignal<AppSettings>({});
  const [selectedProvider, setSelectedProvider] = createSignal<string>("");
  // API Key 显示/隐藏状态：providerKey → bool（true=明文显示）。新 provider 用 NEW_PROVIDER_TEST_KEY。
  const [showApiKeys, setShowApiKeys] = createSignal<Record<string, boolean>>({});
  const toggleApiKey = (key: string) => setShowApiKeys((p) => ({ ...p, [key]: !p[key] }));

  // ── 模型连通性测试状态：providerKey → { modelId → entry } ──
  const [modelTests, setModelTests] = createSignal<Record<string, Record<string, ModelTestEntry>>>({});
  const [batchProgress, setBatchProgress] = createSignal<{ providerKey: string; current: number; total: number; modelId: string } | null>(null);
  const NEW_PROVIDER_TEST_KEY = "__new_provider__";

  // ── 添加新 provider 的临时表单状态 ──
  const [isAddingProvider, setIsAddingProvider] = createSignal(false);
  const [newProviderName, setNewProviderName] = createSignal("");
  const [newProviderEndpoint, setNewProviderEndpoint] = createSignal("");
  const [newProviderApiKey, setNewProviderApiKey] = createSignal("");
  const [newProviderFormat, setNewProviderFormat] = createSignal<ModelProvider["apiFormat"]>("openai");
  const [newProviderModels, setNewProviderModels] = createSignal<ModelItem[]>([]);

  // 编辑 provider 名称的内联输入。
  const [editingProviderId, setEditingProviderId] = createSignal<string | null>(null);
  const [tempName, setTempName] = createSignal("");

  onMount(async () => {
    try {
      const json = await invoke<string>("load_settings_json");
      if (json && json !== "{}") {
        const loaded = JSON.parse(json) as AppSettings;
        setSettings(loaded);
        if (loaded.providers && loaded.providers.length > 0) {
          setSelectedProvider(loaded.providers[0].id);
        }
      }
    } catch (err) {
      logError("ui", "Failed to load settings", err);
    }
  });

  // ── 持久化 helper ──
  const updateSetting = (key: keyof AppSettings, value: unknown) => {
    const updated = { ...settings(), [key]: value };
    setSettings(updated);
    invoke("save_settings_json", { json: JSON.stringify(updated, null, 2) }).catch((err: unknown) => {
      logError("ui", "Failed to save settings", err);
    });
    if (key === "providers") {
      props.onProvidersChanged?.((updated.providers as ModelProvider[]) ?? []);
    }
  };

  const updateProviderProperty = (providerId: string, property: keyof ModelProvider, value: unknown) => {
    if (property === "endpoint" || property === "apiKey" || property === "apiFormat" || property === "models") {
      clearModelTests(providerId);
    }
    const updatedProviders = (settings().providers || []).map((p) => (p.id === providerId ? { ...p, [property]: value } : p));
    updateSetting("providers", updatedProviders);
  };

  const handleDeleteProvider = (id: string) => {
    const updated = (settings().providers || []).filter((p) => p.id !== id);
    updateSetting("providers", updated);
    setSelectedProvider(updated.length > 0 ? updated[0].id : "");
  };

  // ── 连通性测试 ──
  const setModelEntry = (providerKey: string, modelId: string, entry: ModelTestEntry) => {
    setModelTests((prev) => ({
      ...prev,
      [providerKey]: { ...(prev[providerKey] || {}), [modelId]: entry },
    }));
  };

  const clearModelTests = (providerKey: string) => {
    setModelTests((prev) => {
      if (!(providerKey in prev)) return prev;
      const next = { ...prev };
      delete next[providerKey];
      return next;
    });
    if (batchProgress()?.providerKey === providerKey) setBatchProgress(null);
  };

  /** 所有模型测试通过（全绿）才允许启用该 provider。 */
  const providerValidated = (providerKey: string, models: ModelItem[]): boolean => {
    if (!models.length) return false;
    const entries = modelTests()[providerKey] || {};
    return models.every((m) => entries[m.id]?.status === "success");
  };

  const batchTesting = (providerKey: string): boolean => batchProgress()?.providerKey === providerKey;

  // 串行批量测试一个 provider 下的所有模型，逐个更新状态并报告进度。
  const runBatchModelTest = async (
    providerKey: string,
    endpoint: string,
    apiKey: string,
    apiFormat: string,
    models: ModelItem[],
  ): Promise<boolean> => {
    const ep = (endpoint || "").trim();
    const key = (apiKey || "").trim();
    if (!ep || !key || !models.length) {
      for (const m of models) setModelEntry(providerKey, m.id, { status: "error", msg: "Base URL、API Key、模型列表均不能为空" });
      return false;
    }
    let allOk = true;
    for (let i = 0; i < models.length; i++) {
      const m = models[i];
      setBatchProgress({ providerKey, current: i + 1, total: models.length, modelId: m.id });
      setModelEntry(providerKey, m.id, { status: "testing" });
      try {
        await invoke("test_llm_connection", { endpoint: ep, apiKey: key, apiFormat, modelId: m.id });
        setModelEntry(providerKey, m.id, { status: "success" });
      } catch (err) {
        setModelEntry(providerKey, m.id, { status: "error", msg: String(err) });
        allOk = false;
      }
    }
    setBatchProgress(null);
    return allOk;
  };

  const handleTestModelConnection = (prov: ModelProvider) =>
    runBatchModelTest(prov.id, prov.endpoint, prov.apiKey, prov.apiFormat, prov.models || []);

  const handleTestNewProviderConnection = () =>
    runBatchModelTest(NEW_PROVIDER_TEST_KEY, newProviderEndpoint(), newProviderApiKey(), newProviderFormat(), newProviderModels());

  // 启用门禁：只有全绿通过才允许 enabled=true。
  const handleToggleProviderEnabled = (prov: ModelProvider, enabled: boolean) => {
    if (enabled && !providerValidated(prov.id, prov.models || [])) {
      alert("请先点击「测试连通性」并确保全部模型测试通过后再启用。");
      return;
    }
    updateProviderProperty(prov.id, "enabled", enabled);
  };

  // ── 添加新 provider ──
  const handleCreateNewProvider = async () => {
    const name = newProviderName().trim();
    const endpoint = newProviderEndpoint().trim();
    const apiKey = newProviderApiKey().trim();
    const format = newProviderFormat();
    const models = newProviderModels();
    if (!name) { alert("请输入服务商名称"); return; }
    if (!endpoint) { alert("请输入 Base URL"); return; }
    if (!models.length) { alert("请至少添加一个模型"); return; }

    const newId = "custom_" + Date.now();
    const newProvider: ModelProvider = { id: newId, name, endpoint, apiKey, apiFormat: format, models, enabled: false };
    const updated = [...(settings().providers || []), newProvider];
    updateSetting("providers", updated);
    setSelectedProvider(newId);
    setIsAddingProvider(false);
    setNewProviderName("");
    setNewProviderEndpoint("");
    setNewProviderApiKey("");
    setNewProviderFormat("openai");
    setNewProviderModels([]);
    clearModelTests(NEW_PROVIDER_TEST_KEY);
  };

  const handleSaveProviderName = () => {
    const val = tempName().trim();
    if (val && editingProviderId()) updateProviderProperty(editingProviderId()!, "name", val);
    setEditingProviderId(null);
  };

  // 选中 provider（如果只有一个或刚切过来）。
  const currentProviders = () => settings().providers || [];

  return (
    <div class="settings-page">
      <div class="settings-header">
        <h2 class="settings-title">设置</h2>
        <button class="settings-close-btn" title="关闭" onClick={props.onClose}>✕</button>
      </div>
      <div class="settings-body">
        <nav class="settings-nav">
          <div class="settings-nav__items">
            <button
              class="settings-nav-item"
              classList={{ active: activeTab() === "general" }}
              onClick={() => setActiveTab("general")}
            >通用</button>
            <button
              class="settings-nav-item"
              classList={{ active: activeTab() === "modelSettings" }}
              onClick={() => setActiveTab("modelSettings")}
            >模型服务商</button>
          </div>
          {/* 共用品牌区：主题切换 + 返回首页（等同右上角关闭，免去鼠标移动） */}
          <BrandFooter
            onToggleTheme={() => {
              const next: Theme = currentTheme() === "light" ? "geek-dark" : "light";
              setCurrentTheme(next);
            }}
            isDarkTheme={currentTheme() !== "light"}
            onCloseSettings={props.onClose}
          />
        </nav>

        <main class="settings-content">
          <Show when={activeTab() === "general"}>
            <div class="settings-section">
              <h3 class="settings-section-title">通用设置</h3>
              <p class="settings-section-desc">界面主题与缩放等基础偏好。</p>

              <div class="settings-row">
                <label class="settings-label">主题</label>
                <div class="settings-control">
                  <select
                    class="settings-select"
                    value={currentTheme()}
                    onChange={(e) => setCurrentTheme(e.currentTarget.value as Theme)}
                  >
                    <option value="geek-dark">极客深色</option>
                    <option value="classic-dark">经典深色</option>
                    <option value="light">浅色</option>
                  </select>
                </div>
              </div>

              <div class="settings-row">
                <label class="settings-label">缩放</label>
                <div class="settings-control">
                  <select
                    class="settings-select"
                    value={String(currentZoom())}
                    onChange={(e) => setCurrentZoom(parseInt(e.currentTarget.value, 10))}
                  >
                    <option value="80">80%</option>
                    <option value="90">90%</option>
                    <option value="100">100%</option>
                    <option value="110">110%</option>
                    <option value="125">125%</option>
                    <option value="150">150%</option>
                  </select>
                </div>
              </div>
            </div>
          </Show>

          <Show when={activeTab() === "modelSettings"}>
            <div class="settings-section">
              <div class="settings-section-head">
                <div>
                  <h3 class="settings-section-title">模型服务商</h3>
                  <p class="settings-section-desc">配置 LLM provider。每个模型测试通过（全绿）后才能启用。</p>
                </div>
                <button class="settings-add-btn" onClick={() => setIsAddingProvider(true)}>
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                    <line x1="12" y1="5" x2="12" y2="19"></line>
                    <line x1="5" y1="12" x2="19" y2="12"></line>
                  </svg>
                  添加服务商
                </button>
              </div>

              <div class="provider-list">
                <For each={currentProviders()} fallback={<div class="empty-hint">尚未配置任何服务商，点击右上角添加。</div>}>
                  {(prov) => (
                    <div class="provider-card" classList={{ active: prov.id === selectedProvider() }}>
                      <div class="provider-card__head" onClick={() => setSelectedProvider(prev => prev === prov.id ? "" : prov.id)}>
                        <span class="provider-card__chevron">{prov.id === selectedProvider() ? "▼" : "▶"}</span>
                        <Show
                          when={editingProviderId() !== prov.id}
                          fallback={
                            <input
                              class="provider-name-input"
                              value={tempName()}
                              onInput={(e) => setTempName(e.currentTarget.value)}
                              onBlur={handleSaveProviderName}
                              onKeyDown={(e) => { if (e.key === "Enter") handleSaveProviderName(); }}
                              autofocus
                            />
                          }
                        >
                          <span
                            class="provider-card__name"
                            onDblClick={() => { setEditingProviderId(prov.id); setTempName(prov.name); }}
                          >{prov.name}</span>
                        </Show>
                        <span class="provider-card__format">{prov.apiFormat}</span>
                        <span class="provider-card__spacer" />
                        <label class="provider-enable-toggle" title="启用前需全部模型测试通过">
                          <input
                            type="checkbox"
                            checked={prov.enabled}
                            onChange={(e) => handleToggleProviderEnabled(prov, e.currentTarget.checked)}
                          />
                          <span>启用</span>
                        </label>
                      </div>

                      <Show when={prov.id === selectedProvider()}>
                        <div class="provider-card__body">
                          {/* Base URL 单行 */}
                          <div class="provider-field provider-field--full">
                            <label>Base URL <span class="provider-field__hint">不含 /chat/completions</span></label>
                            <input
                              class="settings-input"
                              value={prov.endpoint}
                              placeholder="https://api.openai.com/v1"
                              onInput={(e) => updateProviderProperty(prov.id, "endpoint", e.currentTarget.value)}
                            />
                          </div>
                          {/* API Key + API 格式 两列并排 */}
                          <div class="provider-field-row">
                            <div class="provider-field provider-field--half">
                              <label>API Key <span class="provider-field__hint">服务商后台获取</span></label>
                              <div class="provider-field__password-wrap">
                                <input
                                  class="settings-input"
                                  type={showApiKeys()[prov.id] ? "text" : "password"}
                                  value={prov.apiKey}
                                  placeholder="sk-..."
                                  onInput={(e) => updateProviderProperty(prov.id, "apiKey", e.currentTarget.value)}
                                />
                                <button
                                  class="provider-field__eye-btn"
                                  type="button"
                                  title={showApiKeys()[prov.id] ? "隐藏" : "显示"}
                                  onClick={() => toggleApiKey(prov.id)}
                                >
                                  <Show when={showApiKeys()[prov.id]} fallback={
                                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width:15px;height:15px">
                                      <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/>
                                    </svg>
                                  }>
                                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width:15px;height:15px">
                                      <path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"/><line x1="1" y1="1" x2="23" y2="23"/>
                                    </svg>
                                  </Show>
                                </button>
                              </div>
                            </div>
                            <div class="provider-field provider-field--half">
                              <label>API 格式 <span class="provider-field__hint">OpenAI 兼容选 openai</span></label>
                              <select
                                class="settings-select"
                                value={prov.apiFormat}
                                onChange={(e) => updateProviderProperty(prov.id, "apiFormat", e.currentTarget.value)}
                              >
                                <For each={API_FORMATS}>{(f) => <option value={f}>{f}</option>}</For>
                              </select>
                            </div>
                          </div>

                          <div class="provider-field provider-field--full">
                            <label>模型列表 <span class="provider-field__hint">测试全绿后才能启用</span></label>
                            <ProviderModelEditor
                              models={prov.models || []}
                              testEntries={modelTests()[prov.id] || {}}
                              onChange={(models) => updateProviderProperty(prov.id, "models", models)}
                            />
                          </div>

                          <div class="provider-actions">
                            <button
                              class="settings-secondary-btn"
                              disabled={batchTesting(prov.id)}
                              onClick={() => handleTestModelConnection(prov)}
                            >
                              {batchTesting(prov.id) ? `测试中 ${batchProgress()?.current}/${batchProgress()?.total}` : "测试连通性"}
                            </button>
                            <button
                              class="settings-danger-btn"
                              onClick={() => { if (confirm(`删除服务商「${prov.name}」？`)) handleDeleteProvider(prov.id); }}
                            >删除</button>
                          </div>
                        </div>
                      </Show>
                    </div>
                  )}
                </For>
              </div>

              {/* 添加新 provider 表单 */}
              <Show when={isAddingProvider()}>
                <div class="provider-card new-provider-card">
                  <div class="provider-card__head">
                    <span class="provider-card__name">新服务商</span>
                    <span class="provider-card__spacer" />
                    <button class="settings-danger-btn" onClick={() => { setIsAddingProvider(false); clearModelTests(NEW_PROVIDER_TEST_KEY); }}>取消</button>
                  </div>
                  <div class="provider-card__body">
                    <div class="provider-field provider-field--full">
                      <label>名称</label>
                      <input class="settings-input" value={newProviderName()} placeholder="例如：OpenAI 官方" onInput={(e) => setNewProviderName(e.currentTarget.value)} />
                    </div>
                    <div class="provider-field provider-field--full">
                      <label>Base URL <span class="provider-field__hint">不含 /chat/completions</span></label>
                      <input class="settings-input" value={newProviderEndpoint()} placeholder="https://api.openai.com/v1" onInput={(e) => setNewProviderEndpoint(e.currentTarget.value)} />
                    </div>
                    <div class="provider-field-row">
                      <div class="provider-field provider-field--half">
                        <label>API Key <span class="provider-field__hint">服务商后台获取</span></label>
                        <div class="provider-field__password-wrap">
                          <input class="settings-input" type={showApiKeys()[NEW_PROVIDER_TEST_KEY] ? "text" : "password"} value={newProviderApiKey()} placeholder="sk-..." onInput={(e) => setNewProviderApiKey(e.currentTarget.value)} />
                          <button class="provider-field__eye-btn" type="button" title={showApiKeys()[NEW_PROVIDER_TEST_KEY] ? "隐藏" : "显示"} onClick={() => toggleApiKey(NEW_PROVIDER_TEST_KEY)}>
                            <Show when={showApiKeys()[NEW_PROVIDER_TEST_KEY]} fallback={
                              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width:15px;height:15px">
                                <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/>
                              </svg>
                            }>
                              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width:15px;height:15px">
                                <path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"/><line x1="1" y1="1" x2="23" y2="23"/>
                              </svg>
                            </Show>
                          </button>
                        </div>
                      </div>
                      <div class="provider-field provider-field--half">
                        <label>API 格式 <span class="provider-field__hint">OpenAI 兼容选 openai</span></label>
                        <select class="settings-select" value={newProviderFormat()} onChange={(e) => setNewProviderFormat(e.currentTarget.value as ModelProvider["apiFormat"])}>
                          <For each={API_FORMATS}>{(f) => <option value={f}>{f}</option>}</For>
                        </select>
                      </div>
                    </div>
                    <div class="provider-field provider-field--full">
                      <label>模型列表 <span class="provider-field__hint">测试全绿后才能启用</span></label>
                      <ProviderModelEditor
                        models={newProviderModels()}
                        testEntries={modelTests()[NEW_PROVIDER_TEST_KEY] || {}}
                        onChange={setNewProviderModels}
                      />
                    </div>
                    <div class="provider-actions">
                      <button class="settings-secondary-btn" disabled={batchTesting(NEW_PROVIDER_TEST_KEY)} onClick={() => handleTestNewProviderConnection()}>
                        {batchTesting(NEW_PROVIDER_TEST_KEY) ? `测试中 ${batchProgress()?.current}/${batchProgress()?.total}` : "测试连通性"}
                      </button>
                      <button class="settings-primary-btn" onClick={() => handleCreateNewProvider()}>保存</button>
                    </div>
                  </div>
                </div>
              </Show>
            </div>
          </Show>
        </main>
      </div>
    </div>
  );
}

/**
 * 单个 provider 的模型列表编辑器：增删模型行 + 显示每行连通性测试状态。
 */
function ProviderModelEditor(props: {
  models: ModelItem[];
  testEntries: Record<string, ModelTestEntry>;
  onChange: (models: ModelItem[]) => void;
}) {
  const update = (idx: number, patch: Partial<ModelItem>) => {
    const next = props.models.map((m, i) => (i === idx ? { ...m, ...patch } : m));
    props.onChange(next);
  };
  const remove = (idx: number) => props.onChange(props.models.filter((_, i) => i !== idx));
  const add = () => props.onChange([...props.models, { id: "", contextWindow: 128000 }]);

  return (
    <div class="model-editor">
      <For each={props.models}>
        {(m, idx) => {
          const entry = () => props.testEntries[m.id];
          return (
            <div class="model-editor__row">
              <input
                class="settings-input model-id-input"
                placeholder="模型 id，如 gpt-4o"
                value={m.id}
                onInput={(e) => update(idx(), { id: e.currentTarget.value })}
              />
              <input
                class="settings-input model-ctx-input"
                type="number"
                title="上下文窗口大小（token 数）"
                value={m.contextWindow}
                onInput={(e) => update(idx(), { contextWindow: parseInt(e.currentTarget.value || "0", 10) || 0 })}
              />
              <ModelStatusIcon status={entry()?.status} errorTip={entry()?.msg} />
              <button class="model-remove-btn" title="删除模型" onClick={() => remove(idx())}>✕</button>
            </div>
          );
        }}
      </For>
      <button class="settings-secondary-btn model-add-btn" onClick={add}>+ 添加模型</button>
    </div>
  );
}

/** 单模型连通性测试状态图标（4 态）。 */
function ModelStatusIcon(props: { status?: ModelTestEntry["status"]; errorTip?: string }) {
  return (
    <Show
      when={props.status === "testing"}
      fallback={
        <Show
          when={props.status === "success"}
          fallback={
            <Show
              when={props.status === "error"}
              fallback={<span class="mt-status idle" title="未测试" />}
            >
              <span class="mt-status error" title={props.errorTip || "测试失败"}>✕</span>
            </Show>
          }
        >
          <span class="mt-status success" title="连通正常">✓</span>
        </Show>
      }
    >
      <span class="mt-status testing" title="测试中">…</span>
    </Show>
  );
}
