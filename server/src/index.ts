/**
 * AIOA 工作台服务端入口。
 *
 * Hono app，挂载所有路由。用 @hono/node-server 跑在 Node.js 上。
 * 未来部署到 CloudBase 云函数时，只需把 app 导出，云函数适配器调用它。
 */

import "dotenv/config";

import { Hono } from "hono";
import { cors } from "hono/cors";
import { serve } from "@hono/node-server";

import auth from "./routes/auth.js";
import clientConfig from "./routes/clientConfig.js";
import models from "./routes/models.js";
import chat from "./routes/chat.js";
import search from "./routes/search.js";
import enterprise from "./routes/enterprise.js";
import { initSetupToken } from "./config.js";
import { initDatabase, closePool } from "./db.js";

const app = new Hono();

// CORS--允许桌面客户端（Tauri webview）跨域请求。
app.use("*", cors({ origin: "*" }));

// 健康检查。
app.get("/health", (c) => c.json({ status: "ok" }));

// 路由挂载。
app.route("/auth", auth);
app.route("/client-config", clientConfig);
app.route("/models", models);
app.route("/chat", chat);
app.route("/search", search);
app.route("/enterprise", enterprise);

// 启动（本地开发用；CloudBase 部署时由适配器调用 app）。
const port = parseInt(process.env.PORT || "3000", 10);

/**
 * 启动流程：先初始化 DB（迁移 + 默认企业）-> 首次运行则生成 setup token ->
 * 启动 HTTP 服务。initDatabase 是异步的，必须在 serve 之前 await。
 */
async function main() {
  const enterpriseId = await initDatabase();

  // 首次运行：生成签名 URL（纯内存 token，重启后重新生成）。
  const setupUrl = await initSetupToken(enterpriseId, port);

  serve({ fetch: app.fetch, port }, (info) => {
    console.log(`AIOA 工作台服务端运行在 http://localhost:${info.port}`);

    if (setupUrl) {
      console.log("");
      console.log("═══════════════════════════════════════════════════");
      console.log("  首次启动--复制以下链接到客户端完成认证：");
      console.log("");
      console.log(`  ${setupUrl}`);
      console.log("");
      console.log("  （有效期 15 分钟，认证成功后自动失效）");
      console.log("═══════════════════════════════════════════════════");
      console.log("");
    }
  });
}

main().catch((e) => {
  console.error("启动失败:", e);
  process.exit(1);
});

// 优雅退出：关闭 PG 连接池。
async function shutdown() {
  await closePool();
  process.exit(0);
}
process.on("SIGINT", shutdown);
process.on("SIGTERM", shutdown);

// 导出 app 供 CloudBase 云函数适配器使用。
export default app;
