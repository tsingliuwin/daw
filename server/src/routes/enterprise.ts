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

  if (!body.serverName || !body.providers || body.providers.length === 0) {
    return c.json({ error: "企业名称和至少一个 LLM provider 不能为空" }, 400);
  }

  // 保存到文件 + 更新内存。
  saveEnterpriseConfig({
    serverName: body.serverName,
    providers: body.providers,
    searchEngine: body.searchEngine ?? "",
    searchApiKey: body.searchApiKey ?? "",
  });

  return c.json({ ok: true, serverName: config.serverName });
});

export default enterprise;
