//! Legacy BIOS (CSM) boot sectors for MBR + FAT32 Windows installer media.
//!
//! A Windows stick needs three things to start under legacy BIOS:
//!
//! 1. MBR boot code that chains to the partition flagged active (0x80),
//! 2. a FAT32 volume boot record (PBR) that loads `\BOOTMGR`, and
//! 3. `bootmgr`, `boot\BCD`, and `boot\boot.sdi` on the filesystem.
//!
//! Both boot sectors are original Apache-2.0 code assembled from
//! `assets/bios/*.S`; see `docs/legacy-bios.md` for the licensing decision and
//! the boot contract. Everything here works on any `Read + Write + Seek`, so the
//! same logic is unit-tested on in-memory disks and applied to a raw device.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::error::{Error, Result, io_error};
use crate::windows_media::find_case_insensitive_child;

pub(crate) const SECTOR_SIZE: usize = 512;
/// The MBR bootstrap area; bytes 440..446 hold the disk signature.
pub(crate) const MBR_CODE_LEN: usize = 440;
/// MBR sectors are 32-bit LBAs, so addressable media ends at 2 TiB.
pub(crate) const MAX_MBR_BYTES: u64 = (u32::MAX as u64 + 1) * SECTOR_SIZE as u64;
/// The PBR loads bootmgr below the BIOS data area (0x20000 + 0x7F000 < 0x9FC00).
/// The loader reads whole clusters and refuses, before the read, a cluster that
/// would end past this bound, so the limit applies to `bootmgr`'s size rounded
/// up to the volume's cluster size (see [`bootmgr_footprint`]).
pub(crate) const MAX_BOOTMGR_BYTES: u64 = 0x7_F000;
/// The PBR issues one INT 13h read per cluster; keep it within 32 KiB.
pub(crate) const MAX_SECTORS_PER_CLUSTER: u8 = 64;
/// First 90 bytes of a FAT32 boot sector are jump + BPB; our code starts here.
const PBR_CODE_START: usize = 90;
const FAT32_MIN_CLUSTERS: u64 = 65_525;
const ACTIVE: u8 = 0x80;
const BIOS_DRIVE: u8 = 0x80;

static MBR_CODE: &[u8; MBR_CODE_LEN] = include_bytes!("../assets/bios/mbr.bin");
static FAT32_PBR: &[u8; SECTOR_SIZE] = include_bytes!("../assets/bios/pbr_fat32.bin");

type Sector = [u8; SECTOR_SIZE];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MbrPartition {
    pub(crate) active: bool,
    pub(crate) kind: u8,
    pub(crate) start_lba: u32,
    pub(crate) sectors: u32,
}

impl MbrPartition {
    fn parse(entry: &[u8]) -> Self {
        Self {
            active: entry[0] == ACTIVE,
            kind: entry[4],
            start_lba: le32(entry, 8),
            sectors: le32(entry, 12),
        }
    }

    fn is_empty(entry: &[u8]) -> bool {
        entry.iter().all(|byte| *byte == 0)
    }
}

/// Facts established about the disk before any boot sector is changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DiskLayout {
    /// Index (0..4) of the only partition.
    pub(crate) slot: usize,
    pub(crate) partition: MbrPartition,
    pub(crate) disk_sectors: u64,
}

fn le16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn le32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn invalid(message: impl Into<String>) -> Error {
    Error::UnsupportedImage(format!("legacy BIOS boot: {}", message.into()))
}

fn read_sector<D: Read + Seek>(disk: &mut D, lba: u64) -> Result<Sector> {
    let mut sector = [0_u8; SECTOR_SIZE];
    disk.seek(SeekFrom::Start(lba * SECTOR_SIZE as u64))
        .map_err(|error| io_error("target disk", error))?;
    disk.read_exact(&mut sector)
        .map_err(|error| io_error("target disk", error))?;
    Ok(sector)
}

fn write_sector<D: Write + Seek>(disk: &mut D, lba: u64, sector: &Sector) -> Result<()> {
    disk.seek(SeekFrom::Start(lba * SECTOR_SIZE as u64))
        .map_err(|error| io_error("target disk", error))?;
    disk.write_all(sector)
        .map_err(|error| io_error("target disk", error))
}

fn disk_sectors<D: Seek>(disk: &mut D) -> Result<u64> {
    let bytes = disk
        .seek(SeekFrom::End(0))
        .map_err(|error| io_error("target disk", error))?;
    if bytes > MAX_MBR_BYTES {
        return Err(invalid(
            "MBR cannot address media larger than 2 TiB; use UEFI-only GPT media",
        ));
    }
    Ok(bytes / SECTOR_SIZE as u64)
}

