#!/bin/sh
# deadreckon installer — https://deadreckon.sh
# Resolves the newest release (stable preferred, release candidates included),
# verifies it against SHA256SUMS, and installs the signed binaries.
# Pin a release:  curl -fsSL https://deadreckon.sh/install.sh | DEADRECKON_TAG=vX.Y.Z sh
set -eu

repo="${DEADRECKON_REPO:-gregce/deadreckon}"
tag="${DEADRECKON_TAG:-latest}"
asset="${DEADRECKON_INSTALLER_ASSET:-deadreckon-installer.sh}"
tmp="${TMPDIR:-/tmp}/deadreckon-install.$$"

# ----- presentation (POSIX sh, color only on a TTY, NO_COLOR honored) -------

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-}" != "dumb" ]; then
  esc="$(printf '\033')"
  c_reset="${esc}[0m"
  c_dim="${esc}[2m"
  c_bold="${esc}[1m"
  c_ok="${esc}[32m"
  c_warn="${esc}[33m"
  c_err="${esc}[31m"
  c_step="${esc}[36m"
  c_cmd="${esc}[1;36m"
  g1="${esc}[38;5;24m"
  g2="${esc}[38;5;30m"
  g3="${esc}[38;5;37m"
  g4="${esc}[38;5;44m"
  g5="${esc}[38;5;51m"
else
  c_reset=""; c_dim=""; c_bold=""; c_ok=""; c_warn=""; c_err=""
  c_step=""; c_cmd=""; g1=""; g2=""; g3=""; g4=""; g5=""
fi

step() { printf '%s\n' "${c_step}›${c_reset} ${c_bold}$1${c_reset}  $2"; }
ok()   { printf '%s\n' "${c_ok}✓${c_reset} ${c_bold}$1${c_reset}  $2"; }
warn() { printf '%s\n' "${c_warn}!${c_reset} ${c_bold}$1${c_reset}  $2" >&2; }

banner() {
  printf '\n'
  printf '%s\n' "${g1}  ____                 _ ____           _${c_reset}"
  printf '%s\n' "${g2} |  _ \\  ___  __ _  __| |  _ \\ ___  ___| | _____  _ __${c_reset}"
  printf '%s\n' "${g3} | | | |/ _ \\/ _\` |/ _\` | |_) / _ \\/ __| |/ / _ \\| '_ \\${c_reset}"
  printf '%s\n' "${g4} | |_| |  __/ (_| | (_| |  _ <  __/ (__|   < (_) | | | |${c_reset}"
  printf '%s\n' "${g5} |____/ \\___|\\__,_|\\__,_|_| \\_\\___|\\___|_|\\_\\___/|_| |_|${c_reset}"
  printf '%s\n\n' "${c_dim} run your coding agent unattended, and trust the result${c_reset}"
}

die() {
  printf '%s\n' "${c_err}✗${c_reset} ${c_bold}error${c_reset}     $*" >&2
  printf '%s\n' "${c_dim}  try: rerun, or pin a release with DEADRECKON_TAG=vX.Y.Z${c_reset}" >&2
  exit 1
}

platform_label() {
  os="$(uname -s 2>/dev/null || echo unknown)"
  arch="$(uname -m 2>/dev/null || echo unknown)"
  case "$os" in
    Darwin) os="macOS" ;;
    Linux) os="Linux" ;;
  esac
  case "$arch" in
    arm64 | aarch64) arch="arm64 (Apple Silicon)" ;;
    x86_64) arch="x86_64" ;;
  esac
  case "$os" in
    macOS) [ "$arch" = "x86_64" ] || arch="arm64 (Apple Silicon)" ;;
  esac
  printf '%s %s' "$os" "$arch"
}

# ----- download helpers ------------------------------------------------------

download() {
  url="$1"
  out="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" -o "$out"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$out" "$url"
  else
    die "install curl or wget, then rerun this installer"
  fi
}

fetch_stdout() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$1"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO- "$1"
  else
    die "install curl or wget, then rerun this installer"
  fi
}

