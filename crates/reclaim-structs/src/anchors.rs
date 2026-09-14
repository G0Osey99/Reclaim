//! Filesystem anchor definitions + validators for the lost-structure sweep
//! (docs/plan/04 §2). Each anchor is a magic byte-string at a fixed offset
//! inside a volume; when the sweep finds it, the validator reads the surrounding
//! superblock, sanity-checks the geometry and returns a [`ProposedVolume`] with
//! the volume length, a confidence and human-readable evidence.

use crate::{be_u16, be_u32, be_u64, le_u16, le_u32, le_u64, read, ProposedVolume};
use reclaim_block::BlockSource;
use std::sync::Arc;

type Validate = fn(&Arc<dyn BlockSource>, u64) -> Vec<ProposedVolume>;

/// One filesystem anchor: a magic string at `internal_offset` from volume start.
#[derive(Clone)]
pub struct FsAnchor {
    /// The magic bytes searched for in the sequential sweep.
    pub needle: &'static [u8],
    /// The magic's byte offset from the volume start.
    pub internal_offset: u64,
    /// Reads + validates the candidate volume; returns a proposal or `None`.
    pub validate: Validate,
}

impl std::fmt::Debug for FsAnchor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FsAnchor")
            .field("needle", &String::from_utf8_lossy(self.needle))
            .field("internal_offset", &self.internal_offset)
            .finish()
    }
}

/// The full anchor set.
pub fn all() -> &'static [FsAnchor] {
    &ANCHORS
}

fn plausible_len(src: &Arc<dyn BlockSource>, start: u64, len: u64) -> bool {
    // A volume must be non-empty and not extend absurdly past the source. Allow
    // 2× slack for sparse/truncated images.
    len > 0 && start.saturating_add(len) <= src.len().saturating_mul(2).max(len)
}

fn pow2(v: u64) -> bool {
    (512..=1 << 26).contains(&v) && v.is_power_of_two()
}

/// One proposal as a single-element vec (the common case).
fn pv(start: u64, len: u64, fs: &str, conf: f32, ev: String) -> Vec<ProposedVolume> {
    vec![ProposedVolume {
        start,
        len,
        fs: fs.to_string(),
        confidence: conf,
        evidence: ev,
    }]
}

/// One proposal value (for pushing into a multi-candidate vec).
fn one(start: u64, len: u64, fs: &str, conf: f32, ev: String) -> ProposedVolume {
    ProposedVolume {
        start,
        len,
        fs: fs.to_string(),
        confidence: conf,
        evidence: ev,
    }
}

// ---- validators ----

fn apfs_nxsb(src: &Arc<dyn BlockSource>, start: u64) -> Vec<ProposedVolume> {
    let b = read(src, start, 4096);
    if b.get(32..36) != Some(b"NXSB") {
        return Vec::new();
    }
    let block_size = u64::from(le_u32(&b, 36));
    let block_count = le_u64(&b, 40);
    if !pow2(block_size) {
        return Vec::new();
    }
    let len = block_count.saturating_mul(block_size);
    if !plausible_len(src, start, len) {
        return Vec::new();
    }
    pv(
        start,
        len,
        "apfs",
        0.9,
        format!("APFS NXSB container: block_size={block_size}, blocks={block_count}"),
    )
}

fn apfs_apsb(src: &Arc<dyn BlockSource>, start: u64) -> Vec<ProposedVolume> {
    let b = read(src, start, 512);
    if b.get(32..36) != Some(b"APSB") {
        return Vec::new();
    }
    // A volume superblock inside a container: not independently adoptable, but
    // corroborating. len 0 ⇒ the CLI steers adoption to the NXSB container.
    pv(
        start,
        0,
        "apfs-volume",
        0.5,
        "APFS APSB volume superblock (adopt the enclosing NXSB container)".to_string(),
    )
}

