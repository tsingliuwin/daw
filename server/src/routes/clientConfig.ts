/**
 * 客户端配置路由：GET /client-config
 *
 * 无需认证。返回服务名 + 可用性，让客户端验证服务地址正确且服务在线。
 * 也返回是否需要登录（hasUsers）。
 */

import { Hono } from "hono";
import { config } from "../config.js";

const clientConfig = new Hono();

clientConfig.get("/", (c) => {
  return c.json({
    serverName: config.serverName,
    enterpriseId: config.enterpriseId,
    hasUsers: config.users.length > 0,
    hasProviders: config.providers.length > 0,
    hasSearch: !!(config.searchEngine && config.searchApiKey),
  });
});

export default clientConfig;