# Resolve "latest" through the GitHub API: prefer the latest stable release,
# and fall back to the newest release of any kind — during the
# release-candidate era only prereleases exist, and GitHub's
# releases/latest endpoint excludes those.
resolve_latest_tag() {
  for api in \
    "https://api.github.com/repos/${repo}/releases/latest" \
    "https://api.github.com/repos/${repo}/releases?per_page=1"; do
    resolved=$(fetch_stdout "$api" 2>/dev/null | sed -n 's/.*"tag_name"[^"]*"\([^"]*\)".*/\1/p' | head -n 1)
    if [ -n "$resolved" ]; then
      printf '%s\n' "$resolved"
      return 0
    fi
  done
  return 1
}

# ----- install ---------------------------------------------------------------

banner

resolved_how="pinned by DEADRECKON_TAG"
if [ "$tag" = "latest" ]; then
  tag=$(resolve_latest_tag) \
    || die "could not resolve the latest release tag; pin one with DEADRECKON_TAG=vX.Y.Z"
  resolved_how="newest release"
fi
base_url="https://github.com/${repo}/releases/download/${tag}"

step "release " "${tag} ${c_dim}(${resolved_how} of ${repo})${c_reset}"
step "platform" "$(platform_label)"

cleanup() {
  rm -rf "$tmp"
}

trap cleanup EXIT HUP INT TERM

(umask 077 && mkdir "$tmp") || die "could not create temp dir: $tmp"

step "fetching" "${asset}"
download "${base_url}/${asset}" "${tmp}/${asset}" || die "could not download ${base_url}/${asset}"

if download "${base_url}/SHA256SUMS" "${tmp}/SHA256SUMS"; then
  if grep "  ${asset}\$" "${tmp}/SHA256SUMS" > "${tmp}/SHA256SUMS.${asset}"; then
    if command -v shasum >/dev/null 2>&1; then
      (cd "$tmp" && shasum -a 256 -c "SHA256SUMS.${asset}" >/dev/null) \
        || die "checksum verification failed for ${asset}"
      ok "verified" "${asset} matches SHA256SUMS"
    elif command -v sha256sum >/dev/null 2>&1; then
      (cd "$tmp" && sha256sum -c "SHA256SUMS.${asset}" >/dev/null) \
        || die "checksum verification failed for ${asset}"
      ok "verified" "${asset} matches SHA256SUMS"
    else
      warn "verify  " "shasum/sha256sum not found; skipping checksum verification"
    fi
  else
    warn "verify  " "${asset} was not listed in SHA256SUMS"
  fi
else
  warn "verify  " "could not download SHA256SUMS; skipping checksum verification"
fi

step "running " "release installer"
printf '%s\n' "${c_dim}──────────────────────────────────────────────────────${c_reset}"
sh "${tmp}/${asset}" "$@"
printf '%s\n' "${c_dim}──────────────────────────────────────────────────────${c_reset}"

installed="${DEADRECKON_BIN:-${HOME}/.local/share/deadreckon/bin/deadreckon}"
if [ -x "$installed" ]; then
  bin="$installed"
elif command -v deadreckon >/dev/null 2>&1; then
  bin="$(command -v deadreckon)"
else
  bin="$installed"
fi

install_dir="$(dirname "$bin")"
for helper in \
  dr-gate \
  dr-capture \
  dr-gate-evaluator-aarch64-unknown-linux-musl \
  dr-gate-evaluator-x86_64-unknown-linux-musl
do
  [ -x "${install_dir}/${helper}" ] \
    || die "release installer did not install required helper ${helper} next to deadreckon"
done
ok "helpers " "native gate/capture and both offline evaluator sidecars are installed"

version="$($bin --version 2>/dev/null || printf '%s' "deadreckon ${tag}")"

printf '\n'
printf '%s\n' "${c_ok}╭──────────────────────────────────────────────────────╮${c_reset}"
printf '%s\n' "${c_ok}│${c_reset}  ${c_bold}${version} is installed and verified${c_reset}"
printf '%s\n' "${c_ok}╰──────────────────────────────────────────────────────╯${c_reset}"
printf '\n'
printf '%s\n' "${c_bold}  get going:${c_reset}"
printf '%s\n' "    ${c_cmd}${bin} doctor${c_reset}  ${c_dim}verify setup and provider login state${c_reset}"
printf '%s\n' "    ${c_cmd}${bin} try${c_reset}     ${c_dim}keyless proof run of the whole harness${c_reset}"
printf '%s\n' "    ${c_cmd}${bin} start \"make a small safe change\"${c_reset}"
printf '\n'
printf '%s\n' "${c_dim}  stay current: deadreckon update  ·  docs: https://deadreckon.sh${c_reset}"
