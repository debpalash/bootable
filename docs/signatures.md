# Signature verification

A checksum downloaded from the same server as the image only proves the transfer was not
corrupted: whoever can replace the image can replace the checksum next to it. A signature made by a
key that Bootable already trusts is real integrity evidence. This document describes what
Bootable verifies, what it trusts, and how to extend the trust set.

## What is verified

Bootable authenticates the publisher's **checksum manifest** (`SHA256SUMS`, `sha256sum.txt`,
`CHECKSUM`, ...) and then checks the downloaded image against the digest in that manifest. It
supports:

| Format | Where the signature is | Parser |
| --- | --- | --- |
| OpenPGP clear-signed manifest | inside the manifest itself (Fedora, AlmaLinux `CHECKSUM`) | `pgp` |
| OpenPGP detached signature | `<manifest>.gpg`, `.sign`, `.asc`, `.sig` (Ubuntu, Debian, Linux Mint, Kali) | `pgp` |
| minisign (prehashed `ED` only) | `<manifest>.minisig` | `minisign-verify` |

Both OpenPGP and minisign verification are pure Rust (no `gpg` or `minisign` executable, no
`unsafe`). `pgp` (rPGP) was chosen over Sequoia because Sequoia's default crypto backend is a C library
(Nettle) and the pure-Rust backend is still experimental; rPGP is pure Rust, maintained, and handles exactly the packet types needed
here. `minisign-verify` has no dependencies at all. Keys are never fetched at runtime; only
the manifest and its sibling signature file are downloaded, from the host the checksum came from.

Verification runs before any image byte is transferred. The integrity state is therefore known
before the download starts and a failed signature refuses the download outright, exactly as a
checksum mismatch discards a staged file.

## Integrity states

`IntegrityState` (public in `bootable-core`) is the single source of the label both interfaces show.
Strongest first:

| Rank | State | Label |
| --- | --- | --- |
| 2 | Signature verified | `Signature verified · Ubuntu (key D94A A3F0 EFE2 1092) · SHA-256 matches signed manifest` |
| 1 | Checksum only | `Publisher checksum verified` or `Publisher checksum verified · signature not verified (<reason>)` |
| 0 | Transfer only | `HTTPS transfer and boot structure checked · publisher checksum unavailable` |

Outcomes of the signature step:

* **Verified**: a pinned, unexpired, unrevoked key made a valid signature over exactly the manifest
  bytes (or the signed cleartext). The image digest must then match the manifest, or the usual
  checksum-mismatch refusal applies.
* **Unverified** (degrades to rank 1, with the reason in the label): no signature is published next
  to the manifest; the file is not a signature (for example an HTML "not found" page); the signature
  was made by a key that is not pinned; the pinned key has expired or is revoked; the signature
  itself has expired; the signature is a legacy minisign signature. Bootable never reports a
  verified signature in these cases.
* **Rejected** (the download is refused): the signature names a pinned key as its issuer but does
  not verify. This is what a tampered manifest, or a manifest swapped for another publisher's
  signature, looks like. The error text starts with `download refused: signature verification failed
  for <manifest url>`.

A checksum taken from catalog metadata (for example a SourceForge MD5) or from a stronger embedded
digest is never labelled as signed; only a digest read from an authenticated manifest is.

Where the label appears: the progress messages of the download (`Stage 4/5 ...` and the final
`Ready ...` line) and the message stored for the completed download in the ledger. These come from
core, so the desktop and terminal interfaces show identical text. `IsoRelease::planned_integrity_label`
gives the matching text for a release before it is downloaded; it promises only that a signature
will be looked for.

## Trust model

* The trust root is the table in `crates/bootable-core/src/signature/pinned.rs` together with the
  armored keys in `crates/bootable-core/keys/`. Nothing is trusted on first use, and any key material that
  accompanies a downloaded signature is ignored.
* Pinning is by the full 40-hex v4 fingerprint. At first use each key is parsed and its fingerprint
  recomputed; an entry whose key does not hash to its pinned fingerprint is dropped, and the unit
  test `every_pinned_key_loads_and_matches_its_fingerprint` fails.
* Only the **primary** key is trusted to sign. Signatures made by subkeys are not accepted. (All
  pinned publishers sign with the primary key.)
* A key is unusable once it is expired (from its newest self-signature) or carries any revocation
  signature. Expiry is judged against the current time: a signature made while the key was valid but
  checked after expiry degrades to checksum-only rather than verifying. Update the pinned key when a
  publisher rotates.
* Weak hashes (MD5, SHA-1, RIPEMD-160) are rejected by the OpenPGP implementation.
* The signature file is looked up on the same host as the manifest, so mirrors work: a valid
  signature from a pinned key authenticates the manifest regardless of which mirror served it.

### Known limits

* A **missing** signature degrades rather than fails. An attacker who can block a request can
  therefore force checksum-only integrity, which is exactly the state that existed before this
  feature, and is labelled as such.
* **Replay**: a validly signed old manifest still verifies. It authenticates old images, not the
  newest release.
* Signatures over the ISO itself (Arch Linux `.iso.sig`, Tails, Raspberry Pi OS `.img.xz.sig`,
  Alpine `.asc`) are not checked; that needs a streaming signature check over multi-gigabyte files
  and is future work. Publishers that only sign the ISO are not covered.
* The catalog discovers the manifest, not its signature. A manifest URL that Bootable does not
  recognize as a checksum file is not authenticated.
* No minisign key is pinned yet, because no catalog publisher's minisign key could be confirmed
  from an authoritative page. The code path is implemented and tested with generated keys.

## Pinned publishers

