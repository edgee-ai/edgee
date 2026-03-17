#!/bin/bash
set -u

# A shell script intended to use for installing Edgee for quick usage
# It does platform detection, fetch latest release informations and
# download the corresponding executable binary if available.
#
# Mostly inspired by the Quickwit installer script

# Package metadata

GITHUB_OWNER='edgee-ai'
GITHUB_REPO='edgee'

# Helper utilities

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
EOF
}

err() {
    echo "$_bold$_red$_prompt Error: $*$_normal" >&2
    exit 1
}

has_command() {
    command -v "$1" &>/dev/null
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

# Main utilities

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

download() {
    echo "Downloading: $1"
    curl --proto '=https' --tlsv1.2 --silent --show-error --fail --location "$1" --output "$2"
}

download_latest() {
    local _arch _edgee_version
    _arch="$(get_arch)"

    download "https://github.com/$GITHUB_OWNER/$GITHUB_REPO/releases/latest/download/edgee.$_arch" edgee
    chmod +x edgee
    _edgee_version=$(./edgee --version | cut -d' ' -f2)

    cat <<EOF

${_bold}${_blue}Edgee${_normal} ${_green}$_edgee_version${_normal} binary successfully downloaded as 'edgee' file.

${_underline}Run it:${_normal}

${_gray}$ ./edgee${_normal}

${_underline}Usage:${_normal}

${_gray}$ ./edgee --help${_normal}
EOF
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
    download_latest
}

# Entrypoint
main "$@" || exit 1
