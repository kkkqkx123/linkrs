#!/usr/bin/env bash
set -euo pipefail

SERVICE_NAME="linkrs"
PURGE_DATA="${PURGE_DATA:-no}"   # yes|no
PURGE_CONFIG="${PURGE_CONFIG:-no}" # yes|no

if [[ "$(id -u)" -ne 0 ]]; then
    echo "ERROR: must run as root" >&2
    exit 1
fi

echo "Uninstalling Linkrs service..."

# Stop and disable if present
if systemctl list-unit-files "${SERVICE_NAME}.service" >/dev/null 2>&1; then
    systemctl stop "$SERVICE_NAME" 2>/dev/null || true
    systemctl disable "$SERVICE_NAME" 2>/dev/null || true
fi

rm -f "/etc/systemd/system/${SERVICE_NAME}.service"
systemctl daemon-reload
systemctl reset-failed "$SERVICE_NAME" 2>/dev/null || true

if [[ "$PURGE_CONFIG" == "yes" ]]; then
    rm -rf /etc/linkrs
    echo "Removed /etc/linkrs"
fi

if [[ "$PURGE_DATA" == "yes" ]]; then
    rm -rf /var/lib/linkrs /var/log/linkrs
    echo "Removed data and log directories"
fi

echo "Done. Binary left at /usr/local/bin/linkrs-server (remove manually if desired)."
