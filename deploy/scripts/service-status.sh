#!/usr/bin/env bash
set -euo pipefail

SERVICE_NAME="linkrs"

echo "== systemd unit =="
systemctl status "$SERVICE_NAME" --no-pager || true

echo
echo "== enabled state =="
systemctl is-enabled "$SERVICE_NAME" 2>&1 || true

echo
echo "== active state =="
systemctl is-active "$SERVICE_NAME" 2>&1 || true
