#!/usr/bin/env bash
#
# Temporary i18n verification gate for the linkrs-gpui desktop client.
#
# The project is still unstable, so this is NOT wired into a CI runner yet.
# Run it manually (or later promote it to .github/workflows) once the project
# reaches the deployment stage. It exercises every translation-key check that
# lives as a unit test in crates/linkrs-gpui/src/i18n.rs:
#
#   - catalogues_parse
#   - every_chinese_key_exists_in_english
#   - unknown_keys_surface_as_is
#   - multi_arg_templates_fill_in_order
#   - catalogue_keys_stay_namespaced
#   - stage_one_keys_exist_in_both_locales
#   - catalogues_share_key_set
#   - chinese_entries_are_translated
#   - templates_share_placeholder_count
#   - call_site_keys_exist_in_catalogue
#   - english_entries_have_no_cjk            (added)
#   - catalogue_keys_are_referenced          (added)
#   - call_site_arg_count_matches_template   (added)
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

cd "${REPO_ROOT}"

echo "==> Building and running linkrs-gpui i18n tests"
cargo test --manifest-path crates/linkrs-gpui/Cargo.toml --bin linkrs-gpui i18n

echo "==> i18n checks passed"
