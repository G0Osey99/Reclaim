//! `reclaim image` / `reclaim verify-image` (docs/plan/07 §2). The image writer
//! itself lives in reclaim-block (allow-listed); this module only drives it.

use crate::commands::common::open_source;
use crate::exit::{CmdError, CmdResult, Exit};
use crate::util::format_size;
use reclaim_block::imaging::{image, verify_image, Compression, HashAlgo, ImageOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// Flags for `image`.
pub struct ImageArgs<'a> {
    pub source: &'a str,
    pub out: PathBuf,
    pub map: Option<PathBuf>,
    pub block: Option<u32>,
    pub retries: u32,
    pub reverse_pass: bool,
    pub sparse: bool,
    pub zstd: bool,
    pub hash: String,
    pub resume: bool,
    pub quiet: bool,
}

/// Run `image`.
pub fn run(args: &ImageArgs) -> CmdResult {
    let (_r, src, _info) = open_source(args.source)?;
    let map_path = args
        .map
        .clone()
        .unwrap_or_else(|| default_map_path(&args.out));
    let hash_algo = HashAlgo::parse(&args.hash)
        .ok_or_else(|| CmdError::new(Exit::Usage, format!("bad --hash {:?}", args.hash)))?;
    let opts = ImageOptions {
        chunk_size: args
            .block
            .unwrap_or(reclaim_block::imaging::imager::DEFAULT_CHUNK),
        retries: args.retries,
        reverse_pass: args.reverse_pass,
        sparse: args.sparse && !args.zstd,
        compression: if args.zstd {
            Compression::Zstd
        } else {
            Compression::None
        },
        hash_algo,
        resume: args.resume,
    };

    let cancel = AtomicBool::new(false);
    let quiet = args.quiet;
    let outcome = image(
        &src,
        &args.out,
        &map_path,
        &opts,
        &mut |p| {
            if !quiet && p.total > 0 {
                let pct = p.done as f64 / p.total as f64 * 100.0;
                eprint!(
                    "\rimaging {pct:5.1}%  {} / {}  bad: {}    ",
                    format_size(p.done),
                    format_size(p.total),
                    p.bad_sectors
                );
            }
        },
        &cancel,
    )
    .map_err(|e| CmdError::internal(format!("imaging failed: {e}")))?;
    if !quiet {
        eprintln!();
    }

    if let Some(h) = &outcome.whole_hash {
        eprintln!(
            "image {} · map {} · {} = {}",
            args.out.display(),
            map_path.display(),
            hash_algo.label(),
            h
        );
    }
    if !outcome.complete {
        eprintln!("imaging interrupted — resume with --resume.");
        return Ok(Exit::Interrupted);
    }
    if outcome.had_bad {
        eprintln!(
            "completed with {} bad sector(s) (mapped).",
            outcome.bad_sectors
        );
        return Ok(Exit::Warnings);
    }
    Ok(Exit::Success)
}

/// Run `verify-image`.
pub fn verify(img: &Path, map: Option<&Path>) -> CmdResult {
    let map_path = map
        .map(Path::to_path_buf)
        .unwrap_or_else(|| default_map_path(img));
    let rep = verify_image(img, &map_path)
        .map_err(|e| CmdError::not_found(format!("verify failed: {e}")))?;
    println!(
        "chunks checked: {}  mismatches: {}  whole-hash: {}  bad sectors: {}",
        rep.chunks_checked,
        rep.chunk_mismatches,
        if rep.whole_ok { "OK" } else { "MISMATCH" },
        rep.bad_sectors
    );
    if rep.chunk_mismatches > 0 || !rep.whole_ok {
        return Ok(Exit::Warnings);
    }
    Ok(Exit::Success)
}

fn default_map_path(out: &Path) -> PathBuf {
    let mut s = out.as_os_str().to_os_string();
    s.push(".reclaim-map");
    PathBuf::from(s)
}
