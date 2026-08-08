#!/bin/bash
# Build, verify, sign, notarize, staple, and package the universal macOS app.
#
# Official CI hydrates Resources from the already-finalized arm64 and x86_64
# cargo-dist archives with release/macos-app.mjs. This script never rebuilds or
# re-signs those nested CLI bytes: it verifies them, runs the Swift gates,
# signs only the outer app bundle, notarizes the exact payload, and emits a
# checksummed trust record for the final ZIP.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REPO_ROOT="$(cd "$ROOT/.." && pwd)"
MANIFEST="$ROOT/Resources/bin/manifest.json"
TOOL="$REPO_ROOT/release/macos-app.mjs"
IDENTITY="${DEADRECKON_SIGN_IDENTITY:-Developer ID Application}"
OFFICIAL="${DEADRECKON_RELEASE_MODE:-local}"

manifest_field() {
  node -e 'const fs=require("fs"); const value=JSON.parse(fs.readFileSync(process.argv[1],"utf8"))[process.argv[2]]; if (typeof value !== "string" || value.length === 0) process.exit(1); process.stdout.write(value)' "$MANIFEST" "$1"
}

[ -f "$MANIFEST" ] || {
  echo "error: no vendored manifest; hydrate signed release archives or run scripts/vendor-cli.sh" >&2
  exit 1
}

CLI_VERSION="${DEADRECKON_CLI_VERSION:-$(manifest_field releaseVersion)}"
SOURCE_COMMIT="${DEADRECKON_RELEASE_COMMIT:-$(manifest_field gitCommit)}"
APP_VERSION="${DEADRECKON_APP_VERSION:-${CLI_VERSION%%-rc.*}}"
BUILD_NUMBER="${DEADRECKON_APP_BUILD_NUMBER:-$(git -C "$REPO_ROOT" rev-list --count HEAD)}"
DIST_INPUT="${DEADRECKON_APP_DIST:-$ROOT/build/release}"

case "$OFFICIAL" in
  local|official) ;;
  *)
    echo "error: DEADRECKON_RELEASE_MODE must be local or official" >&2
    exit 1
    ;;
esac

