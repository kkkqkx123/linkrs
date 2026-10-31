#!/usr/bin/env bash
set -euo pipefail

PREFIX="${PREFIX:-/usr}"
CONFIG_DIR="${CONFIG_DIR:-/etc/linkrs}"
DATA_DIR="${DATA_DIR:-/var/lib/linkrs}"
LOG_DIR="${LOG_DIR:-/var/log/linkrs}"
SERVICE_NAME="linkrs"
DEPLOY_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
UNIT_SRC="${DEPLOY_ROOT}/systemd/${SERVICE_NAME}.service"
# Default config shipped with the deployment resources
CONFIG_TEMPLATE="${CONFIG_TEMPLATE:-${DEPLOY_ROOT}/config/linkrs.toml}"

SERVICE_USER="${SERVICE_USER:-linkrs}"
SERVICE_GROUP="${SERVICE_GROUP:-linkrs}"

ENABLE_SERVICE="${ENABLE_SERVICE:-no}"   # yes|no: force enable/disable
START_SERVICE="${START_SERVICE:-no}"     # yes|no: start after install

normalize() {
    # Strip trailing slashes so "${PREFIX}/bin" stays well-formed
    while [[ $# -gt 0 ]]; do
        printf '%s\n' "${1%/}"
        shift
    done
}
PREFIX="$(normalize "$PREFIX")"
CONFIG_DIR="$(normalize "$CONFIG_DIR")"
DATA_DIR="$(normalize "$DATA_DIR")"
LOG_DIR="$(normalize "$LOG_DIR")"
BIN_PATH="${PREFIX}/bin/linkrs-server"

if [[ "$(id -u)" -ne 0 ]]; then
    echo "ERROR: must run as root" >&2
    exit 1
fi

if [[ ! -f "$UNIT_SRC" ]]; then
    echo "ERROR: unit file not found: $UNIT_SRC" >&2
    exit 1
fi

if [[ "$START_SERVICE" == "yes" && ! -x "$BIN_PATH" ]]; then
    echo "ERROR: START_SERVICE=yes but binary not found: $BIN_PATH" >&2
    exit 1
fi

umask 027
echo "Installing Linkrs service files..."

# Create service account if missing
if ! getent group "$SERVICE_GROUP" >/dev/null 2>&1; then
    groupadd --system "$SERVICE_GROUP"
fi
if ! id -u "$SERVICE_USER" >/dev/null 2>&1; then
    useradd --system --gid "$SERVICE_GROUP" --home-dir "$DATA_DIR" --create-home --shell /usr/sbin/nologin "$SERVICE_USER"
fi

# Create directories
install -d -o "$SERVICE_USER" -g "$SERVICE_GROUP" -m 0750 "$CONFIG_DIR" "$DATA_DIR" "$LOG_DIR"

# Install binary if present in ./bin or the current directory
BIN_SRC=""
for candidate in "./bin/linkrs-server" "./linkrs-server" "$BIN_PATH"; do
    if [[ -x "$candidate" ]]; then
        BIN_SRC="$candidate"
        break
    fi
done
if [[ -n "$BIN_SRC" && "$BIN_SRC" != "$BIN_PATH" ]]; then
    install -d -m 0755 "${PREFIX}/bin"
    install -m 0755 "$BIN_SRC" "$BIN_PATH"
    echo "Installed binary from $BIN_SRC"
else
    if [[ ! -x "$BIN_PATH" ]]; then
        echo "WARNING: ${BIN_PATH} not found; install the binary manually" >&2
    fi
fi

# Install default config if missing (template -> config dir)
if [[ ! -f "${CONFIG_DIR}/config.toml" ]]; then
    if [[ -f "$CONFIG_TEMPLATE" ]]; then
        install -m 0640 "$CONFIG_TEMPLATE" "${CONFIG_DIR}/config.toml"
        chown "$SERVICE_USER:$SERVICE_GROUP" "${CONFIG_DIR}/config.toml"
        echo "Installed config template from $CONFIG_TEMPLATE"
    elif [[ -f "./config.toml" ]]; then
        install -m 0640 "./config.toml" "${CONFIG_DIR}/config.toml"
        chown "$SERVICE_USER:$SERVICE_GROUP" "${CONFIG_DIR}/config.toml"
    else
        echo "WARNING: no config template found; create ${CONFIG_DIR}/config.toml manually" >&2
    fi
fi

# Install systemd unit
install -m 0644 "$UNIT_SRC" "/etc/systemd/system/${SERVICE_NAME}.service"
systemctl daemon-reload

echo "Installed /etc/systemd/system/${SERVICE_NAME}.service"

# Enable/disable according to request
if [[ "$ENABLE_SERVICE" == "yes" ]]; then
    systemctl enable "$SERVICE_NAME"
    echo "Enabled $SERVICE_NAME (autostart on boot)"
elif [[ "$ENABLE_SERVICE" == "no" ]]; then
    systemctl disable "$SERVICE_NAME" 2>/dev/null || true
fi

if [[ "$START_SERVICE" == "yes" ]]; then
    systemctl restart "$SERVICE_NAME"
    systemctl --no-pager --full status "$SERVICE_NAME" || true
fi

echo "Done."
echo "Manage with: systemctl {start|stop|restart|status|enable|disable} $SERVICE_NAME"
