/**
 * Auto-update helpers built on top of @tauri-apps/plugin-updater.
 *
 * 单一全局状态机，TitleBar 菜单与左栏品牌区 badge 共用：
 *   - 启动 30s 后首查，此后每 4h 轮询一次；
 *   - 发现新版本后静默后台下载，就绪后由用户在确认弹窗里重启安装；
 *   - 更新流程失败时回退打开 GitHub Releases 下载页。
 *
 * Chrome 之外（浏览器 dev 模式）自动静默：无 __TAURI_INTERNALS__ 时不做任何事。
 */
import { check, type DownloadEvent, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { openUrl } from "@tauri-apps/plugin-opener";
import { createSignal, createRoot } from "solid-js";
import { logError } from "./logger";

/** True when running inside the Tauri webview. */
const inTauri = typeof window !== "undefined" && (window as any).__TAURI_INTERNALS__;

/** Fallback download page when the updater cannot run. */
const DOWNLOAD_PAGE_URL = "https://github.com/tsingliuwin/daw/releases";

export interface UpdateInfo {
  /** New version string, without a leading "v" (from the updater). */
  version: string;
  /** Release notes / changelog body. */
  notes: string;
}

/** Coarse state of the update state machine. */
export type UpdateStatus =
  | "idle" // nothing happened yet / reset
  | "checking" // a check() is in flight
  | "up-to-date" // already at the latest version
  | "available" // new version known, not yet downloaded
  | "downloading" // silent download in progress
  | "ready" // downloaded & staged; waiting for user to relaunch
  | "installing" // relaunch in progress
  | "error";

export interface DownloadProgress {
  /** Fraction downloaded so far in [0, 1]. Stays 0 if total size is unknown. */
  fraction: number;
  /** Human-readable downloaded / total, e.g. "3.2 / 20 MB". Empty until Started. */
  human: string;
}

const POLL_INITIAL_DELAY_MS = 30_000; // first check 30s after start
const POLL_INTERVAL_MS = 4 * 60 * 60 * 1000; // then every 4 hours

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

/** Check for an available update; resolves `null` when none / not in Tauri. */
export async function checkForUpdate(): Promise<UpdateInfo | null> {
  if (!inTauri) return null;
  const update = await check();
  if (!update) return null;
  return { version: update.version, notes: update.body ?? "" };
}

/**
 * Download the update and stage it locally. Does NOT install or restart —
 * 关键：Windows 上 downloadAndInstall 会在下载完成后立即退出应用并运行
 * 安装器（passive 模式自动重启），用户正在跑的任务会被直接打断。因此这里
 * 只做 download 暂存，安装由用户在确认弹窗点击后经 {@link runInstall} 触发。
 * Throws on any failure (caller offers the download-page fallback).
 */
let stagedUpdate: Update | null = null;

async function downloadUpdate(
  onProgress?: (p: DownloadProgress) => void,
): Promise<void> {
  const update = await check();
  if (!update) throw new Error("No update available");

  let total = 0;
  let downloaded = 0;

  await update.download((ev: DownloadEvent) => {
    if (ev.event === "Started" && ev.data.contentLength) {
      total = ev.data.contentLength;
    } else if (ev.event === "Progress") {
      downloaded += ev.data.chunkLength;
      const fraction = total > 0 ? Math.min(downloaded / total, 1) : 0;
      const human = total > 0 ? `${fmtBytes(downloaded)} / ${fmtBytes(total)}` : fmtBytes(downloaded);
      onProgress?.({ fraction, human });
    }
  });

  stagedUpdate = update;
}

/** Restart the app to apply a staged update. */
export async function relaunchApp(): Promise<void> {
  await relaunch();
}

/** Open the download page in the browser (fallback path). */
export async function openDownloadPage(): Promise<void> {
  await openUrl(DOWNLOAD_PAGE_URL);
}

/* ------------------------------------------------------------------ *
 * Global update store — 单一数据源，TitleBar 与 BrandFooter badge 共用
 * ------------------------------------------------------------------ */

const store = createRoot(() => {
  const [status, setStatus] = createSignal<UpdateStatus>("idle");
  const [info, setInfo] = createSignal<UpdateInfo>({ version: "", notes: "" });
  const [progress, setProgress] = createSignal<DownloadProgress>({ fraction: 0, human: "" });
  const [error, setError] = createSignal("");

  let pollTimer: ReturnType<typeof setTimeout> | null = null;
  let started = false; // idempotent guard：start() 可被多个组件挂载调用

  const resetTransient = () => {
    setError("");
    setProgress({ fraction: 0, human: "" });
  };

  /** Schedule the next background poll. */
  const schedulePoll = (delay: number) => {
    if (pollTimer) clearTimeout(pollTimer);
    pollTimer = setTimeout(() => {
      void runCheck(false);
      schedulePoll(POLL_INTERVAL_MS);
    }, delay);
  };

  /**
   * Run an update check.
   * - `userInitiated=true`：菜单手动触发——短暂展示「已是最新/失败」反馈。
   * - `userInitiated=false`：后台静默轮询。
   * 两者发现新版本后都会立即静默下载（进度经左栏 badge 展示）。
   */
  const runCheck = async (userInitiated: boolean) => {
    if (!inTauri) return;
    const prev = status();
    if (prev === "downloading" || prev === "ready" || prev === "checking") {
      return;
    }
    setStatus("checking");
    resetTransient();
    try {
      const found = await checkForUpdate();
      if (!found) {
        if (userInitiated) {
          setStatus("up-to-date");
          setTimeout(() => {
            if (status() === "up-to-date") setStatus("idle");
          }, 5000);
        } else {
          setStatus("idle");
        }
        return;
      }
      setInfo({ version: found.version, notes: found.notes });
      setStatus("available");
      void runDownload();
    } catch (e) {
      logError("system", "Update check failed", e);
      if (userInitiated) {
        setStatus("error");
        setError(e instanceof Error ? e.message : String(e));
        setTimeout(() => {
          if (status() === "error") setStatus("idle");
        }, 5000);
      } else {
        setStatus("idle");
      }
    }
  };

  /** Download the update silently (no install/restart — see downloadUpdate). */
  const runDownload = async () => {
    if (status() === "downloading" || status() === "ready") return;
    setStatus("downloading");
    resetTransient();
    try {
      await downloadUpdate((p) => setProgress(p));
      setStatus("ready");
    } catch (e) {
      logError("system", "Download failed", e);
      setStatus("error");
      setError(e instanceof Error ? e.message : String(e));
      setTimeout(() => {
        if (status() === "error") setStatus("idle");
      }, 5000);
    }
  };

  /**
   * Install the staged update and relaunch. Only ever called from the user's
   * explicit confirmation（确认弹窗「安装并重启」按钮）——绝不自动触发。
   */
  const runInstall = async () => {
    setStatus("installing");
    try {
      if (stagedUpdate) {
        await stagedUpdate.install();
        stagedUpdate = null;
      }
      await relaunch();
    } catch (e) {
      logError("system", "Install/relaunch failed", e);
      setStatus("error");
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const checkInteractively = () => void runCheck(true);
  const installAndRelaunch = () => void runInstall();
  const fallbackDownload = () => {
    void openDownloadPage();
  };

  /** Boot the background poller. Idempotent — safe to call from multiple mounts. */
  const start = () => {
    if (!inTauri || started) return;
    started = true;
    schedulePoll(POLL_INITIAL_DELAY_MS);
  };

  return {
    status,
    info,
    progress,
    error,
    start,
    checkInteractively,
    installAndRelaunch,
    fallbackDownload,
  };
});

export const updater = store;