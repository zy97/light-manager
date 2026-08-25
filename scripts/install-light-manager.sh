#!/usr/bin/env bash

# light-manager 一键安装脚本(TCP 流水线信号灯管理服务,Linux 服务器)
#
# 用法:
#   curl -fsSL https://raw.githubusercontent.com/zy97/light-manager/main/scripts/install-light-manager.sh | sudo bash
# 或本地执行:
#   sudo ./install-light-manager.sh [版本号]
#
# 支持版本格式: 1.0.4 / v1.0.4 / light-manager/1.0.4 / light-manager/v1.0.4
# 重复执行即升级: 先停服务、替换二进制、再启动; 已有的 config.toml 不会被覆盖。
#
# 国内访问 GitHub Release 较慢时,可通过 GH_PROXY 设置代理前缀(末尾带 /):
#   GH_PROXY="https://ghfast.top/" curl -fsSL .../install-light-manager.sh | sudo -E bash

set -euo pipefail

REPO="zy97/light-manager"
INSTALL_DIR="/opt/light-manager"
SERVICE_NAME="light-manager"
PKG_PREFIX="light-manager"

# 根据系统 libc 自动选择 Release 产物; 可通过 TARGET 环境变量强制覆盖
if [[ -n "${TARGET:-}" ]]; then
  ASSET="$TARGET"
elif ldd --version 2>&1 | head -1 | grep -qi musl; then
  ASSET="light-manager-x86_64-unknown-linux-musl.tar.xz"
else
  ASSET="light-manager-x86_64-unknown-linux-gnu.tar.xz"
fi

GH_PROXY="${GH_PROXY:-}"

if [[ $EUID -ne 0 ]]; then
  echo "请用 root 运行(sudo)" >&2
  exit 1
fi

# 下载统一入口: 连接 15s 超时 + 重试,进度条可见
dl() {
  curl -fL --connect-timeout 15 --retry 3 --retry-delay 2 "$@"
}

# 列出所有 Release tag; 使用 sed 兼容无 PCRE 的老系统(如 CentOS 7)
list_tags() {
  dl -s "https://api.github.com/repos/$REPO/releases" | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p'
}

# 解析用户指定的版本,验证 Release 真实存在
resolve_user_tag() {
  local arg="$1" bare cand
  [[ "$arg" == */* ]] && { echo "$arg"; return; }
  bare="${arg#v}"
  for cand in "$PKG_PREFIX/v$bare" "v$bare"; do
    if [[ $(curl -s -o /dev/null -w "%{http_code}" "https://api.github.com/repos/$REPO/releases/tags/$cand") == "200" ]]; then
      echo "$cand"
      return
    fi
  done
  echo "找不到版本 $arg 对应的 Release(试过 $PKG_PREFIX/v$bare 和 v$bare)" >&2
  exit 1
}

# 自动选择最新版本: 优先匹配 $PKG_PREFIX/vX.Y.Z, 回退 vX.Y.Z
resolve_latest_tag() {
  local tags tag
  tags=$(list_tags)
  tag=$(printf '%s\n' "$tags" | grep -E "^$PKG_PREFIX/v?[0-9]+\.[0-9]+\.[0-9]+" | sort -V | tail -1) || true
  [[ -z "$tag" ]] && tag=$(printf '%s\n' "$tags" | grep -E "^v?[0-9]+\.[0-9]+\.[0-9]+" | sort -V | tail -1)
  if [[ -z "$tag" ]]; then
    echo "未找到任何 Release(api.github.com 不可达?),可显式指定: sudo ./install-light-manager.sh 1.0.4" >&2
    exit 1
  fi
  echo "$tag"
}

if [[ -n "${1:-}" ]]; then
  TAG=$(resolve_user_tag "$1")
else
  echo "==> 查询最新 Release..."
  TAG=$(resolve_latest_tag)
fi

echo "==> 安装 light-manager(Release: $TAG, 产物: $ASSET)到 $INSTALL_DIR"
mkdir -p "$INSTALL_DIR"

# 1. 下载并替换二进制; 先下载后停服,缩短服务中断窗口
echo "==> 下载二进制(GitHub Release)..."
dl "${GH_PROXY}https://github.com/$REPO/releases/download/$TAG/$ASSET" -o /tmp/light-manager.tar.xz

systemctl stop "$SERVICE_NAME" 2>/dev/null || true

rm -rf /tmp/light-manager-bin
mkdir -p /tmp/light-manager-bin
tar -xJf /tmp/light-manager.tar.xz -C /tmp/light-manager-bin
rm /tmp/light-manager.tar.xz

# dist 产物通常嵌套在一层同名子目录里,用 find 定位二进制
found=$(find /tmp/light-manager-bin -name light-manager -type f | head -1) || true
[[ -z "$found" ]] && { echo "压缩包里没找到 light-manager 二进制" >&2; exit 1; }

cp "$found" "$INSTALL_DIR/light-manager.new"
chmod +x "$INSTALL_DIR/light-manager.new"
mv "$INSTALL_DIR/light-manager.new" "$INSTALL_DIR/light-manager"
rm -rf /tmp/light-manager-bin

# 2. 补齐默认配置文件(仅首次安装)和日志目录
echo "==> 下载/补齐配置文件..."
dl "${GH_PROXY}https://codeload.github.com/$REPO/tar.gz/refs/tags/$TAG" -o /tmp/light-manager-src.tar.gz
rm -rf /tmp/light-manager-src
mkdir -p /tmp/light-manager-src
tar -xzf /tmp/light-manager-src.tar.gz -C /tmp/light-manager-src --strip-components=1

[[ -f "$INSTALL_DIR/config.toml" ]] || cp /tmp/light-manager-src/config.toml "$INSTALL_DIR/config.toml"
mkdir -p "$INSTALL_DIR/logs"

rm -rf /tmp/light-manager-src /tmp/light-manager-src.tar.gz

# 3. 注册 systemd 服务
echo "==> 注册 systemd 服务..."
cat > "/etc/systemd/system/$SERVICE_NAME.service" <<EOF
[Unit]
Description=light-manager TCP 流水线信号灯管理服务
After=network.target

[Service]
WorkingDirectory=$INSTALL_DIR
ExecStart=$INSTALL_DIR/light-manager
Restart=always
RestartSec=5
KillMode=control-group
TimeoutStopSec=10

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable "$SERVICE_NAME"
systemctl start "$SERVICE_NAME"

# 4. 防火墙放行 3000/tcp(按实际存在的防火墙工具处理)
if command -v ufw >/dev/null 2>&1 && ufw status | grep -q "Status: active"; then
  ufw allow 3000/tcp >/dev/null
  echo "==> ufw 已放行 3000/tcp"
elif command -v firewall-cmd >/dev/null 2>&1 && firewall-cmd --state >/dev/null 2>&1; then
  firewall-cmd --permanent --add-port=3000/tcp >/dev/null && firewall-cmd --reload >/dev/null
  echo "==> firewalld 已放行 3000/tcp"
fi

sleep 2
if systemctl is-active --quiet "$SERVICE_NAME"; then
  echo "安装完成,服务运行中。"
  echo "API 入口: http://<服务器IP>:3000"
  echo "别忘了编辑 $INSTALL_DIR/config.toml 里的 [light] 灯控配置"
else
  echo "服务未能启动,请查看日志: journalctl -u $SERVICE_NAME -e" >&2
  exit 1
fi
