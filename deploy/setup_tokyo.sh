#!/bin/bash
set -e

# Configuration
SS_PORT=8388
SS_PASSWORD="HftSecurePassword2026!" # Replace this with a secure password
SS_METHOD="chacha20-ietf-poly1305"

echo ">>> Setting up Shadowsocks on Tokyo Server..."

# Detect OS
if [ -f /etc/debian_version ]; then
    OS="Debian"
    apt-get update && apt-get install -y snapd curl
elif [ -f /etc/redhat-release ]; then
    OS="RedHat"
    yum install -y epel-release
    yum install -y snapd curl
fi

# Install Shadowsocks-Rust via Snap (Easy and consistent)
# Check if snap is installed
if ! command -v snap &> /dev/null; then
    echo "Installing Snapd..."
    if [ "$OS" == "Debian" ]; then
        apt-get install -y snapd
    else
        yum install -y snapd
        systemctl enable --now snapd.socket
        ln -s /var/lib/snapd/snap /snap
    fi
fi

echo "Installing shadowsocks-rust..."
snap install shadowsocks-rust --edge

# Create Config
mkdir -p /var/snap/shadowsocks-rust/common/etc/shadowsocks-rust
cat <<EOF > /var/snap/shadowsocks-rust/common/etc/shadowsocks-rust/config.json
{
    "server": "0.0.0.0",
    "server_port": $SS_PORT,
    "password": "$SS_PASSWORD",
    "method": "$SS_METHOD",
    "mode": "tcp_and_udp"
}
EOF

# Create Systemd Service for Snap if not auto-created, or use the snap service
# Snap usually handles this, but let's ensure it's running
echo "Starting Shadowsocks..."
snap start shadowsocks-rust

# Allow Firewall
if command -v ufw &> /dev/null; then
    ufw allow $SS_PORT/tcp
    ufw allow $SS_PORT/udp
elif command -v firewall-cmd &> /dev/null; then
    firewall-cmd --permanent --add-port=$SS_PORT/tcp
    firewall-cmd --permanent --add-port=$SS_PORT/udp
    firewall-cmd --reload
else
    # Fallback to iptables
    iptables -I INPUT -p tcp --dport $SS_PORT -j ACCEPT
    iptables -I INPUT -p udp --dport $SS_PORT -j ACCEPT
fi

echo ">>> Shadowsocks Setup Complete."
echo "Port: $SS_PORT"
echo "Password: $SS_PASSWORD"
echo "Method: $SS_METHOD"