fn hfsplus(src: &Arc<dyn BlockSource>, start: u64) -> Vec<ProposedVolume> {
    // The magic anchor puts a volume header at `vh_off`. It is either the MAIN
    // VH (at volume+1024) or the ALTERNATE VH (at volume+len-1024). We compute
    // the volume start under both hypotheses so a zeroed main VH is still
    // located from the surviving alternate at the end of the volume.
    let vh_off = start + 1024;
    let vh = read(src, vh_off, 512);
    let Some(sig) = vh.get(0..2) else {
        return Vec::new();
    };
    if sig != b"H+" && sig != b"HX" {
        return Vec::new();
    }
    let version = be_u16(&vh, 2);
    if version != 4 && version != 5 {
        return Vec::new();
    }
    let block_size = u64::from(be_u32(&vh, 40));
    let total_blocks = u64::from(be_u32(&vh, 44));
    if !pow2(block_size) || total_blocks == 0 {
        return Vec::new();
    }
    let len = total_blocks.saturating_mul(block_size);
    let kind = if sig == b"HX" { "hfsx" } else { "hfsplus" };
    let matches_vh = |off: u64| -> bool {
        let o = read(src, off, 512);
        o.get(0..2) == Some(sig) && u64::from(be_u32(&o, 44)) == total_blocks
    };

    // Hypothesis MAIN: this VH is the primary at volume+1024.
    let main_start = vh_off - 1024;
    let main_alt_loc = main_start.saturating_add(len).saturating_sub(1024);
    let main_confirmed = plausible_len(src, main_start, len) && matches_vh(main_alt_loc);

    // Hypothesis ALT: this VH is the alternate at volume+len-1024 (main zeroed).
    let alt_start = (vh_off + 1024).checked_sub(len);
    let alt_confirmed = alt_start
        .map(|s| plausible_len(src, s, len) && matches_vh(s + 1024))
        .unwrap_or(false);

    // A confirmed hypothesis (the partner VH is present) wins outright.
    if main_confirmed && !alt_confirmed {
        return pv(main_start, len, kind, 0.95,
            format!("HFS+ volume header: block_size={block_size}, blocks={total_blocks}, alt VH confirms"));
    }
    if alt_confirmed && !main_confirmed {
        if let Some(s) = alt_start {
            return pv(s, len, kind, 0.95,
                format!("HFS+ via alternate volume header: block_size={block_size}, blocks={total_blocks}"));
        }
    }

    // Neither partner present (its VH was zeroed) — propose both plausible
    // placements; the scan / cross-validation resolves which is real.
    let mut out = Vec::new();
    if plausible_len(src, main_start, len) {
        out.push(one(main_start, len, kind, 0.7,
            format!("HFS+ volume header (unconfirmed as main): block_size={block_size}, blocks={total_blocks}")));
    }
    if let Some(s) = alt_start {
        if s != main_start && plausible_len(src, s, len) {
            out.push(one(s, len, kind, 0.72,
                format!("HFS+ alternate volume header (main VH zeroed): block_size={block_size}, blocks={total_blocks}")));
        }
    }
    out
}

fn ntfs(src: &Arc<dyn BlockSource>, start: u64) -> Vec<ProposedVolume> {
    let b = read(src, start, 512);
    if b.get(3..11) != Some(b"NTFS    ") {
        return Vec::new();
    }
    if le_u16(&b, 510) != 0xAA55 {
        return Vec::new();
    }
    let bps = u64::from(le_u16(&b, 11));
    if !pow2(bps) {
        return Vec::new();
    }
    let total_sectors = le_u64(&b, 40);
    let len = total_sectors.saturating_add(1).saturating_mul(bps);
    if !plausible_len(src, start, len) {
        return Vec::new();
    }
    pv(
        start,
        len,
        "ntfs",
        0.92,
        format!("NTFS boot sector: bytes/sector={bps}, sectors={total_sectors}"),
    )
}

