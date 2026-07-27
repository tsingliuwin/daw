import type { TokenUsage } from "./types";

// ===========================================================================
// Conversation data-panel metrics — pure functions, no UI dependencies.
//
// The backend emits three kinds of `usage` events (see src-tauri/src/agent.rs):
//   1. estimate   (isEstimate:true)  — pre/during stream, before the API's
//                                       exact FinalResponse arrives.
//   2. real       (isEstimate:false) — one per LLM call, provider-normalized.
//   3. run summary (turnComplete:true)— one per finished agent run (user turn).
//
// `mergeUsage` folds an incoming event into the persisted `TokenUsage`, and
// `derivePanelMetrics` turns a `TokenUsage` into the structured numbers the
// panel renders. Keeping both pure means the math is single-sourced and
// unit-testable; ChatView/App only wire data, they don't compute.
// ===========================================================================

/** Shape of a `usage` event payload (parsed from `payload.text` JSON). */
export interface RawUsageEvent {
  isEstimate?: boolean;
  turnComplete?: boolean;
  // real / estimate per-call fields
  promptTokens?: number;
  /** Per-call completion (output) tokens — accumulated into `_totalCompletion`. */
  completionTokens?: number;
  /** Cumulative real output across all calls in this run so far — shown as
   *  "本轮输出" so it never drops between calls of a multi-turn run. */
  runCompletionTokens?: number;
  cacheReadTokens?: number;
  cacheCreationTokens?: number;
  freshInputTokens?: number;
  estPreambleRaw?: number;
  estToolsRaw?: number;
  /** real/estimated ratio for the first call of a run — refits `_calibK`. */
  kSample?: number;
  // run-summary fields
  runOutputTokens?: number;
  runElapsedMs?: number;
  tokPerSec?: number;
}

/** EMA refit of the per-model calibration factor (mirrors backend `fit_calibration`). */
function fitK(sample: number, prevK: number): number {
  const next = 0.6 * prevK + 0.4 * sample;
  return Number.isFinite(next) && next > 0 ? next : prevK;
}

/**
 * Best-effort migration of legacy persisted `TokenUsage` (written before the
 * refactor) into the new cumulative fields, so opening an old chat does not
 * show all-zero totals. Per-call counts (`_llmCallCount`/`_turnCount`) are
 * genuinely unknown for legacy data and left at 0 — the composition therefore
 * self-corrects as new turns arrive.
 */
function withLegacySeed(prev: TokenUsage | null): TokenUsage | null {
  if (!prev) return null;
  if (prev._totalPrompt != null) return prev; // already new-shape
  return {
    ...prev,
    _totalPrompt: prev._totalInputAllTurns ?? prev.inputTokens ?? 0,
    _totalCacheRead: prev._totalCachedAllTurns ?? prev.cachedInputTokens ?? 0,
    _totalCompletion: prev.outputTokens ?? 0,
    _peakPromptTokens: prev._peakInputTokens ?? prev.inputTokens ?? 0,
    _llmCallCount: 0,
    _turnCount: 0,
    _calibK: 1,
  };
}

/**
 * Fold one incoming usage event into the persisted `TokenUsage`.
 *
 * - **run summary** (`turnComplete`): increment the turn counter, record the
 *   final generation speed. Does not accumulate prompt tokens (it is a
 *   summary, not a per-call sample).
 * - **estimate** (`isEstimate`): freeze the last real cache values (the
 *   estimate cannot know them), let prompt/completion follow the live
 *   estimate. Cumulative totals are untouched.
 * - **real** (per LLM call): set the latest per-call fields, accumulate the
 *   cumulative totals, update the peak, and refit `_calibK` from `kSample`.
 */
