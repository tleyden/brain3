#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"

usage() {
  echo "Usage: $0 <new-version>"
  echo ""
  echo "  <new-version>   New version without 'v' prefix, e.g. 0.2.3"
  echo ""
  echo "The script will update all version references, then ask before tagging and pushing."
  exit 1
}

[ $# -lt 1 ] && usage

NEW="${1#v}"   # strip leading 'v' if present

cd "$REPO_ROOT"

# Detect current version from gateway Cargo.toml
OLD=$(grep '^version' apps/gateway/Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')

if [ "$OLD" = "$NEW" ]; then
  echo "Already at version $NEW — nothing to do."
  exit 0
fi

echo "Bumping $OLD → $NEW"

# 1. apps/gateway/Cargo.toml
sed -i.bak "s/^version = \"$OLD\"/version = \"$NEW\"/" apps/gateway/Cargo.toml
rm -f apps/gateway/Cargo.toml.bak

# 2. brain3-mcp-vault-tools/pyproject.toml
sed -i.bak "s/^version = \"$OLD\"/version = \"$NEW\"/" brain3-mcp-vault-tools/pyproject.toml
rm -f brain3-mcp-vault-tools/pyproject.toml.bak

# 3. README.MD  (URL contains v-prefixed version)
sed -i.bak "s|/v${OLD}/|/v${NEW}/|g" README.MD
rm -f README.MD.bak

# 4. crates/core/src/application/first_run_setup.rs  (v-prefixed)
sed -i.bak "s|\"v${OLD}\"|\"v${NEW}\"|g" crates/core/src/application/first_run_setup.rs
rm -f crates/core/src/application/first_run_setup.rs.bak

# 5. brain3-mcp-vault-tools/tests/test_server_startup.py  (bare version, no v)
sed -i.bak "s/\"${OLD}\"/\"${NEW}\"/g" brain3-mcp-vault-tools/tests/test_server_startup.py
rm -f brain3-mcp-vault-tools/tests/test_server_startup.py.bak

# 6. Update Cargo.lock by running cargo fetch (fast, no build needed)
echo "Updating Cargo.lock..."
cargo fetch --quiet 2>&1 | grep -v '^$' || true

echo ""
echo "Done. Files updated:"
echo "  apps/gateway/Cargo.toml"
echo "  brain3-mcp-vault-tools/pyproject.toml"
echo "  README.MD"
echo "  crates/core/src/application/first_run_setup.rs"
echo "  brain3-mcp-vault-tools/tests/test_server_startup.py"
echo "  Cargo.lock"
echo ""
echo "Review the diff, then commit with:"
echo "  git commit -am \"bump version $NEW\""
echo ""

read -rp "Tag and push v${NEW} now? [y/N] " CONFIRM
if [[ "${CONFIRM,,}" == "y" ]]; then
  git tag -a "v${NEW}" -m "Release v${NEW}"
  git push origin "v${NEW}"
  echo "Tagged and pushed v${NEW}. Monitor the release workflow with: gh run watch"
else
  echo "Skipped tagging. To tag later:"
  echo "  git tag -a \"v${NEW}\" -m \"Release v${NEW}\""
  echo "  git push origin \"v${NEW}\""
fi
