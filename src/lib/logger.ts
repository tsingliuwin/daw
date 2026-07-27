// 统一日志中枢 —— 收敛前端所有日志产出 + 接收后端 app-log 推送。
//
// 每条日志做三件事：
//   1. 立即写入内存 signal（控制台马上可见，不等待网络往返）；
//   2. 异步调用后端 append_log 持久化到 SQLite（失败静默——日志不应拖垮业务）；
//   3. （后端侧的 append_log 会向其它窗口广播 app-log，但发起窗口不再回传）。
//
// 后端 tracing 产生的日志经由 "app-log" 事件推送进来，由 installAppLogListener
// 注入同一个 signal，实现前后端日志在同一个控制台的时间线上混合展示。

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { createSignal } from "solid-js";
import type { LogLevel, LogCategory, UnifiedLog } from "./types";

/** 内存中保留的最新日志条数上限（控制台实时视图）。历史查询走 query_logs，不受此限。 */
const IN_MEMORY_CAP = 500;

const [logs, setLogs] = createSignal<UnifiedLog[]>([]);
let seq = 0;

/** 读取当前内存日志（newest-first）。供控制台组件订阅。 */
export function getLogs(): UnifiedLog[] {
  return logs();
}

/** 订阅日志变化的响应式访问器（SolidJS）。 */
export const logsSignal = logs;

/** 从 signal 清空内存日志（不清库；清库用 clearLogsStore）。 */
export function clearLogsMemory(): void {
  setLogs([]);
}

/** 从内存和 SQLite 同时清除日志。`before` 省略则全清。 */
export async function clearLogsStore(before?: number): Promise<void> {
  clearLogsMemory();
  try {
    await invoke("clear_logs", { before: before ?? null });
  } catch {
    // 持久化失败不影响本地清空——日志操作永不抛错到业务层。
  }
}

/**
 * 写一条日志。前端唯一的日志入口：所有 `console.error` 调用点都应改用它。
 *
 * @param level   级别
 * @param category 分类（必须落在 LogCategory 枚举内）
 * @param message 单行摘要
 * @param detail  结构化明细（sql / rowCount / elapsedMs / error / ...）
 * @param ctx     可选的 workspace / taskId 关联
 */
export function log(
  level: LogLevel,
  category: LogCategory,
  message: string,
  detail?: Record<string, unknown>,
  ctx?: { workspace?: string; taskId?: string },
): void {
  const entry: UnifiedLog = {
    id: --seq, // 内存期用负数临时 id，避免与后端自增正 id 冲突
    ts: Date.now(),
    level,
    category,
    message,
    detail,
    workspace: ctx?.workspace,
    taskId: ctx?.taskId,
  };

  // 1) 立即进内存 signal（newest-first，截断到上限）。
  setLogs((prev) => [entry, ...prev].slice(0, IN_MEMORY_CAP));

  // 2) 异步落库——火并忘，失败静默。
  invoke<number>("append_log", {
    record: {
      ts: entry.ts,
      level: entry.level,
      category: entry.category,
      message: entry.message,
      detail: entry.detail ?? null,
      workspace: entry.workspace ?? null,
      taskId: entry.taskId ?? null,
    },
  }).catch(() => {
    // 持久化失败不阻断日志展示。
  });
}

/** 便捷封装：UI 层错误统一走这里（替换散落的 console.error）。 */
export const logError = (
  category: LogCategory,
  message: string,
  err?: unknown,
  detail?: Record<string, unknown>,
): void =>
  log(
    "error",
    category,
    message,
    { ...(detail ?? {}), ...(err != null ? { error: String(err) } : {}) },
  );

export const logWarn = (
  category: LogCategory,
  message: string,
  detail?: Record<string, unknown>,
): void => log("warn", category, message, detail);

export const logInfo = (
  category: LogCategory,
  message: string,
  detail?: Record<string, unknown>,
): void => log("info", category, message, detail);

export const logDebug = (
  category: LogCategory,
  message: string,
  detail?: Record<string, unknown>,
): void => log("debug", category, message, detail);

/**
 * 安装后端 `app-log` 事件监听器，把后端 tracing 产生的日志注入同一 signal。
 * 在应用 onMount 时调用一次，返回 unlisten 在 onCleanup 时调用。
 *
 * 后端 emit 的日志已带正 id（SQLite 自增），与前端负临时 id 区分，不会重复。
 */
export async function installAppLogListener(): Promise<UnlistenFn> {
  return listen<UnifiedLog>("app-log", (event) => {
    const incoming = event.payload;
    setLogs((prev) => {
      // 若已存在同 id（极少见的本地回环），跳过避免重复。
      if (incoming.id != null && prev.some((l) => l.id === incoming.id)) {
        return prev;
      }
      return [incoming, ...prev].slice(0, IN_MEMORY_CAP);
    });
  });
}
