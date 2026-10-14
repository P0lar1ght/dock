#!/bin/sh
# Dock 一键安装：从 GitHub Release 取预编译二进制，校验 sha256 后装进用户目录。
#
#   curl -fsSL https://raw.githubusercontent.com/P0lar1ght/dock/main/install.sh | sh
#
# 环境变量：
#   DOCK_VERSION=v0.1.0     指定版本（默认 latest）
#   DOCK_INSTALL_DIR=<dir>  安装目录（默认 $HOME/.local/bin）
#   DOCK_BASE_URL=<url>     覆盖下载基址（仅调试安装脚本时用）
#   DOCK_CHANNEL=stable|pre 预留通道（脚本本身仍走 latest / DOCK_VERSION；
#                           升级请用 dock upgrade，它会读 DOCK_CHANNEL）
#   DOCK_SKIP_INIT=1        装完不跑 dock init
#
# 支持：macOS aarch64 / x86_64，Linux aarch64 / x86_64。
# Windows 没有预编译产物（未做适配验证），脚本会明确拒绝而不是装个跑不起来的壳。
#
# 进度：TTY 下分「下载 / 安装 / 初始化」三阶段，下载带进度条 / 速度 / 剩余时间；
#       管道或非 TTY 降级为朴素行输出。
set -eu

REPO="P0lar1ght/dock"
ASSET="dock"

# —— 进度输出（无装饰依赖；非 TTY 自动降级）———————————————
_is_tty() {
    [ -t 1 ]
}

_color() {
    # $1=码 $2=文本；非 TTY 原样打印
    if _is_tty; then
        printf '\033[%sm%s\033[0m' "$1" "$2"
    else
        printf '%s' "$2"
    fi
}

phase() {
    # phase <n> <title>
    _phase_n=$1
    _phase_title=$2
    if _is_tty; then
        printf '\n'
        _color "1;36" "━━"
        printf ' '
        _color "1;36" "阶段 ${_phase_n}/3"
        printf ' · '
        _color "1;37" "${_phase_title}"
        printf '\n'
    else
        printf '\n== 阶段 %s/3：%s ==\n' "$_phase_n" "$_phase_title"
    fi
}

step() {
    # step <msg>
    if _is_tty; then
        printf '  '
        _color "0;36" "→"
        printf ' %s\n' "$1"
    else
        printf '  -> %s\n' "$1"
    fi
}

ok() {
    # ok <msg>
    if _is_tty; then
        printf '  '
        _color "1;32" "✓"
        printf ' %s\n' "$1"
    else
        printf '  [ok] %s\n' "$1"
    fi
}

warn() {
    # warn <msg>
    if _is_tty; then
        printf '  '
        _color "1;33" "!"
        printf ' %s\n' "$1" >&2
    else
        printf '  [!] %s\n' "$1" >&2
    fi
}

die() {
    if _is_tty; then
        printf '  ' >&2
        _color "1;31" "✗" >&2
        printf ' 安装失败：%s\n' "$1" >&2
    else
        printf '安装失败：%s\n' "$1" >&2
    fi
    exit 1
}

need() {
    command -v "$1" >/dev/null 2>&1
}

download() {
    # download <url> <输出文件>
    if need curl; then
        curl -fsSL "$1" -o "$2" || die "下载失败：$1"
    elif need wget; then
        wget -qO "$2" "$1" || die "下载失败：$1"
    else
        die "需要 curl 或 wget"
    fi
}

# —— 下载进度 ——————————————————————————————————————
# TTY + curl：curl 放后台，前台每 0.2s 看一眼落盘字节数，画进度条 / 速度 / 剩余时间；
# 还没收到字节时显示「连接中… Ns」，让人分得清是在等网络还是卡死。
# 非 TTY 或只有 wget：不画动态行（免得刷屏日志），只在完成后打一行汇总。

_human() {
    # _human <字节数> → 12.3 MB / 456 KB / 789 B
    awk -v b="$1" 'BEGIN {
        if (b >= 1048576) printf "%.1f MB", b / 1048576
        else if (b >= 1024) printf "%.0f KB", b / 1024
        else printf "%d B", b
    }'
}

_file_size() {
    if [ -f "$1" ]; then
        wc -c <"$1" | tr -d ' '
    else
        echo 0
    fi
}

_content_length() {
    # -D 头文件里有多段（302 → 200）：只认最后一段的 Content-Length；缺失 / 0 = 未知。
    if [ -f "$1" ]; then
        tr -d '\r' <"$1" | awk '
            toupper(substr($1, 1, 5)) == "HTTP/" { v = 0 }
            tolower($1) == "content-length:" { v = $2 }
            END { print v + 0 }'
    else
        echo 0
    fi
}