/// Validates an MBR sector and returns the single FAT32 partition it describes.
/// Layouts with several partitions are refused rather than reinterpreted: the
/// Windows writer always creates exactly one.
pub(crate) fn inspect_mbr(mbr: &Sector, disk_sectors: u64) -> Result<DiskLayout> {
    if mbr[510] != 0x55 || mbr[511] != 0xAA {
        return Err(invalid("the MBR has no 0x55AA boot signature"));
    }
    let mut found = None;
    for slot in 0..4 {
        let entry = &mbr[446 + slot * 16..446 + (slot + 1) * 16];
        if MbrPartition::is_empty(entry) {
            continue;
        }
        if found.is_some() {
            return Err(invalid("expected exactly one MBR partition"));
        }
        found = Some((slot, MbrPartition::parse(entry)));
    }
    let (slot, partition) = found.ok_or_else(|| invalid("the MBR has no partition"))?;
    if !matches!(partition.kind, 0x0B | 0x0C) {
        return Err(invalid(format!(
            "partition type 0x{:02X} is not FAT32",
            partition.kind
        )));
    }
    if partition.start_lba == 0 || partition.sectors == 0 {
        return Err(invalid("the partition has an empty or zero-based extent"));
    }
    let end = u64::from(partition.start_lba) + u64::from(partition.sectors);
    if end > disk_sectors {
        return Err(invalid("the partition extends past the end of the disk"));
    }
    Ok(DiskLayout {
        slot,
        partition,
        disk_sectors,
    })
}

/// Properties of a FAT32 boot sector that matter to the boot code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fat32Geometry {
    pub(crate) sectors_per_cluster: u8,
    pub(crate) reserved_sectors: u16,
    pub(crate) backup_boot_sector: u16,
    pub(crate) hidden_sectors: u32,
}

pub(crate) fn inspect_fat32(pbr: &Sector, partition_sectors: u32) -> Result<Fat32Geometry> {
    if pbr[510] != 0x55 || pbr[511] != 0xAA {
        return Err(invalid("the FAT32 boot sector has no 0x55AA signature"));
    }
    if &pbr[0x52..0x5A] != b"FAT32   " {
        return Err(invalid("the partition does not contain a FAT32 filesystem"));
    }
    if le16(pbr, 0x0B) != SECTOR_SIZE as u16 {
        return Err(invalid(
            "only 512-byte logical sectors are supported (4Kn media cannot use the BIOS boot record)",
        ));
    }
    let spc = pbr[0x0D];
    if spc == 0 || !spc.is_power_of_two() || spc > MAX_SECTORS_PER_CLUSTER {
        return Err(invalid(format!(
            "{spc} sectors per cluster is unsupported; clusters must be at most 32 KiB"
        )));
    }
    let reserved = le16(pbr, 0x0E);
    let fats = u64::from(pbr[0x10]);
    let fat_size = u64::from(le32(pbr, 0x24));
    if reserved == 0 || fats == 0 || fat_size == 0 {
        return Err(invalid(
            "the FAT32 BPB has zero reserved sectors, FATs, or FAT size",
        ));
    }
    if le16(pbr, 0x11) != 0 || le16(pbr, 0x13) != 0 || le16(pbr, 0x16) != 0 {
        return Err(invalid("the BPB describes FAT12/16, not FAT32"));
    }
    let total = u64::from(le32(pbr, 0x20));
    if total == 0 || total > u64::from(partition_sectors) {
        return Err(invalid("the filesystem is larger than its partition"));
    }
    if le32(pbr, 0x2C) < 2 {
        return Err(invalid("the FAT32 root cluster is invalid"));
    }
    let overhead = u64::from(reserved) + fats * fat_size;
    let clusters = total.saturating_sub(overhead) / u64::from(spc);
    if clusters < FAT32_MIN_CLUSTERS {
        return Err(invalid(
            "the filesystem has too few clusters to be FAT32 (below 65,525)",
        ));
    }
    let backup = le16(pbr, 0x32);
    // Sector 0 is the boot sector itself and sector 1 is FSInfo, which the
    // installer must never overwrite with a copy of the boot record.
    if backup != 0 && (backup < 2 || backup >= reserved) {
        return Err(invalid(
            "the backup boot sector lies outside the reserved area or on the FSInfo sector",
        ));
    }
    Ok(Fat32Geometry {
        sectors_per_cluster: spc,
        reserved_sectors: reserved,
        backup_boot_sector: backup,
        hidden_sectors: le32(pbr, 0x1C),
    })
}

