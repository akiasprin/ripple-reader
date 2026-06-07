#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# build.sh — Local build for ripple-reader
#
# 同时编译 Rust 后端和 UI 前端。
#
# Usage:
#   bash scripts/build.sh
#
# Options:
#   --release    Build in release mode (default)
#   --debug      Build in debug mode (faster, for development)
#   --no-ui      Skip UI build (npm)
# ---------------------------------------------------------------------------
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_DIR"

MODE="release"
BUILD_UI=true

while [[ $# -gt 0 ]]; do
  case "$1" in
    --release) MODE="release" ;;
    --debug)   MODE="debug" ;;
    --no-ui)   BUILD_UI=false ;;
    *)         echo "Unknown arg: $1"; exit 1 ;;
  esac
  shift
done

PROFILE_FLAG=""
TARGET_DIR="debug"
if [ "$MODE" = "release" ]; then
  PROFILE_FLAG="--release"
  TARGET_DIR="release"
fi

echo "── Rust ($MODE) ──"
cargo build $PROFILE_FLAG 2>&1 | tail -3
echo "  → target/$TARGET_DIR/ripple-reader"

if [ "$BUILD_UI" = true ]; then
  echo ""
  echo "── UI (npm) ──"
  cd ui
  npm install --silent 2>/dev/null
  npm run build 2>&1 | tail -3
  echo "  → static/app.js"
  cd "$PROJECT_DIR"
fi

echo ""
echo "✓ Build complete ($MODE)"
