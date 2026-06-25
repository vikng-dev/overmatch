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
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

APP_NAME="Overmatch"
BIN_NAME="overmatch"
BUNDLE_ID="com.vikngdev.overmatch"
VERSION="${VERSION:-$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')}"
ENTITLEMENTS="$ROOT/build/macos/entitlements.plist"
ICON="$ROOT/build/icons/icon.icns"

IDENTITY="${SIGN_IDENTITY:-$(security find-identity -v -p codesigning | awk -F'"' '/Developer ID Application/{print $2; exit}')}"
if [[ -z "$IDENTITY" ]]; then
  echo "No Developer ID Application signing identity found (set SIGN_IDENTITY)." >&2
  exit 65
fi

DIST="$ROOT/dist"
APP="$DIST/$APP_NAME.app"
DMG="$DIST/$BIN_NAME-v$VERSION-universal-apple-darwin.dmg"

echo ">> Version $VERSION, signing as: $IDENTITY"

# ---------- 1. Build universal binary ----------
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
# Runtime assets in Resources/assets (matches asset_root() in main.rs). Prune sources.
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
