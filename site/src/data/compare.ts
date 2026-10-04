// Comparison data. Every competitor claim carries source indexes that point at
// the competitor's own site, repository, or documentation. A cell with no
// source index is either a statement about Bootable or an explicit
// "not checked" marker; unverified competitor claims are left out.

export const CHECKED_ON = '2026-10-04';
export const CHECKED_ON_LABEL = '4 October 2026';

export interface Cell {
  text: string;
  /** 1-based indexes into the competitor's `sources` array. */
  src?: number[];
}
export interface Source {
  label: string;
  url: string;
}
export interface Competitor {
  slug: string;
  name: string;
  /** Short neutral description of what the tool is for. */
  summary: string;
  sources: Source[];
  cells: Record<RowKey, Cell>;
  /** Things the competitor does that Bootable does not (all sourced). */
  theyDoBetter: Cell[];
  /** Where Bootable is the better fit (factual, scoped). */
  bootableFits: string[];
  /** When to pick the competitor. */
  theyFit: string[];
}

export type RowKey =
  | 'platforms'
  | 'interfaces'
  | 'catalog'
  | 'readback'
  | 'sysdisk'
  | 'telemetry'
  | 'license'
  | 'windows'
  | 'bios'
  | 'persistence'
  | 'multiboot'
  | 'signed';

export const rows: { key: RowKey; label: string }[] = [
  { key: 'platforms', label: 'Platforms' },
  { key: 'interfaces', label: 'Interfaces' },
  { key: 'catalog', label: 'Built-in image catalog or downloads' },
  { key: 'readback', label: 'Read-back verification of the write' },
  { key: 'sysdisk', label: 'System-disk protection' },
  { key: 'telemetry', label: 'Telemetry and ads' },
  { key: 'license', label: 'License' },
  { key: 'windows', label: 'Windows installer media options' },
  { key: 'bios', label: 'Legacy BIOS boot' },
  { key: 'persistence', label: 'Persistence' },
  { key: 'multiboot', label: 'Multiboot' },
  { key: 'signed', label: 'Signed builds' },
];

export const bootable: Record<RowKey, Cell> = {
  platforms: { text: 'Linux x86-64, Windows 10+ x86-64, macOS on Apple Silicon.' },
  interfaces: { text: 'Desktop GUI, terminal UI, and CLI. The GUI and TUI expose the same features.' },
  catalog: { text: 'Yes. DistroWatch, Omarchy, and Raspberry Pi catalogs, with publisher checksum verification. Official Microsoft ISO and UEFI Shell downloads are planned, not shipped.' },
  readback: { text: 'Yes for raw images: the written range is read back and compared by SHA-256. Windows installer media gets a boot-tree check instead.' },
  sysdisk: { text: 'Accepts only whole USB or removable drives. Rejects fixed, system, partition, read-only, and undersized targets, and re-finds the drive by stable ID before writing.' },
  telemetry: { text: 'No ads. No telemetry code in the source. Network use is for catalogs and downloads.' },
  license: { text: 'Apache-2.0.' },
  windows: { text: 'GPT or MBR FAT32 UEFI media with split-WIM support on Linux, Windows, and macOS. Setup options: TPM, Secure Boot, and RAM checks; account, region, privacy, BitLocker, quality-of-life, CA 2023, SkuSiPolicy, and S Mode.' },
  bios: { text: 'No. UEFI only today. Legacy BIOS helpers are planned.' },
  persistence: { text: 'No. Planned.' },
  multiboot: { text: 'No. Planned as a separate, explicit strategy.' },
  signed: { text: 'No. Releases are not code-signed and the macOS build is not notarized. SHA-256 files and GitHub provenance are published.' },
};

