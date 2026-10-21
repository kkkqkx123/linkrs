#!/usr/bin/env python3
"""Check that every node captured in highlights.scm exists in node-types.json."""
import json, re, sys
from pathlib import Path

root = Path(__file__).resolve().parents[1]
highlights = (root / "crates/linkrs-grammar/queries/highlights.scm").read_text()
node_types = json.loads((root / "crates/linkrs-grammar/src/node-types.json").read_text())

known = {nt["type"] for nt in node_types}

# capture node kinds like (kw_match), (identifier), (plus)
captures = set(re.findall(r"\(([\w_]+)\)", highlights))

missing = sorted(c for c in captures if c not in known)
if missing:
    print("Missing grammar nodes in highlights.scm:")
    for m in missing:
        print("  -", m)
    sys.exit(1)

# Every keyword node explicitly referenced by grammar.js must be highlighted.
grammar = (root / "crates/linkrs-grammar/grammar.js").read_text()
referenced_kw = {name.split(".")[-1] for name in re.findall(r"\$\.kw_[a-z_]+", grammar)}
missing_kw = sorted(referenced_kw - captures)
if missing_kw:
    print("Grammar keyword nodes missing from highlights.scm:")
    for m in missing_kw:
        print("  -", m)
    sys.exit(1)

print(f"highlights.scm: {len(captures)} captures, all present in node-types.json")
