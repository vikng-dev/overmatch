#!/usr/bin/env bash
# Build, sign, notarize and package the macOS app as a notarized .dmg.
#
# Produces: dist/overmatch-v<VERSION>-universal-apple-darwin.dmg (+ .sha256)
# The .app inside is also the content used for the future Steam macOS depot.
#
# Credentials (two ways, so this works locally AND in CI):
#   - Signing identity: $SIGN_IDENTITY, else auto-detected Developer ID Application.
#   - Notarization: either $NOTARY_KEY (+$NOTARY_KEY_ID +$NOTARY_ISSUER) pointing at
#     an App Store Connect .p8, or $NOTARYTOOL_PROFILE (a stored keychain profile).
#
# Local example:
#   NOTARYTOOL_PROFILE=autoquit-notary ./scripts/package-macos.sh
#
# Modes (so CI can split Apple signing secrets away from the untrusted `cargo build`):
#   --build-only   steps 1-2: universal build + assemble dist/Overmatch.app (unsigned, NO secrets).
#   --sign-only    steps 3-5: sign the existing dist/Overmatch.app -> DMG -> notarize -> staple.
#   (no arg)       full build + sign, unchanged behavior for local use.
set -euo pipefail

MODE="full"
case "${1:-}" in
  --build-only) MODE="build" ;;
  --sign-only)  MODE="sign" ;;
  "")           MODE="full" ;;
  *) echo "Usage: $0 [--build-only|--sign-only]" >&2; exit 64 ;;
esac

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

APP_NAME="Overmatch"
BIN_NAME="overmatch"
BUNDLE_ID="com.vikngdev.overmatch"
VERSION="${VERSION:-$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')}"
ENTITLEMENTS="$ROOT/build/macos/entitlements.plist"
ICON="$ROOT/build/icons/icon.icns"

DIST="$ROOT/dist"
APP="$DIST/$APP_NAME.app"
DMG="$DIST/$BIN_NAME-v$VERSION-universal-apple-darwin.dmg"

# Signing identity is only needed for the sign phase; the build-only path must not require it
# (that is the whole point — no Apple credentials materialized before the untrusted build runs).
if [[ "$MODE" != "build" ]]; then
  IDENTITY="${SIGN_IDENTITY:-$(security find-identity -v -p codesigning | awk -F'"' '/Developer ID Application/{print $2; exit}')}"
  if [[ -z "$IDENTITY" ]]; then
    echo "No Developer ID Application signing identity found (set SIGN_IDENTITY)." >&2
    exit 65
  fi
  echo ">> Version $VERSION, signing as: $IDENTITY"
else
  echo ">> Version $VERSION (build-only, unsigned)"
fi

if [[ "$MODE" != "sign" ]]; then

# ---------- 1. Build universal binary ----------
# `overmatch` is the PVP client. Default features now include `net`, so a plain `--release` build
# is the networked client; if OVERMATCH_DEFAULT_SERVER is set in the environment (the Release
# workflow exports the droplet IP), it is baked in as the client's default server via option_env!.
echo ">> Building universal binary (aarch64 + x86_64)..."
rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
cargo build --release --target aarch64-apple-darwin
cargo build --release --target x86_64-apple-darwin
mkdir -p "$DIST"
lipo -create -output "$DIST/$BIN_NAME" \
  "target/aarch64-apple-darwin/release/$BIN_NAME" \
  "target/x86_64-apple-darwin/release/$BIN_NAME"

# ---------- 2. Assemble the .app ----------
echo ">> Assembling $APP_NAME.app..."
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
mv "$DIST/$BIN_NAME" "$APP/Contents/MacOS/$BIN_NAME"
chmod +x "$APP/Contents/MacOS/$BIN_NAME"
cp "$ICON" "$APP/Contents/Resources/$BIN_NAME.icns"
# Runtime assets in Resources/assets (matches asset_root() in net::client). Prune sources.
cp -R assets "$APP/Contents/Resources/assets"
find "$APP/Contents/Resources/assets" -type f \
  \( -name '*.blend' -o -name '*.blend1' -o -name '.DS_Store' \) -delete

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>$APP_NAME</string>
    <key>CFBundleDisplayName</key><string>$APP_NAME</string>
    <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
    <key>CFBundleExecutable</key><string>$BIN_NAME</string>
    <key>CFBundleIconFile</key><string>$BIN_NAME</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>LSMinimumSystemVersion</key><string>10.15</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

fi  # end build phase

# The build phase is done. In --build-only we stop here with an unsigned dist/Overmatch.app.
if [[ "$MODE" == "build" ]]; then
  echo ">> Done (build-only): $APP"
  exit 0
fi

# --sign-only picks up the .app produced by the build job (downloaded as an artifact).
if [[ ! -d "$APP" ]]; then
  echo "Expected $APP to exist before signing (run --build-only first, or without a mode)." >&2
  exit 66
fi

# ---------- 3. Sign (inside-out: executable, then bundle) ----------
echo ">> Signing with hardened runtime + entitlements..."
codesign --force --options runtime --timestamp \
  --entitlements "$ENTITLEMENTS" --sign "$IDENTITY" "$APP/Contents/MacOS/$BIN_NAME"
codesign --force --options runtime --timestamp \
  --entitlements "$ENTITLEMENTS" --sign "$IDENTITY" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

# ---------- 4. Build DMG ----------
echo ">> Building DMG..."
rm -f "$DMG" "$DMG.sha256"
DMG_ROOT="$(mktemp -d)"
ditto "$APP" "$DMG_ROOT/$APP_NAME.app"
ln -s /Applications "$DMG_ROOT/Applications"
hdiutil create -volname "$APP_NAME" -srcfolder "$DMG_ROOT" -ov -format UDZO "$DMG"
rm -rf "$DMG_ROOT"
codesign --force --timestamp --sign "$IDENTITY" "$DMG"

# ---------- 5. Notarize + staple ----------
echo ">> Notarizing (this can take a few minutes)..."
if [[ -n "${NOTARY_KEY:-}" ]]; then
  xcrun notarytool submit "$DMG" \
    --key "$NOTARY_KEY" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER" --wait
elif [[ -n "${NOTARYTOOL_PROFILE:-}" ]]; then
  xcrun notarytool submit "$DMG" --keychain-profile "$NOTARYTOOL_PROFILE" --wait
else
  echo "No notary credentials: set NOTARY_KEY+NOTARY_KEY_ID+NOTARY_ISSUER or NOTARYTOOL_PROFILE." >&2
  exit 65
fi
xcrun stapler staple "$DMG"
xcrun stapler validate "$DMG"
spctl -a -vvv -t open --context context:primary-signature "$DMG" || true

shasum -a 256 "$DMG" > "$DMG.sha256"
echo ">> Done:"
ls -lh "$DMG" "$DMG.sha256"