_progress_line() {
    # _progress_line <已下载> <总大小，0=未知> <字节/秒，-1=还没测出> <已用秒> <条宽>
    awk -v d="$1" -v t="$2" -v s="$3" -v e="$4" -v w="$5" '
        function h(b) {
            if (b >= 1048576) return sprintf("%.1f MB", b / 1048576)
            if (b >= 1024) return sprintf("%.0f KB", b / 1024)
            return sprintf("%d B", b)
        }
        BEGIN {
            if (d == 0) { printf "  \033[2m连接中… %ds\033[0m", e; exit }
            speed = s < 0 ? "测速中" : h(s) "/s"
            if (t > 0) {
                n = int(d * w / t); if (n > w) n = w
                bar = ""; for (i = 0; i < n; i++) bar = bar "━"
                rest = ""; for (i = n; i < w; i++) rest = rest "─"
                line = sprintf("  \033[32m%s\033[2m%s\033[0m %3d%%  %s / %s  %s", \
                    bar, rest, int(d * 100 / t), h(d), h(t), speed)
                if (s > 0 && t > d) line = line sprintf("  剩余 %ds", int((t - d) / s) + 1)
                printf "%s", line
            } else {
                printf "  已下载 %s  %s", h(d), speed
            }
        }'
}

download_progress() {
    # download_progress <url> <输出文件>
    _dl_start=$(date +%s)
    if _is_tty && need curl; then
        _cols=$(tput cols 2>/dev/null) || _cols=80
        case $_cols in '' | *[!0-9]*) _cols=80 ;; esac
        _width=$((_cols - 56))
        [ "$_width" -le 30 ] || _width=30
        [ "$_width" -ge 10 ] || _width=10
        curl -fsSL -D "$2.headers" "$1" -o "$2" 2>"$2.err" &
        _dl_pid=$!
        _tick=$_dl_start
        _tick_bytes=0
        _speed=-1
        while kill -0 "$_dl_pid" 2>/dev/null; do
            _now=$(date +%s)
            _done=$(_file_size "$2")
            # 速度按整秒采样：date +%s 只到秒，0.2s 一刷的进度条照样顺滑。
            # 收到第一个字节前不采样，否则连接耗时会被算成一个 0 B/s。
            if [ "$_done" -eq 0 ]; then
                _tick=$_now
            elif [ "$_now" -gt "$_tick" ]; then
                _speed=$(((_done - _tick_bytes) / (_now - _tick)))
                _tick=$_now
                _tick_bytes=$_done
            fi
            printf '\r\033[K%s' "$(_progress_line "$_done" "$(_content_length "$2.headers")" \
                "$_speed" $((_now - _dl_start)) "$_width")"
            sleep 0.2 2>/dev/null || sleep 1
        done
        printf '\r\033[K'
        if ! wait "$_dl_pid"; then
            _dl_pid=
            cat "$2.err" >&2
            die "下载失败：$1"
        fi
        _dl_pid=
    else
        download "$1" "$2"
    fi
    _bytes=$(_file_size "$2")
    _secs=$(($(date +%s) - _dl_start))
    [ "$_secs" -gt 0 ] || _secs=1
    ok "已下载 $(_human "$_bytes")（平均 $(_human $((_bytes / _secs)))/s）"
}

cleanup() {
    # 非交互 sh 里后台任务忽略 SIGINT：Ctrl-C 时得亲手停掉 curl，否则它还在后台下。
    if [ -n "${_dl_pid:-}" ]; then
        kill "$_dl_pid" 2>/dev/null || true
    fi
    rm -rf "$tmp"
}

sha256_of() {
    if need sha256sum; then
        sha256sum "$1" | awk '{print $1}'
    elif need shasum; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        die "需要 sha256sum 或 shasum 校验产物"
    fi
}

detect_target() {
    os=$(uname -s)
    arch=$(uname -m)
    case "$os" in
        Darwin)
            case "$arch" in
                arm64 | aarch64) echo "aarch64-apple-darwin" ;;
                x86_64) echo "x86_64-apple-darwin" ;;
                *) die "不支持的 macOS 架构：$arch" ;;
            esac
            ;;
        Linux)
            case "$arch" in
                x86_64 | amd64) echo "x86_64-unknown-linux-gnu" ;;
                aarch64 | arm64) echo "aarch64-unknown-linux-gnu" ;;
                *) die "不支持的 Linux 架构：$arch" ;;
            esac
            ;;
        MINGW* | MSYS* | CYGWIN* | Windows_NT)
            die "Windows 暂无预编译产物；请从源码 cargo build --release -p cordis-app 自行构建"
            ;;
        *)
            die "不支持的系统：$os"
            ;;
    esac
}

