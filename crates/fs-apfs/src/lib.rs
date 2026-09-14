//! Reclaim APFS metadata engine (docs/plan/04 §2 APFS row, §3.1).
//!
//! Implemented **only** from the *Apple File System Reference* (build guide
//! Part 2.3 trap 3); `apfs-fuse` (GPL) and `libfsapfs` (LGPL) were read for
//! understanding but no code was copied. The engine is read-only and never
//! panics on data (build guide Part 1.4 rule 3).
//!
//! # Recovery strategy (docs/plan/04 §3.1)
//! 1. Read block 0 → `nx_superblock_t` (`NXSB`), Fletcher-64 verified; get the
//!    block size, checkpoint descriptor ring, container object map and volume
//!    ids.
//! 2. **Enumerate the checkpoint descriptor ring** → every valid `NXSB` by
//!    `xid` (the container's transaction *history* — free undo).
//! 3. For a checkpoint, resolve the container object map (a physical B-tree) →
//!    each volume's `apfs_superblock_t` (`APSB`) → the volume object map → the
//!    FS tree, whose `j_*` records carry inodes, directory entries (names +
//!    parents), file extents and xattrs.
//! 4. **Diff**: a `(parent, name)` present in an older checkpoint / snapshot but
//!    absent now was deleted in between — reported `Historical` with the path and
//!    extents from the old tree, the score lowered where the space-manager
//!    bitmap shows those blocks reallocated now.
//! 5. **Snapshots** (`apfs_snap_metadata` tree) give consistent older views.
//! 6. **Orphan scan**: unallocated blocks that checksum as an FS-tree node yield
//!    records from trees that have dropped out of every checkpoint.
//!
//! Volume roles: the sealed System volume is skipped by default
//! ([`reclaim_fs_core::WalkOpts::include_system_volume`] includes it); user Data
//! volumes are the target. Encrypted volumes are read only when the OS has
//! unlocked the device (docs/plan/06 §4); Reclaim implements no crypto of its
//! own (build guide Part 3.6).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#![forbid(unsafe_code)]

pub mod btree;
pub mod civil;
pub mod obj;
pub mod records;
pub mod spaceman;

use btree::{walk_leaves, Node, OMAP_KEY, OMAP_VAL};
use obj::{fletcher64_valid, le_u32, le_u64, read, ObjPhys, OBJ_FS, OBJ_NX_SUPERBLOCK, SUB_FSTREE};
use reclaim_block::BlockSource;
use reclaim_fs_core::{
    Bitmap, Entry, EntrySink, EntryState, Extent, FileSystem, FsError, Probe, SnapshotRef,
    WalkOpts, WalkStats,
};
use records::{
    apfs_date, parse_dir_rec, parse_file_extent, parse_inode, split_key, DirRec, FileExtent, Inode,
    TYPE_DIR_REC, TYPE_FILE_EXTENT, TYPE_INODE, TYPE_SNAP_METADATA,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

const NXSB_MAGIC: &[u8] = b"NXSB";
const APSB_MAGIC: &[u8] = b"APSB";

/// Object id of a volume's root directory (`ROOT_DIR_INO_NUM`).
const ROOT_DIR_INO: u64 = 2;

/// A node-visit budget guarding the B-tree walkers against a crafted image.
/// A real volume's omap/FS trees have a handful to a few thousand nodes; this
/// bounds a maliciously deep/wide (or looping) crafted tree.
const NODE_BUDGET: usize = 64_000;

/// Cap on a *bitmap-less* orphan sweep (≈4 GiB at 4 KiB blocks): without a
/// space-manager bitmap we can't distinguish free blocks, so a full sweep of a
/// large device would be too slow — scan only when the volume is this small.
const MAX_UNGUIDED_ORPHAN_BLOCKS: u64 = 1_048_576;

/// Cap on historical checkpoint superblocks diffed in one walk (the ring is
/// small in practice; bounds a crafted ring).
const MAX_HISTORY_WALK: usize = 32;

/// Cap on the checkpoint descriptor-ring length we will iterate (bounds a
/// crafted `nx_xp_desc_blocks`/`nx_xp_desc_len`).
const MAX_DESC_RING: u32 = 1024;

/// Cap on blocks read during the orphan sweep (bounds crafted `block_count`).
const MAX_ORPHAN_SCAN_BLOCKS: u64 = 8_388_608; // ≈32 GiB at 4 KiB

/// Total node-visit budget shared across one walk (all volumes × checkpoints ×
/// snapshots × tree walks). A real container spends a few thousand; this bounds
/// a crafted one where the product would otherwise explode. Combined with the
/// per-walk visited-node dedup, this keeps a hostile image well under a second.
const TOTAL_WORK_BUDGET: usize = 500_000;

/// Whether a [`RecoveryPoint`] is an APFS snapshot or a checkpoint transaction.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RecoveryKind {
    /// An `apfs_snap_metadata` snapshot (a named, consistent older view).
    Snapshot,
    /// A container checkpoint transaction reachable from the descriptor ring.
    Checkpoint,
}

/// A point Reclaim can diff the current volume against (`reclaim snapshots`).
#[derive(Clone, Debug)]
pub struct RecoveryPoint {
    /// Snapshot or checkpoint.
    pub kind: RecoveryKind,
    /// Transaction id.
    pub xid: u64,
    /// Snapshot superblock oid (0 for a checkpoint).
    pub sblock_oid: u64,
    /// Snapshot name (empty for a checkpoint).
    pub name: String,
}

