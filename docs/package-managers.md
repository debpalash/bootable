# Package-manager manifests

Bootable's primary distribution is the GitHub Release (see [releases.md](releases.md)). This
document covers the optional manifests for third-party package managers. Everything here is a
**template plus a renderer**. Nothing publishes or submits automatically: a maintainer reviews the
rendered files and submits each one by hand, using the accounts listed below.

| Ecosystem | Template directory | Consumes release asset | Writing to drives works? |
| --- | --- | --- | --- |
| winget | `packaging/winget/` | `bootable-<v>-x86_64-setup.exe` (NSIS) | Yes, the installer places the helper |
| Scoop | `packaging/scoop/` | `bootable-<v>-x86_64-pc-windows-msvc.zip` | Only after a one-time elevated `install.ps1` |
| Homebrew cask | `packaging/homebrew/` | `bootable-<v>-aarch64.dmg` | Only after a one-time `sudo` helper install |
| AUR (`bootable-bin`) | `packaging/aur/bootable-bin/` | `bootable-<v>-x86_64-unknown-linux-gnu.tar.gz` | Yes, the package installs the helper and polkit action |
| Nix flake | `packaging/nix/` | Linux and macOS `.tar.gz` | Unverified (see below) |
| Flatpak | `packaging/flatpak/` | Linux `.tar.gz` | **No.** The sandbox blocks the write path |

Writes are performed by a small root-owned helper at a fixed path (`/usr/libexec/bootable-helper`
on Linux, `/Library/PrivilegedHelperTools/app.bootable.helper` on macOS,
`C:\Program Files\Bootable\bootable-helper.exe` on Windows). The app refuses to use a helper that
is not root/administrator-owned and protected. That is why packages that cannot place the helper
say so plainly instead of silently installing something less safe. None of these manifests relaxes
the removable-media safety gates.

## Rendering

```sh
scripts/render-package-manifests.sh 0.1.4              # fetch checksums from the release
scripts/render-package-manifests.sh 0.1.4 --sums SUMS  # or use a local "<sha256>  <asset>" file
scripts/render-package-manifests.sh --self-test        # offline test with fake checksums
```

Output goes to `dist/package-manifests/<ecosystem>/` together with `SHA256SUMS.used`, which records
the checksum of every asset that was substituted. Templates are the `*.in` files; `@VERSION@`,
`@RELEASE_DATE@`, `@SHA256_<KEY>@` and `@SHA256_<KEY>_UPPER@` (winget wants upper case) are
replaced. The renderer fails without writing anything if a version is not stable
`MAJOR.MINOR.PATCH`, any checksum is missing, malformed or conflicting, or any `@PLACEHOLDER@`
remains. Re-running it replaces the previous output and is idempotent. The release date comes from
`--date`, then `SOURCE_DATE_EPOCH`, then the GitHub release's `published_at`.

**Checksum source.** The Release workflow publishes one `<asset>.sha256` sidecar per asset, not an
aggregate `SHA256SUMS` file. The renderer tries `SHA256SUMS` first (so a future aggregate file
works unchanged) and falls back to the sidecars. Always cross-check the rendered hashes against
the release page before submitting.

The [`Package manifests`](../.github/workflows/package-manifests.yml) workflow runs the renderer
and uploads `dist/package-manifests` as an artifact. It has read-only permissions, no secrets, and
never pushes anywhere. Run it from **Actions → Package manifests → Run workflow** (leave the
version empty to use `Cargo.toml`), then download the artifact. It also starts when a manually
dispatched `Release` run succeeds. A bare `release: published` trigger exists too, but GitHub does
not deliver events caused by the default `GITHUB_TOKEN`, so it will not fire for releases that the
Release workflow publishes itself.

`dist/` is not in `.gitignore` at the time of writing; do not commit rendered output.

## winget

- **Where:** pull request to [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs)
  adding `manifests/d/debpalash/Bootable/<version>/` (the renderer already produces that path).
- **Accounts:** a GitHub account; agree to the Microsoft CLA when the bot asks on the PR.
- **Steps:** on Windows, `winget validate --manifest <dir>`, then (with local manifests enabled by
  `winget settings --enable LocalManifestFiles`) `winget install --manifest <dir>`, ideally in
  Windows Sandbox. Open the PR; the first submission is reviewed manually. Later versions can use
  `wingetcreate update debpalash.Bootable --version <v> --urls <setup.exe url> --submit` with a
  personal access token held by the maintainer, not by CI.
- **Notes:** the manifest uses the NSIS setup EXE (`InstallerType: nullsoft`, machine scope,
  `/S` silent), which installs the helper under Program Files. The MSI is a valid alternative but
  winget wants its `ProductCode`, which cannot be derived from the release checksums; add it from a
  test install if you prefer the MSI. The installer is not Authenticode-signed, so SmartScreen may
  warn. Reviewers may ask for `ProductCode`/`AppsAndFeaturesEntries`; fill them from a real install.

## Scoop

- **Where:** a bucket repository you own, for example `debpalash/scoop-bootable`, containing
  `bucket/bootable.json`. Users run `scoop bucket add bootable https://github.com/debpalash/scoop-bootable`
  then `scoop install bootable`. The `Extras` bucket has popularity requirements; propose it
  there only once the project meets them.
