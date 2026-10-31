#!/usr/bin/env python3
"""Recounts the status table at the top of FEATURES.md from its feature rows."""
import re
from collections import Counter
from pathlib import Path

PATH = Path(__file__).resolve().parent.parent / "FEATURES.md"
LABELS = [
    ("✅", "Pronto"),
    ("🚧", "Em andamento"),
    ("🟢", "Herdado"),
    ("🔁", "Portar"),
    ("⬜", "A fazer"),
    ("🚫", "Fora do escopo"),
]
ROW = re.compile(r"^\| [A-Z]-\d+ \|[^|]*\| (\S+) \|", re.M)


def main():
    text = PATH.read_text(encoding="utf-8")
    counts = Counter(ROW.findall(text))
    unknown = set(counts) - {mark for mark, _ in LABELS}
    if unknown:
        raise SystemExit(f"Unknown status in FEATURES.md: {sorted(unknown)}")
    lines = ["| Status | Itens |", "| --- | --- |"]
    lines += [f"| {mark} {label} | {counts[mark]} |" for mark, label in LABELS]
    lines.append(f"| **Total** | **{sum(counts.values())}** |")
    table = "\n".join(lines)
    text, replaced = re.subn(
        r"(<!-- summary:start -->\n).*?(\n<!-- summary:end -->)",
        lambda m: m.group(1) + table + m.group(2),
        text,
        flags=re.S,
    )
    if replaced != 1:
        raise SystemExit("Summary markers not found in FEATURES.md")
    PATH.write_text(text, encoding="utf-8")
    print(table)


if __name__ == "__main__":
    main()
