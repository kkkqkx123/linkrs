#!/usr/bin/env python3
"""Coverage matrix between the Rust lexical inventory, dispatch, and grammar."""

import json
import re
import sys
from pathlib import Path

root = Path(__file__).resolve().parents[1]
inventory_path = root / "crates/linkrs-grammar/data/keywords.json"
grammar_js_path = root / "crates/linkrs-grammar/grammar.js"
grammar_json_path = root / "crates/linkrs-grammar/src/grammar.json"
dispatch_path = root / "crates/linkrs-query/src/parser/parsing/stmt_parser/dispatch.rs"
stmt_path = root / "crates/linkrs-query/src/parser/ast/stmt.rs"

inventory = json.loads(inventory_path.read_text())
grammar_js = grammar_js_path.read_text()
grammar_json = json.loads(grammar_json_path.read_text())
dispatch = dispatch_path.read_text()
stmt_source = stmt_path.read_text()

expanded_kw = set()


def walk(obj):
    if isinstance(obj, dict):
        for key, value in obj.items():
            if isinstance(key, str) and key.startswith("kw_"):
                expanded_kw.add(key)
            walk(value)
    elif isinstance(obj, list):
        for item in obj:
            walk(item)


walk(grammar_json)


def slug(word):
    return word.lower().replace(" ", "_")


hard_failures = []

# Known exceptions: these are intentionally not generated as kw_ rules.
SKIPPED_KEYWORDS = {"TRUE", "FALSE", "BALANCE"}
NON_KEYWORD_COMPONENTS = {"EMPTY"}
# `?` is lexed as a placeholder token but no query production consumes it.
PUNCTUATION_ALLOWLIST = {"?"}

# 1. Every inventory keyword/soft keyword must exist as a generated kw_ rule.
for group_name in ("keywords",):
    for entry in inventory.get(group_name, []):
        spelling = entry["spelling"]
        if " " in spelling or spelling in SKIPPED_KEYWORDS:
            continue
        name = "kw_" + slug(spelling)
        if name not in expanded_kw:
            hard_failures.append(f"inventory keyword missing from grammar: {spelling} ({name})")

for word in inventory.get("soft_keywords", []):
    if " " in word or word in SKIPPED_KEYWORDS:
        continue
    name = "kw_" + slug(word)
    if name not in expanded_kw:
        hard_failures.append(f"soft keyword missing from grammar: {word} ({name})")

# 2. Multi-word sequences must have both component keywords available.
for seq_name in ("multiword_unconditional", "multiword_conditional"):
    for sequence in inventory.get(seq_name, []):
        for word in sequence.split():
            if word in NON_KEYWORD_COMPONENTS:
                continue
            name = "kw_" + word.lower()
            if name not in expanded_kw:
                hard_failures.append(f"multiword component missing from grammar: {sequence} -> {word}")

# 3. Dispatch check_keyword literals must exist in the grammar.
dispatch_single = set(re.findall(r'check_keyword\("([A-Z_]+)"\)', dispatch))
dispatch_sequences = re.findall(r'check_keyword_sequence\(&\[(.*?)\]\)', dispatch)
for literal in sorted(dispatch_single):
    name = "kw_" + literal.lower()
    if name not in expanded_kw:
        hard_failures.append(f"dispatch keyword missing from grammar: {literal}")

for raw in dispatch_sequences:
    for word in re.findall(r'"([A-Z_]+)"', raw):
        name = "kw_" + word.lower()
        if name not in expanded_kw:
            hard_failures.append(f"dispatch sequence component missing from grammar: {word}")

# 4. Operator and punctuation symbols must be grammar tokens.
grammar_literals = set(re.findall(r"'([^'\n]+)'", grammar_js))
for entry in inventory.get("operators", []):
    symbol = entry["symbol"]
    if symbol not in grammar_literals:
        hard_failures.append(f"operator symbol absent from grammar source: {symbol}")

for entry in inventory.get("punctuation", []):
    symbol = entry["symbol"]
    if symbol not in grammar_literals and symbol not in PUNCTUATION_ALLOWLIST:
        hard_failures.append(f"punctuation symbol absent from grammar source: {symbol}")

# 5. Informational coverage matrix.
grammar_segments = sorted(set(re.findall(r"[a-z_]+_segment", grammar_js)))
stmt_variants = sorted(set(re.findall(r"^\s*([A-Za-z]+)\([A-Za-z]+\),", stmt_source, re.M)))
print(f"expanded kw rules in grammar.json: {len(expanded_kw)}")
print(f"inventory keywords: {len(inventory.get('keywords', []))}")
print(f"soft keywords: {len(inventory.get('soft_keywords', []))}")
print(f"grammar segments: {len(grammar_segments)}")
print(f"stmt variants: {len(stmt_variants)}")
print(f"dispatch single keywords: {len(dispatch_single)}")
print(f"dispatch sequences: {len(dispatch_sequences)}")

if hard_failures:
    print("grammar coverage failures:")
    for failure in sorted(set(hard_failures)):
        print(f"  - {failure}")
    sys.exit(1)

print("grammar coverage check passed")