- **Accounts:** GitHub only.
- **Steps:** place the file, then run `scoop install ./bootable.json` and `scoop checkver` /
  `scoop update bootable` in a clean VM. `checkver` and `autoupdate` are already configured
  (hashes come from each release's `.sha256` sidecar).
- **Limit:** Scoop extracts the portable ZIP and cannot place the protected helper in
  `C:\Program Files`. The manifest's `notes` tell the user to run the bundled `install.ps1`
  (which self-elevates through UAC) to enable writing. Without it, discovery, downloads and
  verification work but writing is unavailable. This is deliberately not automated inside Scoop.

## Homebrew cask

- **Where:** a tap repository `debpalash/homebrew-bootable` containing `Casks/bootable.rb`. Users
  run `brew install --cask debpalash/bootable/bootable`.
- **Accounts:** GitHub only for a personal tap.
- **Steps:** `brew style --cask`, `brew audit --cask --new debpalash/bootable/bootable`, then
  `brew install --cask` and `brew uninstall --cask` on an Apple Silicon Mac.
- **Not the official `homebrew/cask` yet:** the app is ad-hoc signed and not notarized, and
  Homebrew's policy has been moving against casks that fail Gatekeeper. Submitting upstream would
  need an Apple Developer account and notarization first. Check current Homebrew policy before
  trying.
- **Limits:** the cask installs `Bootable.app` and links the `bootable` TUI/CLI from inside the app
  bundle. It does not install the root helper (that needs `sudo`); the caveat prints the three
  commands, identical to what the DMG's *Install Bootable Helper.command* does. `uninstall`
  removes the helper. Apple Silicon only, macOS 12 or later.

## AUR (`bootable-bin`)

- **Where:** `ssh://aur@aur.archlinux.org/bootable-bin.git`.
- **Accounts:** an AUR account with your SSH public key added to its profile.
- **Steps:** set the `# Maintainer:` line, copy the rendered `PKGBUILD` and `.SRCINFO`, run
  `namcap PKGBUILD`, build in a clean chroot (`extra-x86_64-build`), then commit and push to the
  AUR repository. For updates, re-render and push both files (`.SRCINFO` is rendered, but you
  can regenerate it with `makepkg --printsrcinfo > .SRCINFO`).
- **Behaviour:** installs `/usr/bin/bootable`, `/usr/bin/bootable-desktop`, the root-owned
  `/usr/libexec/bootable-helper`, the polkit action, desktop entry, icon, and license. x86-64
  only. The release has no ARM Linux build.

## Nix

- **Where:** a flake you host (for example a `nix/` subdirectory or its own repository holding
  `flake.nix` and `package.nix`). It repackages the prebuilt release archives for `x86_64-linux`
  and `aarch64-darwin` with `autoPatchelfHook`.
- **Accounts:** GitHub only. nixpkgs itself prefers source builds and would require a
  `rustPlatform.buildRustPackage` derivation instead of this binary repackaging.
- **Status: not built or run.** Nix is not installed where these files were authored.
  Before publishing, run `nix flake check`, `nix build .#bootable`, and start both binaries.
  Expect to adjust the runtime library list (`runtimeDependencies`) for the GUI.
- **Privileged writes are unverified.** The helper is installed next to the executables in
  `$out/bin`, which the app accepts as a fallback because store files are root-owned and not
  writable. The shipped polkit action, however, is bound to `/usr/libexec/bootable-helper`, so a
  Nix install gets pkexec's generic authentication rather than Bootable's dedicated action, and
  NixOS needs polkit enabled and an authentication agent running. Treat writing from Nix as
  untested.

## Flatpak

- **Where:** local build or a self-hosted repository only. **Not eligible for Flathub as is:**
  Flathub requires building from source with vendored cargo sources, a metainfo file with
  screenshots, and a credible permission story; this manifest repackages the release binary.
- **Accounts:** none for local testing.
- **Build:** `flatpak-builder --user --install --force-clean build-dir app.bootable.Bootable.yml`
  from the rendered `flatpak/` directory, then `flatpak run app.bootable.Bootable`. Not run here.
- **Writing to drives does not work in this Flatpak, by design of the sandbox:**
  1. The app starts its helper through `/usr/bin/pkexec` and looks for
     `/usr/libexec/bootable-helper`. Inside the sandbox `/usr` is the Flatpak runtime, which has
     neither, and the polkit action lives on the host.
  2. Raw block devices (`/dev/sdX`, `/dev/nvme*`) are not visible to a sandboxed app without
     `--device=all`.
  3. The only escape hatch is `--talk-name=org.freedesktop.Flatpak` plus `flatpak-spawn --host`,
     which hands the app unrestricted host command execution. That defeats the sandbox and would
     need app changes anyway, so the manifest does not request it.
  Therefore the manifest requests only display, network, GPU, and the Downloads folder, and the
  bundled metainfo says writing is unavailable. Drive discovery inside the sandbox is likewise
  limited and has not been verified. Use the DEB, RPM, AppImage, or AUR package to write media.
  A real Flatpak write story would require an app-side change, such as a portal or a
  host-installed helper reached over a narrow D-Bus interface, which is out of scope here.

## Validation performed on these templates

| Check | Result |
| --- | --- |
| `shellcheck` on the renderer (v0.10.0) | clean |
| `render-package-manifests.sh --self-test` (offline, fake checksums) | passes; covers idempotence, CRLF input, missing/malformed/conflicting checksums, leftover placeholders, bad versions |
| Render against real `v0.1.4` release (11 assets, via sidecars) | succeeds; tarball hash re-verified locally |
| YAML parse (winget, Flatpak, workflow), JSON parse (Scoop), `ruby -c` (cask), XML parse (metainfo), `bash -n` (PKGBUILD) | pass |
| AUR: `makepkg` build of the rendered `PKGBUILD` against the real `v0.1.4` tarball with checksum verification, `.SRCINFO` matches `makepkg --printsrcinfo` | pass |
| winget `validate`/install, Scoop install, `brew audit`/install, `flatpak-builder`, `nix build` | **not run** (tools or platforms unavailable) |

Treat the unrun checks as required steps before each first submission.