/// A parsed container superblock (`nx_superblock_t`) at one descriptor slot.
#[derive(Clone, Debug)]
struct Nxsb {
    xid: u64,
    block_size: u32,
    block_count: u64,
    omap_oid: u64,
    fs_oids: Vec<u64>,
    xp_desc_base: u64,
    xp_desc_blocks: u32,
    xp_desc_index: u32,
    xp_desc_len: u32,
    spaceman_oid: u64,
}

impl Nxsb {
    fn parse(block: &[u8]) -> Option<Nxsb> {
        let obj = ObjPhys::parse(block)?;
        if obj.kind() != OBJ_NX_SUPERBLOCK {
            return None;
        }
        if block.get(32..36) != Some(NXSB_MAGIC) {
            return None;
        }
        let block_size = le_u32(block, 36);
        if !(512..=65536).contains(&block_size) || !block_size.is_power_of_two() {
            return None;
        }
        let max_fs = le_u32(block, 180).min(100) as usize;
        let mut fs_oids = Vec::new();
        for i in 0..max_fs {
            let oid = le_u64(block, 184 + i * 8);
            if oid != 0 {
                fs_oids.push(oid);
            }
        }
        Some(Nxsb {
            xid: obj.xid,
            block_size,
            block_count: le_u64(block, 40),
            omap_oid: le_u64(block, 160),
            fs_oids,
            xp_desc_base: le_u64(block, 112),
            xp_desc_blocks: le_u32(block, 104),
            xp_desc_index: le_u32(block, 136),
            xp_desc_len: le_u32(block, 140),
            spaceman_oid: le_u64(block, 152),
        })
    }
}

/// A parsed volume superblock (`apfs_superblock_t`).
#[derive(Clone, Debug)]
struct Volume {
    name: String,
    omap_oid: u64,
    root_tree_oid: u64,
    snap_meta_tree_oid: u64,
    incompat: u64,
    role: u16,
    fs_flags: u64,
}

impl Volume {
    fn parse(block: &[u8]) -> Option<Volume> {
        let obj = ObjPhys::parse(block)?;
        if obj.kind() != OBJ_FS {
            return None;
        }
        if block.get(32..36) != Some(APSB_MAGIC) {
            return None;
        }
        let name_bytes: Vec<u8> = block
            .get(704..704 + 256)
            .unwrap_or(&[])
            .iter()
            .take_while(|&&b| b != 0)
            .copied()
            .collect();
        Some(Volume {
            name: String::from_utf8_lossy(&name_bytes).into_owned(),
            incompat: le_u64(block, 56),
            omap_oid: le_u64(block, 128),
            root_tree_oid: le_u64(block, 136),
            snap_meta_tree_oid: le_u64(block, 152),
            fs_flags: le_u64(block, 264),
            role: obj_role(block),
        })
    }

    /// The volume uses hashed directory-record keys (case- or
    /// normalization-insensitive) — selects the `j_drec` key layout.
    fn hashed_drec(&self) -> bool {
        const CASE_INSENSITIVE: u64 = 0x1;
        const NORMALIZATION_INSENSITIVE: u64 = 0x8;
        self.incompat & (CASE_INSENSITIVE | NORMALIZATION_INSENSITIVE) != 0
    }

    /// Whether this is the sealed System volume (skipped by default).
    fn is_system(&self) -> bool {
        const APFS_VOL_ROLE_SYSTEM: u16 = 0x0001;
        // apfs.h: APFS_INCOMPAT_SEALED = 0x00000020 (0x200 is a different flag).
        const INCOMPAT_SEALED: u64 = 0x0000_0020;
        self.role == APFS_VOL_ROLE_SYSTEM || self.incompat & INCOMPAT_SEALED != 0
    }
}

fn obj_role(block: &[u8]) -> u16 {
    // apfs_role is a u16 at +964 (after apfs_volname[256] at +704, apfs_next_doc_id u32 at +960).
    obj::le_u16(block, 964)
}

/// Probe a volume/partition source for an APFS container (docs/plan/04 §2).
#[must_use]
pub fn probe(src: &Arc<dyn BlockSource>) -> Option<Probe> {
    // Try the default 4096 block; NXSB records its own size so read enough.
    let head = read(src, 0, 4096);
    let nx = Nxsb::parse(&head)?;
    // NXSB records its own block size; checksum the *full* block.
    let full = if nx.block_size as usize > head.len() {
        read(src, 0, nx.block_size as usize)
    } else {
        head
    };
    let checksum_ok = fletcher64_valid(full.get(..nx.block_size as usize).unwrap_or(&full));
    Some(Probe {
        fs_kind: "apfs",
        confidence: if checksum_ok { 0.99 } else { 0.9 },
        block_size: nx.block_size,
        label: None,
        uuid: None,
        total_bytes: nx.block_count.saturating_mul(u64::from(nx.block_size)),
    })
}

/// An opened APFS container.
pub struct Apfs {
    src: Arc<dyn BlockSource>,
    block_size: u32,
    newest: Nxsb,
    /// All valid container superblocks found in the descriptor ring plus the
    /// block-0 copy, sorted by `xid` descending (the checkpoint history).
    history: Vec<Nxsb>,
    /// A shared node-visit budget for the entire walk, so the *product* of
    /// volumes × checkpoints × tree size on a crafted image cannot blow up the
    /// running time (a soft-DoS the fuzzer surfaced). Reset at each public
    /// entry point.
    work: std::cell::Cell<usize>,
}

impl std::fmt::Debug for Apfs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Apfs")
            .field("block_size", &self.block_size)
            .field("newest_xid", &self.newest.xid)
            .field("checkpoint_xids", &self.recoverable_xids())
            .finish()
    }
}

