//! `reclaim sigs list|test` — signature catalog tools (docs/plan/07 §1).

use crate::exit::{CmdError, CmdResult, Exit};
use comfy_table::{Cell, Table};
use reclaim_carve::engine::CarveEngine;
use reclaim_carve::reader::MemReader;
use std::io::Read;
use std::path::Path;

/// `sigs list`.
pub fn list(json: bool, plain: bool) -> CmdResult {
    let sigs = reclaim_sigs::all();
    if json {
        let rows: Vec<serde_json::Value> = sigs
            .iter()
            .map(|s| {
                serde_json::json!({
                    "id": s.id,
                    "family": s.family,
                    "extensions": s.extensions,
                    "mime": s.mime,
                    "tier": s.tier,
                    "strategy": format!("{:?}", s.strategy).to_lowercase(),
                    "validator": s.validator,
                    "block_aligned": s.block_aligned,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".to_string())
        );
        return Ok(Exit::Success);
    }
    let mut table = Table::new();
    if plain {
        table.load_preset(comfy_table::presets::NOTHING);
    }
    table.set_header(vec!["ID", "FAMILY", "EXT", "TIER", "STRATEGY", "VALIDATOR"]);
    for s in sigs {
        table.add_row(vec![
            Cell::new(s.id),
            Cell::new(s.family),
            Cell::new(s.primary_ext()),
            Cell::new(s.tier),
            Cell::new(format!("{:?}", s.strategy).to_lowercase()),
            Cell::new(s.validator.unwrap_or("-")),
        ]);
    }
    println!("{table}");
    eprintln!("{} signatures.", sigs.len());
    Ok(Exit::Success)
}

/// `sigs test <file>`.
pub fn test(file: &Path, json: bool) -> CmdResult {
    let f = std::fs::File::open(file)
        .map_err(|e| CmdError::not_found(format!("cannot open {}: {e}", file.display())))?;
    reclaim_block::open_log::record_open(file, libc::O_RDONLY);
    // Read a bounded prefix for identification (whole file if small).
    let mut buf = Vec::new();
    f.take(64 * 1024 * 1024)
        .read_to_end(&mut buf)
        .map_err(|e| CmdError::internal(format!("read {}: {e}", file.display())))?;
    let reader = MemReader::new(&buf);
    let engine = CarveEngine::new();
    let results = engine.identify_at(&reader, 0, buf.len() as u64);

    if json {
        let rows: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                serde_json::json!({
                    "format": r.format,
                    "family": r.family,
                    "len": r.len,
                    "validity": r.validity.label(),
                    "score": r.score,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".to_string())
        );
        return Ok(Exit::Success);
    }

    if results.is_empty() {
        println!("no signature matched {}", file.display());
    } else {
        for r in &results {
            println!(
                "{:<14} len={:<10} {:<9} score={}",
                r.format,
                r.len,
                r.validity.label(),
                r.score
            );
        }
    }
    Ok(Exit::Success)
}
