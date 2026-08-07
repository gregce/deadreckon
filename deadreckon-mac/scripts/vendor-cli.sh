#!/bin/zsh
# Vendors the deadreckon CLI into the app bundle inputs.
# Mirrors specstory-mac's vendor-cli.sh: per-arch binaries in Resources/bin
# (gitignored) selected at runtime by BinaryLocator, with a COMMITTED
# manifest.json pinning version + commit + sha256, verified at launch.
#
# Do not run this while another workflow holds the cargo build lock.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CLI_SRC="${DEADRECKON_CLI_SRC:-$ROOT/..}"
BIN_DIR="$ROOT/Resources/bin"
CRATE="deadreckon"

if [[ ! -f "$CLI_SRC/crates/deadreckon/Cargo.toml" ]]; then
  echo "error: deadreckon source not found at $CLI_SRC (set DEADRECKON_CLI_SRC to the repo root)" >&2
  exit 1
fi

# Archs to vendor. Both by default; set DEADRECKON_VENDOR_ARCHS="arm64" for a
# local-only build when the x86_64 Rust target is not installed
# (rustup target add x86_64-apple-darwin).
ARCHS=(${=DEADRECKON_VENDOR_ARCHS:-arm64 x86_64})

typeset -A TRIPLES
TRIPLES[arm64]=aarch64-apple-darwin
TRIPLES[x86_64]=x86_64-apple-darwin

mkdir -p "$BIN_DIR"

COMMIT="$(git -C "$CLI_SRC" rev-parse HEAD)"
VERSION="$(git -C "$CLI_SRC" describe --tags --always 2>/dev/null || echo dev)"

echo "Vendoring deadreckon $VERSION ($COMMIT) from $CLI_SRC for: ${ARCHS[*]}"

typeset -A SHAS
for arch in "${ARCHS[@]}"; do
  triple="${TRIPLES[$arch]}"
  (cd "$CLI_SRC" && cargo build --release -p "$CRATE" --bin "$CRATE" --target "$triple")
  cp "$CLI_SRC/target/$triple/release/$CRATE" "$BIN_DIR/deadreckon_darwin_$arch"
  chmod +x "$BIN_DIR/deadreckon_darwin_$arch"
  SHAS[$arch]="$(shasum -a 256 "$BIN_DIR/deadreckon_darwin_$arch" | cut -d' ' -f1)"
done

# A partial-arch run (DEADRECKON_VENDOR_ARCHS) must not silently drop the
# other arch's committed sha256 pin: carry over any existing manifest entry
# for archs not rebuilt this run, and warn loudly either way, because a
# manifest whose entries come from different builds cannot be committed.
FULL_ARCHS=(arm64 x86_64)
PARTIAL=0
for arch in "${FULL_ARCHS[@]}"; do
  [[ -n "${SHAS[$arch]:-}" ]] && continue
  PARTIAL=1
  if [[ -f "$BIN_DIR/manifest.json" ]]; then
    existing="$(python3 - "$BIN_DIR/manifest.json" "$arch" <<'PY'
import json, sys
try:
    print(json.load(open(sys.argv[1])).get("sha256", {}).get(sys.argv[2], ""))
except Exception:
    pass
PY
)"
    if [[ -n "$existing" ]]; then
      SHAS[$arch]="$existing"
      echo "note: carried over existing sha256 pin for $arch (not rebuilt this run)"
    fi
  fi
done

OUT_ARCHS=()
for arch in "${FULL_ARCHS[@]}"; do
  [[ -n "${SHAS[$arch]:-}" ]] && OUT_ARCHS+=("$arch")
done

{
  echo '{'
  echo "  \"cliVersion\": \"$VERSION\","
  echo "  \"gitCommit\": \"$COMMIT\","
  echo '  "sha256": {'
  first=true
  for arch in "${OUT_ARCHS[@]}"; do
    $first || echo ','
    first=false
    printf '    "%s": "%s"' "$arch" "${SHAS[$arch]}"
  done
  echo ''
  echo '  }'
  echo '}'
} > "$BIN_DIR/manifest.json"

if (( PARTIAL )); then
  echo "" >&2
  echo "WARNING: partial-arch vendor run (built: ${ARCHS[*]}; manifest covers: ${OUT_ARCHS[*]})." >&2
  echo "WARNING: DO NOT COMMIT Resources/bin/manifest.json from this run:" >&2
  echo "WARNING: carried-over pins reference a binary from a different build," >&2
  echo "WARNING: and a missing arch entry fails closed at launch on that arch." >&2
  echo "WARNING: rerun with DEADRECKON_VENDOR_ARCHS unset before committing." >&2
fi

# Smoke test: the vendored binary for this machine must speak JSON against a
# scratch DEADRECKON_HOME (never the operator's real home).
SMOKE_DIR="$(mktemp -d)"
trap 'rm -rf "$SMOKE_DIR"' EXIT
HOST_ARCH="$(uname -m)"
[[ "$HOST_ARCH" == arm64* ]] && HOST_ARCH=arm64
ARCH_BIN="$BIN_DIR/deadreckon_darwin_$HOST_ARCH"
if [[ -x "$ARCH_BIN" ]]; then
  if ! DEADRECKON_HOME="$SMOKE_DIR" "$ARCH_BIN" list --json >/dev/null 2>&1; then
    echo "error: vendored binary failed the list --json smoke test" >&2
    exit 1
  fi
else
  echo "warning: no vendored binary for host arch $HOST_ARCH; smoke test skipped" >&2
fi

echo "Vendored OK:"
cat "$BIN_DIR/manifest.json"
