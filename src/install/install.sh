#!/bin/sh
# Installs Hufu on macOS and Linux.
#
#   curl -fsSL https://raw.githubusercontent.com/kverona-ai/hufu/main/src/install/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/kverona-ai/hufu/main/src/install/install.sh | sh -s -- --up
#
# Options (also settable through the environment):
#   --up               run `hufu up` afterwards: configure, register with your agents,
#                      start the daemon                                     (HUFU_UP=1)
#   --version <tag>    install this release instead of the latest one       (HUFU_VERSION)
#   --dir <path>       install the binary here, default ~/.local/bin        (HUFU_INSTALL_DIR)
#   --from-source      build with cargo instead of downloading a binary     (HUFU_FROM_SOURCE=1)
#
# HUFU_DOWNLOAD_BASE=<url> downloads the archive and its checksum from a mirror instead of GitHub.
#
# The script downloads one archive from the project's GitHub releases, checks its SHA-256 sum,
# and copies one file, `hufu`, into the install directory. It changes nothing else.
#
# An installed Hufu updates itself: `hufu update`.

set -eu

REPO="kverona-ai/hufu"
VERSION="${HUFU_VERSION:-latest}"
DIR="${HUFU_INSTALL_DIR:-$HOME/.local/bin}"
FROM_SOURCE="${HUFU_FROM_SOURCE:-0}"
UP="${HUFU_UP:-0}"

say() { printf '%s\n' "$*"; }
fail() {
    printf 'hufu install: %s\n' "$*" >&2
    exit 1
}
have() { command -v "$1" >/dev/null 2>&1; }
usage() {
    cat <<'EOF_USAGE'
Installs Hufu on macOS and Linux.

  --up               run `hufu up` afterwards: configure, register with your agents,
                     start the daemon                                     (HUFU_UP=1)
  --version <tag>    install this release instead of the latest one       (HUFU_VERSION)
  --dir <path>       install the binary here, default ~/.local/bin        (HUFU_INSTALL_DIR)
  --from-source      build with cargo instead of downloading a binary     (HUFU_FROM_SOURCE=1)

HUFU_DOWNLOAD_BASE=<url> downloads the archive and its checksum from a mirror instead of GitHub.
EOF_USAGE
}

while [ $# -gt 0 ]; do
    case "$1" in
        --up) UP=1 ;;
        --from-source) FROM_SOURCE=1 ;;
        --version)
            [ $# -ge 2 ] || fail "--version needs a tag, for example v0.1.0"
            VERSION="$2"
            shift
            ;;
        --dir)
            [ $# -ge 2 ] || fail "--dir needs a path"
            DIR="$2"
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *) fail "unknown option: $1" ;;
    esac
    shift
done

target() {
    os=$(uname -s)
    arch=$(uname -m)
    case "$arch" in
        x86_64 | amd64) arch=x86_64 ;;
        arm64 | aarch64) arch=aarch64 ;;
        *) return 1 ;;
    esac
    case "$os" in
        Linux) say "$arch-unknown-linux-musl" ;;
        Darwin) say "$arch-apple-darwin" ;;
        *) return 1 ;;
    esac
}

fetch() {
    # fetch <url> <file>
    if have curl; then
        curl -fsSL --retry 3 -o "$2" "$1"
    elif have wget; then
        wget -q -O "$2" "$1"
    else
        fail "neither curl nor wget is installed"
    fi
}

sha256() {
    if have sha256sum; then
        sha256sum "$1" | cut -d' ' -f1
    elif have shasum; then
        shasum -a 256 "$1" | cut -d' ' -f1
    else
        return 1
    fi
}

from_source() {
    have cargo || fail "building from source needs Rust: install it from https://rustup.rs and run this again"
    say "Building Hufu from source with cargo (this takes a minute)..."
    root=$(dirname "$DIR")
    if [ "$(basename "$DIR")" = "bin" ]; then
        cargo install --git "https://github.com/$REPO" hufu-cli --locked --force --root "$root"
    else
        cargo install --git "https://github.com/$REPO" hufu-cli --locked --force
        DIR="${CARGO_HOME:-$HOME/.cargo}/bin"
    fi
}

from_release() {
    t=$(target) || return 1
    if [ -n "${HUFU_DOWNLOAD_BASE:-}" ]; then
        base="${HUFU_DOWNLOAD_BASE%/}"
    elif [ "$VERSION" = "latest" ]; then
        base="https://github.com/$REPO/releases/latest/download"
    else
        base="https://github.com/$REPO/releases/download/$VERSION"
    fi
    asset="hufu-$t.tar.gz"
    tmp=$(mktemp -d 2>/dev/null || mktemp -d -t hufu)
    # shellcheck disable=SC2064
    trap "rm -rf '$tmp'" EXIT
    say "Downloading $asset ($VERSION)..."
    fetch "$base/$asset" "$tmp/$asset" 2>/dev/null || return 1
    fetch "$base/$asset.sha256" "$tmp/$asset.sha256" 2>/dev/null ||
        fail "the release has no checksum for $asset; not installing an unverified binary"
    want=$(cut -d' ' -f1 "$tmp/$asset.sha256")
    got=$(sha256 "$tmp/$asset") || fail "no sha256sum or shasum to verify the download with"
    [ "$want" = "$got" ] || fail "checksum mismatch for $asset: expected $want, got $got"
    tar -xzf "$tmp/$asset" -C "$tmp"
    mkdir -p "$DIR"
    # Replace through a rename, so a running hufu keeps its old file.
    cp "$tmp/hufu-$t/hufu" "$DIR/.hufu.new"
    chmod 755 "$DIR/.hufu.new"
    mv -f "$DIR/.hufu.new" "$DIR/hufu"
}

if [ "$FROM_SOURCE" = "1" ]; then
    from_source
elif ! from_release; then
    say "No prebuilt binary could be downloaded for this machine ($(uname -s) $(uname -m), $VERSION)."
    from_source
fi

[ -x "$DIR/hufu" ] || fail "the install did not produce $DIR/hufu"
say "Installed $("$DIR/hufu" --version) to $DIR/hufu"

case ":$PATH:" in
    *":$DIR:"*) ;;
    *)
        say ""
        say "$DIR is not on your PATH. Add this line to your shell profile:"
        say "  export PATH=\"$DIR:\$PATH\""
        ;;
esac

if [ "$UP" = "1" ]; then
    say ""
    # `hufu up` stops with a message of its own when something is missing, for example when no
    # agent is installed (exit code 10). The program itself is installed either way.
    "$DIR/hufu" up || {
        code=$?
        say ""
        say "hufu is installed at $DIR/hufu, but setup is not finished (exit code $code)."
        say "Do what the message above asks, then run: $DIR/hufu up"
        exit "$code"
    }
else
    say ""
    say "Next: run \`hufu up\` to configure Hufu, register it with your agents and start the daemon."
    say "Later, \`hufu update\` moves to the newest release."
fi