if [[ ! "$CLI_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$ ]]; then
  echo "error: invalid CLI release version $CLI_VERSION" >&2
  exit 1
fi
if [[ ! "$APP_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "error: invalid app marketing version $APP_VERSION" >&2
  exit 1
fi
if [[ ! "$BUILD_NUMBER" =~ ^[1-9][0-9]*$ ]]; then
  echo "error: app build number must be a positive integer" >&2
  exit 1
fi
if [[ "$OFFICIAL" == official ]]; then
  for name in DEADRECKON_CLI_VERSION DEADRECKON_RELEASE_COMMIT DEADRECKON_APP_VERSION DEADRECKON_APP_BUILD_NUMBER DEADRECKON_APP_DIST; do
    if [[ -z "${!name:-}" ]]; then
      echo "error: official app release requires $name" >&2
      exit 1
    fi
  done

  ACTUAL_COMMIT="$(git -C "$REPO_ROOT" rev-parse HEAD)"
  if [[ "$SOURCE_COMMIT" != "$ACTUAL_COMMIT" ]]; then
    echo "error: official app commit $SOURCE_COMMIT does not match checkout $ACTUAL_COMMIT" >&2
    exit 1
  fi
  if ! git -C "$REPO_ROOT" diff --quiet --cached; then
    echo "error: official app release refuses staged source changes" >&2
    exit 1
  fi
  if ! git -C "$REPO_ROOT" diff --quiet -- . \
    ':(exclude)deadreckon-mac/Resources/bin/manifest.json'; then
    echo "error: official app release refuses tracked source changes outside its hydrated manifest" >&2
    exit 1
  fi
  UNTRACKED="$(git -C "$REPO_ROOT" ls-files --others --exclude-standard)"
  if [[ -n "$UNTRACKED" ]]; then
    echo "error: official app release refuses untracked source files:" >&2
    printf '%s\n' "$UNTRACKED" >&2
    exit 1
  fi
fi

node "$TOOL" verify-resources \
  --app-root "$ROOT" \
  --version "$CLI_VERSION" \
  --commit "$SOURCE_COMMIT"

echo "==> Swift package tests"
swift test --package-path "$ROOT/DeadreckonKit"

echo "==> xcodegen + clean unsigned universal Release build"
cd "$ROOT"
xcodegen generate
xcodebuild -project deadreckon.xcodeproj -scheme deadreckon -configuration Release \
  -derivedDataPath build/DerivedData clean build \
  CODE_SIGNING_ALLOWED=NO ONLY_ACTIVE_ARCH=NO ARCHS="arm64 x86_64" \
  MARKETING_VERSION="$APP_VERSION" CURRENT_PROJECT_VERSION="$BUILD_NUMBER"

mkdir -p "$DIST_INPUT"
DIST="$(cd "$DIST_INPUT" && pwd)"
APP="$DIST/deadreckon.app"
ZIP="$DIST/deadreckon-mac.zip"
TRUST="$DIST/trust/macos-universal-apple-darwin-app.json"
rm -rf "$APP"
rm -f "$ZIP" "$TRUST"
ditto build/DerivedData/Build/Products/Release/deadreckon.app "$APP"

# Info.plist has a development default. Bind the distributed app to the
# release lane before any signature is created.
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $APP_VERSION" "$APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $BUILD_NUMBER" "$APP/Contents/Info.plist"

node "$TOOL" verify-bundle \
  --app "$APP" \
  --version "$CLI_VERSION" \
  --commit "$SOURCE_COMMIT"
lipo "$APP/Contents/MacOS/deadreckon" -verify_arch arm64 x86_64

echo "==> verify signed nested CLI payload"
for bin in \
  "$APP/Contents/Resources/bin/deadreckon_darwin_arm64" \
  "$APP/Contents/Resources/bin/deadreckon_darwin_x86_64" \
  "$APP/Contents/Resources/bin/dr-gate" \
  "$APP/Contents/Resources/libexec/deadreckon/dr-gate"
do
  [ -f "$bin" ] || {
    echo "error: official app payload is missing $bin" >&2
    exit 1
  }
  codesign --verify --strict --verbose=2 "$bin"
done

echo "==> sign and verify outer app bundle"
codesign --force --sign "$IDENTITY" --options runtime --timestamp \
  --entitlements Sources/Deadreckon.entitlements "$APP"
codesign --verify --strict --deep --verbose=2 "$APP"

echo "==> archive notarization payload"
ditto -c -k --keepParent "$APP" "$ZIP"

if [[ -z "${APPLE_ID:-}" || -z "${APPLE_TEAM_ID:-}" || -z "${APPLE_APP_PWD:-}" ]]; then
  if [[ "$OFFICIAL" == official ]]; then
    echo "error: official app release requires APPLE_ID, APPLE_TEAM_ID, and APPLE_APP_PWD" >&2
    exit 1
  fi
  shasum -a 256 "$ZIP"
  echo "==> local signed build complete; no release trust evidence was emitted because notarization credentials are absent"
  exit 0
fi

echo "==> notarize, staple, and assess"
xcrun notarytool submit "$ZIP" \
  --apple-id "$APPLE_ID" \
  --team-id "$APPLE_TEAM_ID" \
  --password "$APPLE_APP_PWD" \
  --wait --timeout 30m
xcrun stapler staple "$APP"
xcrun stapler validate "$APP"
spctl --assess --type execute --verbose=2 "$APP"

# Repack after stapling. This is the only app archive that enters checksums,
# manifests, attestations, and the GitHub Release.
rm -f "$ZIP"
ditto -c -k --keepParent "$APP" "$ZIP"
mkdir -p "$(dirname "$TRUST")"
node "$TOOL" trust \
  --app "$APP" \
  --archive "$ZIP" \
  --out "$TRUST" \
  --version "$CLI_VERSION" \
  --app-version "$APP_VERSION" \
  --commit "$SOURCE_COMMIT"

shasum -a 256 "$ZIP"
echo "==> notarized, stapled, and trust-bound: $ZIP"