/// Builds the new volume boot record: our code and jump, the filesystem tool's
/// own BPB (so the volume stays valid), with the fields the BIOS loader needs
/// corrected: hidden sectors = partition start, drive number, and CHS geometry.
pub(crate) fn build_pbr(existing: &Sector, start_lba: u32) -> Sector {
    let mut pbr = *FAT32_PBR;
    pbr[3..PBR_CODE_START].copy_from_slice(&existing[3..PBR_CODE_START]);
    pbr[0x1C..0x20].copy_from_slice(&start_lba.to_le_bytes());
    pbr[0x40] = BIOS_DRIVE;
    if le16(&pbr, 0x18) == 0 {
        pbr[0x18..0x1A].copy_from_slice(&63_u16.to_le_bytes());
    }
    if le16(&pbr, 0x1A) == 0 {
        pbr[0x1A..0x1C].copy_from_slice(&255_u16.to_le_bytes());
    }
    pbr
}

/// Installs the legacy BIOS boot sectors and marks the partition active.
///
/// Order matters for failure safety: the PBR (and its backup copy) are written
/// before the MBR, so the disk only starts chaining to the PBR once it is valid.
/// The caller must flush the underlying device (`File::sync_all`).
pub(crate) fn install<D: Read + Write + Seek>(disk: &mut D) -> Result<DiskLayout> {
    let sectors = disk_sectors(disk)?;
    let mut mbr = read_sector(disk, 0)?;
    let layout = inspect_mbr(&mbr, sectors)?;
    let start = u64::from(layout.partition.start_lba);
    let old_pbr = read_sector(disk, start)?;
    let geometry = inspect_fat32(&old_pbr, layout.partition.sectors)?;
    let pbr = build_pbr(&old_pbr, layout.partition.start_lba);

    write_sector(disk, start, &pbr)?;
    if geometry.backup_boot_sector != 0 {
        write_sector(disk, start + u64::from(geometry.backup_boot_sector), &pbr)?;
    }

    mbr[..MBR_CODE_LEN].copy_from_slice(MBR_CODE);
    for slot in 0..4 {
        mbr[446 + slot * 16] = if slot == layout.slot { ACTIVE } else { 0 };
    }
    write_sector(disk, 0, &mbr)?;
    disk.flush()
        .map_err(|error| io_error("target disk", error))?;
    Ok(layout)
}

/// Re-reads the disk and proves that every byte the BIOS depends on is in place.
pub(crate) fn verify<D: Read + Seek>(disk: &mut D) -> Result<()> {
    let sectors = disk_sectors(disk)?;
    let mbr = read_sector(disk, 0)?;
    let layout = inspect_mbr(&mbr, sectors)?;
    if mbr[..MBR_CODE_LEN] != MBR_CODE[..] {
        return Err(invalid("the MBR bootstrap code was not written correctly"));
    }
    if !layout.partition.active {
        return Err(invalid("the FAT32 partition is not flagged active"));
    }
    let start = u64::from(layout.partition.start_lba);
    let pbr = read_sector(disk, start)?;
    let geometry = inspect_fat32(&pbr, layout.partition.sectors)?;
    if pbr[..3] != FAT32_PBR[..3] || pbr[PBR_CODE_START..] != FAT32_PBR[PBR_CODE_START..] {
        return Err(invalid(
            "the FAT32 boot record code was not written correctly",
        ));
    }
    if geometry.hidden_sectors != layout.partition.start_lba {
        return Err(invalid(
            "BPB hidden sectors do not match the partition start; the volume boot record would read the wrong sectors",
        ));
    }
    if pbr[0x40] != BIOS_DRIVE {
        return Err(invalid("the BPB boot drive number is not 0x80"));
    }
    if geometry.backup_boot_sector != 0 {
        let backup = read_sector(disk, start + u64::from(geometry.backup_boot_sector))?;
        if backup != pbr {
            return Err(invalid(
                "the backup boot sector differs from the primary one",
            ));
        }
    }
    Ok(())
}

/// Bytes of memory the boot sector fills when it loads a file of `len` bytes
/// from a volume with `cluster_bytes` clusters: whole clusters are read.
pub(crate) fn bootmgr_footprint(len: u64, cluster_bytes: u64) -> u64 {
    let cluster = cluster_bytes.max(SECTOR_SIZE as u64);
    len.div_ceil(cluster) * cluster
}

