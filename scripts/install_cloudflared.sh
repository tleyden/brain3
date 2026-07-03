#!/usr/bin/env bash
set -euo pipefail

case "$(uname -s)" in
  Linux) os=linux ;;
  *) echo "unsupported OS for cloudflared install: $(uname -s)" >&2; exit 1 ;;
esac

case "$(uname -m)" in
  x86_64|amd64) arch=amd64 ;;
  aarch64|arm64) arch=arm64 ;;
  *) echo "unsupported architecture for cloudflared install: $(uname -m)" >&2; exit 1 ;;
esac

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

deb="$tmp_dir/cloudflared.deb"
curl -fsSL \
  "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-${os}-${arch}.deb" \
  -o "$deb"
sudo dpkg -i "$deb"
cloudflared --version
