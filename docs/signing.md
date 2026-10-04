# Code signing

**Status: nothing is signed today.** The release pipeline is signing-ready, but no certificates,
accounts, or secrets exist yet. Until a maintainer completes the steps below, every release is
unsigned exactly as the README says: Windows packages carry no Authenticode signature, the macOS app
is only ad-hoc signed and not notarized, and Linux packages rely on SHA-256 sidecars and GitHub build
provenance. Adding the secrets and variables named here turns each platform on independently; no
workflow edit is needed. Removing them turns it back off.

Signing happens only in a manually dispatched stable release from `main`. Release candidates built
from pull requests are never signed, notarized, or given access to any signing secret.

## How the pipeline decides

Each platform job runs `scripts/sign-detect.sh`, which looks only at whether credentials are present:

- Nothing configured: the step reports "not enabled" and the job continues unsigned, exactly as before.
- A complete set configured, on a `workflow_dispatch` run: signing steps run and the matching
  `scripts/sign-verify-*` check must pass before the artifacts are attested and uploaded.
- A **partial** set configured: the job fails with the missing names. This is deliberate, so a typo
  in a secret name cannot quietly ship an unsigned stable release.

Secrets are mapped to the environment of the individual detect and signing steps only, never the
whole job, so third-party `build.rs` scripts run by `cargo build` cannot read them. Outside a
`workflow_dispatch` run the expressions evaluate to empty strings, so pull-request jobs never receive
the values even for same-repository branches.

Signing order matters. Windows executables are signed before packaging, so the MSI, NSIS installer,
portable EXEs, and ZIP all embed signed code, and the MSI and setup EXE are signed again afterwards.
Checksum sidecars are regenerated after signing, and GitHub build attestations cover the final signed
files.

## macOS: Developer ID, notarization, stapling

`scripts/sign-macos.sh` runs between `package-macos.sh` and `verify-macos-packages.sh`. It:

1. Imports the certificate into a temporary keychain (random password, partition list set) and
   deletes it on exit, including on failure or cancellation.
2. Signs with the hardened runtime and a secure timestamp: first the nested `bootable-helper`
   (identifier `app.bootable.helper`) and `bootable`, then the `Bootable.app` bundle. The privileged
   helper is the binary the DMG's installer copies to `/Library/PrivilegedHelperTools`; it is part of
   the signed bundle and also signed inside the `.tar.gz`.
3. Submits the app plus the archive's bare binaries to `notarytool --wait`, requires `Accepted`, and
   staples the app.
4. Rebuilds the DMG from the stapled app, signs it, notarizes it, and staples it.
5. Repacks the `.tar.gz` with the signed binaries and rewrites both `.sha256` sidecars.

Bare binaries in the archive are signed and notarized but cannot carry a stapled ticket; Gatekeeper
checks them online.

### What the maintainer must obtain

1. **Apple Developer Program** membership (US$99 per year). The Account Holder creates the
   certificate.
