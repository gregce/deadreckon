#!/bin/zsh
# Vendors the deadreckon CLI into the app bundle inputs.
# Mirrors specstory-mac's vendor-cli.sh: per-arch binaries in Resources/bin
# (gitignored) selected at runtime by BinaryLocator, with a COMMITTED
# manifest.json pinning version + commit + sha256, verified at launch.
#
# dr-gate rides along: `start --plan --yes` freezes gate artifacts by
# locating the trusted release helper NEXT TO the running CLI (job.rs
# installed_gate_candidates: the exe dir, then ../libexec[/deadreckon]),
# and validates it as a thin Mach-O of the host arch from the SAME build
# bundle. So each vendor run builds dr-gate in the same cargo invocation
# (same DEADRECKON_BUNDLE_BUILD_ID), signs it the same way, and places:
#   arm64  -> Resources/bin/dr-gate            (first lookup candidate)
#   x86_64 -> Resources/libexec/deadreckon/dr-gate  (later candidate; the
#             CLI rejects a wrong-arch dr-gate and keeps searching)
# Without this the app's execute leg fails at job admission ("not found:
# trusted release helper dr-gate next to the DeadReckon installation").
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
OFFICIAL="${DEADRECKON_VENDOR_OFFICIAL:-0}"

typeset -A TRIPLES
TRIPLES[arm64]=aarch64-apple-darwin
TRIPLES[x86_64]=x86_64-apple-darwin

mkdir -p "$BIN_DIR"

COMMIT="$(git -C "$CLI_SRC" rev-parse HEAD)"
SOURCE_DIRTY=false
if [[ -n "$(git -C "$CLI_SRC" status --porcelain --untracked-files=normal)" ]]; then
  SOURCE_DIRTY=true
fi

if [[ "$OFFICIAL" == 1 ]]; then
  if [[ "$SOURCE_DIRTY" == true ]]; then
    echo "error: official vendoring requires a clean source checkout" >&2
    exit 1
  fi
  if [[ "${#ARCHS[@]}" -ne 2 ]] || [[ " ${ARCHS[*]} " != *" arm64 "* ]] || [[ " ${ARCHS[*]} " != *" x86_64 "* ]]; then
    echo "error: official vendoring requires exactly arm64 and x86_64" >&2
    exit 1
  fi
fi

echo "Vendoring deadreckon from $COMMIT (dirty=$SOURCE_DIRTY) for: ${ARCHS[*]}"

# Signing happens BEFORE the sha256 pin: codesign rewrites the binary, so a
# pin taken over unsigned bytes fails BinaryLocator's integrity check the
# moment a release build signs the bundle (found the hard way — the app
# refused its own vendored CLI). Release signing must never re-sign the CLI;
# release-app.sh signs only the outer bundle. Set DEADRECKON_VENDOR_SIGN=0
# for contributors without the Developer ID certificate (dev-only vendoring;
# do not commit that manifest for a release).
SIGN_IDENTITY="${DEADRECKON_SIGN_IDENTITY:-Developer ID Application: Gregory Ceccarelli (4GRQMF5T5U)}"
VENDOR_SIGN="${DEADRECKON_VENDOR_SIGN:-1}"
if [[ "$OFFICIAL" == 1 && "$VENDOR_SIGN" != 1 ]]; then
  echo "error: official vendoring cannot disable Developer ID signing" >&2
  exit 1
fi

# The per-arch destination for the vendored dr-gate. The lookup name is
# fixed ("dr-gate") and the CLI validates a THIN Mach-O of its own arch, so
# a dual-arch bundle needs one copy per candidate directory: the CLI
# rejects the wrong-arch copy and keeps searching the candidate list.
typeset -A GATE_DESTS
GATE_DESTS[arm64]="$BIN_DIR/dr-gate"
GATE_DESTS[x86_64]="$ROOT/Resources/libexec/deadreckon/dr-gate"

