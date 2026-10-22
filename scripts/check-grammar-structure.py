#!/usr/bin/env python3
"""Check that every Rust Stmt variant has a grammar representation.

The grammar is a subset used for highlighting and smoke parsing, so many AST
statements are covered by a generic segment or by a pipe/set-operation rule.
This check makes that mapping explicit: a new Stmt variant without a mapping
fails the build instead of silently regressing tree-sitter coverage.
"""

import re
import sys
from pathlib import Path

root = Path(__file__).resolve().parents[1]
stmt_path = root / "crates/linkrs-query/src/parser/ast/stmt.rs"
grammar_js_path = root / "crates/linkrs-grammar/grammar.js"

stmt_source = stmt_path.read_text()
grammar_js = grammar_js_path.read_text()

variants = sorted(set(re.findall(r"^\s*([A-Za-z]+)\([A-Za-z]+\),", stmt_source, re.M)))
segments = set(re.findall(r"[a-z_]+_segment", grammar_js))
rules = set(re.findall(r"^    ([a-z_]+):", grammar_js, re.M))

SEGMENT_MAP = {
    "Alter": {"alter_segment"},
    "AlterFulltextIndex": {"alter_segment"},
    "AlterUser": {"alter_segment"},
    "Analyze": {"analyze_segment"},
    "AssignVariable": {"let_segment"},
    "Assignment": {"assignment_segment"},
    "AttachDatabase": {"attach_segment"},
    "BeginTransaction": {"begin_segment"},
    "ChangePassword": {"change_segment"},
    "Checkpoint": {"checkpoint_segment"},
    "ClearSpace": {"clear_space_segment"},
    "Collect": {"collect_segment"},
    "CommitTransaction": {"commit_segment"},
    "Copy": {"copy_segment"},
    "Create": {"create_segment"},
    "CreateFulltextIndex": {"create_segment"},
    "CreateMacro": {"create_segment"},
    "CreateType": {"create_segment"},
    "CreateUser": {"create_segment"},
    "CreateVectorIndex": {"create_segment"},
    "Delete": {"delete_segment"},
    "Desc": {"describe_segment"},
    "DescribeFulltextIndex": {"describe_segment"},
    "DescribeUser": {"describe_segment"},
    "DescribeVectorIndex": {"describe_segment"},
    "DetachDatabase": {"detach_segment"},
    "Drop": {"drop_segment"},
    "DropFulltextIndex": {"drop_segment"},
    "DropMacro": {"drop_segment"},
    "DropType": {"drop_segment"},
    "DropUser": {"drop_segment"},
    "DropVectorIndex": {"drop_segment"},
    "Explain": {"explain_segment"},
    "ExportDatabase": {"export_segment"},
    "Extension": {"load_segment", "install_segment", "uninstall_segment"},
    "Fetch": {"fetch_segment"},
    "Filter": {"where_segment"},
    "FindPath": {"find_segment"},
    "Go": {"go_segment"},
    "Grant": {"grant_segment"},
    "GroupBy": {"group_by_segment"},
    "ImportDatabase": {"import_segment"},
    "InQueryCall": {"call_segment"},
    "Insert": {"insert_segment"},
    "KillQuery": {"kill_segment"},
    "LoadFrom": {"load_segment"},
    "Lookup": {"lookup_segment"},
    "LookupFulltext": {"lookup_segment"},
    "LookupVector": {"lookup_segment"},
    "Match": {"match_segment"},
    "MatchFulltext": {"match_segment"},
    "MatchVector": {"match_segment"},
    "Merge": {"merge_segment"},
    "Migrate": {"migrate_segment"},
    "Profile": {"profile_segment"},
    "PurgeExpired": {"purge_segment"},
    "ReleaseSavepoint": {"release_segment"},
    "Remove": {"remove_segment"},
    "Return": {"return_segment"},
    "Revoke": {"revoke_segment"},
    "RollbackTransaction": {"rollback_segment"},
    "Savepoint": {"savepoint_segment"},
    "Search": {"search_segment"},
    "SearchVector": {"search_segment"},
    "Set": {"set_segment"},
    "Show": {"show_segment"},
    "ShowConfigs": {"show_segment"},
    "ShowCreate": {"show_segment"},
    "ShowFulltextIndex": {"show_segment"},
    "ShowQueries": {"show_segment"},
    "ShowRoles": {"show_segment"},
    "ShowSessions": {"show_segment"},
    "ShowUsers": {"show_segment"},
    "ShowVectorIndex": {"show_segment"},
    "Subgraph": {"get_segment"},
    "Unwind": {"unwind_segment"},
    "Update": {"update_segment"},
    "UpdateConfigs": {"update_segment"},
    "Use": {"use_segment"},
    "With": {"with_segment"},
    "Yield": {"yield_segment"},
}

RULE_ALLOWLIST = {
    "Pipe": "pipe_statement",
    "SetOperation": "set_operation_statement",
}

failures = []

for variant in list(SEGMENT_MAP) + list(RULE_ALLOWLIST):
    if variant not in variants:
        failures.append(f"grammar mapping refers to unknown Stmt variant: {variant}")

for variant in variants:
    if variant in SEGMENT_MAP:
        continue
    if variant in RULE_ALLOWLIST:
        continue
    failures.append(f"Stmt variant has no grammar mapping: {variant}")

for variant, mapped in SEGMENT_MAP.items():
    missing = mapped - segments
    if missing:
        failures.append(f"Stmt variant {variant} maps to missing segments: {sorted(missing)}")

print(f"stmt variants: {len(variants)}")
print(f"grammar segments: {len(segments)}")
print(f"grammar rules: {len(rules)}")
print(f"mapped variants: {len(SEGMENT_MAP) + len(RULE_ALLOWLIST)}")

if failures:
    print("grammar structure failures:")
    for failure in sorted(set(failures)):
        print(f"  - {failure}")
    sys.exit(1)

print("grammar structure check passed")
