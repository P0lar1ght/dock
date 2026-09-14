#!/bin/sh
# Dock 一键安装：从 GitHub Release 取预编译二进制，校验 sha256 后装进用户目录。
#
#   curl -fsSL https://raw.githubusercontent.com/P0lar1ght/dock/main/install.sh | sh
#
# 环境变量：
#   DOCK_VERSION=v0.1.0     指定版本（默认 latest）
#   DOCK_INSTALL_DIR=<dir>  安装目录（默认 $HOME/.local/bin）
#   DOCK_BASE_URL=<url>     覆盖下载基址（仅调试安装脚本时用）
#
# 支持：macOS aarch64 / x86_64，Linux aarch64 / x86_64。
# Windows 没有预编译产物（未做适配验证），脚本会明确拒绝而不是装个跑不起来的壳。
set -eu

REPO="P0lar1ght/dock"
ASSET="dock"

die() {
    printf '安装失败：%s\n' "$1" >&2
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
    trap 'rm -rf "$tmp"' EXIT INT TERM

    printf '下载 %s\n' "$file"
    download "$base/$file" "$tmp/$file"
    download "$base/$file.sha256" "$tmp/$file.sha256"

    expected=$(awk '{print $1}' "$tmp/$file.sha256")
    [ -n "$expected" ] || die "sha256 文件为空，拒绝安装未校验的产物"
    actual=$(sha256_of "$tmp/$file")
    # 花括号不能省：$expected 后面紧跟全角标点时，bash 会把标点字节吃进变量名。
    [ "$expected" = "$actual" ] || die "校验失败：期望 ${expected}，实际 ${actual}"

    tar -xzf "$tmp/$file" -C "$tmp" || die "解包失败"
    [ -f "$tmp/$ASSET" ] || die "产物里没有 $ASSET 二进制"

    mkdir -p "$install_dir" || die "无法创建 $install_dir"
    install -m 0755 "$tmp/$ASSET" "$install_dir/$ASSET" 2>/dev/null ||
        { cp "$tmp/$ASSET" "$install_dir/$ASSET" && chmod 0755 "$install_dir/$ASSET"; } ||
        die "无法写入 $install_dir/$ASSET"

    installed=$("$install_dir/$ASSET" --version 2>/dev/null || echo "$ASSET")
    printf '已安装 %s → %s\n' "$installed" "$install_dir/$ASSET"

    # 产物没有代码签名/公证：只提示，不替用户改 Gatekeeper 的标记。
    if [ "$(uname -s)" = "Darwin" ] &&
        xattr -p com.apple.quarantine "$install_dir/$ASSET" >/dev/null 2>&1; then
        printf '提示：该二进制未签名/未公证，首次运行若被 Gatekeeper 拦下，执行：\n  xattr -d com.apple.quarantine "%s"\n' "$install_dir/$ASSET"
    fi

    case ":$PATH:" in
        *":$install_dir:"*) ;;
        *)
            # 提示里的 $PATH 是要给用户复制粘贴的字面量。
            # shellcheck disable=SC2016
            printf '提示：%s 不在 PATH 里，把下面一行加进 shell 配置：\n  export PATH="%s:$PATH"\n' "$install_dir" "$install_dir"
            ;;
    esac

    printf '首次使用：在 ~/.dock/config.toml 里配好模型端点（样例见 https://github.com/%s/blob/main/config.toml.example ），然后运行 dock\n' "$REPO"
}

main
