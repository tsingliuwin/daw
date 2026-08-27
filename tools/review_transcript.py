#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Daw 会话复盘工具——「用-查-改-用」循环里「查」的固定入口。

用法:
    python tools/review_transcript.py                  # 复盘 ~/.daw 下最新的一条会话
    python tools/review_transcript.py <task.jsonl>     # 复盘指定会话文件
    python tools/review_transcript.py --full           # 附带完整可读记录（reasoning/text/工具参数与结果）
    python tools/review_transcript.py -n 3             # 列出最近 3 条会话供选择

输出五块：概览 / 工具时间线 / 慢查询 / 错误 / 一致性体检（reasoning 重复、
同窗对账线索）。检查单（人工+模型一起过）：
  1. 规则遵循：下推规则（[下推表·禁直查] 是否直查了视图）、错误处置决策树、
     时间口径、关键数字交叉验证、知识沉淀及时性
  2. 效率：慢查询（>5s）根因、重复/无效查询、被守卫拦下的次数
  3. 正确性：同窗不同数（payload 对账）、模型自纠错次数
  4. 知识库健康：矛盾口径并存、该沉淀未沉淀
  5. 体验：reasoning 重复、图表引用、结论业务化
"""
import argparse
import glob
import json
import os
import sys
from datetime import datetime

SLOW_MS = 5000

# 用户纠正/拍板/偏好的句式特征（复盘时提示沉淀为 users/concepts 的原料）。
# 2026-08-27 实战教训：偏好表达常不含"纠正感"词（如「很多数据我也没有，
# 很多时候就需要能够估算出来」），"估算/没有/只要能"这类词也要覆盖。
PATTERNS = [
    "没有这么高", "不是这样", "没有这么", "其实", "你们", "我们", "应该", "不对",
    "主要是", "基本上", "平时", "习惯", "以后", "下次", "我想要", "我要的是", "注意",
    "估算", "只要能", "也可以", "多一些", "少一点", "就够", "也行", "没问题", "挺好",
]


def find_latest(n=1):
    home = os.path.expanduser("~/.daw")
    files = glob.glob(os.path.join(home, "**", "chats", "*.jsonl"), recursive=True)
    files.sort(key=os.path.getmtime, reverse=True)
    return files[:n]


def fmt_ts(ms):
    return datetime.fromtimestamp(ms / 1000).strftime("%H:%M:%S") if ms else "?"


def digest(obj, limit=160):
    s = json.dumps(obj, ensure_ascii=False)
    return s[:limit] + ("…" if len(s) > limit else "")


def payload_first_row(seg):
    p = seg.get("payload") or {}
    rows = p.get("rows") or []
    cols = p.get("columns") or []
    if not rows:
        return ""
    head = " | ".join(str(v) for v in rows[0])
    return f"[{len(rows)}行 x {len(cols)}列] 首行: {head[:160]}"


def is_duplicated_text(t):
    """reasoning 整段重复检测：文本 = 前半 + 后半 完全一致。"""
    if len(t) < 40:
        return False
    half = len(t) // 2
    return t[:half] == t[half:]


def review(path, full=False):
    with open(path, encoding="utf-8") as f:
        msgs = [json.loads(line) for line in f if line.strip()]

    print(f"# 会话复盘: {os.path.basename(path)}")
    users = [m for m in msgs if m.get("role") == "user"]
    print(f"消息 {len(msgs)} 条（user {len(users)}），用户提问：")
    for m in users:
        for s in m.get("segments") or []:
            if s.get("type") == "text":
                print(f"  ▶ {s['text'][:100]}")

    total_tools, ok_n, err_n, slow = 0, 0, 0, []
    err_list, timeline = [], []
    dup_reasoning = 0
    for m in msgs:
        segs = m.get("segments") or []
        start_ts = m.get("ts")
        for i, s in enumerate(segs):
            t = s.get("type")
            if t == "reasoning":
                txt = s.get("text") or ""
                if is_duplicated_text(txt):
                    dup_reasoning += 1
                if full:
                    print(f"\n--- [{i}] REASONING{'（重复×2！）' if is_duplicated_text(txt) else ''} ---\n{txt}")
            elif t == "text" and full:
                print(f"\n--- [{i}] TEXT ---\n{s.get('text', '')}")
            elif t == "tool":
                total_tools += 1
                status = s.get("status")
                elapsed = s.get("elapsedMs") or 0
                name = s.get("tool")
                ok_n += status == "ok"
                err_n += status != "ok"
                line = f"{fmt_ts(s.get('startTime'))} [{i:>2}] {name:<22} {status:<5} {elapsed/1000:6.1f}s {digest(s.get('args'))}"
                timeline.append(line)
                if elapsed and elapsed >= SLOW_MS and name in ("execute_query", "render_chart"):
                    slow.append((line, payload_first_row(s)))
                if status != "ok":
                    err_msg = s.get("summary") or s.get("detail") or ""
                    err_list.append(f"[{i}] {name}: {str(err_msg)[:300]}")

    print(f"\n# 工具时间线（{total_tools} 次：ok {ok_n} / error {err_n}）")
    for line in timeline:
        print("  " + line)

    print(f"\n# 慢查询（≥{SLOW_MS/1000:.0f}s，怀疑未下推/未聚合先拉明细）")
    for line, first in slow:
        print("  " + line)
        if first:
            print("      " + first)

    print("\n# 错误明细")
    for e in err_list or ["（无）"]:
        print("  " + e)

    print("\n# 一致性体检")
    print(f"  reasoning 整段重复（ReasoningDelta+Reasoning 双写特征）: {dup_reasoning} 段")
    print("  同窗对账：人工比对 execute_query 里相同时间窗的 firstRow 是否一致")
    print("  （引擎级异常实证案例见 AGENTS.md「用-查-改-用」一节，2026-08-27）")

    print("\n# 用户纠正候选（建议沉淀为 users/concepts 的原料）")
    print("  匹配用户消息里纠正/拍板/偏好的句式，逐字原文列出；确认后可把条目") 
    print("  写进 users 用户画像（沟通偏好/纠错记录）或对应 concepts 口径知识。")
    found = False
    for m in users:
        for s in m.get("segments") or []:
            t = (s.get("text") or "").strip()
            if not t:
                continue
            if any(k in t for k in PATTERNS):
                found = True
                print(f"  - {t[:160]}")
    if not found:
        print("  （本轮无明显纠正句式）")


def main():
    ap = argparse.ArgumentParser(description="Daw 会话复盘（用-查-改-用 之「查」）")
    ap.add_argument("path", nargs="?", help="task jsonl 路径；缺省取最新一条")
    ap.add_argument("-n", type=int, default=1, help="不传 path 时列出最近 N 条供选择")
    ap.add_argument("--full", action="store_true", help="输出完整可读记录")
    args = ap.parse_args()

    if args.path:
        review(args.path, args.full)
        return
    files = find_latest(max(args.n, 1))
    if not files:
        print("~/.daw 下没找到会话 jsonl")
        sys.exit(1)
    if args.n > 1:
        print("最近会话：")
        for p in files:
            print(f"  {datetime.fromtimestamp(os.path.getmtime(p)):%m-%d %H:%M}  {p}")
        return
    review(files[0], args.full)


if __name__ == "__main__":
    main()