fn exfat(src: &Arc<dyn BlockSource>, start: u64) -> Vec<ProposedVolume> {
    let b = read(src, start, 512);
    if b.get(3..11) != Some(b"EXFAT   ") {
        return Vec::new();
    }
    let bps_shift = b.get(108).copied().unwrap_or(0);
    if !(9..=12).contains(&bps_shift) {
        return Vec::new();
    }
    let volume_length = le_u64(&b, 72);
    if volume_length > (u64::MAX >> bps_shift) {
        return Vec::new(); // the shift would drop high bits
    }
    let len = volume_length << bps_shift;
    if !plausible_len(src, start, len) {
        return Vec::new();
    }
    // exFAT keeps a backup boot region 12 logical sectors after the main VBR.
    // Decide whether this VBR is the main or the backup so a zeroed main boot
    // still yields the true volume start (docs/plan/04 §3.4).
    let bps = 1u64 << bps_shift;
    let is_exfat_at = |off: u64| -> bool {
        off < src.len() && read(src, off, 512).get(3..11) == Some(b"EXFAT   ")
    };
    if is_exfat_at(start + 12 * bps) {
        // The backup follows → this is the main VBR.
        return pv(
            start,
            len,
            "exfat",
            0.95,
            format!("exFAT main boot sector: volume_length={volume_length} sectors"),
        );
    }
    if start >= 12 * bps && is_exfat_at(start - 12 * bps) {
        // A main VBR precedes → this is the backup; propose the main start.
        return pv(
            start - 12 * bps,
            len,
            "exfat",
            0.9,
            format!("exFAT (via backup boot): volume_length={volume_length} sectors"),
        );
    }
    // Lone VBR: could be a main with a wiped backup, or a backup whose main was
    // zeroed. Propose both plausible starts; scan/cross-validation resolves it.
    let mut out = vec![one(
        start,
        len,
        "exfat",
        0.65,
        format!("exFAT boot sector (unconfirmed): volume_length={volume_length} sectors"),
    )];
    if start >= 12 * bps {
        out.push(one(
            start - 12 * bps,
            len,
            "exfat",
            0.7,
            format!("exFAT (backup boot survived, main zeroed): volume_length={volume_length}"),
        ));
    }
    out
}

fn fat(src: &Arc<dyn BlockSource>, start: u64) -> Vec<ProposedVolume> {
    let b = read(src, start, 512);
    if le_u16(&b, 510) != 0xAA55 {
        return Vec::new();
    }
    let is_fat32 = b.get(82..90) == Some(b"FAT32   ");
    let is_fat16 = b.get(54..62) == Some(b"FAT16   ");
    let is_fat12 = b.get(54..62) == Some(b"FAT12   ");
    if !(is_fat32 || is_fat16 || is_fat12) {
        return Vec::new();
    }
    let bps = u64::from(le_u16(&b, 11));
    if !pow2(bps) {
        return Vec::new();
    }
    let ts16 = u64::from(le_u16(&b, 19));
    let ts32 = u64::from(le_u32(&b, 32));
    let sectors = if ts16 != 0 { ts16 } else { ts32 };
    let len = sectors.saturating_mul(bps);
    if !plausible_len(src, start, len) {
        return Vec::new();
    }
    let kind = if is_fat32 { "fat32" } else { "fat" };
    pv(
        start,
        len,
        kind,
        0.85,
        format!("FAT BPB ({kind}): bytes/sector={bps}, sectors={sectors}"),
    )
}

fn ext(src: &Arc<dyn BlockSource>, start: u64) -> Vec<ProposedVolume> {
    let sb = read(src, start + 1024, 1024);
    if le_u16(&sb, 0x38) != 0xEF53 {
        return Vec::new();
    }
    let log_bs = le_u32(&sb, 0x18);
    if log_bs > 6 {
        return Vec::new();
    }
    let block_size = 1024u64 << log_bs;
    let blocks_lo = u64::from(le_u32(&sb, 0x04));
    let incompat = le_u32(&sb, 0x60);
    let blocks = if incompat & 0x80 != 0 {
        (u64::from(le_u32(&sb, 0x150)) << 32) | blocks_lo
    } else {
        blocks_lo
    };
    if blocks == 0 || le_u32(&sb, 0x20) == 0 {
        return Vec::new(); // blocks_per_group 0 ⇒ bogus
    }
    let len = blocks.saturating_mul(block_size);
    if !plausible_len(src, start, len) {
        return Vec::new();
    }
    pv(
        start,
        len,
        "ext",
        0.9,
        format!("ext superblock: block_size={block_size}, blocks={blocks}"),
    )
}