/// Open an APFS container (consumes the [`Probe`]).
pub fn open(src: Arc<dyn BlockSource>, _probe: Probe) -> Result<Apfs, FsError> {
    let head = read(&src, 0, 4096);
    let block0 =
        Nxsb::parse(&head).ok_or_else(|| FsError::NotThisFs("no NXSB at block 0".into()))?;
    let bs = block0.block_size;
    // Re-read the full block-0 superblock at its declared size and verify its
    // checksum so a stale/torn block 0 is only a history candidate when intact.
    let block0_ok = fletcher64_valid(&read(&src, 0, bs as usize));

    // Enumerate the checkpoint descriptor ring for historical superblocks.
    let mut history: Vec<Nxsb> = Vec::new();
    let mut seen_xids: HashSet<u64> = HashSet::new();
    let push = |nx: Nxsb, hist: &mut Vec<Nxsb>, seen: &mut HashSet<u64>| {
        if seen.insert(nx.xid) {
            hist.push(nx);
        }
    };
    if block0_ok {
        push(block0.clone(), &mut history, &mut seen_xids);
    }

    let ring = block0.xp_desc_blocks.min(MAX_DESC_RING);
    for i in 0..ring {
        let paddr = block0.xp_desc_base.saturating_add(u64::from(i));
        let blk = read(&src, paddr.saturating_mul(u64::from(bs)), bs as usize);
        if !fletcher64_valid(&blk) {
            continue;
        }
        if let Some(nx) = Nxsb::parse(&blk) {
            if nx.block_size == bs {
                push(nx, &mut history, &mut seen_xids);
            }
        }
    }
    history.sort_by_key(|n| std::cmp::Reverse(n.xid));
    let newest = history.first().cloned().unwrap_or(block0);

    Ok(Apfs {
        src,
        block_size: bs,
        newest,
        history,
        work: std::cell::Cell::new(TOTAL_WORK_BUDGET),
    })
}

/// Boxed [`open`] for the engine registry.
pub fn open_boxed(src: Arc<dyn BlockSource>, probe: Probe) -> Result<Box<dyn FileSystem>, FsError> {
    Ok(Box::new(open(src, probe)?))
}

/// The set of transaction ids recoverable from the checkpoint ring (newest
/// first). Reported by `reclaim snapshots` and the build log's depth metric.
impl Apfs {
    #[must_use]
    pub fn recoverable_xids(&self) -> Vec<u64> {
        self.history.iter().map(|n| n.xid).collect()
    }

    /// Names of the target (non-System) volumes in this container — used by
    /// `reclaim snapshots` to label a volume.
    #[must_use]
    pub fn volume_names(&self) -> Vec<String> {
        self.work.set(TOTAL_WORK_BUDGET);
        self.target_volumes(false)
            .into_iter()
            .map(|(_, v)| v.name)
            .collect()
    }

    /// Recovery points (`reclaim snapshots <VOLUME>`): the volume snapshots plus
    /// the reachable checkpoint transaction ids (docs/plan/06 §6).
    #[must_use]
    pub fn recovery_points(&self) -> Vec<RecoveryPoint> {
        self.work.set(TOTAL_WORK_BUDGET);
        let mut out = Vec::new();
        for (_, vol) in self.target_volumes(false) {
            for (xid, sblock, name) in self.snapshot_points(&vol) {
                out.push(RecoveryPoint {
                    kind: RecoveryKind::Snapshot,
                    xid,
                    sblock_oid: sblock,
                    name,
                });
            }
        }
        for nx in self.history.iter().filter(|n| n.xid < self.newest.xid) {
            out.push(RecoveryPoint {
                kind: RecoveryKind::Checkpoint,
                xid: nx.xid,
                sblock_oid: 0,
                name: String::new(),
            });
        }
        out
    }

    /// Files present at a recovery point (snapshot xid or checkpoint xid) but
    /// absent in the current volume — `reclaim snapshots … diff --from`.
    #[must_use]
    pub fn diff_from(&self, from_xid: u64) -> Vec<Entry> {
        self.work.set(TOTAL_WORK_BUDGET);
        let mut out = Vec::new();
        for (fs_oid, vol) in self.target_volumes(false) {
            let cur = self.walk_fs_tree(&vol, self.newest.xid);
            let present = cur.present_names();

            // Prefer a snapshot with this xid; else a checkpoint superblock.
            let old_view: Option<(VolumeContents, EntryState)> = {
                let snap = self
                    .snapshot_points(&vol)
                    .into_iter()
                    .find(|(x, _, _)| *x == from_xid);
                if let Some((sx, sblock, _)) = snap {
                    let blk = self.read_block(sblock);
                    Volume::parse(&blk)
                        .map(|sv| (self.walk_fs_tree(&sv, sx), EntryState::Historical))
                } else {
                    self.history
                        .iter()
                        .find(|n| n.xid == from_xid)
                        .and_then(|nx| self.resolve_volume(nx.omap_oid, fs_oid, nx.xid))
                        .map(|ov| (self.walk_fs_tree(&ov, from_xid), EntryState::Historical))
                }
            };
            if let Some((old, state)) = old_view {
                let edges = old.edges();
                for d in &old.drecs {
                    let key = (d.parent_id, d.name.clone());
                    if present.contains(&key) {
                        continue;
                    }
                    if let Some(entry) = self.build_entry(d, &old, &edges, state, 0.9) {
                        out.push(entry);
                    }
                }
            }
        }
        out
    }

    fn read_block(&self, paddr: u64) -> Vec<u8> {
        read(
            &self.src,
            paddr.saturating_mul(u64::from(self.block_size)),
            self.block_size as usize,
        )
    }

