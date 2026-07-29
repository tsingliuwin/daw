/**
 * 企业管理路由：GET /enterprise/status + POST /enterprise/setup
 *
 * 管理员首次认证后，用这些端点完成企业基本信息配置
 * （企业名称 + LLM provider + 搜索服务）。
 */

import { Hono } from "hono";
import { config, saveEnterpriseConfig } from "../config.js";
import { jwtAuth } from "../middleware/jwt.js";

const enterprise = new Hono();

/**
 * GET /enterprise/status — 返回企业是否已配置。
 */
enterprise.get("/status", jwtAuth, (c) => {
  return c.json({
    configured: config.providers.length > 0,
    serverName: config.serverName,
    hasProviders: config.providers.length > 0,
    hasSearch: !!(config.searchEngine && config.searchApiKey),
  });
});

/**
 * POST /enterprise/setup — 首次配置企业信息。
 * 接收 { serverName, providers, searchEngine?, searchApiKey? }，保存到
 * enterprise-config.json，同步更新内存 config。
 */
enterprise.post("/setup", jwtAuth, async (c) => {
  const body = await c.req.json<{
    serverName?: string;
    providers?: typeof config.providers;
    searchEngine?: string;
    searchApiKey?: string;
  }>();

  // 首次完整配置（providers 存在时）才要求 serverName + providers 都有值；
  // 部分更新（如只改 serverName 或只改 search）允许只传对应字段。
  if (body.providers !== undefined && (!body.serverName || body.providers.length === 0)) {
    return c.json({ error: "首次配置需要同时提供企业名称和至少一个 LLM provider" }, 400);
  }

  // 只保存传入的字段（saveEnterpriseConfig 内部合并已有配置）。
  const update: Record<string, unknown> = {};
  if (body.serverName !== undefined) update.serverName = body.serverName;
  if (body.providers !== undefined) update.providers = body.providers;
  if (body.searchEngine !== undefined) update.searchEngine = body.searchEngine;
  if (body.searchApiKey !== undefined) update.searchApiKey = body.searchApiKey;

  saveEnterpriseConfig(update);

  return c.json({ ok: true, serverName: config.serverName });
});

export default enterprise;
