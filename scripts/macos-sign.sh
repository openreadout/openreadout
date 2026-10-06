#!/usr/bin/env bash
# Sign a macOS binary with a Developer ID Application certificate and notarize it with Apple.
#
#   scripts/macos-sign.sh [--require] BINARY
#
# The release workflow runs this on the macOS build runners, and you can run it on a Mac to test
# the certificate and the API key before a release. It reads these environment variables (the
# names of the repository secrets; see docs/release-process.md, "Signing and notarization"):
#
#   APPLE_DEVELOPER_ID_P12_BASE64    the certificate and its private key, a base64-encoded .p12
#   APPLE_DEVELOPER_ID_P12_PASSWORD  the .p12 password
#   APPLE_API_KEY_ID                 App Store Connect API key id (10 characters)
#   APPLE_API_ISSUER_ID              the issuer id (a UUID) shown above the list of keys
#   APPLE_API_KEY_P8_BASE64          the downloaded AuthKey_<id>.p8, base64-encoded
#
# Instead of the two .p12 variables you can set APPLE_SIGNING_IDENTITY to the name or SHA-1 of an
# identity already in your login keychain, e.g. "Developer ID Application: Jane Doe (TEAMID1234)".
# APPLE_TEAM_ID, if set, must match the team of the certificate that signed the binary.
#
# With none of these set, the script prints a notice and leaves the binary alone, so builds
# without the secrets still work. --require turns that into an error. With a certificate but no
# API key it signs without notarizing, which is only useful for a local test. In CI (CI=true), a
# partial set of secrets is an error.
#
# The binary is signed in place with the hardened runtime and a secure timestamp, then zipped and
# sent to Apple's notary service. Apple can't staple a ticket to a bare binary, so Gatekeeper
# looks the ticket up online the first time someone runs a downloaded copy.
set -euo pipefail

require=0
if [ "${1:-}" = "--require" ]; then require=1; shift; fi
bin="${1:?usage: macos-sign.sh [--require] BINARY}"
[ -f "$bin" ] || { echo "error: $bin does not exist" >&2; exit 1; }
[ "$(uname -s)" = Darwin ] || { echo "error: signing needs macOS" >&2; exit 1; }

p12="${APPLE_DEVELOPER_ID_P12_BASE64:-}"
p12_password="${APPLE_DEVELOPER_ID_P12_PASSWORD:-}"
identity="${APPLE_SIGNING_IDENTITY:-}"
key_id="${APPLE_API_KEY_ID:-}"
issuer="${APPLE_API_ISSUER_ID:-}"
p8="${APPLE_API_KEY_P8_BASE64:-}"
identifier="${APPLE_CODESIGN_IDENTIFIER:-io.github.openreadout.openreadout}"

notice() { if [ -n "${GITHUB_ACTIONS:-}" ]; then echo "::notice::$1"; else echo "$1"; fi; }

have_cert=0; [ -n "$p12" ] || [ -n "$identity" ] && have_cert=1
have_key=0; [ -n "$key_id" ] && [ -n "$issuer" ] && [ -n "$p8" ] && have_key=1
any_key=0; [ -n "$key_id$issuer$p8" ] && any_key=1

if [ "$have_cert" = 0 ] && [ "$any_key" = 0 ]; then
  if [ "$require" = 1 ]; then echo "error: no signing certificate configured" >&2; exit 1; fi
  notice "macOS signing secrets are not set; $bin stays unsigned (ad-hoc linker signature only)"
  exit 0
fi
if [ -n "$p12" ] && [ -z "$p12_password" ]; then
  echo "error: APPLE_DEVELOPER_ID_P12_BASE64 is set but APPLE_DEVELOPER_ID_P12_PASSWORD is not" >&2; exit 1
fi
if [ "$have_cert" = 0 ]; then
  echo "error: the App Store Connect API key is set but no certificate (APPLE_DEVELOPER_ID_P12_BASE64)" >&2; exit 1
fi
if [ "$have_key" = 0 ] && { [ "$any_key" = 1 ] || [ "${CI:-}" = true ]; }; then
  echo "error: notarizing needs APPLE_API_KEY_ID, APPLE_API_ISSUER_ID and APPLE_API_KEY_P8_BASE64" >&2; exit 1
fi

