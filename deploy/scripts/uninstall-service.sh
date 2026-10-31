#!/usr/bin/env bash
set -euo pipefail

SERVICE_NAME="linkrs"
UNIT_PATH="/etc/systemd/system/${SERVICE_NAME}.service"
PURGE_CONFIG="${PURGE_CONFIG:-no}"   # yes|no: also remove /etc/linkrs
PURGE_DATA="${PURGE_DATA:-no}"       # yes|no: also remove data and log dirs

if [[ "$(id -u)" -ne 0 ]]; then
    echo "ERROR: must run as root" >&2
    exit 1
fi

echo "Uninstalling Linkrs service..."

# Stop and disable only when the unit file exists
if [[ -f "$UNIT_PATH" ]]; then
    systemctl stop "$SERVICE_NAME" 2>/dev/null || true
    systemctl disable "$SERVICE_NAME" 2>/dev/null || true
    rm -f "$UNIT_PATH"
    systemctl daemon-reload
fi
systemctl reset-failed "$SERVICE_NAME" 2>/dev/null || true

if [[ "$PURGE_CONFIG" == "yes" ]]; then
    rm -rf /etc/linkrs
    echo "Removed /etc/linkrs"
fi

if [[ "$PURGE_DATA" == "yes" ]]; then
    rm -rf /var/lib/linkrs /var/log/linkrs
    echo "Removed data and log directories"
fi

echo "Done. Binary left at /usr/bin/linkrs-server (remove manually if desired)."