/// Cluster size in bytes recorded in a FAT32 boot sector the loader accepts.
pub(crate) fn cluster_bytes(pbr: &Sector) -> Result<u64> {
    let spc = pbr[0x0D];
    if le16(pbr, 0x0B) != SECTOR_SIZE as u16
        || spc == 0
        || !spc.is_power_of_two()
        || spc > MAX_SECTORS_PER_CLUSTER
    {
        return Err(invalid(
            "the FAT32 boot sector has a sector or cluster size the BIOS loader cannot use",
        ));
    }
    Ok(u64::from(spc) * SECTOR_SIZE as u64)
}

/// The cluster size `mkfs.fat -F 32` is expected to pick for a partition on a
/// `capacity`-byte device (dosfstools' size table). Only used to refuse an
/// unbootable result before erasure; the exact size is re-read from the new
/// filesystem and checked again before any boot sector is installed.
pub(crate) fn expected_cluster_bytes(capacity: u64) -> u64 {
    const GIB: u64 = 1 << 30;
    match capacity {
        c if c > 32 * GIB => 32 * 1024,
        c if c > 16 * GIB => 16 * 1024,
        c if c > 8 * GIB => 8 * 1024,
        _ => 4 * 1024,
    }
}

/// Checks that a mounted Windows tree can be started by `bootmgr` under BIOS
/// when copied to a volume with `cluster_bytes` clusters. Used on the source
/// before any erasure and on the written media afterwards.
pub(crate) fn preflight_tree(root: &Path, cluster_bytes: u64) -> Result<()> {
    let bootmgr = find_case_insensitive_child(root, "bootmgr").map_err(|_| {
        invalid(
            "the installer has no `bootmgr` file; ARM64 and UEFI-only images cannot boot under BIOS",
        )
    })?;
    let metadata = bootmgr
        .metadata()
        .map_err(|error| io_error(&bootmgr, error))?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(invalid("`bootmgr` is not a non-empty regular file"));
    }
    let footprint = bootmgr_footprint(metadata.len(), cluster_bytes);
    if footprint > MAX_BOOTMGR_BYTES {
        return Err(invalid(format!(
            "`bootmgr` is {} bytes ({footprint} once rounded up to whole {cluster_bytes}-byte clusters); the real-mode loader accepts at most {MAX_BOOTMGR_BYTES}",
            metadata.len()
        )));
    }
    let boot = find_case_insensitive_child(root, "boot")
        .map_err(|_| invalid("the installer has no `boot` directory"))?;
    for name in ["bcd", "boot.sdi"] {
        find_case_insensitive_child(&boot, name)
            .map_err(|_| invalid(format!("the installer has no `boot/{name}`")))?;
    }
    Ok(())
}

