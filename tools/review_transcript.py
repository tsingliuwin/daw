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
import subprocess
import sys
from datetime import datetime

SLOW_MS = 5000

# 用户纠正/拍板/偏好的句式特征（复盘时提示沉淀为 users/concepts 的原料）。
# 2026-08-27 两次实战校准：
# - 偏好表达常不含"纠正感"（「很多数据我也没有，很多时候就需要能够估算
#   出来」）→ "估算/只要能/也可以"这类词要覆盖；
# - 人称词（我们/你们）在普通需求句里太常见（「我们看一下全程班…」），
#   是纯噪音，不进词表——纠正信号由「没有这么高」等具体短语承载。
# 2026-08-31 第三次校准：重复纠正常带「已经说过多次了」「没有 X 这种叫法」
# 句式（苏州系纠错当轮漏报，模型却纠正成功——词表不能比模型迟钝）。
PATTERNS = [
    "没有这么高", "不是这样", "没有这么", "其实", "应该", "不对",
    "主要是", "基本上", "平时", "习惯", "以后", "下次", "我想要", "我要的是", "注意",
    "估算", "只要能", "也可以", "多一些", "少一点", "就够", "也行", "没问题", "挺好",
    "已经说过", "说过多次", "这种叫法", "疑惑",
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
    okf_health()

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


def okf_health():
    """知识库体检：扫 OKF 全部 md 的「空标题板块」。

    真损失特征（2026-08-31 实锤的 write 静默吞正文 bug）：空标题直达 EOF，
    或空标题后紧跟同名标题（dedup 把第二次出现的标题连同正文吃掉）。
    无害残桩：空标题后紧跟不同名标题（正文在模型自己带的标题下）——只计数，
    提示择机清理，不属于数据丢失。
    """
    home = os.path.expanduser("~/.daw")
    roots = [os.path.join(home, "okf")] + glob.glob(os.path.join(home, "*", "okf"))
    loss, debris = [], 0
    for root in roots:
        for dirpath, _, files in os.walk(root):
            for fn in files:
                if not fn.endswith(".md"):
                    continue
                p = os.path.join(dirpath, fn)
                with open(p, encoding="utf-8") as f:
                    lines = f.read().splitlines()
                if lines and lines[0].strip() == "---":  # 跳过 frontmatter
                    for i in range(1, len(lines)):
                        if lines[i].strip() == "---":
                            lines = lines[i + 1:]
                            break
                i = 0
                while i < len(lines):
                    t = lines[i].strip()
                    if not t.startswith("#"):
                        i += 1
                        continue
                    j = i + 1
                    while j < len(lines) and not lines[j].strip():
                        j += 1
                    if j >= len(lines):
                        loss.append(f"{os.path.relpath(p, home)} | {t[:50]} | 直达EOF")
                        break
                    if lines[j].strip().startswith("#"):
                        same = t.lstrip("#").strip().lower() == lines[j].strip().lstrip("#").strip().lower()
                        if same:
                            loss.append(f"{os.path.relpath(p, home)} | {t[:50]} | 同名对撞")
                        else:
                            debris += 1
                        i = j
                        continue
                    i += 1
    print("\n# 知识库体检（OKF 空标题板块）")
    if loss:
        print("  ⚠️ 疑似静默丢正文（空标题直达 EOF 或同名对撞），应从会话回执 detail 恢复：")
        for x in loss:
            print("    - " + x)
    else:
        print("  真损失（EOF/同名对撞型空标题）: 0")
    print(f"  无害残桩（正文在模型自带标题下，择机清理）: {debris} 个")
    okf_git_audit()


def okf_git_audit():
    """知识库 git 审计：okf 仓库本身就是版本库，每次写入由后端自动提交
    （提交信息含板块名与 +N −M 行变更）。复盘把它摆到面前——改了哪几行、
    最近谁在动知识、有没有未提交的手工改动导致历史断档。"""
    home = os.path.expanduser("~/.daw")
    repos = [os.path.join(home, "okf")] + glob.glob(os.path.join(home, "*", "okf"))
    for repo in repos:
        if not os.path.isdir(os.path.join(repo, ".git")):
            continue
        label = os.path.relpath(repo, home)
        print(f"  git 审计（{label}，最近 6 次知识变更）：")
        try:
            log = subprocess.run(
                ["git", "-C", repo, "--no-pager", "log", "-n", "6",
                 "--date=format:%m-%d %H:%M", "--format=%h %ad %s"],
                capture_output=True, text=True, timeout=10, check=True,
            ).stdout.rstrip()
            for line in log.splitlines():
                print("    " + line)
            dirty = subprocess.run(
                ["git", "-C", repo, "status", "--porcelain"],
                capture_output=True, text=True, timeout=10, check=True,
            ).stdout.rstrip()
            if dirty:
                print("    ⚠️ 有未提交的变更（手工改完要提交，否则历史断档）：")
                for line in dirty.splitlines()[:10]:
                    print("      " + line)
        except (subprocess.SubprocessError, OSError) as e:
            print(f"    （git 不可用：{e}）")


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