    /// [`read_block`](Self::read_block), returning `None` unless the block's
    /// Fletcher-64 checksum verifies (a torn or reused block is not an object).
    fn read_verified_block(&self, paddr: u64) -> Option<Vec<u8>> {
        let blk = self.read_block(paddr);
        fletcher64_valid(&blk).then_some(blk)
    }

    /// A bitmap marking the blocks referenced by the **current live files'**
    /// data extents. This is the right "overwritten?" signal for recovery
    /// scoring: a deleted file whose blocks a live file now occupies is truly
    /// overwritten, whereas the raw space-manager marks many still-recoverable
    /// blocks allocated (APFS reference-counts and frees lazily; a freed data
    /// block is often re-allocated to metadata while the old bytes survive — see
    /// docs/build-log/phase-3.md). The space-manager bitmap ([`spaceman_bitmap`])
    /// is still used to guide the orphan sweep to free blocks.
    fn live_extent_bitmap(&self) -> Option<Bitmap> {
        self.work.set(TOTAL_WORK_BUDGET);
        let block_count = self.newest.block_count;
        if block_count == 0 || block_count > 512 << 20 {
            return None; // >512M blocks: skip (documented)
        }
        let bytes = block_count.div_ceil(8) as usize;
        let mut bits = vec![0u8; bytes];
        for (_, vol) in self.target_volumes(false) {
            let cur = self.walk_fs_tree(&vol, self.newest.xid);
            self.mark_live_blocks(&cur, &mut bits, block_count);
        }
        Some(Bitmap::new(self.block_size, 0, block_count, bits))
    }

    /// Set a bit for every block referenced by a live file's data extents.
    fn mark_live_blocks(&self, cur: &VolumeContents, bits: &mut [u8], block_count: u64) {
        let bs = u64::from(self.block_size.max(1));
        for exts in cur.extents.values() {
            for e in exts {
                if e.phys_block == 0 || e.len == 0 {
                    continue;
                }
                let nblocks = e.len.div_ceil(bs).min(1 << 20);
                for b in e.phys_block..e.phys_block.saturating_add(nblocks) {
                    if b >= block_count {
                        break;
                    }
                    if let Some(byte) = bits.get_mut((b / 8) as usize) {
                        *byte |= 1 << (b % 8);
                    }
                }
            }
        }
    }

    /// A bitmap marking only the blocks referenced by `cur`'s live files — used
    /// to steer the orphan sweep away from live *data* while still scanning
    /// freed/reallocated *metadata* blocks (where deleted records survive).
    fn live_bitmap_from(&self, cur: &VolumeContents) -> Option<Bitmap> {
        let block_count = self.newest.block_count;
        if block_count == 0 || block_count > 512 << 20 {
            return None;
        }
        let mut bits = vec![0u8; block_count.div_ceil(8) as usize];
        self.mark_live_blocks(cur, &mut bits, block_count);
        Some(Bitmap::new(self.block_size, 0, block_count, bits))
    }

    /// Resolve the space-manager checkpoint mapping to find the spaceman paddr,
    /// then build the container allocation bitmap (diagnostic; the walk steers
    /// by the live-extent map instead — see [`Apfs::live_extent_bitmap`]).
    #[must_use]
    pub fn spaceman_bitmap(&self) -> Option<Bitmap> {
        let sm_paddr = self.resolve_ephemeral(self.newest.spaceman_oid)?;
        spaceman::build(
            &self.src,
            0,
            sm_paddr,
            self.block_size,
            self.newest.block_count,
        )
    }

    /// Resolve an ephemeral object id to a physical address via the newest
    /// checkpoint's checkpoint-map blocks (the ones preceding the NXSB in the
    /// descriptor ring for `nx_xp_desc_index .. +len`).
    fn resolve_ephemeral(&self, oid: u64) -> Option<u64> {
        let nx = &self.newest;
        // The descriptor ring is at most a few thousand blocks in practice; cap
        // it so a crafted `xp_desc_blocks`/`xp_desc_len` can't spin for billions
        // of iterations (fuzz-found soft-DoS).
        let ring = nx.xp_desc_blocks.clamp(1, MAX_DESC_RING);
        for j in 0..nx.xp_desc_len.min(ring) {
            let idx = nx.xp_desc_index.wrapping_add(j) % ring;
            let paddr = nx.xp_desc_base.saturating_add(u64::from(idx));
            let blk = self.read_block(paddr);
            let obj = match ObjPhys::parse(&blk) {
                Some(o) => o,
                None => continue,
            };
            const OBJ_CHECKPOINT_MAP: u16 = 0x000c;
            if obj.kind() != OBJ_CHECKPOINT_MAP {
                continue;
            }
            let count = le_u32(&blk, 36).min(1000);
            for k in 0..count as usize {
                let e = 40 + k * 40; // checkpoint_mapping is 40 bytes
                let cpm_oid = le_u64(&blk, e + 24);
                let cpm_paddr = le_u64(&blk, e + 32);
                if cpm_oid == oid {
                    return Some(cpm_paddr);
                }
            }
        }
        None
    }

    /// Run a bounded B-tree leaf walk that draws from the shared work budget, so
    /// the total work across every tree in one walk is capped regardless of how
    /// many volumes/checkpoints reference trees.
    fn budgeted_walk<F, V>(&self, root: Node, fetch: &F, fk: usize, fv: usize, visit: &mut V)
    where
        F: Fn(u64) -> Option<Vec<u8>>,
        V: FnMut(&[u8], &[u8]),
    {
        let start = self.work.get();
        if start == 0 {
            return;
        }
        let cap = start.min(NODE_BUDGET);
        let mut local = cap;
        walk_leaves(root, fetch, fk, fv, &mut local, visit);
        self.work.set(start.saturating_sub(cap - local));
    }

