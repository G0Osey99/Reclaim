//! A generic APFS B-tree node walker (Apple File System Reference: "B-Trees",
//! `btree_node_phys_t`).
//!
//! One walker serves both tree shapes Reclaim needs:
//! * the **object map** tree — fixed key/value size, children referenced by
//!   physical block address; and
//! * the **filesystem** tree (`FSTREE`) — variable key/value size, children
//!   referenced by *virtual* object id (resolved through the volume object map).
//!
//! The difference is entirely captured by the `fetch` closure the caller passes
//! (paddr→block for the omap tree, oid→block for the FS tree), so the traversal
//! code is shared. Every access is bounds-checked; a crafted node can waste at
//! most `budget` node visits (DoS guard), never panic.

use crate::obj::{le_u16, le_u32, le_u64, ObjPhys, OBJ_BTREE, OBJ_BTREE_NODE};

const BTNODE_ROOT: u16 = 0x0001;
const BTNODE_LEAF: u16 = 0x0002;
const BTNODE_FIXED_KV_SIZE: u16 = 0x0004;

/// Fixed header size of `btree_node_phys_t` before the table space.
const BTN_HEADER: usize = 56;
/// `btree_info` trailer size present only in a root node.
const BTREE_INFO: usize = 40;

/// A parsed B-tree node header + geometry.
#[derive(Clone, Debug)]
pub struct Node {
    /// The raw node block.
    pub block: Vec<u8>,
    /// `btn_flags`.
    pub flags: u16,
    /// `btn_level` (0 = leaf level).
    pub level: u16,
    /// `btn_nkeys`.
    pub nkeys: u32,
    /// Table-space offset (from the end of the fixed header).
    pub table_off: usize,
    /// Table-space length.
    pub table_len: usize,
    /// Whether this node is a tree root (carries the `btree_info` trailer).
    pub is_root: bool,
    /// Whether keys/values are fixed-size (`BTNODE_FIXED_KV_SIZE`).
    pub fixed: bool,
}

impl Node {
    /// Parse a node from its block. Returns `None` if the block is not a
    /// plausible B-tree node.
    #[must_use]
    pub fn parse(block: Vec<u8>) -> Option<Node> {
        let obj = ObjPhys::parse(&block)?;
        if obj.kind() != OBJ_BTREE && obj.kind() != OBJ_BTREE_NODE {
            return None;
        }
        if block.len() < BTN_HEADER {
            return None;
        }
        let flags = le_u16(&block, 32);
        let level = le_u16(&block, 34);
        let nkeys = le_u32(&block, 36);
        let table_off = le_u16(&block, 40) as usize;
        let table_len = le_u16(&block, 42) as usize;
        Some(Node {
            is_root: flags & BTNODE_ROOT != 0,
            fixed: flags & BTNODE_FIXED_KV_SIZE != 0,
            flags,
            level,
            nkeys,
            table_off,
            table_len,
            block,
        })
    }

    /// True if this is a leaf node (holds records, not child pointers).
    #[must_use]
    pub fn is_leaf(&self) -> bool {
        self.flags & BTNODE_LEAF != 0 || self.level == 0
    }

    fn key_area(&self) -> usize {
        BTN_HEADER + self.table_off + self.table_len
    }

    fn val_base(&self) -> usize {
        // Values grow downward from the end of the node, before the root's
        // `btree_info` trailer.
        self.block
            .len()
            .saturating_sub(if self.is_root { BTREE_INFO } else { 0 })
    }

    fn toc_entry(&self, i: usize) -> Option<(usize, usize, usize, usize)> {
        // Returns (key_off, key_len, val_off, val_len). For fixed nodes the
        // lengths are not stored; callers ignore them for fixed trees.
        let toc = BTN_HEADER + self.table_off;
        if self.fixed {
            let base = toc + i * 4;
            Some((
                le_u16(&self.block, base) as usize,
                0,
                le_u16(&self.block, base + 2) as usize,
                0,
            ))
        } else {
            let base = toc + i * 8;
            Some((
                le_u16(&self.block, base) as usize,
                le_u16(&self.block, base + 2) as usize,
                le_u16(&self.block, base + 4) as usize,
                le_u16(&self.block, base + 6) as usize,
            ))
        }
    }

    /// The `i`-th record's key slice and value slice (for a leaf) or child
    /// pointer value (for an internal node). Fixed-size records fall back to a
    /// generous default length so the caller's own parser bounds-checks.
    fn record(&self, i: usize, fixed_key: usize, fixed_val: usize) -> Option<(&[u8], &[u8])> {
        let (koff, klen, voff, vlen) = self.toc_entry(i)?;
        let kstart = self.key_area().checked_add(koff)?;
        let klen = if self.fixed { fixed_key } else { klen };
        let key = self.block.get(kstart..kstart.saturating_add(klen))?;
        let vbase = self.val_base();
        let vstart = vbase.checked_sub(voff)?;
        let vlen = if self.fixed {
            if self.is_leaf() {
                fixed_val
            } else {
                8
            }
        } else {
            vlen
        };
        let val = self.block.get(vstart..vstart.saturating_add(vlen))?;
        Some((key, val))
    }
}

/// Fixed omap key/value sizes: `omap_key` = 16 (oid + xid), `omap_val` = 16.
pub const OMAP_KEY: usize = 16;
pub const OMAP_VAL: usize = 16;

/// Walk every leaf record of a B-tree, calling `visit(key, val)` for each.
///
/// * `root` is the already-fetched root node block.
/// * `fetch(child_ptr) -> Option<block>` resolves an internal node's child
///   pointer to its block (paddr→block for the omap; oid→block via omap for the
///   FS tree).
/// * `fixed_key`/`fixed_val` are used only for fixed-size trees (the omap).
/// * `budget` caps the number of nodes visited.
pub fn walk_leaves<F, V>(
    root: Node,
    fetch: &F,
    fixed_key: usize,
    fixed_val: usize,
    budget: &mut usize,
    visit: &mut V,
) where
    F: Fn(u64) -> Option<Vec<u8>>,
    V: FnMut(&[u8], &[u8]),
{
    // Dedup child pointers so a crafted tree that references the same node many
    // times (a DAG or cycle within a small image) cannot cause repeated reads —
    // this bounds the walk to the number of *distinct* nodes, not the number of
    // edges, and combines with `budget` to cap total work.
    let mut seen: std::collections::HashSet<u64> = std::collections::HashSet::new();
    let mut stack: Vec<Node> = vec![root];
    while let Some(node) = stack.pop() {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        // Cap by the table space too: each ToC entry is 4 bytes (fixed) or 8.
        let toc_cap = node.table_len / (if node.fixed { 4 } else { 8 });
        let nkeys = (node.nkeys.min(1 << 20) as usize).min(toc_cap);
        if node.is_leaf() {
            for i in 0..nkeys {
                if let Some((k, v)) = node.record(i, fixed_key, fixed_val) {
                    visit(k, v);
                }
            }
        } else {
            // Internal node: each value is an 8-byte child pointer.
            for i in 0..nkeys {
                if let Some((_, v)) = node.record(i, fixed_key, 8) {
                    let child_ptr = le_u64(v, 0);
                    if !seen.insert(child_ptr) {
                        continue; // already visited this node
                    }
                    if let Some(block) = fetch(child_ptr) {
                        if let Some(child) = Node::parse(block) {
                            // Guard against a self-referential/looping tree.
                            if child.level < node.level {
                                stack.push(child);
                            }
                        }
                    }
                }
            }
        }
    }
}
