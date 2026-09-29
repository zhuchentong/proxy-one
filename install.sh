#!/usr/bin/env bash
# proxyone Linux x86_64 安装脚本
#
# 一键安装:
#   curl -fsSL https://raw.githubusercontent.com/zhuchentong/proxy-one/main/install.sh | bash
#
# 本地执行:
#   ./install.sh                 # 安装到 ~/.local/bin
#   ./install.sh --system        # 安装到 /usr/local/bin（需要 sudo/root）
#   ./install.sh --prefix DIR    # 安装到 DIR/bin
#
# 说明:
#   - 从 GitHub Releases latest 直链下载 proxyone-linux-x64 与 .sha256（不走 API，无 rate limit）；
#   - 下载遵循 https_proxy / http_proxy 环境变量（curl/wget 原生行为）；
#   - 安装后用 ldd 冒烟检查缺失动态库（本程序需 glibc >= 2.35 与 OpenSSL 3）。
set -euo pipefail

REPO="zhuchentong/proxy-one"
ASSET="proxyone-linux-x64"
# 覆盖下载基地址（供测试 / 镜像使用），默认 GitHub latest release 直链
BASE="${PROXYONE_DOWNLOAD_BASE:-https://github.com/${REPO}/releases/latest/download}"

log() { printf '==> %s\n' "$*"; }
die() { printf '错误: %s\n' "$*" >&2; exit 1; }

usage() {
  cat <<EOF
用法: install.sh [选项]

选项:
  --prefix DIR   安装到 DIR/bin/proxyone（默认 ~/.local）
  --system       安装到 /usr/local/bin（需要 sudo/root 权限）
  -h, --help     显示本帮助

一键安装:
  curl -fsSL https://raw.githubusercontent.com/${REPO}/main/install.sh | bash
EOF
}

# ---------------------------------------------------------------- 参数解析
PREFIX=""
SYSTEM=0
while [ $# -gt 0 ]; do
  case "$1" in
    --prefix)
      [ $# -ge 2 ] || { printf '错误: --prefix 需要一个目录参数\n\n' >&2; usage >&2; exit 2; }
      PREFIX="$2"; shift 2
      ;;
    --system) SYSTEM=1; shift ;;
    -h | --help) usage; exit 0 ;;
    *)
      printf '错误: 未知参数 %s\n\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
done
if [ "$SYSTEM" -eq 1 ]; then
  [ -z "$PREFIX" ] || die "--system 与 --prefix 不能同时使用"
  PREFIX="/usr/local"
fi
[ -n "$PREFIX" ] || PREFIX="$HOME/.local"
INSTALL_DIR="$PREFIX/bin"

# ---------------------------------------------------------------- 平台检查
[ "$(uname -s)" = "Linux" ] ||
  die "仅支持 Linux（当前: $(uname -s)）；Windows 请到 https://github.com/${REPO}/releases 手动下载 proxyone.exe"
[ "$(uname -m)" = "x86_64" ] ||
  die "仅支持 x86_64（当前: $(uname -m)）；其他架构请到 https://github.com/${REPO}/releases 手动下载"

# ---------------------------------------------------------------- 下载
command -v sha256sum >/dev/null 2>&1 || die "缺少 sha256sum（coreutils），请先安装"

fetch() { # fetch <url> <输出文件>
  if command -v curl >/dev/null 2>&1; then
    curl -fSL --retry 3 --retry-delay 1 --connect-timeout 15 -o "$2" "$1"
  elif command -v wget >/dev/null 2>&1; then
    wget -q --tries=3 --timeout=15 -O "$2" "$1"
  else
    die "需要 curl 或 wget 之一，请先安装"
  fi
}

TMP_DL="$(mktemp -d "${TMPDIR:-/tmp}/proxyone-install.XXXXXX")"
trap 'rm -rf "$TMP_DL"' EXIT

log "下载 ${ASSET}（${BASE}）"
fetch "${BASE}/${ASSET}" "${TMP_DL}/${ASSET}" ||
  die "下载失败: ${BASE}/${ASSET}（网络受限时可设置 https_proxy 环境变量后重试）"
fetch "${BASE}/${ASSET}.sha256" "${TMP_DL}/${ASSET}.sha256" ||
  die "下载失败: ${BASE}/${ASSET}.sha256（网络受限时可设置 https_proxy 环境变量后重试）"

log "校验 sha256"
if ! (cd "$TMP_DL" && sha256sum -c "${ASSET}.sha256" >/dev/null 2>&1); then
  die "sha256 校验失败：下载内容不完整或被篡改，已中止安装"
fi

# ---------------------------------------------------------------- 安装
SUDO=""
if ! mkdir -p "$INSTALL_DIR" 2>/dev/null; then
  [ "$(id -u)" -eq 0 ] || SUDO="sudo"
  log "${INSTALL_DIR} 无法直接创建，改用 sudo"
  $SUDO mkdir -p "$INSTALL_DIR"
fi
if [ ! -w "$INSTALL_DIR" ] && [ "$(id -u)" -ne 0 ]; then
  SUDO="sudo"
fi
if [ -n "$SUDO" ]; then
  $SUDO install -m 755 "${TMP_DL}/${ASSET}" "${INSTALL_DIR}/proxyone"
else
  install -m 755 "${TMP_DL}/${ASSET}" "${INSTALL_DIR}/proxyone"
fi

# ---------------------------------------------------------------- 安装后检查
log "检查动态库依赖"
if command -v ldd >/dev/null 2>&1; then
  MISSING="$(ldd "${INSTALL_DIR}/proxyone" 2>/dev/null | grep -i 'not found' || true)"
  if [ -n "$MISSING" ]; then
    printf '警告: 缺少以下动态库，程序可能无法启动:\n%s\n' "$MISSING" >&2
    printf '本程序需要 glibc >= 2.35 与 OpenSSL 3 的较新发行版。\n' >&2
  fi
fi

case ":$PATH:" in
  *":${INSTALL_DIR}:"*) ;;
  *)
    # shellcheck disable=SC2016  # 刻意原样打印 $PATH 字面量，供用户复制
    printf '提示: %s 不在 PATH 中，请将下一行加入 ~/.bashrc 或 ~/.zshrc 后重新登录:\n  export PATH="%s:$PATH"\n' \
      "$INSTALL_DIR" "$INSTALL_DIR"
    ;;
esac

log "安装完成: ${INSTALL_DIR}/proxyone"
printf '启动: %s/proxyone（GUI）或 %s/proxyone --headless（无界面常驻）\n' "$INSTALL_DIR" "$INSTALL_DIR"
printf '卸载: rm %s/proxyone\n' "$INSTALL_DIR"
