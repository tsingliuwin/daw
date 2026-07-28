/**
 * JWT 验证中间件——保护需要认证的路由。
 *
 * 从 Authorization: Bearer <token> 解析 JWT，验证签名后把 payload 挂到 c.set('user', payload)。
 * 验证失败返回 401。
 */

import { createMiddleware } from "hono/factory";
import * as jose from "jose";
import { config } from "../config.js";

export interface JwtPayload {
  username: string;
  iat: number;
  exp: number;
}

export const jwtAuth = createMiddleware<{
  Variables: { user: JwtPayload };
}>(async (c, next) => {
  const auth = c.req.header("Authorization");
  if (!auth || !auth.startsWith("Bearer ")) {
    return c.json({ error: "未提供认证令牌" }, 401);
  }

  const token = auth.slice(7);
  try {
    const { payload } = await jose.jwtVerify(
      token,
      new TextEncoder().encode(config.jwtSecret),
    );
    c.set("user", payload as unknown as JwtPayload);
    await next();
  } catch {
    return c.json({ error: "认证令牌无效或已过期" }, 401);
  }
});