    /// Build an oid→paddr index for a physical object-map tree at `omap_oid`,
    /// keeping the newest value at or before `at_xid` for each oid.
    fn omap_index(&self, omap_oid: u64, at_xid: u64) -> HashMap<u64, u64> {
        let mut idx: HashMap<u64, (u64, u64)> = HashMap::new(); // oid -> (xid, paddr)
        let omap_block = self.read_block(omap_oid);
        // omap_phys: om_tree_oid at +48.
        let tree_oid = le_u64(&omap_block, 48);
        if tree_oid == 0 {
            return HashMap::new();
        }
        let root = match Node::parse(self.read_block(tree_oid)) {
            Some(n) => n,
            None => return HashMap::new(),
        };
        let fetch = |paddr: u64| -> Option<Vec<u8>> { self.read_verified_block(paddr) };
        self.budgeted_walk(root, &fetch, OMAP_KEY, OMAP_VAL, &mut |k, v| {
            let oid = le_u64(k, 0);
            let xid = le_u64(k, 8);
            if xid > at_xid {
                return;
            }
            let paddr = le_u64(v, 8);
            let e = idx.entry(oid).or_insert((0, 0));
            if xid >= e.0 {
                *e = (xid, paddr);
            }
        });
        idx.into_iter()
            .map(|(oid, (_, paddr))| (oid, paddr))
            .collect()
    }

    /// Resolve a volume superblock (APSB) for `fs_oid` at `at_xid` under the
    /// container omap `omap_oid`.
    fn resolve_volume(&self, omap_oid: u64, fs_oid: u64, at_xid: u64) -> Option<Volume> {
        let idx = self.omap_index(omap_oid, at_xid);
        let paddr = *idx.get(&fs_oid)?;
        let blk = self.read_block(paddr);
        Volume::parse(&blk)
    }

    /// Walk a volume's FS tree at `root_tree_oid` (virtual, resolved through the
    /// volume omap at `omap_oid`, `at_xid`) into a [`VolumeContents`].
    fn walk_fs_tree(&self, vol: &Volume, at_xid: u64) -> VolumeContents {
        let mut vc = VolumeContents::default();
        let vidx = self.omap_index(vol.omap_oid, at_xid);
        let root_paddr = match vidx.get(&vol.root_tree_oid) {
            Some(p) => *p,
            None => return vc,
        };
        let root = match Node::parse(self.read_block(root_paddr)) {
            Some(n) => n,
            None => return vc,
        };
        let fetch = |oid: u64| -> Option<Vec<u8>> {
            let p = vidx.get(&oid)?;
            let blk = self.read_verified_block(*p)?;
            // The block must still be *this* object at or before `at_xid`;
            // otherwise it was reused since the omap entry was written.
            let obj = ObjPhys::parse(&blk)?;
            if obj.oid != oid || obj.xid > at_xid {
                return None;
            }
            Some(blk)
        };
        let hashed = vol.hashed_drec();
        self.budgeted_walk(root, &fetch, 0, 0, &mut |k, v| {
            vc.absorb(k, v, hashed);
        });
        vc
    }
}

/// The parsed contents of one FS-tree walk.
#[derive(Default)]
struct VolumeContents {
    inodes: HashMap<u64, Inode>,
    drecs: Vec<DirRec>,
    extents: HashMap<u64, Vec<FileExtent>>,
}

impl VolumeContents {
    fn absorb(&mut self, key: &[u8], val: &[u8], hashed: bool) {
        if key.len() < 8 {
            return;
        }
        let (obj_id, ty) = split_key(le_u64(key, 0));
        match ty {
            TYPE_INODE => {
                let ino = parse_inode(obj_id, val);
                self.inodes.insert(obj_id, ino);
            }
            TYPE_DIR_REC => {
                if let Some(r) = parse_dir_rec(obj_id, key, val, hashed) {
                    self.drecs.push(r);
                }
            }
            TYPE_FILE_EXTENT => {
                let e = parse_file_extent(obj_id, key, val);
                self.extents.entry(obj_id).or_default().push(e);
            }
            _ => {}
        }
    }

    /// Map each child object id to its (parent, name, is_dir, date) directory
    /// edge for path resolution.
    fn edges(&self) -> HashMap<u64, &DirRec> {
        let mut m = HashMap::new();
        for d in &self.drecs {
            m.insert(d.file_id, d);
        }
        m
    }

    /// Resolve the full path of `file_id` by walking parent edges to the root.
    fn path_of(&self, file_id: u64, edges: &HashMap<u64, &DirRec>) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        let mut cur = file_id;
        let mut guard = 0;
        while guard < 128 {
            guard += 1;
            let d = edges.get(&cur)?;
            parts.push(d.name.clone());
            if d.parent_id == ROOT_DIR_INO || d.parent_id == 0 {
                break;
            }
            cur = d.parent_id;
        }
        parts.reverse();
        Some(parts.join("/"))
    }

    /// The set of `(parent_id, name)` present (live) in this view.
    fn present_names(&self) -> HashSet<(u64, String)> {
        self.drecs
            .iter()
            .map(|d| (d.parent_id, d.name.clone()))
            .collect()
    }

    /// Every object id referenced by this view's inodes or directory records.
    fn live_ids(&self) -> HashSet<u64> {
        self.inodes
            .keys()
            .copied()
            .chain(self.drecs.iter().map(|d| d.file_id))
            .collect()
    }
}