Fingerprints below are copied from the publisher's own documentation. The armored key material
was fetched separately (publisher URL or `keyserver.ubuntu.com`), checked against the documented
fingerprint with `gpg --show-keys --with-fingerprint`, reduced with
`gpg --export-options export-minimal`, and each publisher's live manifest signature was verified with
`gpg --verify` before the key was committed. The unit tests also verify real signed manifests
from these publishers (`crates/bootable-core/testdata/`).

| Publisher | Signed file | Fingerprint | Fingerprint source |
| --- | --- | --- | --- |
| Ubuntu CD Image Automatic Signing Key (2012) | `SHA256SUMS.gpg` | `8439 38DF 228D 22F7 B374 2BC0 D94A A3F0 EFE2 1092` | <https://ubuntu.com/tutorials/how-to-verify-ubuntu> |
| Debian CD signing key (2009) | `SHA256SUMS.sign` | `1046 0DAD 7616 5AD8 1FBC 0CE9 9880 21A9 64E6 EA7D` | <https://www.debian.org/CD/verify> |
| Debian CD signing key (2011) | `SHA256SUMS.sign` | `DF9B 9C49 EAA9 2984 3258 9D76 DA87 E80D 6294 BE9B` | <https://www.debian.org/CD/verify> |
| Debian Testing CDs Automatic Signing Key (2014) | `SHA256SUMS.sign` | `F41D 3034 2F35 4669 5F65 C669 4246 8F40 09EA 8AC3` | <https://www.debian.org/CD/verify> |
| Linux Mint ISO Signing Key | `sha256sum.txt.gpg` | `27DE B156 44C6 B3CF 3BD7 D291 300F 846B A25B AE09` | <https://linuxmint-installation-guide.readthedocs.io/en/latest/verify.html> |
| Kali Linux Archive Automatic Signing Key (2025), expires 2028-04-17 | `SHA256SUMS.gpg` | `827C 8569 F251 8CC6 77FE CA1A ED65 462E C8D5 E4C5` | <https://www.kali.org/docs/introduction/download-images-securely/> |
| Fedora 42 | clear-signed `*-CHECKSUM` | `B0F4 9504 58F6 9E11 50C6 C5ED C8AC 4916 105E F944` | <https://fedoraproject.org/security> |
| Fedora 43 | clear-signed `*-CHECKSUM` | `C6E7 F081 CF80 E131 4667 6E88 829B 6066 3164 5531` | <https://fedoraproject.org/security> |
| Fedora 44 | clear-signed `*-CHECKSUM` | `36F6 12DC F27F 7D1A 48A8 35E4 DBFC F71C 6D9F 90A6` | <https://fedoraproject.org/security> |
| Fedora 45 | clear-signed `*-CHECKSUM` | `4F50 A611 4CD5 C697 6A7F 1179 655A 4B02 F577 861E` | <https://fedoraproject.org/security> |
| AlmaLinux OS 9 | clear-signed `CHECKSUM` | `BF18 AC28 7617 8908 D6E7 1267 D36C B86C B86B 3716` | <https://almalinux.org/security/> |
| AlmaLinux OS 10 | clear-signed `CHECKSUM` | `EE6D B7B9 8F5B F5ED D9DA 0DE5 DEE5 C11C C2A1 E572` | <https://almalinux.org/security/> |

Deliberately not pinned:

* Ubuntu's legacy 1024-bit DSA key `C598 6B4F 1257 FFA8 6632 CBA7 4618 1433 FBB7 5451` (still listed
  on the Ubuntu page): too weak to trust.
* Fedora 46: present in `fedora.gpg` but not yet listed on the security page that the fingerprints
  above were copied from. Add it once Fedora publishes it there.
* Rocky Linux, openSUSE, Arch, Pop!_OS, Raspberry Pi OS and others: either the signed file is the
  image itself (see limits), or no authoritative fingerprint page could be confirmed.

## Adding a key

1. Find the publisher's **own** statement of the key fingerprint (a verification or security page on
   the project's website served over HTTPS). Search results, wikis, forum posts and the keyserver
   itself are not authoritative. If you cannot find one, do not add the key.
2. Fetch the key from the publisher or a keyserver and confirm the fingerprint matches the page:

   ```sh
   gpg --show-keys --with-fingerprint --keyid-format long key.asc
   ```

3. Confirm it really signs the publisher's manifests, using a scratch keyring:

   ```sh
   gpg --homedir "$(mktemp -d)" --import key.asc
   gpg --homedir <same> --verify SHA256SUMS.gpg SHA256SUMS
   ```

4. Reduce the key and store it:

   ```sh
   gpg --export-options export-minimal --armor --export <FINGERPRINT> \
       > crates/bootable-core/keys/<publisher>-<year>.asc
   ```

5. Add an entry to `OPENPGP_KEYS` in `crates/bootable-core/src/signature/pinned.rs` with a unique
   `id`, the short `publisher` label users will see, the uppercase 40-hex fingerprint, and
   `include_str!` of the key. Put the fingerprint source URL and key URL in the comment above it.
6. Add the real signed manifest (and signature) to `crates/bootable-core/testdata/` and a case to
   `signature/tests.rs` that verifies it, plus the tampered variant. Keep fixtures small.
7. Run `cargo test -p bootable-core`. `every_pinned_key_loads_and_matches_its_fingerprint` and
   `pinned_table_is_well_formed` must pass.
8. Document the key in the table above.

To add a minisign key, copy the base64 line of the publisher's `minisign.pub` (confirmed against
their website) into `MINISIGN_KEYS` with the same provenance comment, and add a test with a real
manifest. Only prehashed (`ED`) signatures are accepted.

## Rotating or removing a key

When a publisher rotates keys, add the new key before the old one expires and keep the old key until
the images it signed leave the catalog. Remove a key immediately if the publisher reports it
compromised: deleting the entry and its `.asc` file is sufficient, since downloads fall back to the
checksum-only state.
