use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context, Result};

pub mod octave;
pub mod python;
pub mod rust;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageKind {
    Python,
    OctaveMatlab,
    Rust,
}

#[derive(Debug, Clone)]
pub struct ParseRequest {
    pub input_file: PathBuf,
    pub output_file: PathBuf,
    pub function: String,
    pub no_compact: bool,
    pub compact_max_stmts: usize,
}

pub trait SourceParser {
    fn parse_to_graph_json(&self, req: &ParseRequest) -> Result<()>;
}

fn sibling_bin_path(name: &str) -> Result<PathBuf> {
    let exe = std::env::current_exe().context("failed to resolve current executable path")?;
    let dir = exe.parent().with_context(|| {
        format!(
            "failed to resolve executable directory for {}",
            exe.display()
        )
    })?;
    let candidate = dir.join(name);
    if candidate.exists() {
        Ok(candidate)
    } else {
        bail!("missing sibling parser binary: {}", candidate.display())
    }
}

pub fn run_sibling_parser_binary(name: &str, args: &[String]) -> Result<()> {
    let out = match sibling_bin_path(name) {
        Ok(bin) => Command::new(&bin)
            .args(args)
            .output()
            .with_context(|| format!("failed to execute parser binary {}", bin.display()))?,
        Err(_) => {
            let mut dir = std::env::current_exe()
                .context("failed to resolve current executable path")?
                .parent()
                .map(|p| p.to_path_buf())
                .context("failed to resolve executable directory")?;
            let mut project_root: Option<PathBuf> = None;
            loop {
                if dir.join("Cargo.toml").exists() {
                    project_root = Some(dir.clone());
                    break;
                }
                if !dir.pop() {
                    break;
                }
            }
            let root = project_root.context(
                "required parser binary not found and could not locate Cargo.toml for cargo-run fallback",
            )?;
            Command::new("cargo")
                .arg("run")
                .arg("--release")
                .arg("--bin")
                .arg(name)
                .arg("--")
                .args(args)
                .current_dir(&root)
                .output()
                .with_context(|| format!("failed to execute cargo fallback for {}", name))?
        }
    };
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        bail!(
            "{} failed (status: {}):\nstdout:\n{}\nstderr:\n{}",
            name,
            out.status,
            stdout,
            stderr
        );
    }
    Ok(())
}
