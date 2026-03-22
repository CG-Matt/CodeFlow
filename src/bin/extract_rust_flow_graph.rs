use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Parser;
use serde::Serialize;

#[derive(Parser, Debug)]
#[command(name = "codeflow-extract-rust")]
#[command(about = "CodeFlow Rust parser backend.")]
struct Cli {
    input_file: PathBuf,

    #[arg(short, long)]
    output: Option<PathBuf>,

    #[arg(
        long,
        default_value = "",
        help = "Function name to parse. Empty means module body."
    )]
    function: String,

    #[arg(long)]
    no_compact: bool,

    #[arg(long, default_value_t = 5)]
    compact_max_stmts: usize,
}

#[derive(Debug, Clone)]
enum TokKind {
    Code(String),
    LBrace,
    RBrace,
}

#[derive(Debug, Clone)]
struct Tok {
    kind: TokKind,
    line: usize,
}

#[derive(Debug, Clone)]
enum AstNode {
    Process {
        text: String,
        line_start: usize,
        line_end: usize,
    },
    Function {
        header: String,
        line_start: usize,
        line_end: usize,
        body: Vec<AstNode>,
    },
    Loop {
        header: String,
        line_start: usize,
        line_end: usize,
        body: Vec<AstNode>,
    },
    Decision {
        text: String,
        line_start: usize,
        line_end: usize,
        branches: Vec<(String, Vec<AstNode>)>,
        default_branch: Option<Vec<AstNode>>,
    },
}