fn iso9660(src: &Arc<dyn BlockSource>, start: u64) -> Vec<ProposedVolume> {
    let pvd = read(src, start + 32768, 2048);
    if pvd.first().copied() != Some(1) || pvd.get(1..6) != Some(b"CD001") {
        return Vec::new();
    }
    let block_size = u64::from(le_u16(&pvd, 128)).max(2048);
    let space = u64::from(le_u32(&pvd, 80)); // volume space size (blocks), LE half
    let len = space.saturating_mul(block_size);
    if !plausible_len(src, start, len) {
        return Vec::new();
    }
    pv(
        start,
        len,
        "iso9660",
        0.9,
        format!("ISO 9660 PVD: block_size={block_size}, blocks={space}"),
    )
}

fn xfs(src: &Arc<dyn BlockSource>, start: u64) -> Vec<ProposedVolume> {
    let b = read(src, start, 512);
    if b.get(0..4) != Some(b"XFSB") {
        return Vec::new();
    }
    let block_size = u64::from(be_u32(&b, 4));
    let dblocks = be_u64(&b, 8);
    if !pow2(block_size) {
        return Vec::new();
    }
    let len = dblocks.saturating_mul(block_size);
    if !plausible_len(src, start, len) {
        return Vec::new();
    }
    pv(
        start,
        len,
        "xfs",
        0.85,
        format!("XFS superblock (detect+carve): block_size={block_size}, blocks={dblocks}"),
    )
}

fn btrfs(src: &Arc<dyn BlockSource>, start: u64) -> Vec<ProposedVolume> {
    let sb = read(src, start + 65536, 4096);
    if sb.get(64..72) != Some(b"_BHRfS_M") {
        return Vec::new();
    }
    let total_bytes = le_u64(&sb, 0x70);
    let sectorsize = u64::from(le_u32(&sb, 0x90));
    if !pow2(sectorsize) {
        return Vec::new();
    }
    if !plausible_len(src, start, total_bytes) {
        return Vec::new();
    }
    pv(
        start,
        total_bytes,
        "btrfs",
        0.85,
        format!("Btrfs superblock (detect+carve): sectorsize={sectorsize}, total={total_bytes}"),
    )
}

fn f2fs(src: &Arc<dyn BlockSource>, start: u64) -> Vec<ProposedVolume> {
    let sb = read(src, start + 1024, 1024);
    if le_u32(&sb, 0) != 0xF2F5_2010 {
        return Vec::new();
    }
    let log_blocksize = le_u32(&sb, 16);
    if log_blocksize > 16 {
        return Vec::new();
    }
    let block_size = 1u64 << log_blocksize;
    let block_count = le_u64(&sb, 36);
    let len = block_count.saturating_mul(block_size.max(1));
    // F2FS block_count is in 4 KiB units regardless of log_blocksize; accept
    // either interpretation for the estimate.
    let len = if len == 0 {
        block_count.saturating_mul(4096)
    } else {
        len
    };
    pv(
        start,
        len,
        "f2fs",
        0.8,
        format!("F2FS superblock (detect+carve): block_count={block_count}"),
    )
}

