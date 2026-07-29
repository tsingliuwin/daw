/**
 * 服务端配置--仅保留运行时不可变的内存状态。
 *
 * 企业配置（serverName / providers / searchEngine / users 等）已迁移到
 * PostgreSQL，通过 Drizzle ORM 查询（见 db.ts / schema.ts），不再读 JSON 文件。
 * 此处只保留：
 *   - jwtSecret：从 env 读，进程内不变。
 *   - setupToken：首次运行时生成的随机 token（纯内存，重启后重新生成）。
 */

import crypto from "crypto";
import { isFirstRun } from "./db.js";

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

/**
 * 运行时内存状态：只有 jwtSecret（从 env 读）和 setupToken（首次运行时生成，
 * 纯内存，重启后重新生成）。其余配置从 DB 实时查询。
 */
export const config: { jwtSecret: string; setupToken: string | null } = {
  jwtSecret: process.env.JWT_SECRET || "aioa-dev-secret-change-me",
  setupToken: null,
};

/**
 * 首次运行（无用户）时生成 setup token 并返回签名 URL（在 index.ts 启动时调用）。
 * token 纯内存保存，重启后重新生成；用完即清空（一次性）。
 */
export async function initSetupToken(
  enterpriseId: string,
  port: number,
): Promise<string | null> {
  const firstRun = await isFirstRun(enterpriseId);
  if (!firstRun) {
    config.setupToken = null;
    return null;
  }
  const token = crypto.randomUUID();
  config.setupToken = token;
  return `http://localhost:${port}/auth/setup?token=${token}`;
}
