#!/usr/bin/env bash
set -euo pipefail

PREFIX="${PREFIX:-/usr/local}"
CONFIG_DIR="${CONFIG_DIR:-/etc/linkrs}"
DATA_DIR="${DATA_DIR:-/var/lib/linkrs}"
LOG_DIR="${LOG_DIR:-/var/log/linkrs}"
SERVICE_NAME="linkrs"
UNIT_SRC="$(cd "$(dirname "$0")/.." && pwd)/systemd/${SERVICE_NAME}.service"

SERVICE_USER="${SERVICE_USER:-linkrs}"
SERVICE_GROUP="${SERVICE_GROUP:-linkrs}"

ENABLE_SERVICE="${ENABLE_SERVICE:-no}"   # yes|no
START_SERVICE="${START_SERVICE:-no}"     # yes|no
FORCE="${FORCE:-no}"                    # yes|no

if [[ "$(id -u)" -ne 0 ]]; then
    echo "ERROR: must run as root" >&2
    exit 1
fi

if [[ ! -f "$UNIT_SRC" ]]; then
    echo "ERROR: unit file not found: $UNIT_SRC" >&2
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
install -d -o "$SERVICE_USER" -g "$SERVICE_GROUP" -m 0750 "$CONFIG_DIR"
install -d -o "$SERVICE_USER" -g "$SERVICE_GROUP" -m 0750 "$DATA_DIR"
install -d -o "$SERVICE_USER" -g "$SERVICE_GROUP" -m 0750 "$LOG_DIR"

# Install binary if present in ./bin or current directory
BIN_SRC=""
for candidate in "./bin/linkrs-server" "./linkrs-server" "${PREFIX}/bin/linkrs-server"; do
    if [[ -x "$candidate" ]]; then
        BIN_SRC="$candidate"
        break
    fi
done
if [[ -n "$BIN_SRC" && "$BIN_SRC" != "${PREFIX}/bin/linkrs-server" ]]; then
    install -m 0755 "$BIN_SRC" "${PREFIX}/bin/linkrs-server"
    echo "Installed binary from $BIN_SRC"
else
    if [[ ! -x "${PREFIX}/bin/linkrs-server" ]]; then
        echo "WARNING: ${PREFIX}/bin/linkrs-server not found; install binary manually" >&2
    fi
fi

# Install default config if missing
if [[ ! -f "${CONFIG_DIR}/config.toml" ]]; then
    if [[ -f "./config.toml" ]]; then
        install -m 0640 "./config.toml" "${CONFIG_DIR}/config.toml"
        chown "$SERVICE_USER:$SERVICE_GROUP" "${CONFIG_DIR}/config.toml"
    else
        echo "WARNING: no config.toml found; create ${CONFIG_DIR}/config.toml manually" >&2
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
else
    # Ensure explicit disabled state if previously enabled
    if systemctl is-enabled "$SERVICE_NAME" >/dev/null 2>&1; then
        if [[ "$FORCE" == "yes" ]]; then
            systemctl disable "$SERVICE_NAME"
            echo "Disabled $SERVICE_NAME"
        else
            echo "Service already enabled; pass ENABLE_SERVICE=no FORCE=yes to disable"
        fi
    fi
fi

if [[ "$START_SERVICE" == "yes" ]]; then
    systemctl restart "$SERVICE_NAME"
    systemctl --no-pager --full status "$SERVICE_NAME" || true
fi

echo "Done."
echo "Manage with: systemctl {start|stop|restart|status|enable|disable} $SERVICE_NAME"
