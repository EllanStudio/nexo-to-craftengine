//! nexo2ce CLI entry point (Rust rewrite).
//!
//! Port of legacy/src/cli.ts.

use std::path::PathBuf;
use std::process::ExitCode;

use nexo2ce::converter::{convert, ConvertOptions};
use nexo2ce::{ClientMode, CmdPolicy};

const HELP: &str = "Nexo 1.26 -> CraftEngine 26.8 semantic converter (Rust)

Usage:
  nexo2ce <Nexo目录> <CE输出目录> [options]

Options:
  --namespace <id>                Explicitly rename IDs (default: auto-detect Nexo source namespace)
  --client-mode <mode>            modern | hybrid | legacy (default: hybrid)
  --cmd-policy <policy>           preserve | allocate | omit (default: preserve)
  --strict                        Fail if any conversion is diagnosed as lossy
  --force                         Replace a non-empty output directory
  --no-audit                      Skip model/texture resource graph audit
  -h, --help                      Show this help
  -v, --version                   Show version

Important:
  Unqualified Minecraft resource locations keep the vanilla default namespace 'minecraft'.
  Use --cmd-policy allocate only when all Nexo item configs are present; allocation is material-scoped.";

enum Parsed {
    Help,
    Version,
    Run(ConvertOptions),
}

fn parse_arguments(args: &[String]) -> Result<Parsed, String> {
    let mut positional: Vec<String> = Vec::new();
    let mut values: Vec<(String, String)> = Vec::new();
    let mut strict = false;
    let mut force = false;
    let mut audit = true;
    let mut index = 0usize;
    while index < args.len() {
        let argument = &args[index];
        if argument == "-h" || argument == "--help" {
            return Ok(Parsed::Help);
        }
        if argument == "-v" || argument == "--version" {
            return Ok(Parsed::Version);
        }
        if argument == "--strict" {
            strict = true;
            index += 1;
            continue;
        }
        if argument == "--force" {
            force = true;
            index += 1;
            continue;
        }
        if argument == "--no-audit" {
            audit = false;
            index += 1;
            continue;
        }
        if ["--namespace", "--client-mode", "--cmd-policy"].contains(&argument.as_str()) {
            index += 1;
            let Some(next) = args.get(index) else {
                return Err(format!("Missing value after {}", argument));
            };
            values.push((argument.clone(), next.clone()));
            index += 1;
            continue;
        }
        if argument.starts_with('-') {
            return Err(format!("Unknown option: {}", argument));
        }
        if argument == "convert" && positional.is_empty() {
            index += 1;
            continue;
        }
        positional.push(argument.clone());
        index += 1;
    }
    if positional.len() != 2 {
        return Err(format!("Expected Nexo input and CraftEngine output directories\n\n{}", HELP));
    }
    let value = |name: &str| values.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone());
    let client_mode_raw = value("--client-mode").unwrap_or_else(|| "hybrid".to_string());
    let cmd_policy_raw = value("--cmd-policy").unwrap_or_else(|| "preserve".to_string());
    let Some(client_mode) = ClientMode::parse(&client_mode_raw) else {
        return Err(format!("Invalid --client-mode: {}", client_mode_raw));
    };
    let Some(cmd_policy) = CmdPolicy::parse(&cmd_policy_raw) else {
        return Err(format!("Invalid --cmd-policy: {}", cmd_policy_raw));
    };
    let resolve = |path: &str| std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
    Ok(Parsed::Run(ConvertOptions {
        input: resolve(&positional[0]).display().to_string(),
        output: resolve(&positional[1]).display().to_string(),
        namespace: value("--namespace"),
        source_namespace: None,
        client_mode,
        cmd_policy,
        strict,
        force,
        audit,
    }))
}
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = match parse_arguments(&args) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("nexo2ce: {}", message);
            return ExitCode::from(1);
        }
    };
    match parsed {
        Parsed::Help => {
            println!("{}", HELP);
            ExitCode::SUCCESS
        }
        Parsed::Version => {
            println!("{}", nexo2ce::VERSION);
            ExitCode::SUCCESS
        }
        Parsed::Run(options) => match convert(&options) {
            Ok(result) => {
                let counts = result.diagnostics.counts();
                println!(
                    "Converted {} items, {} categories, {} furniture, {} blocks, {} recipes, {} sounds, {} glyph images.",
                    result.item_count,
                    result.category_count,
                    result.furniture_count,
                    result.block_count,
                    result.recipe_count,
                    result.sound_count,
                    result.glyph_count
                );
                println!(
                    "Copied {} resource files. Diagnostics: {} errors, {} warnings, {} lossy.",
                    result.resource_count, counts.error, counts.warning, counts.lossy
                );
                for line in result.diagnostics.format_lines().iter().take(100) {
                    eprintln!("{}", line);
                }
                if result.diagnostics.items.len() > 100 {
                    eprintln!("... {} more diagnostics are in the JSON report.", result.diagnostics.items.len() - 100);
                }
                if let Some(report_file) = &result.report_file {
                    println!("Report: {}", report_file);
                }
                if result.success {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::from(2)
                }
            }
            Err(error) => {
                eprintln!("nexo2ce: {}", error);
                ExitCode::from(1)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn parses_positional_and_flags() {
        let Parsed::Run(options) = parse_arguments(&args(&["in", "out", "--strict", "--force", "--no-audit"])).unwrap()
        else {
            panic!("expected run");
        };
        assert!(options.input.ends_with("in"));
        assert!(options.output.ends_with("out"));
        assert!(options.strict && options.force && !options.audit);
    }

    #[test]
    fn rejects_unknown_options_and_bad_modes() {
        assert!(parse_arguments(&args(&["in", "out", "--bogus"])).is_err());
        assert!(parse_arguments(&args(&["in", "out", "--client-mode", "nope"])).is_err());
        assert!(matches!(parse_arguments(&args(&["--help"])).unwrap(), Parsed::Help));
    }
}