2. A **Developer ID Application** certificate (Certificates, IDs & Profiles, "Developer ID
   Application"). Create it with a CSR from Keychain Access, install it, then export the certificate
   and private key as a `.p12` with a password.
3. Notarization credentials, either:
   - an **app-specific password** (account.apple.com, Sign-In and Security) plus your Apple ID and
     10-character Team ID, or
   - an **App Store Connect API key** (Users and Access, Integrations, Team Keys) with Developer
     access or higher: the `.p8` file, its Key ID, and the Issuer ID. Preferred for CI because it is
     not tied to a person's password.

Encode the files: `base64 -i DeveloperID.p12 | pbcopy` and `base64 -i AuthKey_XXXX.p8 | pbcopy`.

### Secrets to add (Settings, Secrets and variables, Actions, Secrets)

| Name | Value |
| --- | --- |
| `APPLE_CERTIFICATE` | base64 of the Developer ID Application `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | the `.p12` export password |

Plus **one** of these notarization sets:

| Name | Value |
| --- | --- |
| `APPLE_ID` | Apple ID email |
| `APPLE_TEAM_ID` | 10-character Team ID |
| `APPLE_APP_PASSWORD` | app-specific password |

| Name | Value |
| --- | --- |
| `APPLE_API_KEY_P8` | base64 of the `AuthKey_*.p8` file |
| `APPLE_API_KEY_ID` | API key ID |
| `APPLE_API_ISSUER_ID` | issuer UUID |

If both sets exist, the API key is used.

### How users verify

```sh
codesign --verify --deep --strict --verbose=2 /Applications/Bootable.app
codesign -dvv /Applications/Bootable.app 2>&1 | grep -E 'Authority|TeamIdentifier|flags'
spctl --assess --type execute -vv /Applications/Bootable.app     # "accepted, source=Notarized Developer ID"
xcrun stapler validate /Applications/Bootable.app
spctl --assess --type open --context context:primary-signature -vv bootable-VERSION-aarch64.dmg
codesign -dvv /Library/PrivilegedHelperTools/app.bootable.helper  # after running the helper installer
```

Expect `Authority=Developer ID Application: <name> (<TEAMID>)`, a `Timestamp`, and `runtime` in the
flags. The Team ID must match the one published by the maintainer.

## Windows: Authenticode

Two providers are wired in. **Azure Artifact Signing is the documented default**; SignPath is the
alternative. If both are fully configured, Azure is used. Both sign `bootable.exe`,
`bootable-desktop.exe`, and `bootable-helper.exe` before packaging, then the MSI and setup EXE after,
with an RFC 3161 timestamp so signatures outlive the certificate.

### Default: Azure Artifact Signing (formerly Trusted Signing)

- **Cost:** a pay-as-you-go Azure subscription; the Basic SKU was about US$10 per month at the time of
  writing. Check current pricing.
- **Eligibility:** identity validation is mandatory, and the service has restricted who can enroll
  (at the time of writing, organizations in a short list of countries and individual developers in the
  US and Canada). Confirm eligibility in the Azure portal before relying on it.
- Authentication uses GitHub OIDC with a federated credential, so there is no long-lived client
  secret.

Maintainer steps:

1. Create an Azure subscription and register the `Microsoft.CodeSigning` resource provider.
2. Create an **Artifact Signing account**. Note its region endpoint (for example
   `https://eus.codesigning.azure.net/`).
3. Complete **identity validation**, then create a **Public Trust certificate profile**. Private
   Trust profiles will not validate on user machines.
4. Create a Microsoft Entra **app registration** and add a **federated credential** for GitHub
   Actions with subject `repo:debpalash/bootable:ref:refs/heads/main`.
5. In the Artifact Signing account, give that app the **Artifact Signing Certificate Profile Signer**
   role.

Secrets:

| Name | Value |
| --- | --- |
| `AZURE_CLIENT_ID` | app registration (client) ID |
| `AZURE_TENANT_ID` | Entra tenant ID |
| `AZURE_SUBSCRIPTION_ID` | subscription ID |

Variables (Settings, Secrets and variables, Actions, **Variables**; not sensitive, kept as variables
so short names are not masked throughout the logs):

| Name | Value |
| --- | --- |
| `AZURE_SIGNING_ENDPOINT` | regional endpoint URL |
| `AZURE_SIGNING_ACCOUNT` | Artifact Signing account name |
| `AZURE_SIGNING_PROFILE` | certificate profile name |

### Alternative: SignPath (free for open source)

SignPath Foundation provides free certificates to qualifying open-source projects. The trade-off:
the publisher shown to users is **SignPath Foundation**, not Bootable, and you must apply and be
approved.

1. Apply at signpath.org for the OSS program and create the project in SignPath.
2. Connect the GitHub repository as a trusted build system (SignPath's GitHub Actions integration)
   and create a CI user with an API token.
3. Create an artifact configuration that signs the staged ZIP. Both phases use the same one, so
   `min-matches="0"` is required because the first phase has no MSI and the second no bare EXEs:

   ```xml
   <artifact-configuration xmlns="http://signpath.io/artifact-configuration/v1">
     <zip-file>
       <pe-file-set>
         <include path="*.exe" min-matches="0" max-matches="unbounded"/>
         <authenticode-sign/>
       </pe-file-set>
       <msi-file-set>
         <include path="*.msi" min-matches="0" max-matches="unbounded"/>
         <authenticode-sign/>
       </msi-file-set>
     </zip-file>
   </artifact-configuration>
   ```

4. Create a release signing policy for it.

Secret: `SIGNPATH_API_TOKEN`. Variables: `SIGNPATH_ORGANIZATION_ID`, `SIGNPATH_PROJECT_SLUG`,
`SIGNPATH_SIGNING_POLICY_SLUG`, and optionally `SIGNPATH_ARTIFACT_CONFIGURATION_SLUG` (omit to use
the project default). Each phase submits one request, so expect two approvals if the policy requires
manual approval.

### How users verify

PowerShell:

```powershell
Get-AuthenticodeSignature .\bootable-VERSION-x86_64-setup.exe | Format-List Status, SignerCertificate, TimeStamperCertificate
```

`Status` must be `Valid`. Or, with the Windows SDK: `signtool verify /pa /all /v <file>`. In
Explorer: Properties, Digital Signatures. The signer is the validated identity (Azure) or
`SignPath Foundation` (SignPath). A valid signature does not instantly clear Microsoft SmartScreen;
reputation for a new publisher builds over time, so a warning can persist for early downloads.

## Linux: attestations and signed checksums

Linux DEB, RPM, AppImage, and tar.gz packages are **not individually signed** (no GPG or repository
signing). Two independent proofs exist instead.

**GitHub build provenance** is already produced for every asset of a stable release. Verify with the
GitHub CLI:

```sh
gh attestation verify bootable_VERSION_amd64.deb --repo debpalash/bootable
```

**Signed `SHA256SUMS`** is optional and needs **no secret**: it uses keyless Sigstore signing tied
to this workflow's GitHub OIDC identity. Enable it by adding the repository **variable**
`SIGN_CHECKSUMS` with the value `true`. The stable release then also contains `SHA256SUMS` and
`SHA256SUMS.sigstore.json` (24 assets instead of 22). Verify with [cosign](https://docs.sigstore.dev/):

```sh
cosign verify-blob \
  --bundle SHA256SUMS.sigstore.json \
  --certificate-identity https://github.com/debpalash/bootable/.github/workflows/release.yml@refs/heads/main \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  SHA256SUMS
sha256sum --check --ignore-missing SHA256SUMS      # macOS: shasum -a 256 -c
```

## Limits and honest caveats

- **Nothing here has run end to end.** The workflow, shell scripts, and PowerShell scripts were
  linted and parsed, and the configuration detection was exercised locally, but the macOS and
  Windows signing steps need real credentials and macOS or Windows runners. Expect to fix small
  issues on the first real run.
- There is no dry run: signing is gated to a stable release, and the final job publishes it. Rehearse
  in a personal fork with its own secrets (dispatch from that fork's `main`) before the real
  repository, and remember stable tags are immutable.
- Secrets are repository-wide. For stronger protection, move them to a GitHub Environment limited to
  `main` with required reviewers and attach that environment to the signing jobs. That edit is not
  made here because the same jobs also run for pull requests.
- The hardened runtime is enabled without entitlements. If the signed app fails to start because a
  library is blocked, the fix is an entitlements file passed to `codesign`, which needs testing on a
  real Mac.
- When signing goes live, update the README line "Bootable is unsigned" and the site copy. The
  stable-release notes adjust themselves from what was actually signed.
- Rotate the secrets when a maintainer leaves; Developer ID certificates expire after five years.
