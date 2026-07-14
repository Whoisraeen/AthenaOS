#!/usr/bin/env bash
# athena-ssh-setup.sh — enable SSH on Athena (Arch) so the AthenaOS dev box can read
# the live GPU/PSP register state over the LAN (umr/debugfs). READS ONLY from the
# dev box side; this script only sets up the sshd + an authorized key.
#
#   sudo bash scripts/athena-ssh-setup.sh "ssh-ed25519 AAAA...devbox-pubkey"
#   # (or run with no arg and it tells you where to paste the key)
set -u
if [ "$(id -u)" -ne 0 ]; then echo "run with sudo"; exit 1; fi

# the non-root user you'll SSH in as (owns authorized_keys)
TARGET_USER="${SUDO_USER:-$(logname 2>/dev/null || echo root)}"
TARGET_HOME="$(getent passwd "$TARGET_USER" | cut -d: -f6)"

echo "== install + enable sshd =="
pacman -Sy --needed --noconfirm openssh >/dev/null 2>&1 || echo "(openssh present, or pacman busy)"
systemctl enable --now sshd
systemctl --no-pager status sshd 2>/dev/null | sed -n '1,4p'

echo
echo "== firewall (best effort — Arch has none by default) =="
command -v ufw >/dev/null 2>&1 && ufw allow 22/tcp 2>/dev/null || true

echo
echo "== LAN IP(s) — use one of these as <ATHENA-IP> =="
ip -4 -o addr show scope global 2>/dev/null | awk '{print "  "$2": "$4}'

echo
echo "== authorized_keys for user: $TARGET_USER  ($TARGET_HOME) =="
install -d -m 700 -o "$TARGET_USER" -g "$TARGET_USER" "$TARGET_HOME/.ssh"
AK="$TARGET_HOME/.ssh/authorized_keys"
if [ -n "${1:-}" ]; then
  grep -qF "$1" "$AK" 2>/dev/null || echo "$1" >> "$AK"
  chown "$TARGET_USER:$TARGET_USER" "$AK"; chmod 600 "$AK"
  echo "OK — added the supplied dev-box pubkey to $AK"
else
  echo "NO pubkey passed. Either:"
  echo "  1) re-run:  sudo bash scripts/athena-ssh-setup.sh \"<paste the dev-box pubkey>\""
  echo "  2) append the dev box's ~/.ssh/id_ed25519.pub line into: $AK"
fi

echo
echo "== then, from the AthenaOS dev box: =="
echo "  ssh $TARGET_USER@<ATHENA-IP>     # should log in with no password prompt"
echo "Done."
