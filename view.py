r"""cfp.db の中身をざっと見る。

    python view.py              # 直近 30 行
    python view.py -n 100       # 直近 100 行
    python view.py --db path\to\cfp.db

読み取り専用で開くので、cfp の記録中に実行してよい。
"""

import argparse
import sqlite3
from datetime import datetime
from pathlib import Path

DEFAULT_DB = Path(__file__).parent / "target" / "release" / "cfp.db"

# (見出し, SQL式)
COLUMNS = [
    ("act", "active_secs"),
    ("char", "key_char"),
    ("bs", "key_bs"),
    ("ent", "key_enter"),
    ("short", "key_shortcut"),
    ("ime", "key_ime"),
    ("mouse", "CAST(mouse_dist AS INTEGER)"),
    ("click", "click_l + click_r + click_m"),
    ("wheel", "CAST(wheel_v + wheel_h AS INTEGER)"),
    ("switch", "hand_switches"),
]


def print_table(header, rows):
    widths = [max(len(str(x)) for x in col) for col in zip(header, *rows)]
    for r in [header, *rows]:
        print("  ".join(str(x).rjust(w) for x, w in zip(r, widths)))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("-n", type=int, default=30, help="表示する行数")
    ap.add_argument("--db", type=Path, default=DEFAULT_DB)
    args = ap.parse_args()

    if not args.db.exists():
        raise SystemExit(f"DB が見つかりません: {args.db}")
    conn = sqlite3.connect(f"file:{args.db}?mode=ro", uri=True)

    exprs = ", ".join(e for _, e in COLUMNS)
    rows = conn.execute(
        f"SELECT t, {exprs} FROM buckets ORDER BY t DESC LIMIT ?", (args.n,)
    ).fetchall()
    rows.reverse()

    header = ["time"] + [h for h, _ in COLUMNS]
    body = [
        [datetime.fromtimestamp(t).strftime("%m-%d %H:%M:%S"), *rest]
        for t, *rest in rows
    ]
    print_table(header, body)

    # 今日（ローカル時刻）の合計
    midnight = datetime.now().replace(hour=0, minute=0, second=0, microsecond=0)
    sums = ", ".join(f"SUM({e})" for _, e in COLUMNS)
    count, *totals = conn.execute(
        f"SELECT COUNT(*), {sums} FROM buckets WHERE t >= ?",
        (int(midnight.timestamp()),),
    ).fetchone()
    print()
    print(f"今日: {count} 行（{count * 10 // 60} 分観測）")
    print_table([h for h, _ in COLUMNS], [[x or 0 for x in totals]])


if __name__ == "__main__":
    main()
