#!/bin/sh
set -eu

GITHUB_OWNER='edgee-ai'
GITHUB_REPO='edgee'

_normal=$(printf '\033[0m')
_bold=$(printf '\033[0;1m')
_underline=$(printf '\033[0;4m')
_purple=$(printf '\033[0;35m')
_blue=$(printf '\033[1;34m')
_green=$(printf '\033[0;32m')
_red=$(printf '\033[1;31m')
_gray=$(printf '\033[0;37m')

_divider='--------------------------------------------------------------------------------'
_prompt='>>>'
_indent='    '

_header() {
    cat 1>&2 <<EOF
                                    ${_bold}${_blue}E D G E E${_normal}
                                    ${_purple}Installer${_normal}

$_divider
${_bold}Website${_normal}:        https://www.edgee.ai
${_bold}Documentation${_normal}:  https://www.edgee.ai/docs/introduction
$_divider

EOF
}

_usage() {
    cat 1>&2 <<EOF
edgee-installer
The installer for Edgee (https://www.edgee.ai)

${_bold}USAGE${_normal}:
    edgee-installer [-h/--help]

${_bold}FLAGS${_normal}:
    -h, --help      Print help informations

${_bold}ENVIRONMENT${_normal}:
    INSTALL_DIR     Override the install directory (default: /usr/local/bin or ~/.local/bin)
EOF
}

err() {
    echo "$_bold$_red$_prompt Error: $*$_normal" >&2
    exit 1
}

has_command() {
    command -v "$1" >/dev/null 2>&1
}

check_command() {
    if ! has_command "$1"; then
        err "This install script requires \`$1\` and was not found."
    fi
}

check_commands() {
    for cmd in "$@"; do
        check_command "$cmd"
    done
}

check_dependencies() {
    check_commands curl chmod
}

get_arch() {
    local _ostype _cputype
    _cputype="$(uname -m)"
    _ostype="$(uname -s)"

    case "$_cputype" in
        x86_64 | x86-64 | x64 | amd64)
            _cputype=x86_64
            ;;
        aarch64 | arm64)
            _cputype=aarch64
            ;;
        *)
            err "Unrecognized CPU type: $_cputype"
            ;;
    esac

    case "$_ostype" in
        Linux)
            # musl build: statically linked, no glibc version dependency
            _ostype="unknown-linux-musl"
            ;;
        Darwin)
            _ostype="apple-darwin"
            ;;
        *)
            err "Unrecognized OS type: $_ostype"
            ;;
    esac

    echo "$_cputype-$_ostype"
}

_checksum() {
    # sha256sum on Linux, shasum on macOS
    if has_command sha256sum; then
        sha256sum "$1" | cut -d' ' -f1
    elif has_command shasum; then
        shasum -a 256 "$1" | cut -d' ' -f1
    else
        err "sha256sum or shasum not found — cannot verify download integrity"
    fi
}

get_install_dir() {
    if [ -n "${INSTALL_DIR:-}" ]; then
        echo "$INSTALL_DIR"
    elif [ -w "/usr/local/bin" ]; then
        echo "/usr/local/bin"
    else
        echo "$HOME/.local/bin"
    fi
}

download() {
    echo "$_indent Downloading: $1" >&2
    curl --proto '=https' --tlsv1.2 --silent --show-error --fail --location "$1" --output "$2"
}

download_and_install() {
    local _arch _install_dir _tmp_dir _edgee_version _expected _actual
    _arch="$(get_arch)"
    _install_dir="$(get_install_dir)"
    _tmp_dir="$(mktemp -d)"
    trap 'rm -rf "$_tmp_dir"' EXIT

    local _base_url="https://github.com/$GITHUB_OWNER/$GITHUB_REPO/releases/latest/download"
    download "$_base_url/edgee.$_arch"        "$_tmp_dir/edgee"
    download "$_base_url/edgee.$_arch.sha256" "$_tmp_dir/edgee.sha256"

    echo "$_indent Verifying checksum..." >&2
    _expected="$(cat "$_tmp_dir/edgee.sha256")"
    _actual="$(_checksum "$_tmp_dir/edgee")"
    if [ "$_expected" != "$_actual" ]; then
        err "Checksum mismatch!\n  Expected: $_expected\n  Got:      $_actual"
    fi

    chmod +x "$_tmp_dir/edgee"
    mkdir -p "$_install_dir"
    mv "$_tmp_dir/edgee" "$_install_dir/edgee"

    _edgee_version=$("$_install_dir/edgee" --version | cut -d' ' -f2)

    cat <<EOF

${_bold}${_blue}Edgee${_normal} ${_green}$_edgee_version${_normal} installed to ${_bold}$_install_dir/edgee${_normal}.

${_underline}Run it:${_normal}

${_gray}\$ edgee --help${_normal}
EOF

    # Warn if the install directory is not in PATH
    case ":${PATH}:" in
        *":$_install_dir:"*) ;;
        *)
            cat 1>&2 <<EOF
${_bold}${_red}Warning:${_normal} $_install_dir is not in your PATH.
Add this to your shell profile:

${_gray}  export PATH="\$PATH:$_install_dir"${_normal}

EOF
            ;;
    esac
}

main() {
    case "${1:-}" in
        -h|--help)
            _usage
            exit 0
            ;;
    esac

    _header

    check_dependencies
    download_and_install
}

main "$@"
