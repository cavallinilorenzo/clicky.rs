# Stable local signing with a self-signed certificate

macOS ties the Input Monitoring permission to the app's code signature. Ad-hoc signatures change with every build, so macOS forgets the permission after each rebuild, which breaks the "set it up once" goal. A paid Apple Developer identity would be stable, but it is not available long term. So Clicky is signed with a self-signed code-signing certificate created once in the developer's login keychain. Its designated requirement stays the same across rebuilds. If the certificate is missing, the build falls back to ad-hoc signing and warns that the permission will need to be granted again.

## Consequences

The app is not notarised and runs only on Macs where it was built, or where the user trusts it explicitly. Notarisation and distribution are out of scope (issue #2).