#[derive(Debug, Clone, Serialize)]
struct Node {
    id: String,
    shape: String,
    text: String,
    line_start: Option<usize>,
    line_end: Option<usize>,
    lane: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    level: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
struct Edge {
    from: String,
    to: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    back_edge: bool,
}

#[derive(Debug, Serialize)]
struct Payload {
    source: String,
    target: String,
    node_count: usize,
    edge_count: usize,
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    start: String,
    end: String,
}

#[derive(Debug, Clone)]
struct Exit {
    node_id: String,
    label: Option<String>,
}

fn strip_comments(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0usize;
    let mut in_line = false;
    let mut in_block = 0i32;
    let mut in_single = false;
    let mut in_double = false;

    while i < chars.len() {
        let ch = chars[i];
        let nx = if i + 1 < chars.len() {
            Some(chars[i + 1])
        } else {
            None
        };

        if in_line {
            if ch == '\n' {
                in_line = false;
                out.push('\n');
            }
            i += 1;
            continue;
        }
        if in_block > 0 {
            if ch == '/' && nx == Some('*') {
                in_block += 1;
                i += 2;
                continue;
            }
            if ch == '*' && nx == Some('/') {
                in_block -= 1;
                i += 2;
                continue;
            }
            if ch == '\n' {
                out.push('\n');
            }
            i += 1;
            continue;
        }

        if !in_single && !in_double {
            if ch == '/' && nx == Some('/') {
                in_line = true;
                i += 2;
                continue;
            }
            if ch == '/' && nx == Some('*') {
                in_block = 1;
                i += 2;
                continue;
            }
        }

        if ch == '\\' && (in_single || in_double) {
            out.push(ch);
            if let Some(n) = nx {
                out.push(n);
                i += 2;
                continue;
            }
        }

        if ch == '\'' && !in_double {
            in_single = !in_single;
        } else if ch == '"' && !in_single {
            in_double = !in_double;
        }

        out.push(ch);
        i += 1;
    }

    out
}

fn tokenize(src: &str) -> Vec<Tok> {
    let mut out: Vec<Tok> = vec![];
    for (ln0, line) in src.lines().enumerate() {
        let ln = ln0 + 1;
        let chars: Vec<char> = line.chars().collect();
        let mut cur = String::new();
        let mut in_single = false;
        let mut in_double = false;
        let mut i = 0usize;
        while i < chars.len() {
            let ch = chars[i];
            if ch == '\\' && (in_single || in_double) {
                cur.push(ch);
                if i + 1 < chars.len() {
                    cur.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
            }
            if ch == '\'' && !in_double {
                in_single = !in_single;
                cur.push(ch);
                i += 1;
                continue;
            }
            if ch == '"' && !in_single {
                in_double = !in_double;
                cur.push(ch);
                i += 1;
                continue;
            }
            if !in_single && !in_double {
                if ch == '{' {
                    let t = cur.trim();
                    if !t.is_empty() {
                        out.push(Tok {
                            kind: TokKind::Code(t.to_string()),
                            line: ln,
                        });
                    }
                    cur.clear();
                    out.push(Tok {
                        kind: TokKind::LBrace,
                        line: ln,
                    });
                    i += 1;
                    continue;
                }
                if ch == '}' {
                    let t = cur.trim();
                    if !t.is_empty() {
                        out.push(Tok {
                            kind: TokKind::Code(t.to_string()),
                            line: ln,
                        });
                    }
                    cur.clear();
                    out.push(Tok {
                        kind: TokKind::RBrace,
                        line: ln,
                    });
                    i += 1;
                    continue;
                }
                if ch == ';' {
                    let t = cur.trim();
                    if !t.is_empty() {
                        out.push(Tok {
                            kind: TokKind::Code(t.to_string()),
                            line: ln,
                        });
                    }
                    cur.clear();
                    i += 1;
                    continue;
                }
            }
            cur.push(ch);
            i += 1;
        }
        let t = cur.trim();
        if !t.is_empty() {
            out.push(Tok {
                kind: TokKind::Code(t.to_string()),
                line: ln,
            });
        }
    }
    out
}

fn extract_function_slice(src: &str, fn_name: &str) -> Result<String> {
    let lines: Vec<&str> = src.lines().collect();
    let needle = format!("fn {}", fn_name);
    let mut start_line: Option<usize> = None;
    for (i, ln) in lines.iter().enumerate() {
        let t = ln.trim_start();
        if t.starts_with("fn ") && t.contains(&needle) {
            start_line = Some(i);
            break;
        }
    }
    let s = start_line.with_context(|| format!("Function '{}' not found", fn_name))?;

    let mut out = String::new();
    let mut found_open = false;
    let mut depth = 0i32;
    for ln in &lines[s..] {
        out.push_str(ln);
        out.push('\n');
        for ch in ln.chars() {
            if ch == '{' {
                depth += 1;
                found_open = true;
            } else if ch == '}' {
                depth -= 1;
            }
        }
        if found_open && depth <= 0 {
            break;
        }
    }

    if !found_open {
        bail!("Function '{}' block not found", fn_name);
    }
    Ok(out)
}

fn summarize_stmt(code: &str) -> String {
    let s = code.trim();
    if s.starts_with("return") {
        return "Return".to_string();
    }
    if s == "break" {
        return "break".to_string();
    }
    if s == "continue" {
        return "continue".to_string();
    }
    if s.starts_with("let ") && s.contains('=') {
        let rest = s.trim_start_matches("let ").trim_start();
        let rest = rest.trim_start_matches("mut ").trim_start();
        let name = rest.split([':', '=', ' ', ',']).next().unwrap_or("").trim();
        if !name.is_empty() {
            return format!("Assign: {}", name);
        }
    }
    if s.contains("+=") || s.contains("-=") || s.contains("*=") || s.contains("/=") {
        let lhs = s.split('=').next().unwrap_or("").trim();
        return format!("Update: {}", lhs);
    }
    if s.starts_with("fn ") {
        let head = s
            .split('{')
            .next()
            .unwrap_or(s)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        return head;
    }
    format!("Expr: {}", s.chars().take(120).collect::<String>())
}

fn parse_match(tok: &Tok, toks: &[Tok], i: &mut usize) -> AstNode {
    let text = match &tok.kind {
        TokKind::Code(s) => s.clone(),
        _ => String::new(),
    };
    let expr = text.trim_start_matches("match").trim().to_string();
    let start_line = tok.line;

    let mut branches: Vec<(String, Vec<AstNode>)> = vec![];
    let mut default_branch: Option<Vec<AstNode>> = None;

    if *i < toks.len() && matches!(toks[*i].kind, TokKind::LBrace) {
        *i += 1;
        while *i < toks.len() {
            if matches!(toks[*i].kind, TokKind::RBrace) {
                *i += 1;
                break;
            }
            if let TokKind::Code(code) = &toks[*i].kind {
                if let Some((lhs, rhs)) = code.split_once("=>") {
                    let label = lhs.trim().trim_end_matches(',').to_string();
                    let mut body: Vec<AstNode> = vec![];
                    let rhs_t = rhs.trim().trim_end_matches(',').trim();
                    *i += 1;
                    if *i < toks.len() && matches!(toks[*i].kind, TokKind::LBrace) {
                        *i += 1;
                        body = parse_block(toks, i, true);
                    } else if !rhs_t.is_empty() {
                        body.push(AstNode::Process {
                            text: summarize_stmt(rhs_t),
                            line_start: tok.line,
                            line_end: tok.line,
                        });
                    }
                    if label == "_" {
                        default_branch = Some(body);
                    } else {
                        branches.push((format!("case {}", label), body));
                    }
                    continue;
                }
            }
            *i += 1;
        }
    }

    AstNode::Decision {
        text: format!("match {}", expr),
        line_start: start_line,
        line_end: tok.line,
        branches,
        default_branch,
    }
}

fn parse_if(tok: &Tok, toks: &[Tok], i: &mut usize) -> AstNode {
    let text = match &tok.kind {
        TokKind::Code(s) => s.clone(),
        _ => String::new(),
    };
    let cond = text.trim_start_matches("if").trim().to_string();
    let start_line = tok.line;

    let mut branches: Vec<(String, Vec<AstNode>)> = vec![];
    let mut default_branch: Option<Vec<AstNode>> = None;

    if *i < toks.len() && matches!(toks[*i].kind, TokKind::LBrace) {
        *i += 1;
        let then_body = parse_block(toks, i, true);
        branches.push(("yes".to_string(), then_body));
    }

    // else / else if chain
    while *i < toks.len() {
        match &toks[*i].kind {
            TokKind::Code(c) if c.trim_start().starts_with("else if ") => {
                let ec = c
                    .trim_start()
                    .trim_start_matches("else if")
                    .trim()
                    .to_string();
                let mut body: Vec<AstNode> = vec![];
                *i += 1;
                if *i < toks.len() && matches!(toks[*i].kind, TokKind::LBrace) {
                    *i += 1;
                    body = parse_block(toks, i, true);
                }
                let dn = AstNode::Decision {
                    text: format!("if {}", ec),
                    line_start: tok.line,
                    line_end: tok.line,
                    branches: vec![("yes".to_string(), body)],
                    default_branch: None,
                };
                if let Some(ref mut d) = default_branch {
                    d.push(dn);
                } else {
                    default_branch = Some(vec![dn]);
                }
            }
            TokKind::Code(c) if c.trim() == "else" => {
                *i += 1;
                if *i < toks.len() && matches!(toks[*i].kind, TokKind::LBrace) {
                    *i += 1;
                    default_branch = Some(parse_block(toks, i, true));
                }
                break;
            }
            _ => break,
        }
    }

    AstNode::Decision {
        text: format!("if {}", cond),
        line_start: start_line,
        line_end: tok.line,
        branches,
        default_branch,
    }
}

fn parse_block(toks: &[Tok], i: &mut usize, stop_on_rbrace: bool) -> Vec<AstNode> {
    let mut out: Vec<AstNode> = vec![];

    while *i < toks.len() {
        if stop_on_rbrace && matches!(toks[*i].kind, TokKind::RBrace) {
            *i += 1;
            break;
        }

        let tok = toks[*i].clone();
        match tok.kind {
            TokKind::LBrace => {
                *i += 1;
            }
            TokKind::RBrace => {
                *i += 1;
                if stop_on_rbrace {
                    break;
                }
            }
            TokKind::Code(code) => {
                let c = code.trim();
                *i += 1;
                if c.is_empty() {
                    continue;
                }

                if c.starts_with("if ") {
                    let node = parse_if(
                        &Tok {
                            kind: TokKind::Code(c.to_string()),
                            line: tok.line,
                        },
                        toks,
                        i,
                    );
                    out.push(node);
                    continue;
                }
                if c.starts_with("for ")
                    || c.starts_with("while ")
                    || c == "loop"
                    || c.starts_with("loop ")
                {
                    let mut body: Vec<AstNode> = vec![];
                    if *i < toks.len() && matches!(toks[*i].kind, TokKind::LBrace) {
                        *i += 1;
                        body = parse_block(toks, i, true);
                    }
                    out.push(AstNode::Loop {
                        header: c.to_string(),
                        line_start: tok.line,
                        line_end: tok.line,
                        body,
                    });
                    continue;
                }
                if c.starts_with("match ") {
                    let node = parse_match(
                        &Tok {
                            kind: TokKind::Code(c.to_string()),
                            line: tok.line,
                        },
                        toks,
                        i,
                    );
                    out.push(node);
                    continue;
                }
                if c.starts_with("fn ") {
                    let header = c.to_string();
                    let mut body: Vec<AstNode> = vec![];
                    if *i < toks.len() && matches!(toks[*i].kind, TokKind::LBrace) {
                        *i += 1;
                        body = parse_block(toks, i, true);
                    }
                    out.push(AstNode::Function {
                        header: summarize_stmt(&header),
                        line_start: tok.line,
                        line_end: tok.line,
                        body,
                    });
                    continue;
                }

                out.push(AstNode::Process {
                    text: summarize_stmt(c),
                    line_start: tok.line,
                    line_end: tok.line,
                });
            }
        }
    }

    out
}

fn compact_ast(stmts: Vec<AstNode>, max_group: usize) -> Vec<AstNode> {
    let mut out: Vec<AstNode> = vec![];
    let mut pending: Vec<(String, usize, usize)> = vec![];
    let group_size = max_group.max(1);

    let flush_pending = |out: &mut Vec<AstNode>, pending: &mut Vec<(String, usize, usize)>| {
        if pending.is_empty() {
            return;
        }
        let mut idx = 0usize;
        while idx < pending.len() {
            let end = (idx + group_size).min(pending.len());
            let chunk = &pending[idx..end];
            let text = if chunk.len() == 1 {
                chunk[0].0.clone()
            } else {
                chunk
                    .iter()
                    .map(|(t, _, _)| t.clone())
                    .collect::<Vec<_>>()
                    .join(" | ")
            };
            out.push(AstNode::Process {
                text,
                line_start: chunk.first().map(|v| v.1).unwrap_or(1),
                line_end: chunk.last().map(|v| v.2).unwrap_or(1),
            });
            idx = end;
        }
        pending.clear();
    };

    for st in stmts {
        match st {
            AstNode::Process {
                text,
                line_start,
                line_end,
            } => {
                pending.push((text, line_start, line_end));
            }
            AstNode::Function {
                header,
                line_start,
                line_end,
                body,
            } => {
                flush_pending(&mut out, &mut pending);
                out.push(AstNode::Function {
                    header,
                    line_start,
                    line_end,
                    body: compact_ast(body, group_size),
                });
            }
            AstNode::Loop {
                header,
                line_start,
                line_end,
                body,
            } => {
                flush_pending(&mut out, &mut pending);
                out.push(AstNode::Loop {
                    header,
                    line_start,
                    line_end,
                    body: compact_ast(body, group_size),
                });
            }
            AstNode::Decision {
                text,
                line_start,
                line_end,
                branches,
                default_branch,
            } => {
                flush_pending(&mut out, &mut pending);
                let branches = branches
                    .into_iter()
                    .map(|(label, body)| (label, compact_ast(body, group_size)))
                    .collect();
                let default_branch = default_branch.map(|body| compact_ast(body, group_size));
                out.push(AstNode::Decision {
                    text,
                    line_start,
                    line_end,
                    branches,
                    default_branch,
                });
            }
        }
    }

    flush_pending(&mut out, &mut pending);
    out
}

fn add_node(
    nodes: &mut Vec<Node>,
    next_id: &mut usize,
    shape: &str,
    text: String,
    line_start: Option<usize>,
    line_end: Option<usize>,
    lane: i32,
) -> String {
    let id = format!("n{}", *next_id);
    *next_id += 1;
    nodes.push(Node {
        id: id.clone(),
        shape: shape.to_string(),
        text: text.chars().take(180).collect(),
        line_start,
        line_end,
        lane,
        level: None,
    });
    id
}

fn connect(edges: &mut Vec<Edge>, incoming: &[Exit], dst: &str, default_label: Option<&str>) {
    for ex in incoming {
        edges.push(Edge {
            from: ex.node_id.clone(),
            to: dst.to_string(),
            label: ex
                .label
                .clone()
                .or_else(|| default_label.map(|s| s.to_string())),
            back_edge: false,
        });
    }
}

fn walk_block(
    nodes: &mut Vec<Node>,
    edges: &mut Vec<Edge>,
    next_id: &mut usize,
    stmts: &[AstNode],
    incoming: Vec<Exit>,
    lane: i32,
) -> Vec<Exit> {
    let mut cur = incoming;
    for st in stmts {
        match st {
            AstNode::Process {
                text,
                line_start,
                line_end,
            } => {
                let nid = add_node(
                    nodes,
                    next_id,
                    "process",
                    text.clone(),
                    Some(*line_start),
                    Some(*line_end),
                    lane,
                );
                connect(edges, &cur, &nid, None);
                cur = vec![Exit {
                    node_id: nid,
                    label: None,
                }];
            }
            AstNode::Function {
                header,
                line_start,
                line_end,
                body,
            } => {
                let fnn = add_node(
                    nodes,
                    next_id,
                    "process",
                    header.clone(),
                    Some(*line_start),
                    Some(*line_end),
                    lane,
                );
                connect(edges, &cur, &fnn, None);
                if body.is_empty() {
                    cur = vec![Exit {
                        node_id: fnn,
                        label: None,
                    }];
                } else {
                    cur = walk_block(
                        nodes,
                        edges,
                        next_id,
                        body,
                        vec![Exit {
                            node_id: fnn,
                            label: Some("enter".to_string()),
                        }],
                        lane + 1,
                    );
                }
            }
            AstNode::Loop {
                header,
                line_start,
                line_end,
                body,
            } => {
                let lid = add_node(
                    nodes,
                    next_id,
                    "loop",
                    header.clone(),
                    Some(*line_start),
                    Some(*line_end),
                    lane,
                );
                connect(edges, &cur, &lid, None);

                let body_out = if body.is_empty() {
                    vec![Exit {
                        node_id: lid.clone(),
                        label: Some("enter".to_string()),
                    }]
                } else {
                    walk_block(
                        nodes,
                        edges,
                        next_id,
                        body,
                        vec![Exit {
                            node_id: lid.clone(),
                            label: Some("enter".to_string()),
                        }],
                        lane,
                    )
                };
                for ex in body_out {
                    edges.push(Edge {
                        from: ex.node_id,
                        to: lid.clone(),
                        label: Some("next".to_string()),
                        back_edge: false,
                    });
                }
                cur = vec![Exit {
                    node_id: lid,
                    label: Some("done".to_string()),
                }];
            }
            AstNode::Decision {
                text,
                line_start,
                line_end,
                branches,
                default_branch,
            } => {
                let did = add_node(
                    nodes,
                    next_id,
                    "decision",
                    text.clone(),
                    Some(*line_start),
                    Some(*line_end),
                    lane,
                );
                connect(edges, &cur, &did, None);

                let mut out: Vec<Exit> = vec![];
                for (bi, (lbl, body)) in branches.iter().enumerate() {
                    let blane = if bi == 0 {
                        lane - 1
                    } else {
                        lane + bi as i32 - 1
                    };
                    if body.is_empty() {
                        out.push(Exit {
                            node_id: did.clone(),
                            label: Some(lbl.clone()),
                        });
                    } else {
                        out.extend(walk_block(
                            nodes,
                            edges,
                            next_id,
                            body,
                            vec![Exit {
                                node_id: did.clone(),
                                label: Some(lbl.clone()),
                            }],
                            blane,
                        ));
                    }
                }

                if let Some(defb) = default_branch {
                    if defb.is_empty() {
                        out.push(Exit {
                            node_id: did.clone(),
                            label: Some("else".to_string()),
                        });
                    } else {
                        out.extend(walk_block(
                            nodes,
                            edges,
                            next_id,
                            defb,
                            vec![Exit {
                                node_id: did.clone(),
                                label: Some("else".to_string()),
                            }],
                            lane + 1,
                        ));
                    }
                } else if branches.len() == 1 {
                    out.push(Exit {
                        node_id: did,
                        label: Some("no".to_string()),
                    });
                } else {
                    out.push(Exit {
                        node_id: did,
                        label: None,
                    });
                }
                cur = out;
            }
        }
    }
    cur
}

fn assign_levels(nodes: &mut [Node], edges: &mut [Edge], start_id: &str, end_id: &str) {
    let mut line_vals: Vec<usize> = nodes
        .iter()
        .filter_map(|n| n.line_start)
        .filter(|v| *v > 0)
        .collect();
    line_vals.sort_unstable();
    line_vals.dedup();
    let mut rank = HashMap::<usize, i32>::new();
    for (i, ln) in line_vals.iter().enumerate() {
        rank.insert(*ln, i as i32 + 1);
    }

    let mut level = HashMap::<String, i32>::new();
    for n in nodes.iter() {
        if n.id == start_id {
            level.insert(n.id.clone(), 0);
            continue;
        }
        if let Some(ls) = n.line_start {
            level.insert(n.id.clone(), *rank.get(&ls).unwrap_or(&1));
        } else {
            let mx = rank.values().copied().max().unwrap_or(1);
            level.insert(n.id.clone(), mx + 1);
        }
    }

    for _ in 0..(nodes.len() * 4) {
        let mut changed = false;
        for e in edges.iter() {
            if e.label.as_deref() == Some("next") {
                continue;
            }
            let a = *level.get(&e.from).unwrap_or(&0);
            let b = *level.get(&e.to).unwrap_or(&0);
            if b <= a {
                level.insert(e.to.clone(), a + 1);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    for n in nodes.iter_mut() {
        n.level = Some(*level.get(&n.id).unwrap_or(&0));
    }

    let mx = nodes.iter().filter_map(|n| n.level).max().unwrap_or(0);
    for n in nodes.iter_mut() {
        if n.id == end_id {
            n.level = Some(mx + 1);
            break;
        }
    }

    let mut lv = HashMap::<String, i32>::new();
    for n in nodes.iter() {
        lv.insert(n.id.clone(), n.level.unwrap_or(0));
    }
    for e in edges.iter_mut() {
        let src = *lv.get(&e.from).unwrap_or(&0);
        let dst = *lv.get(&e.to).unwrap_or(&0);
        e.back_edge = e.label.as_deref() == Some("next") || dst <= src;
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let input = cli.input_file;
    let output = cli.output.unwrap_or_else(|| input.with_extension("graph"));

    let raw = fs::read_to_string(&input)
        .with_context(|| format!("failed to read {}", input.display()))?;
    let source = if cli.function.trim().is_empty() {
        raw
    } else {
        extract_function_slice(&raw, cli.function.trim())?
    };
    let stripped = strip_comments(&source);
    let toks = tokenize(&stripped);

    let mut i = 0usize;
    let mut ast = parse_block(&toks, &mut i, false);
    if !cli.no_compact {
        ast = compact_ast(ast, cli.compact_max_stmts);
    }

    let mut nodes: Vec<Node> = vec![];
    let mut edges: Vec<Edge> = vec![];
    let mut next_id = 1usize;

    let start = add_node(
        &mut nodes,
        &mut next_id,
        "terminator",
        "Start".to_string(),
        Some(1),
        Some(1),
        0,
    );
    let exits = walk_block(
        &mut nodes,
        &mut edges,
        &mut next_id,
        &ast,
        vec![Exit {
            node_id: start.clone(),
            label: None,
        }],
        0,
    );
    let end = add_node(
        &mut nodes,
        &mut next_id,
        "terminator",
        "End".to_string(),
        None,
        None,
        0,
    );
    connect(&mut edges, &exits, &end, None);

    assign_levels(&mut nodes, &mut edges, &start, &end);

    let payload = Payload {
        source: input.display().to_string(),
        target: if cli.function.trim().is_empty() {
            "module".to_string()
        } else {
            cli.function.trim().to_string()
        },
        node_count: nodes.len(),
        edge_count: edges.len(),
        nodes,
        edges,
        start,
        end,
    };

    let text = serde_json::to_string_pretty(&payload)?;
    fs::write(&output, text).with_context(|| format!("failed to write {}", output.display()))?;
    println!("Wrote {}", output.display());
    println!(
        "Nodes: {}, edges: {}",
        payload.node_count, payload.edge_count
    );
    Ok(())
}
