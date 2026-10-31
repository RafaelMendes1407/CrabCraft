#!/usr/bin/env python3
"""Sets the status and plan of FEATURES.md rows: features_set.py ID MARK "plan" [ID MARK "plan"...]"""
import re
import sys
from pathlib import Path

PATH = Path(__file__).resolve().parent.parent / "FEATURES.md"
args = sys.argv[1:]
if len(args) % 3:
    raise SystemExit(__doc__)
text = PATH.read_text(encoding="utf-8")
for i in range(0, len(args), 3):
    fid, mark, plan = args[i:i + 3]
    text, n = re.subn(
        rf"^(\| {re.escape(fid)} \| [^|]*\| )\S+( \| [^|]*\| )[^\n]*\|$",
        lambda m: f"{m.group(1)}{mark}{m.group(2)}{plan} |",
        text,
        flags=re.M,
    )
    if n != 1:
        raise SystemExit(f"Row not found: {fid}")
PATH.write_text(text, encoding="utf-8")
