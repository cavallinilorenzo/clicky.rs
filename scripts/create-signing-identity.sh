#!/bin/sh
# Creates the self-signed code-signing identity "Clicky Local Signing" in your
# login keychain, once per Mac (ADR 0002).
#
# Why: macOS ties the Input Monitoring permission to the app's code signature.
# Ad-hoc signatures change with every build, so macOS forgets the permission
# after each rebuild. A certificate that stays the same gives Clicky a stable
# designated requirement, so the permission survives rebuilds.
#
# What it does:
#   1. generates a 2048-bit RSA key and a 10-year self-signed certificate with
#      the codeSigning extended key usage (files in a temporary directory,
#      deleted on exit);
#   2. imports them into the login keychain, letting /usr/bin/codesign use the key;
#   3. marks the certificate as trusted for code signing, for your user only
#      (macOS asks for your password).
#
# After it, `cargo xtask bundle` signs with this identity automatically. Grant
# Input Monitoring to the app once more after the first build signed with it.
#
# To undo: open Keychain Access, delete the "Clicky Local Signing" certificate
# and its private key from the login keychain.
set -eu

NAME="${CLICKY_SIGNING_IDENTITY:-Clicky Local Signing}"
KEYCHAIN="$HOME/Library/Keychains/login.keychain-db"
# The system's LibreSSL writes PKCS#12 files that `security import` accepts.
OPENSSL=/usr/bin/openssl

if security find-identity -p codesigning "$KEYCHAIN" | grep -q "\"$NAME\""; then
    echo "The code-signing identity \"$NAME\" already exists; nothing to do."
    exit 0
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

cat > "$WORK/cert.cnf" <<EOF
[req]
distinguished_name = dn
prompt = no
x509_extensions = ext

[dn]
CN = $NAME

[ext]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
subjectKeyIdentifier = hash
EOF

echo "Creating the key and the self-signed certificate..."
"$OPENSSL" req -x509 -newkey rsa:2048 -nodes -days 3650 \
    -config "$WORK/cert.cnf" -keyout "$WORK/key.pem" -out "$WORK/cert.pem" 2>/dev/null

# A throwaway password, only for moving the key into the keychain.
PASSWORD="$("$OPENSSL" rand -hex 16)"
"$OPENSSL" pkcs12 -export -name "$NAME" -inkey "$WORK/key.pem" -in "$WORK/cert.pem" \
    -out "$WORK/identity.p12" -passout "pass:$PASSWORD"

echo "Importing it into the login keychain..."
security import "$WORK/identity.p12" -k "$KEYCHAIN" -f pkcs12 -P "$PASSWORD" -T /usr/bin/codesign

echo "Trusting it for code signing (macOS asks for your password)..."
if ! security add-trusted-cert -r trustRoot -p codeSign -k "$KEYCHAIN" "$WORK/cert.pem"; then
    echo "warning: could not mark the certificate as trusted; codesign can still use it." >&2
fi

echo
security find-identity -p codesigning "$KEYCHAIN" | grep "\"$NAME\"" || {
    echo "error: the identity \"$NAME\" is not listed after the import." >&2
    exit 1
}
echo
echo "Done. Build with: cargo xtask bundle"
echo "The first codesign run may ask to use the key: choose \"Always Allow\"."