impl Apfs {
    /// Build an [`Entry`] for one directory record, given its view contents.
    fn build_entry(
        &self,
        d: &DirRec,
        vc: &VolumeContents,
        edges: &HashMap<u64, &DirRec>,
        state: EntryState,
        confidence: f32,
    ) -> Option<Entry> {
        if d.is_dir {
            return None; // directories are reconstructed from paths
        }
        let inode = vc.inodes.get(&d.file_id);
        let size = inode.and_then(|i| i.size).unwrap_or(0);
        let mut extents: Vec<Extent> = Vec::new();
        if let Some(exts) = vc.extents.get(&d.file_id) {
            // The orphan scan aggregates records from every surviving COW copy of
            // a node, so the same extent can appear many times; keep one per
            // logical address (newest phys wins) and order by logical.
            let mut by_logical: std::collections::BTreeMap<u64, &FileExtent> =
                std::collections::BTreeMap::new();
            for e in exts {
                by_logical.insert(e.logical, e);
            }
            for e in by_logical.values() {
                if e.phys_block == 0 || e.len == 0 {
                    continue; // hole
                }
                extents.push(Extent {
                    offset: e.phys_block.saturating_mul(u64::from(self.block_size)),
                    len: e.len,
                });
            }
        }
        // Truncate the extents to the inode's logical size (the last extent is
        // block-rounded; recovering exactly `size` bytes matches the original).
        let size = if size == 0 {
            extents.iter().fold(0u64, |a, e| a.saturating_add(e.len))
        } else {
            trim_extents_to(&mut extents, size);
            size
        };
        let path = vc.path_of(d.file_id, edges);
        let mut entry = Entry::new_file(d.file_id, d.name.clone(), d.raw_name.clone(), size, state);
        entry.parent_id = Some(d.parent_id);
        entry.path = path;
        entry.extents = extents;
        entry.confidence = confidence;
        entry.created = inode.and_then(|i| apfs_date(i.create_time));
        entry.modified = inode
            .and_then(|i| apfs_date(i.mod_time))
            .or_else(|| apfs_date(d.date_added));
        Some(entry)
    }

    /// Choose target volumes (skip the sealed System volume unless requested).
    fn target_volumes(&self, include_system: bool) -> Vec<(u64, Volume)> {
        let nx = &self.newest;
        let mut out = Vec::new();
        for &fs_oid in &nx.fs_oids {
            if let Some(vol) = self.resolve_volume(nx.omap_oid, fs_oid, nx.xid) {
                if include_system || !vol.is_system() {
                    out.push((fs_oid, vol));
                }
            }
        }
        // If everything was filtered (a lone system-looking volume), fall back.
        if out.is_empty() {
            for &fs_oid in &nx.fs_oids {
                if let Some(vol) = self.resolve_volume(nx.omap_oid, fs_oid, nx.xid) {
                    out.push((fs_oid, vol));
                }
            }
        }
        out
    }

    /// Orphan scan: sweep every block; any that checksums as an FS-tree *leaf*
    /// node contributes its records (for object ids not seen live) as
    /// `Orphaned` entries (docs/plan/04 §3.1 step 7).
    #[allow(clippy::too_many_arguments)]
    fn orphan_scan(
        &self,
        hashed: bool,
        present: &HashSet<(u64, String)>,
        live_ids: &HashSet<u64>,
        bitmap: Option<&Bitmap>,
        sink: &mut dyn EntrySink,
        stats: &mut WalkStats,
        opts: &WalkOpts,
        emitted: &mut HashSet<(u64, String)>,
    ) {
        let bs = self.block_size as usize;
        // Bound the sweep by the smaller of the declared block count and the
        // source's actual size, then a hard cap (crafted `block_count` guard).
        let src_blocks = self.src.len() / u64::from(self.block_size.max(1));
        let total_blocks = self
            .newest
            .block_count
            .min(src_blocks.saturating_add(1))
            .min(MAX_ORPHAN_SCAN_BLOCKS);
        // Without a bitmap we cannot tell free blocks from live ones; only scan
        // an unbounded volume when it is small enough (golden images), else skip
        // to avoid a full-device sweep on a multi-TB disk (documented gap).
        if bitmap.is_none() && total_blocks > MAX_UNGUIDED_ORPHAN_BLOCKS {
            return;
        }
        let mut vc = VolumeContents::default();
        // Read in 1 MiB chunks (256×4 KiB blocks) to keep syscalls down.
        let per_chunk: u64 = (1 << 20) / u64::from(self.block_size.max(1));
        let mut b = 0u64;
        while b < total_blocks {
            let chunk_blocks = per_chunk.min(total_blocks - b);
            let chunk = read(
                &self.src,
                b.saturating_mul(u64::from(self.block_size)),
                (chunk_blocks as usize).saturating_mul(bs),
            );
            for i in 0..chunk_blocks {
                let blk_idx = b + i;
                if let Some(bm) = bitmap {
                    if bm.is_block_allocated(blk_idx) {
                        continue; // deleted metadata lives in freed blocks
                    }
                }
                let start = (i as usize).saturating_mul(bs);
                let block = match chunk.get(start..start + bs) {
                    Some(s) => s,
                    None => continue,
                };
                let obj = match ObjPhys::parse(block) {
                    Some(o) => o,
                    None => continue,
                };
                if obj.sub_kind() != SUB_FSTREE || !fletcher64_valid(block) {
                    continue;
                }
                let node = match Node::parse(block.to_vec()) {
                    Some(n) if n.is_leaf() => n,
                    _ => continue,
                };
                let mut budget = 1usize;
                walk_leaves(node, &|_| None, 0, 0, &mut budget, &mut |k, v| {
                    vc.absorb(k, v, hashed);
                });
            }
            b += chunk_blocks;
        }
        let edges = vc.edges();
        for d in &vc.drecs {
            if d.is_dir {
                continue;
            }
            let key = (d.parent_id, d.name.clone());
            if present.contains(&key) || emitted.contains(&key) || live_ids.contains(&d.file_id) {
                continue;
            }
            if let Some(entry) = self.build_entry(d, &vc, &edges, EntryState::Orphaned, 0.6) {
                emitted.insert(key);
                stats.emitted += 1;
                stats.deleted += 1;
                if opts.include_deleted {
                    sink.emit(entry);
                }
            }
        }
    }
}

