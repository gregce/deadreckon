#!/bin/bash
# Build, sign, and (with credentials) notarize the deadreckon Mac app.
#
# Mirrors release/trust/sign-macos-artifacts.mjs for the app bundle:
# unsigned Release build, then manual nested-first Developer ID signing
# with hardened runtime + timestamp, strict verification, and a
# notarytool submit + staple gated on the same APPLE_ID / APPLE_TEAM_ID /
# APPLE_APP_PWD environment variables the CLI release pipeline uses
# (docs/RELEASE.md). Without credentials the script stops after local
# verification and prints the one command left to run.
#
# Prerequisite: scripts/vendor-cli.sh has pinned the CLI into
# Resources/bin (BinaryLocator fails closed on an unvendored manifest).
set -euo pipefail

cd "$(dirname "$0")/.."

IDENTITY="${DEADRECKON_SIGN_IDENTITY:-Developer ID Application: Gregory Ceccarelli (4GRQMF5T5U)}"
DIST="build/release"
APP="$DIST/deadreckon.app"
ZIP="$DIST/deadreckon-mac.zip"

VENDORED=(Resources/bin/deadreckon_darwin_*)
[ -e "${VENDORED[0]}" ] || {
  echo "error: no vendored CLI under Resources/bin — run scripts/vendor-cli.sh first" >&2
  exit 1
}
# The execute leg needs the trusted release helper next to the CLI: without
# a vendored dr-gate, `start --plan --yes` fails at job admission in the
# shipped app. vendor-cli.sh places it (bin/dr-gate for arm64,
# libexec/deadreckon/dr-gate for x86_64).
GATES=()
[ -e Resources/bin/dr-gate ] && GATES+=(Resources/bin/dr-gate)
[ -e Resources/libexec/deadreckon/dr-gate ] && GATES+=(Resources/libexec/deadreckon/dr-gate)
[ "${#GATES[@]}" -gt 0 ] || {
  echo "error: no vendored dr-gate next to the CLI — rerun scripts/vendor-cli.sh" >&2
  exit 1
}

echo "==> xcodegen + unsigned Release build"
xcodegen generate
xcodebuild -project deadreckon.xcodeproj -scheme deadreckon -configuration Release \
  -derivedDataPath build/DerivedData build \
  CODE_SIGNING_ALLOWED=NO | grep -E 'BUILD (SUCCEEDED|FAILED)'

rm -rf "$DIST"
mkdir -p "$DIST"
ditto build/DerivedData/Build/Products/Release/deadreckon.app "$APP"

echo "==> signing the app bundle (the vendored CLI and dr-gate were signed at"
echo "    vendor time; re-signing them here would break the manifest's pins)"
for bin in "$APP"/Contents/Resources/bin/deadreckon_darwin_* \
           "$APP"/Contents/Resources/bin/dr-gate \
           "$APP"/Contents/Resources/libexec/deadreckon/dr-gate; do
  [ -e "$bin" ] || continue
  codesign --verify --strict "$bin" || {
    echo "error: vendored binary $bin is not validly signed — rerun scripts/vendor-cli.sh" >&2
    exit 1
  }
done
codesign --force --sign "$IDENTITY" --options runtime --timestamp \
  --entitlements Sources/Deadreckon.entitlements "$APP"

echo "==> verifying"
codesign --verify --strict --deep --verbose=2 "$APP"

echo "==> archiving for notarization"
ditto -c -k --keepParent "$APP" "$ZIP"
shasum -a 256 "$ZIP"

if [ -z "${APPLE_ID:-}" ] || [ -z "${APPLE_TEAM_ID:-}" ] || [ -z "${APPLE_APP_PWD:-}" ]; then
  cat <<EOF
==> signed and verified locally; notarization needs credentials.
    Export APPLE_ID, APPLE_TEAM_ID, APPLE_APP_PWD (docs/RELEASE.md) and run:
      $0
    or notarize the existing archive directly:
      xcrun notarytool submit "$ZIP" --apple-id "\$APPLE_ID" \\
        --team-id "\$APPLE_TEAM_ID" --password "\$APPLE_APP_PWD" --wait
      xcrun stapler staple "$APP"
EOF
  exit 0
fi

echo "==> notarizing"
xcrun notarytool submit "$ZIP" \
  --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PWD" \
  --wait --timeout 30m
xcrun stapler staple "$APP"
# Re-zip so the distributed archive carries the staple.
ditto -c -k --keepParent "$APP" "$ZIP"
shasum -a 256 "$ZIP"
echo "==> notarized and stapled: $APP"
