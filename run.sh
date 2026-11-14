#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT_DIR"

# 自动探测补充 Node / Bun / Cargo 环境变量路径（兼容非交互式终端与 IDE 沙箱）
export PATH="$HOME/.cargo/bin:$HOME/.bun/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
if [ -d "$HOME/.nvm/versions/node" ]; then
  LATEST_NODE_BIN="$(ls -d "$HOME/.nvm/versions/node"/*/bin 2>/dev/null | tail -n 1 || true)"
  if [ -n "$LATEST_NODE_BIN" ]; then
    export PATH="$LATEST_NODE_BIN:$PATH"
  fi
fi

echo "========================================================"
echo "📦 [1/2] 正在编译 Vue 3 + Vite 前端工程 -> web_ui/ ..."
echo "========================================================"
cd "$ROOT_DIR/frontend"

if [ ! -d "node_modules" ]; then
  if command -v bun >/dev/null 2>&1; then
    echo "🔧 首次运行，正在使用 bun 安装前端依赖..."
    bun install
  elif command -v npm >/dev/null 2>&1; then
    echo "🔧 首次运行，正在使用 npm 安装前端依赖..."
    npm install
  else
    echo "❌ 未检测到 bun 或 npm，请先安装 Node.js 或 Bun"
    exit 1
  fi
fi

if command -v node >/dev/null 2>&1 && [ -f "./node_modules/vite/bin/vite.js" ]; then
  node ./node_modules/vite/bin/vite.js build
elif command -v bun >/dev/null 2>&1; then
  bun run build
elif command -v npm >/dev/null 2>&1; then
  npm run build
else
  echo "❌ 未检测到 node / bun / npm，请先安装 Node.js 或 Bun"
  exit 1
fi

cd "$ROOT_DIR"
echo "✅ 前端静态产物已输出至: $ROOT_DIR/web_ui/"

if [ "${1:-}" = "--build-only" ]; then
  echo "========================================================"
  echo "🦀 [2/2] 正在编译 Rust 服务端单二进制 (rust-embed 内嵌 web_ui/)..."
  echo "========================================================"
  cargo build "${@:2}"
  echo "✅ 编译完成！"
else
  echo "========================================================"
  echo "🚀 [2/2] 正在编译并启动 RX-Emby Rust 服务端..."
  echo "========================================================"
  exec cargo run "$@"
fi
