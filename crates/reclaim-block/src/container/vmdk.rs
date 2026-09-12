//! VMware VMDK images (`.vmdk`) — docs/plan/04 §5.
//!
//! Two shapes are handled:
//! * **Hosted sparse** (`KDMV` magic): a grain-directory / grain-table map over
//!   the file (`monolithicSparse`, qemu-img's default `-O vmdk`).
//! * **Descriptor / flat** (text `# Disk DescriptorFile`): an extent list of
//!   `FLAT` / `ZERO` / `SPARSE` extents, concatenated (`monolithicFlat`,
//!   `twoGbMaxExtentFlat`).

use super::concat::ConcatSource;
use super::map::{Piece, SegmentMap};
use super::{le_u32, le_u64, read_exact_vec};
use crate::error::BlockError;
use crate::image_file::ImageFile;
use crate::offset_view::OffsetView;
use crate::source::{BlockSource, SourceId};
use std::path::Path;
use std::sync::Arc;

const KDMV: u32 = 0x564D_444B; // "KDMV" little-endian
const SECTOR: u64 = 512;
const MAX_GRAINS: u64 = 64 * 1024 * 1024;

/// Open either shape.
pub(crate) fn open(path: &Path) -> Result<Arc<dyn BlockSource>, BlockError> {
    let img = ImageFile::open(path)?;
    let backing: Arc<dyn BlockSource> = Arc::new(img);
    let head = read_exact_vec(&backing, 0, 512).unwrap_or_default();
    if le_u32(&head, 0) == Some(KDMV) {
        Ok(Arc::new(open_sparse(path, &backing)?))
    } else {
        open_descriptor(path, &backing)
    }
}

/// Parse a hosted sparse extent's grain directory/tables.
fn open_sparse(
    path: &Path,
    backing: &Arc<dyn BlockSource>,
) -> Result<super::map::MappedSource, BlockError> {
    let hdr = read_exact_vec(backing, 0, 512)
        .ok_or_else(|| BlockError::Container("vmdk: short header".into()))?;
    // SparseExtentHeader (little-endian).
    let flags = le_u32(&hdr, 8).unwrap_or(0);
    let capacity =
        le_u64(&hdr, 12).ok_or_else(|| BlockError::Container("vmdk: no capacity".into()))?; // sectors
    let grain_size =
        le_u64(&hdr, 20).ok_or_else(|| BlockError::Container("vmdk: no grainSize".into()))?; // sectors
    let num_gtes_per_gt =
        le_u32(&hdr, 44).ok_or_else(|| BlockError::Container("vmdk: no numGTEsPerGT".into()))?;
    let gd_offset =
        le_u64(&hdr, 56).ok_or_else(|| BlockError::Container("vmdk: no gdOffset".into()))?; // sectors
    if grain_size == 0 || num_gtes_per_gt == 0 {
        return Err(BlockError::Container("vmdk: zero grain geometry".into()));
    }
    // Compressed grains (streamOptimized) use bit 16 of flags + grain markers,
    // which need a different walk; not supported in round 1.
    let compressed = flags & 0x1_0000 != 0;
    if compressed {
        return Err(BlockError::Container(
            "vmdk: streamOptimized (compressed grains) unsupported".into(),
        ));
    }

    let out_len = capacity.saturating_mul(SECTOR);
    let grain_bytes = grain_size.saturating_mul(SECTOR);
    let total_grains = capacity.div_ceil(grain_size);
    if total_grains > MAX_GRAINS {
        return Err(BlockError::Container("vmdk: too many grains".into()));
    }
    let grains_per_gt = u64::from(num_gtes_per_gt);
    let num_gts = total_grains.div_ceil(grains_per_gt);

    // Grain directory: num_gts u32 entries at gd_offset.
    let gd_bytes = usize::try_from(num_gts.saturating_mul(4))
        .map_err(|_| BlockError::Container("vmdk: GD too large".into()))?;
    let gd = read_exact_vec(backing, gd_offset.saturating_mul(SECTOR), gd_bytes)
        .ok_or_else(|| BlockError::Container("vmdk: cannot read grain directory".into()))?;

    let mut smap = SegmentMap::new();
    let mut gt_cache: std::collections::HashMap<u64, Vec<u8>> = std::collections::HashMap::new();

    for g in 0..total_grains {
        let gt_index = g / grains_per_gt;
        let gt_ent = g % grains_per_gt;
        let this_out = core::cmp::min(grain_bytes, out_len.saturating_sub(g * grain_bytes));
        if this_out == 0 {
            break;
        }
        let gt_sector = u64::from(
            le_u32(&gd, usize::try_from(gt_index * 4).unwrap_or(usize::MAX)).unwrap_or(0),
        );
        if gt_sector == 0 {
            smap.push(this_out, Piece::Zero); // whole grain table sparse
            continue;
        }
        let gt = match gt_cache.get(&gt_sector) {
            Some(t) => t,
            None => {
                let gt_len = usize::try_from(grains_per_gt.saturating_mul(4))
                    .map_err(|_| BlockError::Container("vmdk: GT too large".into()))?;
                let t = read_exact_vec(backing, gt_sector.saturating_mul(SECTOR), gt_len)
                    .ok_or_else(|| BlockError::Container("vmdk: cannot read grain table".into()))?;
                gt_cache.entry(gt_sector).or_insert(t)
            }
        };
        let grain_sector =
            u64::from(le_u32(gt, usize::try_from(gt_ent * 4).unwrap_or(usize::MAX)).unwrap_or(0));
        if grain_sector == 0 {
            smap.push(this_out, Piece::Zero);
        } else {
            smap.push(
                this_out,
                Piece::Raw {
                    pos: grain_sector.saturating_mul(SECTOR),
                },
            );
        }
    }
    if out_len > smap.out_len() {
        smap.push(out_len - smap.out_len(), Piece::Zero);
    }
    let id = SourceId::new(format!("vmdk:{}:{}", path.display(), out_len));
    Ok(smap.build(backing.clone(), out_len, SECTOR as u32, id))
}

