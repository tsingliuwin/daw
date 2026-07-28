/**
 * 服务端配置——从环境变量读取。
 *
 * 部署时通过 env 注入（CloudBase 云函数的 env 配置 / docker -e / .env 文件）。
 * 本地开发可在 server/ 目录下创建 .env 文件，tsx 会自动加载。
 */

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
  const fs = require("fs");
  const path = require("path");
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

export const config: ServerConfig & { enterpriseId: string } = {
  jwtSecret: process.env.JWT_SECRET || "aioa-dev-secret-change-me",
  serverName: process.env.SERVER_NAME || "AIOA 工作台",
  providers: parseProviders(),
  searchEngine: process.env.SEARCH_ENGINE || "",
  searchApiKey: process.env.SEARCH_API_KEY || "",
  users: parseUsers(),
  enterpriseId: getOrCreateEnterpriseId(),
};
