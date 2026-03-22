use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum LanguageArg {
    Python,
    Octave,
    Rust,
}

#[derive(Parser, Debug)]
#[command(name = "codeflow")]
#[command(about = "CodeFlow unified CLI: extract graphs, render flowcharts, or run both.")]
struct Cli {
    #[command(subcommand)]
    cmd: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Extract(ExtractArgs),
    Render(RenderArgs),
    Build(BuildArgs),
}

#[derive(Args, Debug)]
struct ExtractArgs {
    input_file: PathBuf,

    #[arg(short, long)]
    output: Option<PathBuf>,

    #[arg(long = "lang", value_enum)]
    lang: LanguageArg,

    #[arg(long, default_value = "")]
    function: String,

    #[arg(long)]
    no_compact: bool,

    #[arg(long, default_value_t = 5)]
    compact_max_stmts: usize,
}

#[derive(Args, Debug)]
struct RenderArgs {
    graph_json: PathBuf,

    #[arg(short, long)]
    output: Option<PathBuf>,

    #[arg(long)]
    font_size: Option<f32>,

    #[arg(long)]
    strict_report: bool,

    #[arg(long)]
    hide_labels: bool,

    #[arg(long)]
    transparent_bg: bool,

    #[arg(long)]
    separate_layers: bool,
}

#[derive(Args, Debug)]
struct BuildArgs {
    input_file: PathBuf,

    #[arg(long = "lang", value_enum)]
    lang: LanguageArg,

    #[arg(long)]
    graph: Option<PathBuf>,

    #[arg(short, long)]
    output: Option<PathBuf>,

    #[arg(long, default_value = "")]
    function: String,

    #[arg(long)]
    no_compact: bool,

    #[arg(long, default_value_t = 5)]
    compact_max_stmts: usize,

    #[arg(long)]
    font_size: Option<f32>,

    #[arg(long)]
    strict_report: bool,

    #[arg(long)]
    hide_labels: bool,

    #[arg(long)]
    transparent_bg: bool,

    #[arg(long)]
    separate_layers: bool,
}

fn sibling_bin_path(name: &str) -> Result<PathBuf> {
    let exe = std::env::current_exe().context("failed to resolve current executable path")?;
    let dir = exe.parent().with_context(|| {
        format!(
            "failed to resolve executable directory for {}",
            exe.display()
        )
    })?;
    Ok(dir.join(name))
}

fn find_project_root() -> Result<PathBuf> {
    let mut dir = std::env::current_dir().context("failed to resolve current directory")?;
    loop {
        if dir.join("Cargo.toml").exists() {
            return Ok(dir);
        }
        if !dir.pop() {
            bail!("could not locate Cargo.toml for cargo fallback");
        }
    }
}

fn run_bin(name: &str, args: &[String]) -> Result<()> {
    let candidate = sibling_bin_path(name)?;
    let exe = std::env::current_exe().context("failed to resolve current executable path")?;
    let running_from_target = exe
        .components()
        .any(|c| c.as_os_str().to_string_lossy() == "target");

    // In dev flows (`cargo run ... --bin codeflow`), prefer cargo-run subcommands
    // so parser/renderer changes are always picked up even if sibling bins are stale.
    let status = if running_from_target {
        let root = find_project_root()?;
        Command::new("cargo")
            .arg("run")
            .arg("--release")
            .arg("--bin")
            .arg(name)
            .arg("--")
            .args(args)
            .current_dir(root)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .with_context(|| format!("failed to execute cargo-run for {}", name))?
    } else if candidate.exists() {
        Command::new(&candidate)
            .args(args)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .with_context(|| format!("failed to execute {}", candidate.display()))?
    } else {
        let root = find_project_root()?;
        Command::new("cargo")
            .arg("run")
            .arg("--release")
            .arg("--bin")
            .arg(name)
            .arg("--")
            .args(args)
            .current_dir(root)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .with_context(|| format!("failed to execute cargo fallback for {}", name))?
    };

    if !status.success() {
        bail!("{} failed with status {}", name, status);
    }
    Ok(())
}

fn default_png_path_from_input(input: &Path) -> PathBuf {
    let parent = input.parent().unwrap_or_else(|| Path::new("."));
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("flowchart");
    parent.join(format!("{}_flowchart.png", stem))
}

fn default_png_path(graph: &Path) -> PathBuf {
    let parent = graph.parent().unwrap_or_else(|| Path::new("."));
    let stem = graph
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("flowchart");
    parent.join(format!("{}_flowchart.png", stem))
}

fn temp_graph_path_for_build(input: &Path) -> PathBuf {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("source");
    let pid = std::process::id();
    std::env::temp_dir().join(format!("codeflow_{}_{}.graph", stem, pid))
}

fn run_extract(a: ExtractArgs) -> Result<()> {
    let mut args = vec![a.input_file.display().to_string()];
    args.push("--lang".to_string());
    args.push(
        match a.lang {
            LanguageArg::Python => "python",
            LanguageArg::Octave => "octave",
            LanguageArg::Rust => "rust",
        }
        .to_string(),
    );
    if let Some(o) = a.output {
        args.push("-o".to_string());
        args.push(o.display().to_string());
    }
    if !a.function.is_empty() {
        args.push("--function".to_string());
        args.push(a.function);
    }
    if a.no_compact {
        args.push("--no-compact".to_string());
    }
    if a.compact_max_stmts != 5 {
        args.push("--compact-max-stmts".to_string());
        args.push(a.compact_max_stmts.to_string());
    }

    run_bin("codeflow-extract", &args)
}

fn run_render(a: RenderArgs) -> Result<()> {
    let mut args = vec![a.graph_json.display().to_string()];
    if let Some(o) = a.output {
        args.push("-o".to_string());
        args.push(o.display().to_string());
    }
    if let Some(sz) = a.font_size {
        args.push("--font-size".to_string());
        args.push(sz.to_string());
    }
    if a.strict_report {
        args.push("--strict-report".to_string());
    }
    if a.hide_labels {
        args.push("--hide-labels".to_string());
    }
    if a.transparent_bg {
        args.push("--transparent-bg".to_string());
    }
    if a.separate_layers {
        args.push("--separate-layers".to_string());
    }

    run_bin("codeflow-render", &args)
}

fn run_build(a: BuildArgs) -> Result<()> {
    let using_temp_graph = a.graph.is_none();
    let graph_path = a
        .graph
        .clone()
        .unwrap_or_else(|| temp_graph_path_for_build(&a.input_file));
    let png_path = a.output.clone().unwrap_or_else(|| {
        if using_temp_graph {
            default_png_path_from_input(&a.input_file)
        } else {
            default_png_path(&graph_path)
        }
    });

    run_extract(ExtractArgs {
        input_file: a.input_file,
        output: Some(graph_path.clone()),
        lang: a.lang,
        function: a.function,
        no_compact: a.no_compact,
        compact_max_stmts: a.compact_max_stmts,
    })?;

    let render_result = run_render(RenderArgs {
        graph_json: graph_path.clone(),
        output: Some(png_path),
        font_size: a.font_size,
        strict_report: a.strict_report,
        hide_labels: a.hide_labels,
        transparent_bg: a.transparent_bg,
        separate_layers: a.separate_layers,
    });

    if using_temp_graph {
        let _ = std::fs::remove_file(&graph_path);
    }

    render_result
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Commands::Extract(a) => run_extract(a),
        Commands::Render(a) => run_render(a),
        Commands::Build(a) => run_build(a),
    }
}
