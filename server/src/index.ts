/**
 * AIOA 工作台服务端入口。
 *
 * Hono app，挂载所有路由。用 @hono/node-server 跑在 Node.js 上。
 * 未来部署到 CloudBase 云函数时，只需把 app 导出，云函数适配器调用它。
 */

import { Hono } from "hono";
import { cors } from "hono/cors";
import { serve } from "@hono/node-server";

import auth from "./routes/auth.js";
import clientConfig from "./routes/clientConfig.js";
import models from "./routes/models.js";
import chat from "./routes/chat.js";
import search from "./routes/search.js";
import { config } from "./config.js";

const app = new Hono();

// CORS——允许桌面客户端（Tauri webview）跨域请求。
app.use("*", cors({ origin: "*" }));

// 健康检查。
app.get("/health", (c) => c.json({ status: "ok", serverName: config.serverName }));

// 路由挂载。
app.route("/auth", auth);
app.route("/client-config", clientConfig);
app.route("/models", models);
app.route("/chat", chat);
app.route("/search", search);

// 启动（本地开发用；CloudBase 部署时由适配器调用 app）。
const port = parseInt(process.env.PORT || "3000", 10);

serve({ fetch: app.fetch, port }, (info) => {
  console.log(`AIOA 工作台服务端运行在 http://localhost:${info.port}`);
  console.log(`  服务名: ${config.serverName}`);
  console.log(`  用户数: ${config.users.length}`);
  console.log(`  LLM Provider 数: ${config.providers.length}`);
  console.log(`  搜索引擎: ${config.searchEngine || "未配置"}`);
});

// 导出 app 供 CloudBase 云函数适配器使用。
export default app;