fn ufs(src: &Arc<dyn BlockSource>, start: u64) -> Vec<ProposedVolume> {
    // Detect-only (T3): the magic already matched; report the block size.
    let sb1 = read(src, start + 8192, 2048);
    let sb2 = read(src, start + 65536, 2048);
    // fs_bsize is at offset 48 of the superblock (both UFS1/UFS2).
    let bs = if le_u32(&sb1, 1372) == 0x0001_1954 {
        le_u32(&sb1, 48)
    } else if le_u32(&sb2, 1372) == 0x1954_0119 {
        le_u32(&sb2, 48)
    } else {
        0
    };
    pv(
        start,
        0,
        "ufs",
        0.7,
        format!("UFS superblock (detect+carve): block_size={bs}"),
    )
}

fn zfs(src: &Arc<dyn BlockSource>, start: u64) -> Vec<ProposedVolume> {
    let b = read(src, start, 4096);
    // uberblock magic 0x00bab10c (native/LE) at start; validate the version.
    let magic = le_u64(&b, 0);
    if magic != 0x0000_0000_00ba_b10c && magic != 0x0cb1_ba00_0000_0000 {
        return Vec::new();
    }
    pv(
        start,
        0,
        "zfs",
        0.6,
        "ZFS uberblock (detect+carve)".to_string(),
    )
}

fn vmfs(src: &Arc<dyn BlockSource>, start: u64) -> Vec<ProposedVolume> {
    let b = read(src, start, 16);
    if le_u32(&b, 0) != 0xC001_D00D {
        return Vec::new();
    }
    pv(
        start,
        0,
        "vmfs",
        0.6,
        "VMFS magic (detect+carve)".to_string(),
    )
}

// ---- the anchor table ----

const ANCHORS: [FsAnchor; 17] = [
    FsAnchor {
        needle: b"NXSB",
        internal_offset: 32,
        validate: apfs_nxsb,
    },
    FsAnchor {
        needle: b"APSB",
        internal_offset: 32,
        validate: apfs_apsb,
    },
    FsAnchor {
        needle: b"H+",
        internal_offset: 1024,
        validate: hfsplus,
    },
    FsAnchor {
        needle: b"HX",
        internal_offset: 1024,
        validate: hfsplus,
    },
    FsAnchor {
        needle: b"NTFS    ",
        internal_offset: 3,
        validate: ntfs,
    },
    FsAnchor {
        needle: b"EXFAT   ",
        internal_offset: 3,
        validate: exfat,
    },
    FsAnchor {
        needle: b"FAT32   ",
        internal_offset: 82,
        validate: fat,
    },
    FsAnchor {
        needle: b"FAT1",
        internal_offset: 54,
        validate: fat,
    },
    // ext: 0xEF53 little-endian at superblock+0x38 (volume+1080).
    FsAnchor {
        needle: &[0x53, 0xEF],
        internal_offset: 1080,
        validate: ext,
    },
    FsAnchor {
        needle: b"XFSB",
        internal_offset: 0,
        validate: xfs,
    },
    FsAnchor {
        needle: b"_BHRfS_M",
        internal_offset: 65600,
        validate: btrfs,
    },
    // F2FS magic 0xF2F52010 little-endian at volume+1024.
    FsAnchor {
        needle: &[0x10, 0x20, 0xF5, 0xF2],
        internal_offset: 1024,
        validate: f2fs,
    },
    FsAnchor {
        needle: b"CD001",
        internal_offset: 32769,
        validate: iso9660,
    },
    // UFS1/UFS2 fs_magic (LE) at superblock+1372, superblock @ 8 KiB / 64 KiB.
    FsAnchor {
        needle: &[0x54, 0x19, 0x01, 0x00],
        internal_offset: 9564,
        validate: ufs,
    },
    FsAnchor {
        needle: &[0x19, 0x01, 0x54, 0x19],
        internal_offset: 66908,
        validate: ufs,
    },
    // ZFS/VMFS detect-only anchors keyed on their magic at volume start.
    FsAnchor {
        needle: &[0x0c, 0xb1, 0xba, 0x00],
        internal_offset: 0,
        validate: zfs,
    },
    FsAnchor {
        needle: &[0x0d, 0xd0, 0x01, 0xc0],
        internal_offset: 0,
        validate: vmfs,
    },
];