export const bootableGaps = [
  'No legacy BIOS boot yet (UEFI only).',
  'No Linux persistence and no multiboot.',
  'No general formatting (FAT, NTFS, UDF, exFAT, ext), Windows To Go, or DOS media.',
  'Backup is raw IMG/RAW/DD only; VHD, VHDX, and FFU are planned. Windows RAW backup is unavailable.',
  'Unsigned builds today, and no localization.',
  'Early-stage software: v0.1.1, with only the latest release supported.',
];

const NA = { text: 'Not checked for this page.' };

export const competitors: Record<string, Competitor> = {
  rufus: {
    slug: 'bootable-vs-rufus',
    name: 'Rufus',
    summary: 'Rufus is a long-established Windows utility for creating bootable USB drives.',
    sources: [
      { label: 'Rufus home page (rufus.ie)', url: 'https://rufus.ie/en/' },
      { label: 'Rufus README: feature list and license', url: 'https://github.com/pbatard/rufus/blob/master/README.md' },
      { label: 'Rufus FAQ', url: 'https://github.com/pbatard/rufus/wiki/FAQ' },
    ],
    cells: {
      platforms: { text: 'Windows 8 or later (x64, x86, ARM64 builds). The FAQ says there are no plans to port Rufus to other operating systems.', src: [1, 3] },
      interfaces: { text: 'Graphical interface. CLI and TUI: not checked.', src: [2] },
      catalog: { text: 'Downloads official Microsoft Windows 8, 10, and 11 retail ISOs and UEFI Shell ISOs. No Linux distribution catalog listed.', src: [2] },
      readback: NA,
      sysdisk: NA,
      telemetry: { text: 'The home page states the website shows ads. App telemetry: not stated in the pages checked.', src: [1] },
      license: { text: 'GPL v3 or later.', src: [1, 2] },
      windows: { text: 'Windows 11 media for PCs without TPM or Secure Boot, OOBE setup (local account, privacy options), and Windows To Go.', src: [2] },
      bios: { text: 'Yes. Creates BIOS or UEFI bootable drives.', src: [2] },
      persistence: { text: 'Yes. Creates persistent Linux partitions.', src: [2] },
      multiboot: { text: 'Not listed in its feature list.', src: [2] },
      signed: { text: 'Yes. Binaries are signed with a Windows Authenticode signature.', src: [3] },
    },
    theyDoBetter: [
      { text: 'Formats drives to FAT, FAT32, NTFS, UDF, exFAT, ReFS, and ext2/3.', src: [2] },
      { text: 'Creates DOS (FreeDOS or MS-DOS) bootable drives and Windows To Go drives.', src: [2] },
      { text: 'Creates VHD/DD, VHDX, and FFU images of an existing drive.', src: [2] },
      { text: 'Natively supports 38 languages.', src: [2] },
      { text: 'Authenticode-signed builds.', src: [3] },
    ],
    bootableFits: [
      'You are on Linux or macOS, where Rufus does not run.',
      'You want one tool on Linux, Windows, and macOS with the same interface and behavior.',
      'You prefer a terminal UI or CLI alongside the GUI.',
      'You want a distribution catalog with publisher checksum verification inside the tool.',
    ],
    theyFit: [
      'You are on Windows and need legacy BIOS boot, persistence, formatting, DOS media, or Windows To Go.',
      'You need signed binaries today.',
      'You need a localized interface.',
    ],
  },
  etcher: {
    slug: 'bootable-vs-balenaetcher',
    name: 'balenaEtcher',
    summary: 'balenaEtcher is a cross-platform image flasher with a three-step graphical interface.',
    sources: [
      { label: 'balenaEtcher home page', url: 'https://etcher.balena.io/' },
      { label: 'balenaEtcher README', url: 'https://github.com/balena-io/etcher/blob/master/README.md' },
      { label: 'balenaEtcher repository (license)', url: 'https://github.com/balena-io/etcher' },
      { label: 'balenaEtcher changelog', url: 'https://raw.githubusercontent.com/balena-io/etcher/master/CHANGELOG.md' },
    ],
    cells: {
      platforms: { text: 'Linux, Windows 10 and later, and macOS (Intel and Apple Silicon). Linux and Windows builds are 64-bit Intel per the README.', src: [1, 2] },
      interfaces: { text: 'Graphical interface (Electron). CLI and TUI: not checked.', src: [1] },
      catalog: { text: 'Not described on its site or README.', src: [1, 2] },
      readback: { text: 'Yes. Its site lists "Validated Flashing" to confirm the write completed correctly.', src: [1] },
      sysdisk: { text: 'Yes. Its site says it warns users and hides system drives by default.', src: [1] },
      telemetry: { text: 'The changelog records analytics removal in v2.1.2 (2025-05-08) and earlier opt-out error and usage reporting. We did not test what the current build sends.', src: [4] },
      license: { text: 'Apache-2.0.', src: [3] },
      windows: { text: 'None described; it writes the image you supply.', src: [1, 2] },
      bios: { text: 'Not described.', src: [1, 2] },
      persistence: { text: 'Not described.', src: [1, 2] },
      multiboot: { text: 'Not described.', src: [1, 2] },
      signed: NA,
    },
    theyDoBetter: [
      { text: 'Flashes several drives at once.', src: [1] },
      { text: 'Offers package-manager installs: apt/deb, yum, pacman, WinGet, and Chocolatey.', src: [2] },
      { text: 'Supports Intel Macs as well as Apple Silicon.', src: [2] },
      { text: 'Is a mature, widely used flasher with a deliberately small three-step interface.', src: [1] },
    ],
    bootableFits: [
      'You want a catalog and verified downloads inside the same tool.',
      'You need UEFI Windows installer media with setup options, not only a raw image write.',
      'You want a terminal UI or CLI.',
      'You want a statement that the source contains no telemetry code.',
    ],
    theyFit: [
      'You flash many drives at once.',
      'You run an Intel Mac or want a package-manager install on Windows.',
      'You only need a simple, established image writer.',
    ],
  },
  ventoy: {
    slug: 'bootable-vs-ventoy',
    name: 'Ventoy',
    summary: 'Ventoy takes a different approach: it installs a boot menu once, and you copy ISO files onto the drive.',
    sources: [
      { label: 'Ventoy home page', url: 'https://www.ventoy.net/en/index.html' },
      { label: 'Ventoy quick start', url: 'https://www.ventoy.net/en/doc_start.html' },
      { label: 'Ventoy repository (license)', url: 'https://github.com/ventoy/Ventoy' },
    ],
    cells: {
      platforms: { text: 'Installer documented for Windows (Ventoy2Disk.exe) and Linux (Ventoy2Disk.sh). A macOS installer is not documented in the pages checked. Boots x86 Legacy BIOS, IA32/x86_64/ARM64/MIPS64EL UEFI machines.', src: [1, 2] },
      interfaces: { text: 'Windows installer program and a Linux shell script. Other interfaces: not checked.', src: [2] },
      catalog: { text: 'Not described. You copy image files onto the drive yourself.', src: [1, 2] },
      readback: { text: 'Not described. Ventoy copies files rather than raw-writing an image.', src: [2] },
      sysdisk: NA,
      telemetry: NA,
      license: { text: 'GPL-3.0.', src: [3] },
      windows: { text: 'Boots Windows ISO, WIM, and VHD(x) files directly with no extraction. Windows setup options: not described.', src: [1] },
      bios: { text: 'Yes. Supports Legacy BIOS and UEFI.', src: [1, 2] },
      persistence: { text: 'Yes. Linux persistence is supported.', src: [1, 3] },
      multiboot: { text: 'Yes. Copy several images to the drive and pick one from a boot menu.', src: [1, 2] },
      signed: { text: 'Secure Boot is supported for IA32 and x86_64 UEFI. Signing of the installer itself: not checked.', src: [1] },
    },
    theyDoBetter: [
      { text: 'Carries many images on one drive and boots them from a menu without rewriting the drive.', src: [1, 2] },
      { text: 'Boots in Legacy BIOS and UEFI, including Secure Boot, on several architectures.', src: [1, 2] },
      { text: 'Supports Linux persistence.', src: [1, 3] },
      { text: 'Keeps the drive usable for ordinary files; updates preserve files in the data partition.', src: [1, 2] },
    ],
    bootableFits: [
      'You want a single image written to a drive and verified by read-back, with the drive left as that image.',
      'You want a built-in catalog with publisher checksum verification.',
      'You want a macOS or terminal workflow for creating Windows installer media.',
    ],
    theyFit: [
      'You keep many ISOs on one drive and choose at boot time.',
      'You need legacy BIOS, persistence, or Secure Boot support for the boot menu.',
      'You often swap images and do not want to rewrite the drive each time.',
    ],
  },
  rpi: {
    slug: 'bootable-vs-raspberry-pi-imager',
    name: 'Raspberry Pi Imager',
    summary: 'Raspberry Pi Imager is the Raspberry Pi project’s tool for writing operating systems to storage for Raspberry Pi devices.',
    sources: [
      { label: 'Raspberry Pi software page', url: 'https://www.raspberrypi.com/software/' },
      { label: 'Raspberry Pi documentation: getting started', url: 'https://www.raspberrypi.com/documentation/computers/getting-started.html' },
      { label: 'rpi-imager README (telemetry, install)', url: 'https://github.com/raspberrypi/rpi-imager/blob/main/README.md' },
      { label: 'rpi-imager license file', url: 'https://raw.githubusercontent.com/raspberrypi/rpi-imager/main/license.txt' },
      { label: 'rpi-imager issue #323 ("Verifying write failed")', url: 'https://github.com/raspberrypi/rpi-imager/issues/323' },
    ],
    cells: {
      platforms: { text: 'Windows, macOS, and Linux (x86_64). Also installable on Raspberry Pi OS with apt.', src: [1, 3] },
      interfaces: { text: 'Graphical interface. CLI and TUI: not checked.', src: [2] },
      catalog: { text: 'Yes. Includes many OS images for Raspberry Pi devices and can write a custom image from your computer.', src: [2] },
      readback: { text: 'Yes. It runs a verification step after writing and reports "Verifying write failed" on a mismatch.', src: [5] },
      sysdisk: NA,
      telemetry: { text: 'Collects anonymous usage statistics (selected OS, Imager version, host OS details). Opt out under App Options.', src: [3] },
      license: { text: 'Apache-2.0 for the main code; the license file also lists third-party components such as Qt under LGPL v3.', src: [4] },
      windows: { text: 'Not applicable: it targets Raspberry Pi devices.', src: [3] },
      bios: { text: 'Not described (Raspberry Pi focus).', src: [3] },
      persistence: { text: 'Not described.', src: [2, 3] },
      multiboot: { text: 'Not described.', src: [2, 3] },
      signed: NA,
    },
    theyDoBetter: [
      { text: 'Preconfigures hostname, timezone, user account, Wi-Fi, and SSH before first boot.', src: [2] },
      { text: 'Lets you pick a device model and shows the recommended OS for it at the top of the list.', src: [2] },
      { text: 'Is the Raspberry Pi project’s own tool, with a catalog maintained for its devices.', src: [2, 3] },
    ],
    bootableFits: [
      'You write more than Raspberry Pi images: Linux ISOs, other raw images, or Windows installer media.',
      'You want one tool for USB drives and SD cards across Linux, Windows, and macOS.',
      'You want terminal and CLI interfaces and no telemetry code in the source.',
    ],
    theyFit: [
      'You are setting up a Raspberry Pi and want Wi-Fi, SSH, and user settings preconfigured.',
      'You want the official, device-aware catalog from the Raspberry Pi project.',
    ],
  },
};