/// Total-size and geometry limits that can be decided from the plan alone.
pub(crate) fn validate_capacity(capacity: u64) -> Result<()> {
    if capacity > MAX_MBR_BYTES {
        return Err(invalid(
            "MBR cannot address media larger than 2 TiB; choose UEFI-only GPT media",
        ));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    pub(crate) const START_LBA: u32 = 2048;

    /// CHS encoding used by partition tables (clamped to the 1023/254/63 limit).
    pub(crate) fn chs(lba: u32) -> [u8; 3] {
        let cylinder = lba / (255 * 63);
        if cylinder > 1023 {
            return [0xFE, 0xFF, 0xFF];
        }
        let head = (lba / 63) % 255;
        let sector = lba % 63 + 1;
        [
            head as u8,
            (sector as u8) | ((cylinder >> 2) as u8 & 0xC0),
            cylinder as u8,
        ]
    }

    pub(crate) fn partition_entry(
        active: bool,
        kind: u8,
        start_lba: u32,
        sectors: u32,
    ) -> [u8; 16] {
        let mut entry = [0_u8; 16];
        entry[0] = if active { ACTIVE } else { 0 };
        entry[1..4].copy_from_slice(&chs(start_lba));
        entry[4] = kind;
        entry[5..8].copy_from_slice(&chs(start_lba + sectors - 1));
        entry[8..12].copy_from_slice(&start_lba.to_le_bytes());
        entry[12..16].copy_from_slice(&sectors.to_le_bytes());
        entry
    }

    /// A boot sector shaped like `mkfs.fat -F 32` output, with deliberately
    /// wrong hidden sectors and foreign boot code to prove they are replaced.
    pub(crate) fn mkfs_fat_boot_sector(total: u32, spc: u8) -> Sector {
        let mut pbr = [0_u8; SECTOR_SIZE];
        pbr[..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
        pbr[3..11].copy_from_slice(b"mkfs.fat");
        pbr[0x0B..0x0D].copy_from_slice(&512_u16.to_le_bytes());
        pbr[0x0D] = spc;
        pbr[0x0E..0x10].copy_from_slice(&32_u16.to_le_bytes());
        pbr[0x10] = 2;
        pbr[0x15] = 0xF8;
        pbr[0x18..0x1A].copy_from_slice(&32_u16.to_le_bytes());
        pbr[0x1A..0x1C].copy_from_slice(&64_u16.to_le_bytes());
        pbr[0x1C..0x20].copy_from_slice(&7_u32.to_le_bytes());
        pbr[0x20..0x24].copy_from_slice(&total.to_le_bytes());
        let clusters = total / u32::from(spc);
        let fat_sectors = clusters.div_ceil(128) + 1;
        pbr[0x24..0x28].copy_from_slice(&fat_sectors.to_le_bytes());
        pbr[0x2C..0x30].copy_from_slice(&2_u32.to_le_bytes());
        pbr[0x30..0x32].copy_from_slice(&1_u16.to_le_bytes());
        pbr[0x32..0x34].copy_from_slice(&6_u16.to_le_bytes());
        pbr[0x40] = 0x80;
        pbr[0x42] = 0x29;
        pbr[0x43..0x47].copy_from_slice(&0xCAFE_F00D_u32.to_le_bytes());
        pbr[0x47..0x52].copy_from_slice(b"WINDOWS    ");
        pbr[0x52..0x5A].copy_from_slice(b"FAT32   ");
        pbr[0x5A..0x5A + 29].copy_from_slice(b"This is not a bootable disk\r\n");
        pbr[510] = 0x55;
        pbr[511] = 0xAA;
        pbr
    }

    pub(crate) fn blank_mbr(entries: &[[u8; 16]]) -> Sector {
        let mut mbr = [0_u8; SECTOR_SIZE];
        mbr[..27].copy_from_slice(b"parted-or-foreign-boot-code");
        mbr[440..444].copy_from_slice(&0x1234_5678_u32.to_le_bytes());
        for (slot, entry) in entries.iter().enumerate() {
            mbr[446 + slot * 16..446 + (slot + 1) * 16].copy_from_slice(entry);
        }
        mbr[510] = 0x55;
        mbr[511] = 0xAA;
        mbr
    }

    /// 64 MiB disk with one inactive FAT32 partition at 1 MiB.
    pub(crate) fn disk(active: bool) -> Vec<u8> {
        let mut image = vec![0_u8; 64 * 1024 * 1024];
        let sectors = (image.len() / SECTOR_SIZE) as u32 - START_LBA;
        let mbr = blank_mbr(&[partition_entry(active, 0x0C, START_LBA, sectors)]);
        image[..SECTOR_SIZE].copy_from_slice(&mbr);
        let start = START_LBA as usize * SECTOR_SIZE;
        image[start..start + SECTOR_SIZE].copy_from_slice(&mkfs_fat_boot_sector(sectors, 1));
        image
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use std::fs;
    use std::io::Cursor;

    fn sector(image: &[u8], lba: usize) -> Sector {
        let mut sector = [0_u8; SECTOR_SIZE];
        sector.copy_from_slice(&image[lba * SECTOR_SIZE..(lba + 1) * SECTOR_SIZE]);
        sector
    }

    #[test]
    fn embedded_boot_code_has_the_documented_layout() {
        assert_eq!(MBR_CODE.len(), 440);
        assert_eq!(FAT32_PBR.len(), 512);
        assert_eq!(&FAT32_PBR[..3], &[0xEB, 0x58, 0x90]);
        assert_eq!(&FAT32_PBR[510..], &[0x55, 0xAA]);
        // The jump must land exactly on the first code byte after the BPB.
        assert_eq!(2 + usize::from(FAT32_PBR[1]), PBR_CODE_START);
        // Both stubs begin with `cli` so they own the interrupt state.
        assert_eq!(MBR_CODE[0], 0xFA);
        assert_eq!(FAT32_PBR[PBR_CODE_START], 0xFA);
        // The BPB template area is empty: only the installer supplies it.
        assert!(FAT32_PBR[3..PBR_CODE_START].iter().all(|byte| *byte == 0));
        assert!(MBR_CODE.iter().any(|byte| *byte != 0));
    }

    #[test]
    fn install_sets_mbr_code_active_flag_signature_and_preserves_the_table() {
        let mut cursor = Cursor::new(disk(false));
        let layout = install(&mut cursor).expect("install");
        assert_eq!(layout.partition.start_lba, START_LBA);
        let image = cursor.into_inner();
        let mbr = sector(&image, 0);

        assert_eq!(&mbr[..440], &MBR_CODE[..]);
        assert_eq!(
            &mbr[440..444],
            &0x1234_5678_u32.to_le_bytes(),
            "disk signature"
        );
        assert_eq!(&mbr[510..], &[0x55, 0xAA]);
        assert_eq!(mbr[446], 0x80, "active flag");
        assert_eq!(
            &mbr[446 + 16..446 + 64],
            &[0_u8; 48][..],
            "other slots stay empty"
        );
        let entry = MbrPartition::parse(&mbr[446..462]);
        assert_eq!(entry.kind, 0x0C);
        assert_eq!(entry.start_lba, START_LBA);
        assert_eq!(entry.sectors, (64 * 1024 * 1024 / 512) - START_LBA);
    }

    #[test]
    fn install_writes_pbr_with_corrected_bpb_fields_and_backup() {
        let mut cursor = Cursor::new(disk(false));
        install(&mut cursor).expect("install");
        let image = cursor.into_inner();
        let original = mkfs_fat_boot_sector((64 * 1024 * 1024 / 512) - START_LBA, 1);
        let pbr = sector(&image, START_LBA as usize);

        assert_eq!(&pbr[..3], &[0xEB, 0x58, 0x90]);
        assert_eq!(&pbr[510..], &[0x55, 0xAA]);
        assert_eq!(&pbr[PBR_CODE_START..], &FAT32_PBR[PBR_CODE_START..]);
        assert_eq!(le32(&pbr, 0x1C), START_LBA, "BPB hidden sectors");
        assert_eq!(pbr[0x40], 0x80, "BS_DrvNum");
        // Everything else in the BPB is preserved verbatim.
        assert_eq!(&pbr[3..0x1C], &original[3..0x1C]);
        assert_eq!(&pbr[0x20..0x40], &original[0x20..0x40]);
        assert_eq!(&pbr[0x41..PBR_CODE_START], &original[0x41..PBR_CODE_START]);
        assert_eq!(le16(&pbr, 0x18), 32, "existing CHS geometry is kept");
        assert_eq!(sector(&image, START_LBA as usize + 6), pbr, "backup copy");
        assert!(!String::from_utf8_lossy(&pbr).contains("not a bootable"));
    }

    #[test]
    fn zero_chs_geometry_gets_conventional_defaults() {
        let mut existing = mkfs_fat_boot_sector(200_000, 1);
        existing[0x18..0x1C].fill(0);
        let pbr = build_pbr(&existing, 8192);
        assert_eq!(le16(&pbr, 0x18), 63);
        assert_eq!(le16(&pbr, 0x1A), 255);
        assert_eq!(le32(&pbr, 0x1C), 8192);
    }

    #[test]
    fn verify_accepts_installed_media_and_catches_each_defect() {
        let mut cursor = Cursor::new(disk(false));
        install(&mut cursor).expect("install");
        verify(&mut cursor).expect("fresh install verifies");
        let good = cursor.into_inner();

        let corrupt = |offset: usize, value: u8| {
            let mut image = good.clone();
            image[offset] = value;
            verify(&mut Cursor::new(image)).expect_err("defect must be detected");
        };
        corrupt(446, 0x00); // active flag cleared
        corrupt(0, 0x90); // MBR code
        corrupt(510, 0x00); // MBR signature
        let pbr = START_LBA as usize * SECTOR_SIZE;
        corrupt(pbr + 0x1C, 0x01); // hidden sectors
        corrupt(pbr + 0x40, 0x00); // drive number
        corrupt(pbr + 200, 0xCC); // PBR code
        corrupt(pbr + 6 * SECTOR_SIZE + 0x0D, 0x02); // backup differs
    }

    #[test]
    fn install_is_idempotent() {
        let mut cursor = Cursor::new(disk(true));
        install(&mut cursor).expect("first");
        let first = cursor.get_ref().clone();
        install(&mut cursor).expect("second");
        assert_eq!(&first, cursor.get_ref());
    }

    #[test]
    fn mbr_validation_refuses_unsafe_layouts() {
        let sectors = 131_072_u64;
        let blank = |entries: &[[u8; 16]]| blank_mbr(entries);
        let good = partition_entry(false, 0x0C, 2048, 100_000);

        // Two partitions: never reinterpret a layout we did not create.
        assert!(
            inspect_mbr(
                &blank(&[good, partition_entry(false, 0x07, 110_000, 10_000)]),
                sectors
            )
            .is_err()
        );
        // No partitions.
        assert!(inspect_mbr(&blank(&[]), sectors).is_err());
        // NTFS/other type.
        assert!(
            inspect_mbr(
                &blank(&[partition_entry(false, 0x07, 2048, 100_000)]),
                sectors
            )
            .is_err()
        );
        // Extends past the disk.
        assert!(
            inspect_mbr(
                &blank(&[partition_entry(false, 0x0C, 2048, 200_000)]),
                sectors
            )
            .is_err()
        );
        // Starts at LBA 0 (would overwrite the MBR).
        assert!(inspect_mbr(&blank(&[partition_entry(false, 0x0C, 0, 100)]), sectors).is_err());
        // Missing signature.
        let mut unsigned = blank(&[good]);
        unsigned[511] = 0;
        assert!(inspect_mbr(&unsigned, sectors).is_err());
        assert!(inspect_mbr(&blank(&[good]), sectors).is_ok());
    }

    #[test]
    fn fat32_validation_refuses_what_the_loader_cannot_read() {
        let partition = 131_072_u32;
        let good = mkfs_fat_boot_sector(partition, 1);
        inspect_fat32(&good, partition).expect("valid");

        let mutate = |edit: &dyn Fn(&mut Sector)| {
            let mut sector = good;
            edit(&mut sector);
            inspect_fat32(&sector, partition).expect_err("must be refused");
        };
        mutate(&|s| s[0x0B..0x0D].copy_from_slice(&4096_u16.to_le_bytes())); // 4Kn
        mutate(&|s| s[0x0D] = 128); // 64 KiB clusters
        mutate(&|s| s[0x0D] = 3); // not a power of two
        mutate(&|s| s[0x52..0x5A].copy_from_slice(b"NTFS    "));
        mutate(&|s| s[0x16..0x18].copy_from_slice(&9_u16.to_le_bytes())); // FAT16 size
        mutate(&|s| s[0x2C..0x30].copy_from_slice(&0_u32.to_le_bytes()));
        mutate(&|s| s[0x32..0x34].copy_from_slice(&40_u16.to_le_bytes()));
        mutate(&|s| s[510] = 0);
        // Filesystem claims more sectors than the partition holds.
        assert!(inspect_fat32(&good, partition - 1).is_err());
        // Too few clusters to be FAT32 at all.
        assert!(inspect_fat32(&mkfs_fat_boot_sector(40_000, 1), 40_000).is_err());
        // A 32 KiB cluster is the largest the loader accepts.
        inspect_fat32(&mkfs_fat_boot_sector(4_300_000, 64), 4_300_000).expect("32 KiB clusters");
    }

    #[test]
    fn install_refuses_media_beyond_the_mbr_limit_and_leaves_foreign_disks_alone() {
        // A disk with an NTFS partition must not be modified at all.
        let mut image = disk(false);
        image[446 + 4] = 0x07;
        let before = image.clone();
        let mut cursor = Cursor::new(image);
        assert!(install(&mut cursor).is_err());
        assert_eq!(cursor.into_inner(), before);

        assert!(validate_capacity(MAX_MBR_BYTES).is_ok());
        assert!(validate_capacity(MAX_MBR_BYTES + 1).is_err());
    }

    #[test]
    fn partition_entry_encoding_matches_the_mbr_specification() {
        let entry = partition_entry(true, 0x0C, 2048, 1000);
        assert_eq!(entry[0], 0x80);
        assert_eq!(entry[4], 0x0C);
        assert_eq!(&entry[8..12], &2048_u32.to_le_bytes());
        assert_eq!(&entry[12..16], &1000_u32.to_le_bytes());
        // LBA 2048 -> cylinder 0, head 32, sector 33 (1-based).
        assert_eq!(&entry[1..4], &[32, 33, 0]);
        assert_eq!(chs(u32::MAX), [0xFE, 0xFF, 0xFF], "beyond CHS range");
    }

    #[test]
    fn tree_preflight_requires_bootmgr_bcd_and_boot_sdi_within_limits() {
        let root = tempfile::tempdir().expect("fixture");
        assert!(preflight_tree(root.path(), 512).is_err(), "empty tree");
        fs::create_dir_all(root.path().join("Boot")).expect("boot dir");
        fs::write(root.path().join("BOOTMGR"), vec![1_u8; 4096]).expect("bootmgr");
        assert!(preflight_tree(root.path(), 512).is_err(), "no BCD");
        fs::write(root.path().join("Boot/BCD"), b"bcd").expect("bcd");
        assert!(preflight_tree(root.path(), 512).is_err(), "no boot.sdi");
        fs::write(root.path().join("Boot/boot.sdi"), b"sdi").expect("sdi");
        preflight_tree(root.path(), 512).expect("complete BIOS tree");

        fs::write(
            root.path().join("BOOTMGR"),
            vec![1_u8; MAX_BOOTMGR_BYTES as usize + 1],
        )
        .expect("oversized bootmgr");
        assert!(
            preflight_tree(root.path(), 512).is_err(),
            "bootmgr too large"
        );
        fs::write(root.path().join("BOOTMGR"), b"").expect("empty bootmgr");
        assert!(preflight_tree(root.path(), 512).is_err(), "empty bootmgr");
    }

    #[test]
    fn bootmgr_limit_applies_to_the_cluster_rounded_size_the_loader_reads() {
        assert_eq!(bootmgr_footprint(1, 512), 512);
        assert_eq!(bootmgr_footprint(512, 512), 512);
        assert_eq!(bootmgr_footprint(513, 4096), 4096);
        assert_eq!(bootmgr_footprint(0x7_E001, 32 * 1024), 0x8_0000);

        let root = tempfile::tempdir().expect("fixture");
        fs::create_dir_all(root.path().join("boot")).expect("boot dir");
        fs::write(root.path().join("boot/bcd"), b"bcd").expect("bcd");
        fs::write(root.path().join("boot/boot.sdi"), b"sdi").expect("sdi");
        // Fits the byte limit but not once rounded up to 32 KiB clusters.
        let size = MAX_BOOTMGR_BYTES as usize - 100;
        fs::write(root.path().join("bootmgr"), vec![1_u8; size]).expect("bootmgr");
        preflight_tree(root.path(), 512).expect("512-byte clusters round to the limit");
        let error = preflight_tree(root.path(), 32 * 1024).expect_err("32 KiB clusters overflow");
        assert!(error.to_string().contains("once rounded up"), "{error}");
        // Exactly the limit is allowed; one byte more rounds past it.
        fs::write(
            root.path().join("bootmgr"),
            vec![1_u8; MAX_BOOTMGR_BYTES as usize],
        )
        .expect("bootmgr at the limit");
        preflight_tree(root.path(), 4096).expect("at the limit");
        fs::write(
            root.path().join("bootmgr"),
            vec![1_u8; MAX_BOOTMGR_BYTES as usize + 1],
        )
        .expect("bootmgr over the limit");
        preflight_tree(root.path(), 512).expect_err("one byte over");
    }

    #[test]
    fn cluster_size_is_read_from_the_boot_sector_and_unusable_ones_are_refused() {
        let pbr = mkfs_fat_boot_sector(4_300_000, 64);
        assert_eq!(cluster_bytes(&pbr).expect("32 KiB"), 32 * 1024);
        let mut bad = pbr;
        bad[0x0D] = 128;
        cluster_bytes(&bad).expect_err("64 KiB clusters");
        bad[0x0D] = 0;
        cluster_bytes(&bad).expect_err("zero");
        assert!(expected_cluster_bytes(2 << 30) <= expected_cluster_bytes(64 << 30));
        assert_eq!(expected_cluster_bytes(64 << 30), 32 * 1024);
    }

    #[test]
    fn backup_boot_sector_must_not_sit_on_the_fsinfo_sector() {
        let partition = 131_072_u32;
        let mut sector = mkfs_fat_boot_sector(partition, 1);
        for backup in [1_u16, 32, 40] {
            sector[0x32..0x34].copy_from_slice(&backup.to_le_bytes());
            inspect_fat32(&sector, partition).expect_err("backup sector 1 / outside reserved");
        }
        for backup in [0_u16, 2, 6, 31] {
            sector[0x32..0x34].copy_from_slice(&backup.to_le_bytes());
            inspect_fat32(&sector, partition).expect("valid backup position");
        }
    }

    /// Applies the installer to a disk image file; used by
    /// `scripts/qemu-usb-bios-smoke.sh`, which builds the image without root.
    #[test]
    #[ignore = "driven by scripts/qemu-usb-bios-smoke.sh with BOOTABLE_BIOS_IMAGE"]
    fn install_boot_sectors_on_image_file() {
        let path = std::env::var("BOOTABLE_BIOS_IMAGE").expect("BOOTABLE_BIOS_IMAGE");
        let mut file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("open image");
        install(&mut file).expect("install");
        file.sync_all().expect("sync");
        verify(&mut file).expect("verify");
    }
}