/// Parse a text descriptor into concatenated flat/zero extents.
fn open_descriptor(
    path: &Path,
    backing: &Arc<dyn BlockSource>,
) -> Result<Arc<dyn BlockSource>, BlockError> {
    // The descriptor is small text; read up to 64 KiB.
    let n = core::cmp::min(backing.len(), 64 * 1024) as usize;
    let text = read_exact_vec(backing, 0, n).unwrap_or_default();
    let text = String::from_utf8_lossy(&text);
    let dir = path.parent().unwrap_or_else(|| Path::new("."));

    let mut parts: Vec<Arc<dyn BlockSource>> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        // Extent line: `RW 20480 FLAT "disk-flat.vmdk" 0` or `RW 20480 ZERO`.
        if !(line.starts_with("RW ")
            || line.starts_with("RDONLY ")
            || line.starts_with("NOACCESS "))
        {
            continue;
        }
        let toks: Vec<&str> = line.split_whitespace().collect();
        let Some(sectors) = toks.get(1).and_then(|s| s.parse::<u64>().ok()) else {
            continue;
        };
        let kind = toks.get(2).copied().unwrap_or("");
        let bytes = sectors.saturating_mul(SECTOR);
        match kind {
            "ZERO" => {
                // Represent a zero extent as a MappedSource hole.
                let mut z = SegmentMap::new();
                z.push(bytes, Piece::Zero);
                let zs = z.build(
                    backing.clone(),
                    bytes,
                    SECTOR as u32,
                    SourceId::new(format!("vmdk-zero:{bytes}")),
                );
                parts.push(Arc::new(zs));
            }
            "FLAT" | "VMFS" | "VMFSRAW" => {
                let fname = toks.get(3).map(|s| s.trim_matches('"')).unwrap_or_default();
                let off_sectors = toks.get(4).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
                if fname.is_empty() {
                    continue;
                }
                let sub = dir.join(fname);
                let subimg = ImageFile::open(&sub)?;
                let sub_arc: Arc<dyn BlockSource> = Arc::new(subimg);
                let view = OffsetView::new(sub_arc, off_sectors.saturating_mul(SECTOR), bytes)?;
                parts.push(Arc::new(view));
            }
            "SPARSE" => {
                let fname = toks.get(3).map(|s| s.trim_matches('"')).unwrap_or_default();
                if fname.is_empty() {
                    continue;
                }
                let sub = dir.join(fname);
                let sub_backing: Arc<dyn BlockSource> = Arc::new(ImageFile::open(&sub)?);
                parts.push(Arc::new(open_sparse(&sub, &sub_backing)?));
            }
            _ => {}
        }
    }
    if parts.is_empty() {
        return Err(BlockError::Container(
            "vmdk: descriptor had no usable extents".into(),
        ));
    }
    if parts.len() == 1 {
        if let Some(single) = parts.into_iter().next() {
            return Ok(single);
        }
        return Err(BlockError::Container("vmdk: empty descriptor".into()));
    }
    let id = SourceId::new(format!("vmdk-flat:{}:{}", path.display(), parts.len()));
    Ok(Arc::new(ConcatSource::new(parts, id)))
}