export function mergeUsage(prev: TokenUsage | null, incoming: RawUsageEvent): TokenUsage {
  const base = withLegacySeed(prev);

  // ── Run summary: one per finished agent run. ──
  if (incoming.turnComplete) {
    return {
      ...(base ?? {}),
      isEstimate: false,
      _turnCount: (base?._turnCount ?? 0) + 1,
      _lastTokPerSec: incoming.tokPerSec ?? base?._lastTokPerSec ?? 0,
      // Final real run-total output (authoritative) for "本轮输出".
      completionTokens: incoming.runOutputTokens ?? base?.completionTokens,
    };
  }

  // ── Estimate: pre/during stream, before the API's exact usage. ──
  if (incoming.isEstimate) {
    if (base) {
      // Freeze real cache values (estimate is silent on cache); let prompt &
      // completion follow the live estimate so the user watches the response
      // grow. Structural cumulative totals stay frozen.
      return {
        ...base,
        isEstimate: true,
        promptTokens: incoming.promptTokens ?? base.promptTokens,
        completionTokens: incoming.completionTokens ?? base.completionTokens,
        estPreambleRaw: incoming.estPreambleRaw ?? base.estPreambleRaw,
        estToolsRaw: incoming.estToolsRaw ?? base.estToolsRaw,
      };
    }
    // First message ever — no real baseline yet; use the estimate as-is.
    return {
      isEstimate: true,
      promptTokens: incoming.promptTokens ?? 0,
      completionTokens: incoming.completionTokens ?? 0,
      estPreambleRaw: incoming.estPreambleRaw,
      estToolsRaw: incoming.estToolsRaw,
      _calibK: 1,
    };
  }

  // ── Real: one per LLM call, provider-normalized by the backend. ──
  const prompt = incoming.promptTokens ?? 0;
  // Per-call completion → accumulated into the conversation total. The display
  // value ("本轮输出") uses the cumulative run total (runCompletionTokens) so
  // it never drops between calls of a multi-turn (tool-using) run.
  const perCallCompletion = incoming.completionTokens ?? 0;
  const runCompletion = incoming.runCompletionTokens ?? perCallCompletion;
  const cacheRead = incoming.cacheReadTokens ?? 0;
  const cacheCreation = incoming.cacheCreationTokens ?? 0;
  const k = incoming.kSample != null
    ? fitK(incoming.kSample, base?._calibK ?? 1)
    : (base?._calibK ?? 1);

  return {
    ...(base ?? {}),
    isEstimate: false,
    promptTokens: prompt,
    completionTokens: runCompletion,
    cacheReadTokens: cacheRead,
    cacheCreationTokens: cacheCreation,
    freshInputTokens: incoming.freshInputTokens,
    estPreambleRaw: incoming.estPreambleRaw ?? base?.estPreambleRaw,
    estToolsRaw: incoming.estToolsRaw ?? base?.estToolsRaw,
    _calibK: k,
    _totalPrompt: (base?._totalPrompt ?? 0) + prompt,
    _totalCompletion: (base?._totalCompletion ?? 0) + perCallCompletion,
    _totalCacheRead: (base?._totalCacheRead ?? 0) + cacheRead,
    _totalCacheCreation: (base?._totalCacheCreation ?? 0) + cacheCreation,
    _llmCallCount: (base?._llmCallCount ?? 0) + 1,
    _peakPromptTokens: Math.max(prompt, base?._peakPromptTokens ?? 0),
    _turnCount: base?._turnCount ?? 0,
    _lastTokPerSec: base?._lastTokPerSec,
  };
}

// ── Formatting helpers ──

/** Compact capacity formatting: ≥ 10k shown in 万 (one decimal). */
export function fmtCap(n: number): string {
  if (!Number.isFinite(n)) return "0";
  if (n >= 10000) return `${(n / 10000).toFixed(1)}万`;
  return Math.round(n).toLocaleString();
}

/** Detail number formatting: commas up to 100k, then 万. */
export function fmtNum(n: number): string {
  if (!Number.isFinite(n)) return "0";
  if (n >= 100000) return `${Math.round(n / 10000)}万`;
  return Math.round(n).toLocaleString();
}

export function fmtPct(n: number): string {
  if (!Number.isFinite(n)) return "0%";
  const r = Math.round(n * 10) / 10;
  return Number.isInteger(r) ? `${r}%` : `${r.toFixed(1)}%`;
}

// ── Panel derivation ──

export interface BarSlice {
  /** Token count for this slice (cumulative). */
  tokens: number;
  /** Share of the total, 0–100. */
  pct: number;
}

export interface PanelMetrics {
  /** Context-window capacity (peak single-call prompt / window). */
  capacity: { peak: number; ctx: number; pct: number };
  /** Latest LLM call (real, or live estimate while streaming). */
  perTurn: {
    prompt: number;
    completion: number;
    cacheRead: number;
    cacheCreation: number;
  };
  /** Whole-conversation cumulative (real, across every LLM call). */
  cumulative: {
    turnCount: number;
    prompt: number;
    completion: number;
    cacheRead: number;
    hitRate: number;
  };
  /** Cumulative prompt composition (preamble / tools / messages). */
  composition: {
    preamble: BarSlice;
    tools: BarSlice;
    messages: BarSlice;
  };
  /** Generation speed (tok/s). Live during streaming, else last run's. */
  tokPerSec: number | null;
  /** Whether the current values are a pre-FinalResponse estimate. */
  isEstimate: boolean;
}

