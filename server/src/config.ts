/**
 * 服务端配置——从环境变量读取。
 *
 * 部署时通过 env 注入（CloudBase 云函数的 env 配置 / docker -e / .env 文件）。
 * 本地开发可在 server/ 目录下创建 .env 文件，tsx 会自动加载。
 */

import fs from "fs";
import path from "path";
import crypto from "crypto";

export interface LlmProviderConfig {
  id: string;
  name: string;
  endpoint: string;
  apiKey: string;
  apiFormat: "openai" | "anthropic" | "responses";
  models: { id: string; contextWindow: number; maxTokens?: number }[];
  enabled: boolean;
}

export interface UserConfig {
  username: string;
  /** bcrypt 或 argon2 hash。首期用明文比较（后续改 hash）。 */
  password: string;
}

export interface ServerConfig {
  /** JWT 签发密钥。 */
  jwtSecret: string;
  /** 服务名（客户端连接后显示）。 */
  serverName: string;
  /** LLM provider 列表。 */
  providers: LlmProviderConfig[];
  /** 搜索引擎（exa / brave）。 */
  searchEngine: string;
  /** 搜索 API Key。 */
  searchApiKey: string;
  /** 允许登录的用户列表。 */
  users: UserConfig[];
}

function parseProviders(): LlmProviderConfig[] {
  const raw = process.env.LLM_PROVIDERS;
  if (!raw) return [];
  try {
    return JSON.parse(raw);
  } catch {
    return [];
  }
}

function parseUsers(): UserConfig[] {
  const raw = process.env.USERS;
  if (!raw) {
    // 默认一个 demo 用户
    return [{ username: "admin", password: "admin" }];
  }
  try {
    return JSON.parse(raw);
  } catch {
    return [{ username: "admin", password: "admin" }];
  }
}

/**
 * 企业唯一 ID——服务端首次启动时生成 UUID，持久化到 .enterprise-id 文件。
 * 客户端首次连接时从 /client-config 获取，作为数据隔离的 key。
 */
function getOrCreateEnterpriseId(): string {
  const idFile = path.resolve(process.cwd(), ".enterprise-id");
  try {
    if (fs.existsSync(idFile)) {
      return fs.readFileSync(idFile, "utf-8").trim();
    }
  } catch { /* 文件读取失败，继续生成新的 */ }
  const id = crypto.randomUUID();
  try {
    fs.writeFileSync(idFile, id, "utf-8");
  } catch { /* 写入失败也不影响运行——内存里有 ID */ }
  return id;
}

/**
 * 首次启动设置 token——服务端首次运行（无用户配置）时生成一个随机 token，
 * 持久化到 .setup-token 文件，有效期 15 分钟。管理员复制签名 URL 粘贴到
 * 客户端完成首次认证。用完即删（一次性）。
 */
function getOrCreateSetupToken(): string | null {
  const tokenFile = path.resolve(process.cwd(), ".setup-token");
  try {
    if (fs.existsSync(tokenFile)) {
      const content = fs.readFileSync(tokenFile, "utf-8").trim();
      const parsed = JSON.parse(content);
      // 检查是否过期（15 分钟）。
      if (Date.now() - parsed.createdAt < 15 * 60 * 1000) {
        return parsed.token;
      }
      // 过期了，删除文件。
      fs.unlinkSync(tokenFile);
    }
  } catch { /* 文件读取失败，继续生成新的 */ }
  const token = crypto.randomUUID();
  try {
    fs.writeFileSync(tokenFile, JSON.stringify({ token, createdAt: Date.now() }), "utf-8");
  } catch { /* 写入失败也不影响运行 */ }
  return token;
}

/** 删除 setup token（认证成功后调用，一次性）。 */
export function deleteSetupToken() {
  const tokenFile = path.resolve(process.cwd(), ".setup-token");
  try { fs.unlinkSync(tokenFile); } catch { /* 忽略 */ }
}

/** 是否首次运行（用户列表只有默认 demo 或为空）。 */
export function isFirstRun(): boolean {
  return config.users.length === 0 ||
    (config.users.length === 1 && config.users[0].username === "admin" && config.users[0].password === "admin");
}

export const config: ServerConfig & { enterpriseId: string; setupToken: string | null } = {
  jwtSecret: process.env.JWT_SECRET || "aioa-dev-secret-change-me",
  serverName: process.env.SERVER_NAME || "AIOA 工作台",
  providers: parseProviders(),
  searchEngine: process.env.SEARCH_ENGINE || "",
  searchApiKey: process.env.SEARCH_API_KEY || "",
  users: parseUsers(),
  enterpriseId: getOrCreateEnterpriseId(),
  setupToken: null, // 在 index.ts 启动时设置（需要知道端口）
};

/** 生成 setup token 并返回签名 URL（在 index.ts 启动时调用）。 */
export function initSetupToken(port: number): string | null {
  if (!isFirstRun()) {
    config.setupToken = null;
    return null;
  }
  const token = getOrCreateSetupToken();
  config.setupToken = token;
  if (token) {
    return `http://localhost:${port}/auth/setup?token=${token}`;
  }
  return null;
}

/**
 * 从 enterprise-config.json 加载企业配置，覆盖 env 的默认值。
 * 在服务端启动时调用（如果文件存在，说明已经配过企业信息）。
 */
export function loadEnterpriseConfig() {
  const configFile = path.resolve(process.cwd(), "enterprise-config.json");
  try {
    if (!fs.existsSync(configFile)) return;
    const raw = fs.readFileSync(configFile, "utf-8");
    const saved = JSON.parse(raw) as Partial<ServerConfig>;
    if (saved.serverName) config.serverName = saved.serverName;
    if (saved.providers) config.providers = saved.providers;
    if (saved.searchEngine !== undefined) config.searchEngine = saved.searchEngine;
    if (saved.searchApiKey !== undefined) config.searchApiKey = saved.searchApiKey;
  } catch { /* 文件读取失败，用 env 默认值 */ }
}

/**
 * 保存企业配置到 enterprise-config.json。
 * 管理员首次配置后调用——持久化 serverName/providers/searchEngine/searchApiKey。
 */
export function saveEnterpriseConfig(data: {
  serverName?: string;
  providers?: LlmProviderConfig[];
  searchEngine?: string;
  searchApiKey?: string;
}) {
  const configFile = path.resolve(process.cwd(), "enterprise-config.json");
  try {
    // 读现有配置（如果文件已存在），合并新值。
    let existing: Partial<ServerConfig> = {};
    try {
      if (fs.existsSync(configFile)) {
        existing = JSON.parse(fs.readFileSync(configFile, "utf-8"));
      }
    } catch { /* 忽略 */ }
    const merged = { ...existing, ...data };
    fs.writeFileSync(configFile, JSON.stringify(merged, null, 2), "utf-8");
    // 同步更新内存里的 config。
    if (data.serverName) config.serverName = data.serverName;
    if (data.providers) config.providers = data.providers;
    if (data.searchEngine !== undefined) config.searchEngine = data.searchEngine;
    if (data.searchApiKey !== undefined) config.searchApiKey = data.searchApiKey;
  } catch { /* 写入失败不影响运行 */ }
}
