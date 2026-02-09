#!/bin/bash
set -e

# Configuration
TOKYO_IP="149.104.78.218"
PORT=8388

echo ">>> Setting up Relay on Khabarovsk Server..."
echo "Redirecting Local:$PORT -> $TOKYO_IP:$PORT"

# Enable IP Forwarding
echo "Enabling IP Forwarding..."
echo "net.ipv4.ip_forward = 1" > /etc/sysctl.d/99-relay.conf
sysctl -p /etc/sysctl.d/99-relay.conf

# Flushing existing nat rules (Optional: Be careful if other things are running)
# iptables -t nat -F 

# Add Forwarding Rules
# 1. DNAT: Incoming on PORT -> Destination TOKYO_IP:PORT
iptables -t nat -A PREROUTING -p tcp --dport $PORT -j DNAT --to-destination $TOKYO_IP:$PORT
iptables -t nat -A PREROUTING -p udp --dport $PORT -j DNAT --to-destination $TOKYO_IP:$PORT

# 2. MASQUERADE: Outgoing traffic looks like it comes from this server
iptables -t nat -A POSTROUTING -j MASQUERADE

# Save IPTables (Persist after reboot)
if command -v netfilter-persistent &> /dev/null; then
    netfilter-persistent save
elif [ -f /etc/redhat-release ]; then
    service iptables save
else
    echo "Warning: Make sure to install iptables-persistent or similar to save rules."
    if [ -f /etc/debian_version ]; then
        DEBIAN_FRONTEND=noninteractive apt-get install -y iptables-persistent
    fi
fi

echo ">>> Relay Setup Complete."
echo "You can now connect to THIS server ($HOST_IP) on port $PORT."