impl FileSystem for Apfs {
    fn kind(&self) -> &'static str {
        "apfs"
    }

    fn block_size(&self) -> u32 {
        self.block_size
    }

    fn allocation_bitmap(&self) -> Option<Bitmap> {
        // Scoring/carver use the live-referenced map (a true "overwritten?"
        // signal); the space-manager bitmap guides the orphan sweep internally.
        self.live_extent_bitmap()
    }

    fn walk(&self, sink: &mut dyn EntrySink, opts: &WalkOpts) -> Result<WalkStats, FsError> {
        self.work.set(TOTAL_WORK_BUDGET);
        let mut stats = WalkStats::default();
        // The orphan sweep is steered by the live-referenced map (not the
        // space-manager bitmap) so it still examines freed metadata blocks that
        // APFS has re-allocated; see [`Apfs::spaceman_bitmap`] for diagnostics.
        let targets = self.target_volumes(opts.include_system_volume);

        for (fs_oid, vol) in &targets {
            // Current (newest) view.
            let cur = self.walk_fs_tree(vol, self.newest.xid);
            let cur_edges = cur.edges();
            let present = cur.present_names();
            // Object ids that are live *now*: a stale (parent, name) drec for a
            // renamed/moved file carries the same file_id and must not be
            // presented as deleted (APFS never reuses object ids).
            let live_ids = cur.live_ids();
            let live_bm = self.live_bitmap_from(&cur);
            let mut emitted: HashSet<(u64, String)> = HashSet::new();

            if opts.include_live {
                for d in &cur.drecs {
                    if let Some(entry) =
                        self.build_entry(d, &cur, &cur_edges, EntryState::Live, 0.95)
                    {
                        stats.emitted += 1;
                        stats.live += 1;
                        sink.emit(entry);
                    }
                }
            }

            // Historical checkpoints: older container superblocks resolve the
            // same volume oid → older APSB → older FS tree. Present-then-absent
            // = deleted between then and now.
            if opts.include_deleted {
                for nx in self
                    .history
                    .iter()
                    .filter(|n| n.xid < self.newest.xid)
                    .take(MAX_HISTORY_WALK)
                {
                    if let Some(old_vol) = self.resolve_volume(nx.omap_oid, *fs_oid, nx.xid) {
                        let old = self.walk_fs_tree(&old_vol, nx.xid);
                        let old_edges = old.edges();
                        for d in &old.drecs {
                            let key = (d.parent_id, d.name.clone());
                            if present.contains(&key)
                                || emitted.contains(&key)
                                || live_ids.contains(&d.file_id)
                            {
                                continue;
                            }
                            if let Some(mut entry) =
                                self.build_entry(d, &old, &old_edges, EntryState::Historical, 0.85)
                            {
                                entry.created =
                                    entry.created.or_else(|| Some(format!("xid {}", nx.xid)));
                                emitted.insert(key);
                                stats.emitted += 1;
                                stats.deleted += 1;
                                sink.emit(entry);
                            }
                        }
                    }
                }

                // Snapshots.
                for (snap_xid, sblock_oid, _name) in self.snapshot_points(vol) {
                    let blk = self.read_block(sblock_oid);
                    if let Some(snap_vol) = Volume::parse(&blk) {
                        let snapv = self.walk_fs_tree(&snap_vol, snap_xid);
                        let snap_edges = snapv.edges();
                        for d in &snapv.drecs {
                            let key = (d.parent_id, d.name.clone());
                            if present.contains(&key)
                                || emitted.contains(&key)
                                || live_ids.contains(&d.file_id)
                            {
                                continue;
                            }
                            if let Some(entry) = self.build_entry(
                                d,
                                &snapv,
                                &snap_edges,
                                EntryState::Historical,
                                0.9,
                            ) {
                                emitted.insert(key);
                                stats.emitted += 1;
                                stats.deleted += 1;
                                sink.emit(entry);
                            }
                        }
                    }
                }

                // Orphan scan: examine every block not holding live file data
                // (freed *and* reallocated metadata included) for FS-tree nodes.
                self.orphan_scan(
                    vol.hashed_drec(),
                    &present,
                    &live_ids,
                    live_bm.as_ref(),
                    sink,
                    &mut stats,
                    opts,
                    &mut emitted,
                );
            }
            let _ = vol.fs_flags;
        }
        Ok(stats)
    }

    fn snapshots(&self) -> Vec<SnapshotRef> {
        self.work.set(TOTAL_WORK_BUDGET);
        let mut out = Vec::new();
        for (_, vol) in self.target_volumes(false) {
            for (xid, _sblock, name) in self.snapshot_points(&vol) {
                out.push(SnapshotRef {
                    id: xid,
                    label: if name.is_empty() {
                        format!("snapshot xid {xid}")
                    } else {
                        name
                    },
                });
            }
        }
        // Also expose reachable checkpoint xids (excluding the current one).
        for nx in self.history.iter().filter(|n| n.xid < self.newest.xid) {
            out.push(SnapshotRef {
                id: nx.xid,
                label: format!("checkpoint xid {}", nx.xid),
            });
        }
        out
    }
}

