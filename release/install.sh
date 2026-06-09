#!/bin/sh
set -eu

repo="${DEADRECKON_REPO:-gregce/deadreckon}"
tag="${DEADRECKON_TAG:-latest}"
asset="${DEADRECKON_INSTALLER_ASSET:-deadreckon-installer.sh}"
tmp="${TMPDIR:-/tmp}/deadreckon-install.$$"

die() {
  printf '%s\n' "deadreckon install: $*" >&2
  exit 1
}

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

if [ "$tag" = "latest" ]; then
  tag=$(resolve_latest_tag) \
    || die "could not resolve the latest release tag; pin one with DEADRECKON_TAG=vX.Y.Z"
fi
base_url="https://github.com/${repo}/releases/download/${tag}"

cleanup() {
  rm -rf "$tmp"
}

trap cleanup EXIT HUP INT TERM

(umask 077 && mkdir "$tmp") || die "could not create temp dir: $tmp"

printf '%s\n' "deadreckon install: ${repo} ${tag}"
download "${base_url}/${asset}" "${tmp}/${asset}" || die "could not download ${base_url}/${asset}"

if download "${base_url}/SHA256SUMS" "${tmp}/SHA256SUMS"; then
  if grep "  ${asset}\$" "${tmp}/SHA256SUMS" > "${tmp}/SHA256SUMS.${asset}"; then
    if command -v shasum >/dev/null 2>&1; then
      (cd "$tmp" && shasum -a 256 -c "SHA256SUMS.${asset}") \
        || die "checksum verification failed for ${asset}"
    elif command -v sha256sum >/dev/null 2>&1; then
      (cd "$tmp" && sha256sum -c "SHA256SUMS.${asset}") \
        || die "checksum verification failed for ${asset}"
    else
      printf '%s\n' "deadreckon install: warning: shasum/sha256sum not found; skipping checksum verification" >&2
    fi
  else
    printf '%s\n' "deadreckon install: warning: ${asset} was not listed in SHA256SUMS" >&2
  fi
else
  printf '%s\n' "deadreckon install: warning: could not download SHA256SUMS; skipping checksum verification" >&2
fi

sh "${tmp}/${asset}" "$@"

installed="${DEADRECKON_BIN:-${HOME}/.local/share/deadreckon/bin/deadreckon}"
if [ -x "$installed" ]; then
  bin="$installed"
elif command -v deadreckon >/dev/null 2>&1; then
  bin="deadreckon"
else
  bin="$installed"
fi

printf '\n%s\n' "deadreckon install: next"
printf '  %s\n' "${bin} doctor"
printf '  %s\n' "${bin} try"
printf '  %s\n' "${bin} start \"make a small safe change\""
