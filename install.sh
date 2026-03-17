#!/bin/sh
set -eu

GITHUB_OWNER='edgee-ai'
GITHUB_REPO='edgee'

_normal=$(printf '\033[0m')
_bold=$(printf '\033[0;1m')
_dim=$(printf '\033[2m')
_underline=$(printf '\033[0;4m')
_purple=$(printf '\033[0;35m')
_blue=$(printf '\033[1;34m')
_cyan=$(printf '\033[0;36m')
_green=$(printf '\033[0;32m')
_yellow=$(printf '\033[0;33m')
_red=$(printf '\033[1;31m')
_gray=$(printf '\033[0;37m')

_tick="${_green}✓${_normal}"
_cross="${_red}✗${_normal}"
_arrow="${_cyan}→${_normal}"

_header() {
    cat 1>&2 <<EOF

  ${_bold}     ◢████◤${_normal}
  ${_bold} ${_normal}
  ${_bold}◢██████◤${_normal}
  ${_bold} ${_normal}
  ${_bold}◢████████◤${_normal}
  ${_bold} ${_normal}

  ${_dim}Token compression gateway for Claude Code, Codex & Opencode${_normal}
  ${_dim}https://www.edgee.ai${_normal}

EOF
}

_usage() {
    cat 1>&2 <<EOF
${_bold}edgee-installer${_normal}
The installer for Edgee (https://www.edgee.ai)

${_bold}USAGE${_normal}:
    edgee-installer [-h/--help]

${_bold}FLAGS${_normal}:
    -h, --help      Print help information

${_bold}ENVIRONMENT${_normal}:
    INSTALL_DIR     Override the install directory (default: /usr/local/bin or ~/.local/bin)
EOF
}

err() {
    printf "\n  %s ${_bold}Error:${_normal} %s\n\n" "$_cross" "$*" >&2
    exit 1
}

step() {
    printf "  %s %s\n" "$_arrow" "$*" >&2
}

ok() {
    printf "  %s %s\n" "$_tick" "$*" >&2
}

has_command() {
    command -v "$1" >/dev/null 2>&1
}

check_command() {
    if ! has_command "$1"; then
        err "Required command \`$1\` was not found."
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
    curl --proto '=https' --tlsv1.2 --silent --show-error --fail --location "$1" --output "$2"
}

_tmp_dir=""

download_and_install() {
    local _arch _install_dir _edgee_version _expected _actual _os_label _cpu_label
    _arch="$(get_arch)"
    _install_dir="$(get_install_dir)"
    _tmp_dir="$(mktemp -d)"
    trap 'rm -rf "$_tmp_dir"' EXIT

    # Human-readable platform label
    case "$_arch" in
        *apple-darwin*)  _os_label="macOS" ;;
        *linux*)         _os_label="Linux" ;;
        *)               _os_label="$_arch" ;;
    esac
    case "$_arch" in
        aarch64*) _cpu_label="arm64" ;;
        x86_64*)  _cpu_label="x86_64" ;;
        *)        _cpu_label="$_arch" ;;
    esac

    printf "\n  ${_bold}Platform${_normal}  %s (%s)\n" "$_os_label" "$_cpu_label" >&2
    printf "  ${_bold}Directory${_normal} %s\n\n" "$_install_dir" >&2

    local _base_url="https://github.com/$GITHUB_OWNER/$GITHUB_REPO/releases/latest/download"

    step "Downloading binary..."
    download "$_base_url/edgee.$_arch" "$_tmp_dir/edgee"
    ok "Binary downloaded"

    step "Downloading checksum..."
    download "$_base_url/edgee.$_arch.sha256" "$_tmp_dir/edgee.sha256"
    ok "Checksum downloaded"

    step "Verifying integrity..."
    _expected="$(cat "$_tmp_dir/edgee.sha256")"
    _actual="$(_checksum "$_tmp_dir/edgee")"
    if [ "$_expected" != "$_actual" ]; then
        err "Checksum mismatch!\n  Expected: $_expected\n  Got:      $_actual"
    fi
    ok "Checksum verified"

    step "Installing to ${_bold}$_install_dir${_normal}..."
    chmod +x "$_tmp_dir/edgee"
    mkdir -p "$_install_dir"
    mv "$_tmp_dir/edgee" "$_install_dir/edgee"

    _edgee_version=$("$_install_dir/edgee" --version | cut -d' ' -f2)
    ok "Installed ${_bold}edgee v$_edgee_version${_normal}"

    _box_width=47
    _success_text="  Edgee v${_edgee_version} installed successfully!"
    _success_pad=$((_box_width - ${#_success_text}))
    printf '  ╔═══════════════════════════════════════════════╗\n' 1>&2
    printf '  ║  %s%*s║\n' "${_bold}${_green}Edgee v${_edgee_version} installed successfully!${_normal}" "$_success_pad" "" 1>&2
    printf '  ╚═══════════════════════════════════════════════╝\n' 1>&2

    cat 1>&2 <<EOF

  ${_bold}Get started:${_normal}

    ${_cyan}edgee auth login${_normal}   ${_dim}# authenticate with your Edgee account${_normal}
    ${_cyan}edgee launch claude${_normal} ${_dim}# launch Claude Code with token compression${_normal}
    ${_cyan}edgee --help${_normal}        ${_dim}# show all available commands${_normal}

EOF

    # Warn if the install directory is not in PATH
    case ":${PATH}:" in
        *":$_install_dir:"*) ;;
        *)
            cat 1>&2 <<EOF
  ${_bold}${_yellow}⚠ Warning:${_normal} $_install_dir is not in your PATH.
  Add this line to your shell profile (${_dim}~/.zshrc${_normal}, ${_dim}~/.bashrc${_normal}, etc.):

    ${_gray}export PATH="\$PATH:$_install_dir"${_normal}

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
