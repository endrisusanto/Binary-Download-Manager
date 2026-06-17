#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CRATE_MANIFEST="$ROOT_DIR/tools/mdvh-agent-probe/Cargo.toml"
PACKAGE_NAME="mdvh-agent-probe"
TAURI_DIR="$ROOT_DIR/tools/mdvh-download-manager"
TAURI_MANIFEST="$TAURI_DIR/src-tauri/Cargo.toml"
TAURI_CONF="$TAURI_DIR/src-tauri/tauri.conf.json"
TAURI_PACKAGE_JSON="$TAURI_DIR/package.json"

usage() {
  cat <<'EOF'
Usage:
  scripts/release.sh [patch|minor|major|X.Y.Z] [--no-push]

Examples:
  scripts/release.sh patch
  scripts/release.sh minor
  scripts/release.sh 0.2.0 --no-push

The script will:
  1. Bump tools/mdvh-agent-probe/Cargo.toml version.
  2. Run cargo fmt, test, and release build.
  3. Commit the release.
  4. Create git tag vX.Y.Z.
  5. Push commit and tag unless --no-push is used.
EOF
}

bump="${1:-patch}"
push_enabled="1"
if [[ "${2:-}" == "--no-push" || "${1:-}" == "--no-push" ]]; then
  push_enabled="0"
  [[ "$bump" == "--no-push" ]] && bump="patch"
fi

if [[ "$bump" == "-h" || "$bump" == "--help" ]]; then
  usage
  exit 0
fi

cd "$ROOT_DIR"

if [[ ! -f "$CRATE_MANIFEST" ]]; then
  echo "Missing manifest: $CRATE_MANIFEST" >&2
  exit 1
fi

if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "Working tree is dirty. Commit or stash changes before releasing." >&2
  git status --short
  exit 1
fi

current_version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$CRATE_MANIFEST" | head -n 1)"
if [[ ! "$current_version" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
  echo "Unsupported version format: $current_version" >&2
  exit 1
fi

major="${BASH_REMATCH[1]}"
minor="${BASH_REMATCH[2]}"
patch="${BASH_REMATCH[3]}"

case "$bump" in
  patch)
    patch=$((patch + 1))
    ;;
  minor)
    minor=$((minor + 1))
    patch=0
    ;;
  major)
    major=$((major + 1))
    minor=0
    patch=0
    ;;
  *)
    if [[ ! "$bump" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
      echo "Invalid bump '$bump'. Use patch, minor, major, or X.Y.Z." >&2
      exit 1
    fi
    major="${bump%%.*}"
    rest="${bump#*.}"
    minor="${rest%%.*}"
    patch="${rest#*.}"
    ;;
esac

next_version="$major.$minor.$patch"
tag="v$next_version"

if git rev-parse "$tag" >/dev/null 2>&1; then
  echo "Tag already exists locally: $tag" >&2
  exit 1
fi

if git ls-remote --tags origin "$tag" | grep -q "$tag"; then
  echo "Tag already exists on origin: $tag" >&2
  exit 1
fi

echo "Releasing $PACKAGE_NAME and Tauri App $current_version -> $next_version"

# Bump Rust library package version
perl -0pi -e "s/(\\[package\\]\\nname = \"$PACKAGE_NAME\"\\nversion = \")$current_version(\"\\n)/\${1}$next_version\${2}/" "$CRATE_MANIFEST"

# Bump Tauri App Rust Package version
perl -0pi -e "s/(\\[package\\]\\nname = \"toolsmdvh-download-manager\"\\nversion = \")$current_version(\"\\n)/\${1}$next_version\${2}/" "$TAURI_MANIFEST"

# Bump package.json version
perl -pi -e "s/\"version\": \"$current_version\"/\"version\": \"$next_version\"/" "$TAURI_PACKAGE_JSON"

# Bump tauri.conf.json version
perl -pi -e "s/\"version\": \"$current_version\"/\"version\": \"$next_version\"/" "$TAURI_CONF"

cargo fmt --all
cargo test --workspace
cargo build --release --package "$PACKAGE_NAME"
cargo build --release --package "toolsmdvh-download-manager"

git add "$CRATE_MANIFEST" "$TAURI_MANIFEST" "$TAURI_CONF" "$TAURI_PACKAGE_JSON" Cargo.lock
git add -u
git commit -m "chore: release $tag"
git tag -a "$tag" -m "Release $tag"

if [[ "$push_enabled" == "1" ]]; then
  git push origin main
  git push origin "$tag"
else
  echo "Created commit and tag locally. Push manually with:"
  echo "  git push origin main"
  echo "  git push origin $tag"
fi

echo "Release $tag complete."
