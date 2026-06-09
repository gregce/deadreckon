#!/bin/sh
set -eu

repo="${DEADRECKON_REPO:-gregce/deadreckon}"
tag="${DEADRECKON_TAG:-latest}"
asset="${DEADRECKON_INSTALLER_ASSET:-deadreckon-installer.sh}"
if [ "$tag" = "latest" ]; then
  base_url="https://github.com/${repo}/releases/latest/download"
else
  base_url="https://github.com/${repo}/releases/download/${tag}"
fi
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