main() {
    if _is_tty; then
        printf '\n'
        _color "1;35" "Dock"
        printf ' 安装程序\n'
    else
        printf 'Dock 安装程序\n'
    fi

    target=$(detect_target)

    install_dir=${DOCK_INSTALL_DIR:-"$HOME/.local/bin"}
    [ -n "$install_dir" ] || die "安装目录为空"

    version=${DOCK_VERSION:-}
    if [ -n "${DOCK_BASE_URL:-}" ]; then
        base=$DOCK_BASE_URL
    elif [ -n "$version" ]; then
        case "$version" in
            v*) ;;
            *) version="v$version" ;;
        esac
        base="https://github.com/$REPO/releases/download/$version"
    else
        base="https://github.com/$REPO/releases/latest/download"
    fi

    file="$ASSET-$target.tar.gz"

    tmp=$(mktemp -d "${TMPDIR:-/tmp}/dock-install.XXXXXX") || die "无法创建临时目录"
    trap cleanup EXIT
    trap 'cleanup; exit 130' INT TERM

    # —— 1/3 下载 ————————————————————————————————
    phase 1 "下载"
    step "目标平台 $target"
    step "获取 $file"
    download_progress "$base/$file" "$tmp/$file"
    step "获取校验和 $file.sha256"
    download "$base/$file.sha256" "$tmp/$file.sha256"

    expected=$(awk '{print $1}' "$tmp/$file.sha256")
    [ -n "$expected" ] || die "sha256 文件为空，拒绝安装未校验的产物"
    actual=$(sha256_of "$tmp/$file")
    # 花括号不能省：$expected 后面紧跟全角标点时，bash 会把标点字节吃进变量名。
    [ "$expected" = "$actual" ] || die "校验失败：期望 ${expected}，实际 ${actual}"
    ok "校验通过（sha256）"

    # —— 2/3 安装 ————————————————————————————————
    phase 2 "安装"
    step "解包"
    tar -xzf "$tmp/$file" -C "$tmp" || die "解包失败"
    [ -f "$tmp/$ASSET" ] || die "产物里没有 $ASSET 二进制"

    step "写入 ${install_dir}（原子替换）"
    mkdir -p "$install_dir" || die "无法创建 $install_dir"
    # 先写同目录的临时名再 mv 覆盖：升级时旧 dock 可能正在跑，直接写会被 Linux
    # 拒成 ETXTBSY（Text file busy）；rename(2) 只换目录项，对已映射的旧进程无影响。
    staged="$install_dir/.$ASSET.new.$$"
    cp "$tmp/$ASSET" "$staged" || die "无法写入 ${install_dir}（目录不可写？）"
    chmod 0755 "$staged" || die "无法给 $staged 加执行位"
    mv -f "$staged" "$install_dir/$ASSET" || {
        rm -f "$staged"
        die "无法替换 $install_dir/$ASSET"
    }

    # 装完必须真的能跑起来：架构选错、缺 libssl.so.3 / glibc 太旧都只在这一步暴露。
    # 失败要连 stderr 一起吐出来，不能静默当成功。
    step "自检 --version"
    if ! installed=$("$install_dir/$ASSET" --version 2>&1); then
        printf '%s\n' "$installed" >&2
        die "$install_dir/$ASSET 装上了但跑不起来（见上方输出）"
    fi
    ok "已安装 $installed → $install_dir/$ASSET"

    # 产物没有代码签名/公证：只提示，不替用户改 Gatekeeper 的标记。
    if [ "$(uname -s)" = "Darwin" ] &&
        xattr -p com.apple.quarantine "$install_dir/$ASSET" >/dev/null 2>&1; then
        warn "该二进制未签名/未公证，首次运行若被 Gatekeeper 拦下，执行："
        printf '    xattr -d com.apple.quarantine "%s"\n' "$install_dir/$ASSET"
    fi

    case ":$PATH:" in
        *":$install_dir:"*) ;;
        *)
            # 提示里的 $PATH 是要给用户复制粘贴的字面量。
            # shellcheck disable=SC2016
            warn "$install_dir 不在 PATH 里，把下面一行加进 shell 配置："
            # shellcheck disable=SC2016
            printf '    export PATH="%s:$PATH"\n' "$install_dir"
            ;;
    esac

    # —— 3/3 初始化 ————————————————————————————————
    phase 3 "初始化"
    if [ "${DOCK_SKIP_INIT:-}" = "1" ]; then
        warn "已设置 DOCK_SKIP_INIT=1，跳过 dock init"
    elif [ -x "$install_dir/$ASSET" ]; then
        step "运行 dock init（幂等；不覆盖已有 config.toml）"
        # init 配不齐模型时仍 exit 0 并打印下一步；失败才非 0。
        if "$install_dir/$ASSET" init; then
            ok "初始化完成"
        else
            warn "dock init 未成功；可稍后手动运行：$install_dir/$ASSET init"
        fi
    else
        warn "找不到可执行的 dock，请稍后手动运行 dock init"
    fi

    if _is_tty; then
        printf '\n'
        _color "1;32" "完成"
        printf '。有问题先跑：'
        _color "1;37" "dock doctor"
        printf '\n\n'
    else
        printf '\n完成。有问题先跑：dock doctor\n\n'
    fi
}

main
