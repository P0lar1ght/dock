#!/bin/zsh
# 起 MCP App 端到端要的三样：假模型(18998)、隔离 Dock(18997)、worktree 的 GUI 前端(1430)。
# 用法：./start-env.sh [fake|deepseek-flash]   停：./start-env.sh stop
set -e
HERE=${0:A:h}
WT=${HERE:h:h:h}                       # dock-wt-mcp-apps
DOCK=/Users/polar/Desktop/AILab/dock/target/debug/dock
pkill -f "dock serve --remote --bind 127.0.0.1:18997" 2>/dev/null || true
pkill -f "fake-model.mjs" 2>/dev/null || true
pkill -f "vite --port 1430" 2>/dev/null || true
[[ "$1" == stop ]] && { echo stopped; exit 0; }
MODEL=${1:-fake}
sed -i '' "s/^default = \".*\"/default = \"$MODEL\"/" /tmp/dock-e2e/home/config.toml
rm -rf /tmp/dock-e2e/home/apps/whiteboard          # 每次从空白板开始
(cd $HERE && nohup node fake-model.mjs 18998 > /tmp/dock-e2e/fake.log 2>&1 &)
(cd /tmp/dock-e2e/proj && DOCK_HOME=/tmp/dock-e2e/home nohup $DOCK serve --remote --bind 127.0.0.1:18997 > /tmp/dock-e2e/serve.log 2>&1 &)
(cd $WT/dock-gui && nohup npx vite --port 1430 --strictPort > /tmp/dock-e2e/vite.log 2>&1 &)
sleep 4
echo "model=$MODEL  dock: $(tail -1 /tmp/dock-e2e/serve.log)"