impl Apfs {
    /// Enumerate a volume's snapshots as `(xid, sblock_oid, name)` from the
    /// `apfs_snap_metadata` tree (docs/plan/04 §3.1 step 6).
    fn snapshot_points(&self, vol: &Volume) -> Vec<(u64, u64, String)> {
        let mut out = Vec::new();
        if vol.snap_meta_tree_oid == 0 {
            return out;
        }
        // The snap-meta tree is a physical tree; its root oid is a paddr.
        let root = match Node::parse(self.read_block(vol.snap_meta_tree_oid)) {
            Some(n) => n,
            None => return out,
        };
        let fetch = |paddr: u64| -> Option<Vec<u8>> { self.read_verified_block(paddr) };
        self.budgeted_walk(root, &fetch, 0, 0, &mut |k, v| {
            if k.len() < 8 {
                return;
            }
            let (xid, ty) = split_key(le_u64(k, 0));
            if ty != TYPE_SNAP_METADATA {
                return;
            }
            // j_snap_metadata_val_t: extentref_tree_oid u64, sblock_oid u64, ...
            let sblock_oid = le_u64(v, 8);
            // name is at the tail: name_len u16 then name (after several fields).
            let name = snapshot_name(v);
            if sblock_oid != 0 {
                out.push((xid, sblock_oid, name));
            }
        });
        out
    }
}

/// Trim an ordered extent list so its total length is exactly `size` bytes
/// (APFS extents are block-rounded; the logical size is the true file length).
fn trim_extents_to(extents: &mut Vec<Extent>, size: u64) {
    let mut acc = 0u64;
    let mut keep = 0usize;
    for e in extents.iter_mut() {
        if acc >= size {
            break;
        }
        let remaining = size - acc;
        if e.len > remaining {
            e.len = remaining;
        }
        acc = acc.saturating_add(e.len);
        keep += 1;
    }
    extents.truncate(keep);
}

/// Extract the snapshot name from a `j_snap_metadata_val_t` value (best effort).
fn snapshot_name(v: &[u8]) -> String {
    // Fixed portion: extentref_tree_oid(8) sblock_oid(8) create_time(8) change_time(8)
    // inum(8) extentref_tree_type(4) flags(4) name_len(2) then name.
    let name_len = obj::le_u16(v, 48) as usize;
    match v.get(50..50 + name_len) {
        Some(b) => String::from_utf8_lossy(
            &b.iter()
                .take_while(|&&c| c != 0)
                .copied()
                .collect::<Vec<u8>>(),
        )
        .into_owned(),
        None => String::new(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;
    use reclaim_block::{ImageFile, MemorySource};
    use std::path::Path;

    fn golden() -> Option<Arc<dyn BlockSource>> {
        let p = Path::new("../../testdata/build/apfs-delete-history.img");
        if !p.is_file() {
            return None;
        }
        Some(Arc::new(ImageFile::open(p).ok()?))
    }

    fn container_view(disk: &Arc<dyn BlockSource>) -> Arc<dyn BlockSource> {
        // GPT: the APFS partition begins at LBA 40 on the golden image.
        use reclaim_block::OffsetView;
        let map = reclaim_part::scan(disk);
        let (start, len) = map
            .volume_windows(disk.len())
            .into_iter()
            .next()
            .expect("a partition");
        Arc::new(OffsetView::new(Arc::clone(disk), start, len).unwrap())
    }

    #[test]
    fn probe_and_open_golden() {
        let Some(disk) = golden() else {
            eprintln!("skip: golden image not built");
            return;
        };
        let view = container_view(&disk);
        let probe = probe(&view).expect("APFS probe");
        assert_eq!(probe.fs_kind, "apfs");
        assert_eq!(probe.block_size, 4096);
        let fs = open(view, probe).expect("open APFS");
        // The churned image has a shallow checkpoint ring.
        assert!(!fs.recoverable_xids().is_empty());
    }

    #[test]
    fn walk_recovers_live_files() {
        let Some(disk) = golden() else {
            return;
        };
        let view = container_view(&disk);
        let probe = probe(&view).unwrap();
        let fs = open(view, probe).unwrap();
        let mut sink = reclaim_fs_core::VecSink::default();
        let stats = fs.walk(&mut sink, &WalkOpts::default()).unwrap();
        assert!(stats.live > 0, "expected live files, got {stats:?}");
        // The live set includes file_012..file_039 (deleted 0..11).
        let names: Vec<&str> = sink.entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.iter().any(|n| n.starts_with("file_012")));
    }

    #[test]
    fn spaceman_bitmap_present() {
        let Some(disk) = golden() else {
            return;
        };
        let view = container_view(&disk);
        let probe = probe(&view).unwrap();
        let fs = open(view, probe).unwrap();
        let bm = fs.allocation_bitmap().expect("live-extent bitmap");
        assert!(bm.allocated_count() > 0);
        assert!(bm.block_count() > 0);
        let sm = fs.spaceman_bitmap().expect("spaceman bitmap");
        assert!(sm.allocated_count() > 0);
        assert_eq!(sm.block_count(), bm.block_count());
    }

    #[test]
    fn garbage_never_panics() {
        let src: Arc<dyn BlockSource> = Arc::new(MemorySource::new(vec![0xABu8; 200_000]));
        assert!(probe(&src).is_none());
    }
}
