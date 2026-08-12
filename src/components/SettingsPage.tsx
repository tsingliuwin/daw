import { Index, Show, For, createSignal, createEffect, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { logError } from "../lib/logger";
import { currentTheme, persistTheme, currentZoom, setCurrentZoom, type Theme } from "../lib/theme";
import type { DataSourceConfig } from "../lib/types";
import BrandFooter from "./BrandFooter";
import Select from "./Select";

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

type SettingsTab = "general" | "modelSettings" | "dataSources";

// 供 Select 组件用：value 是后端格式名，label 是更友好的展示。
const API_FORMAT_OPTIONS = [
  { value: "openai", label: "openai（兼容/OpenAI）" },
  { value: "anthropic", label: "anthropic（Claude）" },
  { value: "responses", label: "responses（OpenAI Responses）" },
] as const;

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
  /** 当前活跃工作区路径（数据源 link 用）。 */
  workspacePath?: string;
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

  // 读取 settings.json 刷新本地状态（onMount 时调一次），保证通用设置页里的
  // 搜索服务、provider 等字段与磁盘一致。
  const loadSettings = async () => {
    try {
      const json = await invoke<string>("load_settings_json");
      if (json && json !== "{}") {
        const loaded = JSON.parse(json) as AppSettings;
        setSettings(loaded);
        if (loaded.providers && loaded.providers.length > 0) {
          setSelectedProvider(loaded.providers[0].id);
        }
      } else {
        setSettings({});
      }
    } catch (err) {
      logError("ui", "Failed to load settings", err);
    }
  };

  onMount(() => { void loadSettings(); });

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

  // ── 数据源管理（db_connections，独立于 settings.json）──
  const [connections, setConnections] = createSignal<DataSourceConfig[]>([]);
  const [editingConn, setEditingConn] = createSignal<DataSourceConfig | null>(null);
  const [testStatus, setTestStatus] = createSignal<{ status: "idle" | "testing" | "success" | "error"; msg?: string }>({ status: "idle" });
  const [linkedConns, setLinkedConns] = createSignal<Record<string, boolean>>({});
  // 表单信号
  const [formId, setFormId] = createSignal("");
  const [formName, setFormName] = createSignal("");
  const [formType, setFormType] = createSignal<DataSourceConfig["dbType"]>("postgres");
  const [formHost, setFormHost] = createSignal("localhost");
  const [formPort, setFormPort] = createSignal(5432);
  const [formDatabase, setFormDatabase] = createSignal("");
  const [formUser, setFormUser] = createSignal("");
  const [formPassword, setFormPassword] = createSignal("");
  const [formSslMode, setFormSslMode] = createSignal("disable");
  const [formProduct, setFormProduct] = createSignal<string>("postgresql");
  const [formDbMode, setFormDbMode] = createSignal<string>("standard");

  const loadConnections = async () => {
    try {
      const list = await invoke<DataSourceConfig[]>("get_db_connections");
      setConnections(list);
    } catch (err) { logError("ui", "Failed to load db connections", err); }
  };
  const loadWorkspaceLinks = async () => {
    if (!props.workspacePath) return;
    try {
      const linked = await invoke<DataSourceConfig[]>("list_workspace_connections", { wsPath: props.workspacePath });
      setLinkedConns(Object.fromEntries(linked.map((c) => [c.id, true])));
    } catch (err) { logError("ui", "Failed to load workspace links", err); }
  };

  const startAddConnection = () => {
    setFormId(`ds-${Date.now()}`);
    setFormName(""); setFormType("postgres");
    setFormHost("localhost"); setFormPort(5432);
    setFormDatabase(""); setFormUser(""); setFormPassword(""); setFormSslMode("disable");
    setTestStatus({ status: "idle" });
    setEditingConn({} as DataSourceConfig);
  };
  const startEditConnection = (c: DataSourceConfig) => {
    setFormId(c.id); setFormName(c.name); setFormType(c.dbType);
    setFormHost(c.host); setFormPort(c.port); setFormDatabase(c.databaseName);
    setFormUser(c.username); setFormPassword(c.password); setFormSslMode(c.sslMode);
    setFormProduct(c.dbProduct ?? "postgresql");
    setFormDbMode(c.dbMode ?? "standard");
    setTestStatus({ status: "idle" });
    setEditingConn(c);
  };
  const formToConnData = (): DataSourceConfig => ({
    id: formId(), name: formName(), dbType: formType(),
    dbProduct: formProduct(), dbMode: formDbMode(),
    host: formHost(), port: formPort(), databaseName: formDatabase(),
    username: formUser(), password: formPassword(), sslMode: formSslMode(),
  });
  const handleTestConnection = async () => {
    setTestStatus({ status: "testing" });
    try {
      const msg = await invoke<string>("test_db_connection", { config: formToConnData() });
      setTestStatus({ status: "success", msg });
    } catch (err) {
      setTestStatus({ status: "error", msg: String(err) });
    }
  };
  const handleSaveConnection = async () => {
    try {
      await invoke("upsert_db_connection", { config: formToConnData() });
      setEditingConn(null);
      await loadConnections();
    } catch (err) { logError("ui", "Failed to save connection", err); alert(`保存失败: ${err}`); }
  };
  const handleDeleteConnection = async (id: string) => {
    if (!confirm("确定删除此数据源？")) return;
    try {
      await invoke("delete_db_connection", { id });
      await loadConnections();
      await loadWorkspaceLinks();
    } catch (err) { alert(`删除失败: ${err}`); }
  };
  const handleToggleLink = async (connId: string) => {
    if (!props.workspacePath) { alert("无法确定当前工作区"); return; }
    const isLinked = !!linkedConns()[connId];
    try {
      if (isLinked) {
        await invoke("unlink_connection_from_workspace", { wsPath: props.workspacePath, connId });
      } else {
        await invoke("link_connection_to_workspace", { wsPath: props.workspacePath, connId });
      }
      await loadWorkspaceLinks();
    } catch (err) { alert(`操作失败: ${err}`); }
  };
  const selectDbType = (t: DataSourceConfig["dbType"]) => {
    setFormType(t);
    if (t === "postgres" && (formPort() === 3306 || formPort() === 0)) setFormPort(5432);
    if (t === "mysql" && (formPort() === 5432 || formPort() === 0)) setFormPort(3306);
  };

  // 数据源 tab 打开时加载数据
  createEffect(() => {
    if (activeTab() === "dataSources") {
      void loadConnections();
      void loadWorkspaceLinks();
    }
  });

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

  /** 所有（非空 id 的）模型测试通过（全绿）才允许启用该 provider。 */
  const providerValidated = (providerKey: string, models: ModelItem[]): boolean => {
    const real = models.filter((m) => m.id.trim());
    if (!real.length) return false;
    const entries = modelTests()[providerKey] || {};
    return real.every((m) => entries[m.id]?.status === "success");
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

  // 启用门禁：首次启用要求所有（非空 id 的）模型测试通过。
  // 已启用的 provider 增删模型后再勾选不会被拦截——用户可以先用着，新模型按需测。
  const handleToggleProviderEnabled = (prov: ModelProvider, enabled: boolean) => {
    const realModels = (prov.models || []).filter((m) => m.id.trim());
    const wasEnabled = prov.enabled;
    // 要启用、且当前不是已启用状态（首次启用或从禁用切回）→ 检查全绿
    if (enabled && !wasEnabled && !providerValidated(prov.id, realModels)) {
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

  // 名称编辑刚结束的时间戳——用于阻止紧随其后的 click 冒泡到 header 触发折叠。
  // 时序：mousedown→input 失焦(onBlur→这里设 timestamp)→mouseup→click 冒泡到 header。
  // header onClick 检查这个时间戳，200ms 内的 click 视为编辑收尾，不折叠。
  let nameEditClosedAt = 0;
  const handleSaveProviderName = () => {
    const val = tempName().trim();
    if (val && editingProviderId()) updateProviderProperty(editingProviderId()!, "name", val);
    setEditingProviderId(null);
    nameEditClosedAt = Date.now();
  };

  // 选中 provider（如果只有一个或刚切过来）。
  const currentProviders = () => settings().providers || [];

  return (
    <div class="settings-page">
      <div class="settings-header" data-tauri-drag-region>
        <h2 class="settings-title" data-tauri-drag-region>设置</h2>
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
            <button
              class="settings-nav-item"
              classList={{ active: activeTab() === "dataSources" }}
              onClick={() => setActiveTab("dataSources")}
            >数据源</button>
          </div>
          {/* 共用品牌区：主题切换 + 返回首页（等同右上角关闭，免去鼠标移动） */}
          <BrandFooter
            onToggleTheme={() => {
              const next: Theme = currentTheme() === "light" ? "geek-dark" : "light";
              persistTheme(next);
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
                    onChange={(e) => persistTheme(e.currentTarget.value as Theme)}
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

              {/* 搜索服务配置 */}
              <div class="settings-section-head" style="margin-top: 20px;">
                <h4 class="settings-section-title">搜索服务</h4>
              </div>
              <p class="settings-section-desc">配置互联网搜索工具使用的服务。Agent 调用 search 工具时用此配置。</p>
              <div class="settings-row">
                <label class="settings-label">搜索引擎</label>
                <div class="settings-control">
                  <select
                    class="settings-select"
                    value={(settings().search as any)?.engine ?? ""}
                    onChange={(e) => updateSetting("search", { ...(settings().search as any ?? {}), engine: e.currentTarget.value, apiKey: (settings().search as any)?.apiKey ?? "" })}
                  >
                    <option value="">未配置</option>
                    <option value="exa">Exa</option>
                    <option value="doubao">豆包</option>
                    <option value="brave">Brave（待实现）</option>
                  </select>
                </div>
              </div>
              <div class="settings-row">
                <label class="settings-label">API Key</label>
                <div class="settings-control">
                  <input
                    class="settings-input"
                    type="password"
                    value={(settings().search as any)?.apiKey ?? ""}
                    placeholder={
                      (settings().search as any)?.engine === "doubao"
                        ? "在火山引擎控制台获取 API Key"
                        : (settings().search as any)?.engine === "exa"
                          ? "在 exa.ai/dashboard 获取"
                          : "输入搜索服务 API Key"
                    }
                    onInput={(e) => updateSetting("search", { ...(settings().search as any ?? {}), engine: (settings().search as any)?.engine ?? "exa", apiKey: e.currentTarget.value })}
                  />
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
                <Index each={currentProviders()} fallback={<div class="empty-hint">尚未配置任何服务商，点击右上角添加。</div>}>
                  {(provAcc) => {
                    // Index 按"位置"key，内容更新时只就地改 DOM 不重建，输入框不会失焦。
                    // provAcc 是 Accessor——在事件回调里现取（prov()）保证拿到最新值，
                    // 而不是闭包里的快照。
                    const prov = () => provAcc();
                    return (
                    <div class="provider-card" classList={{ active: prov().id === selectedProvider() }}>
                      <div class="provider-card__head" onClick={() => {
                        // 编辑名称刚结束（onBlur 触发）后的 200ms 内，紧随的 click 冒泡
                        // 不应折叠面板——否则点 input 外侧就会收起。
                        if (Date.now() - nameEditClosedAt < 200) return;
                        setSelectedProvider(prev => prev === prov().id ? "" : prov().id);
                      }}>
                        <span class="provider-card__chevron">{prov().id === selectedProvider() ? "▼" : "▶"}</span>
                        <Show
                          when={editingProviderId() !== prov().id}
                          fallback={
                            <input
                              class="provider-name-input"
                              value={tempName()}
                              onInput={(e) => setTempName(e.currentTarget.value)}
                              onBlur={handleSaveProviderName}
                              onKeyDown={(e) => {
                                if (e.key === "Enter") handleSaveProviderName();
                                if (e.key === "Escape") { setEditingProviderId(null); }
                              }}
                              ref={(el) => {
                                if (!el) return;
                                // 延迟聚焦：等 input 完全挂载到文档流后再 focus，
                                // 否则 onClick 结束时浏览器会把焦点还给 body，触发立即 blur。
                                setTimeout(() => { el.focus(); el.select(); }, 0);
                              }}
                            />
                          }
                        >
                          <span
                            class="provider-card__name provider-card__name--editable"
                            title="点击编辑名称"
                            onClick={(e) => {
                              e.stopPropagation();
                              setEditingProviderId(prov().id);
                              setTempName(prov().name);
                            }}
                          >{prov().name}</span>
                        </Show>
                        <span class="provider-card__format">{prov().apiFormat}</span>
                        <span class="provider-card__spacer" />
                        <label
                          class="provider-enable-toggle"
                          title="启用前需全部模型测试通过"
                          onClick={(e) => {
                            e.stopPropagation();
                            e.preventDefault();
                            handleToggleProviderEnabled(prov(), !prov().enabled);
                          }}
                        >
                          <span
                            class="provider-enable-switch"
                            classList={{ "provider-enable-switch--on": prov().enabled }}
                          />
                          <span>启用</span>
                        </label>
                      </div>

                      <Show when={prov().id === selectedProvider()}>
                        <div class="provider-card__body">
                          {/* Base URL 单行 */}
                          <div class="provider-field provider-field--full">
                            <label>Base URL <span class="provider-field__hint">填到 /v1 为止，多余路径会自动去除</span></label>
                            <input
                              class="settings-input"
                              value={prov().endpoint}
                              placeholder="https://api.openai.com/v1"
                              onInput={(e) => updateProviderProperty(prov().id, "endpoint", e.currentTarget.value)}
                            />
                          </div>
                          {/* API Key + API 格式 两列并排 */}
                          <div class="provider-field-row">
                            <div class="provider-field provider-field--half">
                              <label>API Key <span class="provider-field__hint">服务商后台获取</span></label>
                              <div class="provider-field__password-wrap">
                                <input
                                  class="settings-input"
                                  type={showApiKeys()[prov().id] ? "text" : "password"}
                                  value={prov().apiKey}
                                  placeholder="sk-..."
                                  onInput={(e) => updateProviderProperty(prov().id, "apiKey", e.currentTarget.value)}
                                />
                                <button
                                  class="provider-field__eye-btn"
                                  type="button"
                                  title={showApiKeys()[prov().id] ? "隐藏" : "显示"}
                                  onClick={() => toggleApiKey(prov().id)}
                                >
                                  <Show when={showApiKeys()[prov().id]} fallback={
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
                              <label>API 格式 <span class="provider-field__hint">openai 兼容 / anthropic=claude</span></label>
                              <Select
                                width="100%"
                                value={prov().apiFormat}
                                options={API_FORMAT_OPTIONS}
                                onChange={(v) => updateProviderProperty(prov().id, "apiFormat", v)}
                              />
                            </div>
                          </div>

                          <div class="provider-field provider-field--full">
                            <label>模型列表 <span class="provider-field__hint">测试全绿后才能启用</span></label>
                            <ProviderModelEditor
                              models={prov().models || []}
                              testEntries={modelTests()[prov().id] || {}}
                              onChange={(models) => updateProviderProperty(prov().id, "models", models)}
                            />
                          </div>

                          <div class="provider-actions">
                            <button
                              class="settings-secondary-btn"
                              disabled={batchTesting(prov().id)}
                              onClick={() => handleTestModelConnection(prov())}
                            >
                              {batchTesting(prov().id) ? `测试中 ${batchProgress()?.current}/${batchProgress()?.total}` : "测试连通性"}
                            </button>
                            <button
                              class="settings-danger-btn"
                              onClick={() => { if (confirm(`删除服务商「${prov().name}」？`)) handleDeleteProvider(prov().id); }}
                            >删除</button>
                          </div>
                        </div>
                      </Show>
                    </div>
                    );
                  }}
                </Index>
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
                      <label>Base URL <span class="provider-field__hint">填到 /v1 为止，多余路径会自动去除</span></label>
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
                        <label>API 格式 <span class="provider-field__hint">openai 兼容 / anthropic=claude</span></label>
                        <Select
                          width="100%"
                          value={newProviderFormat()}
                          options={API_FORMAT_OPTIONS}
                          onChange={(v) => setNewProviderFormat(v as ModelProvider["apiFormat"])}
                        />
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

          {/* 数据源管理 tab */}
          <Show when={activeTab() === "dataSources"}>
            <div class="settings-section">
              <Show when={editingConn()} fallback={
                <div>
                  <div class="settings-section-head">
                    <div>
                      <h3 class="settings-section-title">数据源管理</h3>
                      <p class="settings-section-desc">配置数据分析任务可连接的数据库。link 到当前工作区后即可查询。</p>
                    </div>
                    <button class="settings-add-btn" onClick={startAddConnection}>+ 新建连接</button>
                  </div>
                  <For each={connections()} fallback={<div class="empty-hint">尚未配置数据源，点击右上角"新建连接"。</div>}>
                    {(c) => (
                      <div class="ds-list-item">
                        <span class="ds-type-badge" data-type={c.dbType}>{c.dbType}</span>
                        <div class="ds-list-info">
                          <span class="ds-list-name">{c.name}</span>
                          <span class="ds-list-summary">
                            {c.dbType === "sqlite" ? c.databaseName : `${c.username}@${c.host}:${c.port}/${c.databaseName}`}
                          </span>
                        </div>
                        <Show when={props.workspacePath}>
                          <button
                            class="ds-link-btn"
                            classList={{ linked: !!linkedConns()[c.id] }}
                            title={linkedConns()[c.id] ? "已启用，点击禁用" : "点击启用到当前工作区"}
                            onClick={() => void handleToggleLink(c.id)}
                          >
                            {linkedConns()[c.id] ? "已启用" : "启用"}
                          </button>
                        </Show>
                        <button class="ds-edit-btn" title="编辑" onClick={() => startEditConnection(c)}>编辑</button>
                        <button class="ds-del-btn" title="删除" onClick={() => void handleDeleteConnection(c.id)}>✕</button>
                      </div>
                    )}
                  </For>
                </div>
              }>
                <div class="ds-form">
                  <div class="ds-form__header">
                    <h4 class="settings-section-title">{connections().find((c) => c.id === formId()) ? "编辑连接" : "添加连接"}</h4>
                    <button class="ds-form__cancel" onClick={() => setEditingConn(null)}>取消</button>
                  </div>
                  <div class="ds-type-cards">
                    <div class="ds-type-card" classList={{ selected: formType() === "postgres" }} onClick={() => selectDbType("postgres")}>
                      <div class="ds-type-card__icon" style="background: rgba(80,160,255,0.15); color: #50a0ff;">PG</div>
                      <div class="ds-type-card__name">PostgreSQL</div>
                    </div>
                    <div class="ds-type-card" classList={{ selected: formType() === "mysql" }} onClick={() => selectDbType("mysql")}>
                      <div class="ds-type-card__icon" style="background: rgba(255,140,0,0.15); color: #ff8c00;">MY</div>
                      <div class="ds-type-card__name">MySQL</div>
                    </div>
                    <div class="ds-type-card" classList={{ selected: formType() === "sqlite" }} onClick={() => selectDbType("sqlite")}>
                      <div class="ds-type-card__icon" style="background: rgba(16,185,129,0.15); color: #10b981;">DB</div>
                      <div class="ds-type-card__name">SQLite</div>
                    </div>
                  </div>
                  {/* postgres 类型展开产品/库类型选择 */}
                  <Show when={formType() === "postgres"}>
                    <div class="settings-row">
                      <label class="settings-label">数据库产品</label>
                      <div class="settings-control">
                        <select class="settings-select" value={formProduct()} onChange={(e) => { setFormProduct(e.currentTarget.value); if (e.currentTarget.value !== "hologres") setFormDbMode("standard"); }}>
                          <option value="postgresql">PostgreSQL</option>
                          <option value="hologres">Hologres</option>
                          <option value="oceanbase">OceanBase</option>
                          <option value="unknown">其他</option>
                        </select>
                      </div>
                    </div>
                    <Show when={formProduct() === "hologres"}>
                      <div class="settings-row">
                        <label class="settings-label">库类型</label>
                        <div class="settings-control">
                          <select class="settings-select" value={formDbMode()} onChange={(e) => setFormDbMode(e.currentTarget.value)}>
                            <option value="standard">标准库</option>
                            <option value="external">外部库</option>
                          </select>
                        </div>
                      </div>
                    </Show>
                  </Show>
                  <div class="settings-row">
                    <label class="settings-label">连接名称</label>
                    <div class="settings-control">
                      <input class="settings-input" value={formName()} placeholder="如 local_postgres" onInput={(e) => setFormName(e.currentTarget.value)} />
                    </div>
                  </div>
                  <Show when={formType() !== "sqlite"}>
                    <div class="settings-row">
                      <label class="settings-label">主机</label>
                      <div class="settings-control"><input class="settings-input" value={formHost()} onInput={(e) => setFormHost(e.currentTarget.value)} /></div>
                    </div>
                    <div class="settings-row">
                      <label class="settings-label">端口</label>
                      <div class="settings-control"><input class="settings-input" type="number" value={formPort()} onInput={(e) => setFormPort(parseInt(e.currentTarget.value) || 0)} /></div>
                    </div>
                    <div class="settings-row">
                      <label class="settings-label">数据库</label>
                      <div class="settings-control"><input class="settings-input" value={formDatabase()} onInput={(e) => setFormDatabase(e.currentTarget.value)} /></div>
                    </div>
                    <div class="settings-row">
                      <label class="settings-label">用户名</label>
                      <div class="settings-control"><input class="settings-input" value={formUser()} onInput={(e) => setFormUser(e.currentTarget.value)} /></div>
                    </div>
                    <div class="settings-row">
                      <label class="settings-label">密码</label>
                      <div class="settings-control"><input class="settings-input" type="password" value={formPassword()} onInput={(e) => setFormPassword(e.currentTarget.value)} /></div>
                    </div>
                    <Show when={formType() === "postgres"}>
                      <div class="settings-row">
                        <label class="settings-label">SSL</label>
                        <div class="settings-control">
                          <select class="settings-select" value={formSslMode()} onChange={(e) => setFormSslMode(e.currentTarget.value)}>
                            <option value="disable">disable</option>
                            <option value="require">require</option>
                            <option value="verify-ca">verify-ca</option>
                            <option value="verify-full">verify-full</option>
                          </select>
                        </div>
                      </div>
                    </Show>
                  </Show>
                  <Show when={formType() === "sqlite"}>
                    <div class="settings-row">
                      <label class="settings-label">数据库文件</label>
                      <div class="settings-control"><input class="settings-input" value={formDatabase()} placeholder="如 C:/data/my.db" onInput={(e) => setFormDatabase(e.currentTarget.value)} /></div>
                    </div>
                  </Show>
                  <div class="ds-form__actions">
                    <button class="ds-test-btn" onClick={() => void handleTestConnection()} disabled={testStatus().status === "testing"}>
                      {testStatus().status === "testing" ? "测试中…" : "测试连接"}
                    </button>
                    <Show when={testStatus().status === "success"}>
                      <span class="ds-test-ok">✓ {testStatus().msg}</span>
                    </Show>
                    <Show when={testStatus().status === "error"}>
                      <span class="ds-test-err">✕ {testStatus().msg}</span>
                    </Show>
                    <button class="settings-add-btn" style="margin-left: auto;" onClick={() => void handleSaveConnection()}>保存</button>
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
  const add = () => props.onChange([...props.models, { id: "", contextWindow: 256000, maxTokens: 64000 }]);

  return (
    <div class="model-editor">
      {/* 表头 */}
      <div class="model-editor__head">
        <span class="model-editor__col-id">模型 ID</span>
        <span class="model-editor__col-ctx" title="上下文窗口大小（token 数）">上下文</span>
        <span class="model-editor__col-max" title="最大输出（token 数）">最大输出</span>
        <span class="model-editor__col-status" />
        <span class="model-editor__col-del" />
      </div>
      <Index each={props.models}>
        {(mAcc, idx) => {
          const m = () => mAcc();
          const entry = () => props.testEntries[m().id];
          return (
            <div class="model-editor__row">
              <input
                class="settings-input model-id-input"
                placeholder="如 gpt-4o"
                value={m().id}
                onInput={(e) => update(idx, { id: e.currentTarget.value })}
              />
              <input
                class="settings-input model-ctx-input"
                type="number"
                title="上下文窗口大小（token 数）"
                value={m().contextWindow}
                onInput={(e) => update(idx, { contextWindow: parseInt(e.currentTarget.value || "0", 10) || 0 })}
              />
              <input
                class="settings-input model-max-input"
                type="number"
                title="最大输出 token 数（单次回复上限）"
                value={m().maxTokens ?? 64000}
                onInput={(e) => update(idx, { maxTokens: parseInt(e.currentTarget.value || "0", 10) || 0 })}
              />
              <ModelStatusIcon status={entry()?.status} errorTip={entry()?.msg} />
              <button class="model-remove-btn" title="删除模型" onClick={() => remove(idx)}>✕</button>
            </div>
          );
        }}
      </Index>
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
