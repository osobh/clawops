#!/bin/bash
# Install clawnode on a target server
# Usage: ./install.sh <hostname> <ssh_target> <config_file>
set -euo pipefail

HOST="$1"
SSH="$2"
CONFIG="$3"
BINARY="$(dirname "$0")/../target/release/clawnode"

echo "==> Deploying clawnode to $HOST ($SSH)"

# Copy binary
scp "$BINARY" "$SSH":/tmp/clawnode
ssh "$SSH" "sudo mv /tmp/clawnode /usr/local/bin/clawnode && sudo chmod +x /usr/local/bin/clawnode"

# Copy config
ssh "$SSH" "sudo mkdir -p /etc/clawnode /var/lib/clawnode"
scp "$CONFIG" "$SSH":/tmp/clawnode-config.json
ssh "$SSH" "sudo mv /tmp/clawnode-config.json /etc/clawnode/config.json"

# Install systemd service
scp "$(dirname "$0")/clawnode.service" "$SSH":/tmp/clawnode.service
ssh "$SSH" "sudo mv /tmp/clawnode.service /etc/systemd/system/clawnode.service && sudo systemctl daemon-reload && sudo systemctl enable clawnode && sudo systemctl restart clawnode"

echo "==> Checking status..."
sleep 2
ssh "$SSH" "sudo systemctl status clawnode --no-pager | head -15"
echo "==> Done: $HOST"