typeset -A SHAS
typeset -A GATE_SHAS
typeset -A SOURCE_NAMES
VERSION=""
for arch in "${ARCHS[@]}"; do
  triple="${TRIPLES[$arch]}"
  # One cargo invocation for both binaries: dr-gate must carry the SAME
  # DEADRECKON_BUNDLE_BUILD_ID as the CLI or job admission refuses it as
  # belonging to a different build bundle.
  (cd "$CLI_SRC" && cargo build --release -p "$CRATE" --bin "$CRATE" --bin dr-gate --target "$triple")
  gate_dest="${GATE_DESTS[$arch]}"
  mkdir -p "${gate_dest:h}"
  cp "$CLI_SRC/target/$triple/release/$CRATE" "$BIN_DIR/deadreckon_darwin_$arch"
  cp "$CLI_SRC/target/$triple/release/dr-gate" "$gate_dest"
  chmod +x "$BIN_DIR/deadreckon_darwin_$arch" "$gate_dest"
  built_version="$("$BIN_DIR/deadreckon_darwin_$arch" --version | awk 'NR == 1 { print $2 }')"
  if [[ -z "$built_version" ]]; then
    echo "error: vendored $arch binary did not report a version" >&2
    exit 1
  fi
  if [[ -n "$VERSION" && "$VERSION" != "$built_version" ]]; then
    echo "error: vendored architectures report different versions ($VERSION vs $built_version)" >&2
    exit 1
  fi
  VERSION="$built_version"
  if [[ "$VENDOR_SIGN" == 1 ]]; then
    for vendored in "$BIN_DIR/deadreckon_darwin_$arch" "$gate_dest"; do
      codesign --force --sign "$SIGN_IDENTITY" --options runtime --timestamp "$vendored"
      codesign --verify --strict "$vendored"
    done
  else
    echo "note: DEADRECKON_VENDOR_SIGN=0 — unsigned vendor; not releasable" >&2
  fi
  SHAS[$arch]="$(shasum -a 256 "$BIN_DIR/deadreckon_darwin_$arch" | cut -d' ' -f1)"
  GATE_SHAS[$arch]="$(shasum -a 256 "$gate_dest" | cut -d' ' -f1)"
  SOURCE_NAMES[$arch]="local-source-$triple"
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
    existing="$(python3 - "$BIN_DIR/manifest.json" "sha256" "$arch" <<'PY'
import json, sys
try:
    print(json.load(open(sys.argv[1])).get(sys.argv[2], {}).get(sys.argv[3], ""))
except Exception:
    pass
PY
)"
    if [[ -n "$existing" ]]; then
      SHAS[$arch]="$existing"
      echo "note: carried over existing sha256 pin for $arch (not rebuilt this run)"
    fi
    existing_gate="$(python3 - "$BIN_DIR/manifest.json" "gateSha256" "$arch" <<'PY'
import json, sys
try:
    print(json.load(open(sys.argv[1])).get(sys.argv[2], {}).get(sys.argv[3], ""))
except Exception:
    pass
PY
)"
    if [[ -n "$existing_gate" ]]; then
      GATE_SHAS[$arch]="$existing_gate"
      echo "note: carried over existing dr-gate sha256 pin for $arch (not rebuilt this run)"
    fi
  fi
done

OUT_ARCHS=()
for arch in "${FULL_ARCHS[@]}"; do
  [[ -n "${SHAS[$arch]:-}" ]] && OUT_ARCHS+=("$arch")
done

GATE_OUT_ARCHS=()
for arch in "${FULL_ARCHS[@]}"; do
  [[ -n "${GATE_SHAS[$arch]:-}" ]] && GATE_OUT_ARCHS+=("$arch")
done

# gateSha256 is a provenance pin for review/diffing: the CLI itself refuses
# any dr-gate whose embedded protocol marker or bundle build id mismatches,
# so launch-time integrity does not depend on this entry (BinaryLocator's
# decoder ignores unknown manifest keys).
{
  echo '{'
  echo '  "schemaVersion": 1,'
  echo "  \"cliVersion\": \"$VERSION\","
  echo "  \"releaseVersion\": \"$VERSION\","
  echo "  \"gitCommit\": \"$COMMIT\","
  echo "  \"sourceDirty\": $SOURCE_DIRTY,"
  if (( PARTIAL )); then
    echo '  "complete": false,'
  else
    echo '  "complete": true,'
  fi
  if [[ "$VENDOR_SIGN" == 1 ]]; then
    echo '  "signed": true,'
  else
    echo '  "signed": false,'
  fi
  echo '  "sha256": {'
  first=true
  for arch in "${OUT_ARCHS[@]}"; do
    $first || echo ','
    first=false
    printf '    "%s": "%s"' "$arch" "${SHAS[$arch]}"
  done
  echo ''
  echo '  },'
  echo '  "gateSha256": {'
  first=true
  for arch in "${GATE_OUT_ARCHS[@]}"; do
    $first || echo ','
    first=false
    printf '    "%s": "%s"' "$arch" "${GATE_SHAS[$arch]}"
  done
  echo ''
  echo '  },'
  echo '  "sourceArchives": {'
  first=true
  for arch in "${ARCHS[@]}"; do
    $first || echo ','
    first=false
    printf '    "%s": {"name": "%s", "sha256": "%s"}' "$arch" "${SOURCE_NAMES[$arch]}" "${SHAS[$arch]}"
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