export interface DeriveOpts {
  contextWindow: number;
  /** Wall-clock ms now (drives live tok/s while streaming). */
  nowMs?: number;
  /** Wall-clock ms when the current run started. */
  runStartMs?: number;
  streaming?: boolean;
}

/**
 * Derive every number the panel renders from a `TokenUsage`.
 *
 * Composition (cumulative): preamble & tools are the fixed per-call costs,
 * estimated as `k * raw` (k converges to reality over turns) and scaled by the
 * LLM-call count; `messages` is the exact remainder of the real cumulative
 * prompt — so the three slices always sum to the real `_totalPrompt`.
 */
export function derivePanelMetrics(
  u: TokenUsage | null,
  opts: DeriveOpts,
): PanelMetrics | null {
  if (!u) return null;
  const ctx = opts.contextWindow > 0 ? opts.contextWindow : 128000;

  // Capacity: peak single-call prompt (never shrinks between turns).
  const peak = Math.max(u._peakPromptTokens ?? 0, u.promptTokens ?? 0, u._peakInputTokens ?? 0, u.inputTokens ?? 0);
  const capPct = peak > 0 ? Math.min(100, (peak / ctx) * 100) : 0;

  // Per-turn (latest call).
  const perTurn = {
    prompt: u.promptTokens ?? u.inputTokens ?? 0,
    completion: u.completionTokens ?? u.outputTokens ?? 0,
    cacheRead: u.cacheReadTokens ?? u.cachedInputTokens ?? 0,
    cacheCreation: u.cacheCreationTokens ?? 0,
  };

  // Cumulative.
  const totalPrompt = (u._totalPrompt ?? 0) + (u.isEstimate ? perTurn.prompt : 0);
  const totalCompletion = (u._totalCompletion ?? 0) + (u.isEstimate ? perTurn.completion : 0);
  const totalCacheRead = u._totalCacheRead ?? 0;
  const turnCount = u._turnCount ?? 0;
  const hitRate = totalPrompt > 0 ? (totalCacheRead / totalPrompt) * 100 : 0;

  // Composition (cumulative). preamble/tools sent on every LLM call.
  const k = u._calibK ?? 1;
  const calls = (u._llmCallCount ?? 0) + (u.isEstimate ? 1 : 0);
  const perCallPreamble = Math.round(k * (u.estPreambleRaw ?? 0));
  const perCallTools = Math.round(k * (u.estToolsRaw ?? 0));
  const cumPreamble = perCallPreamble * calls;
  const cumTools = perCallTools * calls;
  // messages is the exact remainder → slices sum to totalPrompt.
  const cumMessages = Math.max(0, totalPrompt - cumPreamble - cumTools);
  const compPct = (n: number) => (totalPrompt > 0 ? (n / totalPrompt) * 100 : 0);
  const composition = {
    preamble: { tokens: cumPreamble, pct: compPct(cumPreamble) },
    tools: { tokens: cumTools, pct: compPct(cumTools) },
    messages: { tokens: cumMessages, pct: compPct(cumMessages) },
  };

  // tok/s: live during streaming, else the last completed run's.
  let tokPerSec: number | null = null;
  if (opts.streaming && opts.runStartMs && opts.nowMs && perTurn.completion > 0) {
    const elapsedSec = Math.max(0.001, (opts.nowMs - opts.runStartMs) / 1000);
    tokPerSec = Math.round(perTurn.completion / elapsedSec);
  } else if (u._lastTokPerSec != null && u._lastTokPerSec > 0) {
    tokPerSec = u._lastTokPerSec;
  }

  return {
    capacity: { peak, ctx, pct: capPct },
    perTurn,
    cumulative: {
      turnCount,
      prompt: totalPrompt,
      completion: totalCompletion,
      cacheRead: totalCacheRead,
      hitRate,
    },
    composition,
    tokPerSec,
    isEstimate: u.isEstimate ?? false,
  };
}
