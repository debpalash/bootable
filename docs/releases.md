# Release channels

Bootable has two GitHub release channels. The rules live in CI and are not optional release
conventions.

## Release candidates

Every same-repository pull request runs the complete CI and native package matrices. Once both
workflows pass, CI publishes an automatic GitHub prerelease named
`v<version>-rc.pr<number>.<attempt>` with the Linux, macOS, and Windows assets from that exact pull
request commit.

Pull requests from forks are deliberately excluded from publishing: their code is tested with a
read-only token, but it is never distributed under the project's GitHub Releases account.

## Stable releases

A stable release is never created by a tag push or merge. A maintainer must explicitly run the
`Release` workflow from the `main` branch. CI rebuilds and verifies every native package, attests
the stable artifacts, and publishes the Cargo workspace version as `v<version>`.

Before dispatching a stable release:

1. Confirm the intended version is committed in `Cargo.toml` and `Cargo.lock` on `main`.
2. Confirm the release-candidate assets were installed or opened on their target platforms.
3. Open **Actions → Release → Run workflow**, select `main`, and approve the run.
4. Confirm the resulting release is neither a draft nor a prerelease and contains all 22 assets
   (24 when signed checksums are enabled).
5. If signing is enabled, spot-check one asset per platform using [signing.md](signing.md).

Stable tags and releases are immutable. Corrections use a new patch version.

## Signing

The stable workflow can sign every platform, but **it signs nothing until secrets are added**; with
none configured it produces unsigned packages exactly as before, and the release notes say so. Each
platform switches on independently when its credentials exist:

| Platform | Enabled by | Result |
| --- | --- | --- |
| macOS | `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD` and an Apple ID or API key notarization set | Developer ID signed, hardened, notarized, and stapled app and DMG, including the privileged helper |
| Windows | Azure Artifact Signing (default) or SignPath settings | Authenticode-signed executables, MSI, and setup EXE with timestamps |
| Linux | repository variable `SIGN_CHECKSUMS=true` (no secret) | Keyless Sigstore-signed `SHA256SUMS` plus the existing build attestations |

Signing runs only for a manual dispatch from `main`. A partially configured platform fails the job
on purpose. `scripts/sign-verify-macos.sh` and `scripts/sign-verify-windows.ps1` assert the result
before artifacts are attested and uploaded. The exact secret and variable names, what to purchase,
and how users verify are in [signing.md](signing.md).