work="$(mktemp -d)"
keychain=
original_keychains=
cleanup() {
  if [ -n "$keychain" ]; then
    if [ -n "${original_keychains:-}" ]; then
      # shellcheck disable=SC2086 # one argument per keychain path
      security list-keychains -d user -s $original_keychains || true
    fi
    security delete-keychain "$keychain" 2>/dev/null || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT

if [ -n "$p12" ]; then
  # A throwaway keychain that holds only this certificate, removed when the script ends.
  keychain="$work/signing.keychain-db"
  keychain_password="$(openssl rand -hex 24)"
  printf '%s' "$p12" | base64 --decode > "$work/cert.p12"
  security create-keychain -p "$keychain_password" "$keychain"
  security set-keychain-settings -lut 3600 "$keychain"
  security unlock-keychain -p "$keychain_password" "$keychain"
  security import "$work/cert.p12" -k "$keychain" -P "$p12_password" -f pkcs12 -T /usr/bin/codesign >/dev/null
  rm -f "$work/cert.p12"
  # Let codesign use the key without a password prompt.
  security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$keychain_password" "$keychain" >/dev/null
  # The intermediate certificate between Apple's root and Developer ID certificates issued since
  # 2021, pinned by its SHA-256. codesign embeds the chain, and a .p12 usually holds only the leaf.
  g2_sha256=f16cd3c54c7f83cea4bf1a3e6a0819c8aaa8e4a1528fd144715f350643d2df3a
  if curl -fsSL --retry 3 -o "$work/DeveloperIDG2CA.cer" https://www.apple.com/certificateauthority/DeveloperIDG2CA.cer &&
    [ "$(shasum -a 256 "$work/DeveloperIDG2CA.cer" | cut -d' ' -f1)" = "$g2_sha256" ]; then
    # It may already be in a keychain on the search list, which makes the import fail harmlessly.
    security import "$work/DeveloperIDG2CA.cer" -k "$keychain" >/dev/null 2>&1 || true
  else
    echo "warning: could not fetch Apple's Developer ID G2 intermediate certificate; relying on the system keychain" >&2
  fi
  # codesign only finds the identity and its chain in keychains on the user's search list, so add
  # this one for the run. On your own Mac, cleanup puts the original list back.
  original_keychains="$(security list-keychains -d user | tr -d '"' | xargs)"
  # shellcheck disable=SC2086 # one argument per keychain path
  security list-keychains -d user -s "$keychain" $original_keychains
  identity="$(security find-identity -p codesigning "$keychain" | awk -F'"' '/Developer ID Application/ { split($1, a, " "); print a[2]; exit }')"
  [ -n "$identity" ] || { echo "error: the .p12 holds no Developer ID Application identity" >&2; security find-identity -p codesigning "$keychain" >&2; exit 1; }
fi

keychain_args=()
[ -z "$keychain" ] || keychain_args=(--keychain "$keychain")
echo "signing $bin as $identifier"
codesign --force --options runtime --timestamp --identifier "$identifier" --sign "$identity" \
  ${keychain_args[@]+"${keychain_args[@]}"} "$bin"
codesign --verify --strict --verbose=2 "$bin"
codesign --display --verbose=2 "$bin" 2>&1 | grep -E '^(Identifier|Authority|TeamIdentifier|Timestamp|CodeDirectory)'
if [ -n "${APPLE_TEAM_ID:-}" ]; then
  team="$(codesign --display --verbose=2 "$bin" 2>&1 | sed -n 's/^TeamIdentifier=//p')"
  [ "$team" = "$APPLE_TEAM_ID" ] || { echo "error: signed by team $team, expected $APPLE_TEAM_ID" >&2; exit 1; }
fi

if [ "$have_key" = 0 ]; then
  notice "signed $bin but did not notarize it (no App Store Connect API key)"
  exit 0
fi

printf '%s' "$p8" | base64 --decode > "$work/AuthKey.p8"
ditto -c -k --keepParent "$bin" "$work/submission.zip"
echo "submitting to the notary service (usually a few minutes)"
xcrun notarytool submit "$work/submission.zip" --key "$work/AuthKey.p8" --key-id "$key_id" --issuer "$issuer" \
  --wait --timeout 45m --output-format json > "$work/result.json" || true
cat "$work/result.json"; echo
status="$(plutil -extract status raw -o - "$work/result.json" 2>/dev/null || echo unknown)"
submission="$(plutil -extract id raw -o - "$work/result.json" 2>/dev/null || true)"
if [ "$status" != Accepted ]; then
  echo "error: notarization status is '$status'" >&2
  if [ -n "$submission" ]; then
    xcrun notarytool log "$submission" --key "$work/AuthKey.p8" --key-id "$key_id" --issuer "$issuer" >&2 || true
  fi
  exit 1
fi

# The signature must come from a Developer ID certificate issued by Apple.
codesign --verify --strict -R='anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists' "$bin"
# Gatekeeper's verdict, as for a downloaded copy. The notarization ticket can take a minute to
# show up in Apple's online lookup, so try a few times.
for attempt in 1 2 3 4 5 6; do
  if out="$(spctl --assess --type open --context context:primary-signature -vv "$bin" 2>&1)"; then
    echo "$out"
    case "$out" in *"Notarized Developer ID"*) echo "notarized: $bin"; exit 0 ;; esac
  fi
  echo "Gatekeeper does not see the ticket yet (attempt $attempt): $out"
  sleep 20
done
echo "error: Gatekeeper does not accept $bin as notarized" >&2
exit 1
