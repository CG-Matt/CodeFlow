use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use serde_json::Value;

mod source_parsers;

use source_parsers::octave::OctaveSourceParser;
use source_parsers::python::PythonSourceParser;
use source_parsers::rust::RustSourceParser;
use source_parsers::{LanguageKind, ParseRequest, SourceParser};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum LanguageArg {
    Python,
    Octave,
    Rust,
}

#[derive(Parser, Debug)]
#[command(name = "codeflow-extract")]
#[command(about = "CodeFlow source parser with language-aware backends.")]
struct Cli {
    input_file: PathBuf,

    #[arg(short, long)]
    output: Option<PathBuf>,

    #[arg(long = "lang", value_enum)]
    lang: LanguageArg,

    #[arg(
        long,
        default_value = "",
        help = "Function name for Python parser (default: module body). Use 'main' to target main()."
    )]
    function: String,

    #[arg(long, help = "Disable process-block compaction for Python parser.")]
    no_compact: bool,

    #[arg(
        long,
        default_value_t = 5,
        help = "Max linear process statements merged into one block for Python parser."
    )]
    compact_max_stmts: usize,
}

fn infer_output(input: &PathBuf, explicit: Option<PathBuf>) -> PathBuf {
    explicit.unwrap_or_else(|| input.with_extension("graph"))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let output = infer_output(&cli.input_file, cli.output.clone());
    let language = match cli.lang {
        LanguageArg::Python => LanguageKind::Python,
        LanguageArg::Octave => LanguageKind::OctaveMatlab,
        LanguageArg::Rust => LanguageKind::Rust,
    };

    let req = ParseRequest {
        input_file: cli.input_file.clone(),
        output_file: output.clone(),
        function: cli.function.clone(),
        no_compact: cli.no_compact,
        compact_max_stmts: cli.compact_max_stmts.max(1),
    };

    match language {
        LanguageKind::Python => {
            let parser = PythonSourceParser;
            parser.parse_to_graph_json(&req)?;
            println!("Language: python");
        }
        LanguageKind::OctaveMatlab => {
            let parser = OctaveSourceParser;
            parser.parse_to_graph_json(&req)?;
            println!("Language: octave/matlab");
        }
        LanguageKind::Rust => {
            let parser = RustSourceParser;
            parser.parse_to_graph_json(&req)?;
            println!("Language: rust");
        }
    }

    let bytes = std::fs::metadata(&output)
        .with_context(|| format!("failed to stat output {}", output.display()))?
        .len();
    let raw = std::fs::read_to_string(&output)
        .with_context(|| format!("failed to read {}", output.display()))?;
    let graph: Value = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse output graph JSON {}", output.display()))?;
    let nodes = graph
        .get("node_count")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            graph
                .get("nodes")
                .and_then(|v| v.as_array())
                .map(|a| a.len() as u64)
        })
        .unwrap_or(0);
    let edges = graph
        .get("edge_count")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            graph
                .get("edges")
                .and_then(|v| v.as_array())
                .map(|a| a.len() as u64)
        })
        .unwrap_or(0);
    let start = graph.get("start").and_then(|v| v.as_str()).unwrap_or("?");
    let end = graph.get("end").and_then(|v| v.as_str()).unwrap_or("?");
    println!("Wrote {} ({} bytes)", output.display(), bytes);
    println!(
        "Graph: nodes={}, edges={}, start={}, end={}",
        nodes, edges, start, end
    );
    Ok(())
}
