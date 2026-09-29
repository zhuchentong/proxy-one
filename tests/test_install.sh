#!/usr/bin/env bash
# install.sh 行为测试：无需 root，不触碰真实 ~/.local。
# 网络用例（真实 GitHub 下载）在无法访问时自动跳过；设 SKIP_NETWORK=1 可强制跳过。
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="$ROOT/install.sh"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/proxyone-install-test.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
# 隔离 XDG 数据目录，避免测试污染真实的 ~/.local/share/applications
export XDG_DATA_HOME="$WORK/xdg"

pass=0
fail=0
ok()  { printf 'ok   - %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL - %s\n' "$1"; fail=$((fail + 1)); }

check() { # check <描述> <条件命令...>
  local desc="$1"; shift
  if "$@"; then ok "$desc"; else bad "$desc"; fi
}

# 生成一份假 release 资产：<dir>/{proxyone-linux-x64, proxyone-linux-x64.sha256}
make_fixture() {
  local dir="$1"
  mkdir -p "$dir"
  printf '#!/bin/sh\necho fixture-ok\n' > "$dir/proxyone-linux-x64"
  chmod +x "$dir/proxyone-linux-x64"
  (cd "$dir" && sha256sum proxyone-linux-x64 > proxyone-linux-x64.sha256)
}

# 篡改校验和的资产
make_bad_fixture() {
  local dir="$1"
  make_fixture "$dir"
  (cd "$dir" && sed -i 's/^\([0-9a-f]\)/0\1/;s/^\(0\)\{2,\}/00/' proxyone-linux-x64.sha256)
  # 确保与真实值不同
  (cd "$dir" && echo "0000000000000000000000000000000000000000000000000000000000000000  proxyone-linux-x64" > proxyone-linux-x64.sha256)
}

# ---------------------------------------------------------------- 前提
if [ ! -f "$SCRIPT" ]; then
  bad "install.sh 存在（$SCRIPT 不存在，先实现它）"
  printf '\n%d 通过, %d 失败\n' "$pass" "$fail"
  exit 1
fi
ok "install.sh 存在"

# ---------------------------------------------------------------- 用例 1: 正常安装（本地 fixture，--prefix）
dir="$WORK/fixture1"; dest="$WORK/dest1"; make_fixture "$dir"
PROXYONE_DOWNLOAD_BASE="file://$dir" bash "$SCRIPT" --prefix "$dest" > "$WORK/out1" 2>&1
rc=$?
check "正常安装退出码 0" test "$rc" -eq 0
check "安装到 \$prefix/bin/proxyone" test -x "$dest/bin/proxyone"
[ -x "$dest/bin/proxyone" ] && check "安装产物可执行且内容正确" test "$("$dest/bin/proxyone")" = "fixture-ok"
check "成功输出包含安装路径" grep -qF "$dest/bin/proxyone" "$WORK/out1"
check "非 PATH 目录安装后给出 PATH 提示" grep -q "PATH" "$WORK/out1"

# ---------------------------------------------------------------- 用例 1b: 桌面项（launcher 可搜索）
desktop="$WORK/xdg/applications/proxyone.desktop"
check "桌面项已写入 XDG applications" test -f "$desktop"
check "桌面项 Exec 为安装绝对路径" grep -qF "$dest/bin/proxyone" "$desktop"
check "桌面项含 Keywords 关键字" grep -q "^Keywords=" "$desktop"
check "卸载提示包含桌面项" grep -q "proxyone.desktop" "$WORK/out1"

# ---------------------------------------------------------------- 用例 2: sha256 校验失败
dir="$WORK/fixture2"; dest="$WORK/dest2"; make_bad_fixture "$dir"
PROXYONE_DOWNLOAD_BASE="file://$dir" bash "$SCRIPT" --prefix "$dest" > "$WORK/out2" 2>&1
rc=$?
check "校验失败退出码非 0" test "$rc" -ne 0
check "校验失败时不落盘二进制" test ! -e "$dest/bin/proxyone"
check "校验失败输出中文错误（含「校验」）" grep -q "校验" "$WORK/out2"

# ---------------------------------------------------------------- 用例 3: 资产不存在（404 语义）
dir="$WORK/empty"; dest="$WORK/dest3"; mkdir -p "$dir"
PROXYONE_DOWNLOAD_BASE="file://$dir" bash "$SCRIPT" --prefix "$dest" > "$WORK/out3" 2>&1
rc=$?
check "下载失败退出码非 0" test "$rc" -ne 0
check "下载失败时不落盘二进制" test ! -e "$dest/bin/proxyone"
check "下载失败输出中文错误（含「下载」）" grep -q "下载" "$WORK/out3"

# ---------------------------------------------------------------- 用例 4: 参数校验
bash "$SCRIPT" --bogus-flag > "$WORK/out4" 2>&1
check "未知参数退出码非 0" test $? -ne 0
bash "$SCRIPT" --system --prefix "$WORK/x" > "$WORK/out5" 2>&1
check "--system 与 --prefix 冲突退出码非 0" test $? -ne 0
bash "$SCRIPT" --help > "$WORK/out6" 2>&1
check "--help 退出码 0" test $? -eq 0
check "--help 说明 --prefix" grep -q -- "--prefix" "$WORK/out6"

# ---------------------------------------------------------------- 用例 5: 管道模式（模拟 curl | bash）
dir="$WORK/fixture1"; dest="$WORK/dest5"
PROXYONE_DOWNLOAD_BASE="file://$dir" bash -s -- --prefix "$dest" < "$SCRIPT" > "$WORK/out7" 2>&1
check "管道模式（stdin 执行 + 传参）安装成功" test -x "$dest/bin/proxyone"

# ---------------------------------------------------------------- 用例 6: 真实 GitHub latest 下载（可达时执行）
if [ "${SKIP_NETWORK:-0}" = "1" ]; then
  printf 'skip - 真实下载用例（SKIP_NETWORK=1）\n'
elif curl -fsS -m 10 -o /dev/null https://github.com >/dev/null 2>&1; then
  dest="$WORK/dest-real"
  bash "$SCRIPT" --prefix "$dest" > "$WORK/out8" 2>&1
  rc=$?
  check "真实下载安装退出码 0" test "$rc" -eq 0
  check "真实二进制已落盘且可执行" test -x "$dest/bin/proxyone"
  if [ -x "$dest/bin/proxyone" ]; then
    size="$(stat -c %s "$dest/bin/proxyone" 2>/dev/null || echo 0)"
    check "真实二进制体积 > 1MB" test "$size" -gt 1000000
    check "本机动态库完整（ldd 无 not found）" bash -c "! ldd '$dest/bin/proxyone' 2>/dev/null | grep -q 'not found'"
  fi
else
  printf 'skip - 真实下载用例（GitHub 不可达）\n'
fi

# ---------------------------------------------------------------- 汇总
printf '\n%d 通过, %d 失败\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
