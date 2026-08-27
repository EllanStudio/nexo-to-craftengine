//! nexo2ce CLI entry point (Rust rewrite).
//!
//! The full CLI semantics (namespace inference, client modes, cmd policy,
//! strict/force/no-audit flags) are ported from `legacy/src/cli.ts` as the
//! core library lands. This stub keeps the binary buildable in the meantime.

fn main() {
    eprintln!(
        "nexo2ce {}: Rust port in progress - CLI not yet available (reference: legacy/src/cli.ts)",
        nexo2ce::VERSION
    );
    std::process::exit(2);
}
