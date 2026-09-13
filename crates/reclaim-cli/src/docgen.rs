//! Documentation generator (feature `docgen`, invoked via `RECLAIM_GEN_DOCS`).
//!
//! Emits, all from the single clap definition in `main.rs` (so the docs can
//! never drift from the CLI):
//!   * `<out>/man/reclaim*.1`     — roff man pages (root + every subcommand)
//!   * `<out>/completions/`       — bash / zsh / fish completions
//!   * `<out>/cli.md`             — a Markdown reference (this becomes docs/cli.md)
//!
//! Not part of the shipped binary — see the `docgen` feature note in Cargo.toml.

use crate::Cli;
use clap::CommandFactory;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;
use std::process::ExitCode;

pub fn run(out: &Path) -> ExitCode {
    match generate(out) {
        Ok(()) => {
            eprintln!("wrote CLI docs to {}", out.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("docgen error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn generate(out: &Path) -> std::io::Result<()> {
    let man_dir = out.join("man");
    let comp_dir = out.join("completions");
    std::fs::create_dir_all(&man_dir)?;
    std::fs::create_dir_all(&comp_dir)?;

    let mut cmd = Cli::command();
    cmd.build();

    // --- man pages: root + one per subcommand ---------------------------------
    write_man(&cmd, "reclaim", &man_dir)?;
    for sub in cmd.get_subcommands() {
        let name = format!("reclaim-{}", sub.get_name());
        write_man(sub, &name, &man_dir)?;
    }

    // --- shell completions ----------------------------------------------------
    use clap_complete::shells::{Bash, Fish, Zsh};
    clap_complete::generate_to(Bash, &mut cmd, "reclaim", &comp_dir)?;
    clap_complete::generate_to(Zsh, &mut cmd, "reclaim", &comp_dir)?;
    clap_complete::generate_to(Fish, &mut cmd, "reclaim", &comp_dir)?;

    // --- Markdown reference ---------------------------------------------------
    std::fs::write(out.join("cli.md"), render_markdown(&mut cmd))?;
    Ok(())
}

fn write_man(cmd: &clap::Command, name: &str, dir: &Path) -> std::io::Result<()> {
    let man = clap_mangen::Man::new(cmd.clone()).title(name.to_uppercase());
    let mut buf: Vec<u8> = Vec::new();
    man.render(&mut buf)?;
    let mut f = std::fs::File::create(dir.join(format!("{name}.1")))?;
    f.write_all(&buf)
}

/// Render a Markdown reference by walking the clap command tree. Each command's
/// own `--help` text is embedded verbatim so the doc is authoritative.
fn render_markdown(cmd: &mut clap::Command) -> String {
    let mut s = String::new();
    let version = cmd.get_version().unwrap_or("");
    let _ = writeln!(s, "# `reclaim` command-line reference\n");
    let _ = writeln!(
        s,
        "_Generated from the clap definition (`cargo run -p reclaim-cli --features \
         docgen`, via `scripts/gen-cli-docs.sh`) — do not edit by hand. Version {version}._\n"
    );
    let _ = writeln!(
        s,
        "Every device open is `O_RDONLY`; sources are never written. See \
         [docs/plan/07-cli-spec.md](plan/07-cli-spec.md) for the design spec and \
         the exit-code contract below.\n"
    );

    // Top-level usage + global options.
    let _ = writeln!(s, "## Synopsis\n\n```text");
    let _ = write!(s, "{}", cmd.render_long_help());
    let _ = writeln!(s, "```\n");

    // Exit codes (mirrors src/exit.rs). Documented here because clap does not
    // carry them.
    let _ = writeln!(s, "## Exit codes\n");
    let _ = writeln!(s, "| code | meaning |");
    let _ = writeln!(s, "|---|---|");
    for (code, meaning) in EXIT_CODES {
        let _ = writeln!(s, "| {code} | {meaning} |");
    }
    let _ = writeln!(s);

    // One section per subcommand.
    let _ = writeln!(s, "## Commands\n");
    let names: Vec<String> = cmd
        .get_subcommands()
        .map(|c| c.get_name().to_string())
        .collect();
    for name in names {
        if let Some(sub) = cmd.find_subcommand_mut(&name) {
            let about = sub.get_about().map(|a| a.to_string()).unwrap_or_default();
            let _ = writeln!(s, "### `reclaim {name}`\n");
            if !about.is_empty() {
                let _ = writeln!(s, "{about}\n");
            }
            let _ = writeln!(s, "```text");
            let _ = write!(s, "{}", sub.render_long_help());
            let _ = writeln!(s, "```\n");
        }
    }
    s
}

/// The exit-code contract (src/exit.rs / docs/plan/07 §3).
const EXIT_CODES: &[(&str, &str)] = &[
    ("0", "success"),
    ("1", "usage / bad arguments"),
    ("2", "permission / root / Full Disk Access problem (`reclaim doctor` explains)"),
    ("3", "completed with warnings (bad sectors, partial/truncated files)"),
    ("4", "interrupted — session is resumable (`scan --resume`)"),
    ("5", "refused for safety (same-device destination, or a writable mount) — override with `--allow-same-device-i-accept-data-loss`"),
    ("6", "source not found or vanished mid-scan"),
    ("7", "internal error (bug)"),
];
