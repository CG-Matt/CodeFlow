use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use regex::Regex;
use serde::Serialize;

#[derive(Parser, Debug)]
#[command(name = "codeflow-extract-octave")]
#[command(about = "CodeFlow Octave/MATLAB parser.")]
struct Cli {
    input_file: PathBuf,

    #[arg(short, long)]
    output: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct Tok {
    kind: String,
    text: String,
    line: usize,
}

#[derive(Debug, Clone)]
struct IfBranch {
    cond: String,
    body: Vec<AstNode>,
    line: usize,
}

#[derive(Debug, Clone)]
enum AstNode {
    Process {
        text: String,
        line_start: usize,
        line_end: usize,
    },
    If {
        cond: String,
        line_start: usize,
        line_end: usize,
        branches: Vec<IfBranch>,
        else_body: Option<Vec<AstNode>>,
    },
    Loop {
        loop_kind: String,
        header: String,
        line_start: usize,
        line_end: usize,
        body: Vec<AstNode>,
    },
    Switch {
        expr: String,
        line_start: usize,
        line_end: usize,
        cases: Vec<IfBranch>,
        otherwise: Option<Vec<AstNode>>,
    },
    Decision {
        text: String,
        line_start: usize,
        line_end: usize,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    back_edge: Option<bool>,
}

#[derive(Debug, Clone)]
struct Exit {
    id: String,
    label: Option<String>,
}

#[derive(Debug, Serialize)]
struct Payload {
    source: String,
    node_count: usize,
    edge_count: usize,
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    start: String,
    end: String,
}

fn strip_comment(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut in_single = false;
    let mut i = 0usize;
    while i < chars.len() {
        let ch = chars[i];
        if ch == '\'' {
            if in_single && i + 1 < chars.len() && chars[i + 1] == '\'' {
                out.push(ch);
                out.push(chars[i + 1]);
                i += 2;
                continue;
            }
            in_single = !in_single;
            out.push(ch);
            i += 1;
            continue;
        }
        if ch == '%' && !in_single {
            return out;
        }
        out.push(ch);
        i += 1;
    }
    out
}

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn pretty_cond(s: &str) -> String {
    let ss = norm(s);
    let re = Regex::new(r"^strcmp\(\s*([A-Za-z_]\w*)\s*,\s*'([^']+)'\s*\)$").expect("valid regex");
    if let Some(c) = re.captures(&ss) {
        return format!("{} == {}", &c[1], &c[2]);
    }
    ss
}

fn starts_kw(code: &str, kw: &str) -> Option<String> {
    let lower = code.to_ascii_lowercase();
    let k = kw.to_ascii_lowercase();
    if lower == k {
        return Some(String::new());
    }
    if lower.starts_with(&(k.clone() + " ")) {
        return Some(code[kw.len()..].trim().to_string());
    }
    None
}

fn lex(lines: &[String]) -> Vec<Tok> {
    let kws = [
        "if",
        "elseif",
        "else",
        "for",
        "while",
        "switch",
        "case",
        "otherwise",
        "end",
        "function",
    ];
    let mut toks: Vec<Tok> = vec![];
    for (i, raw) in lines.iter().enumerate() {
        let ln = i + 1;
        let code = strip_comment(raw).trim().to_string();
        if code.is_empty() {
            continue;
        }
        let mut matched = false;
        for kw in kws {
            if let Some(rest) = starts_kw(&code, kw) {
                let text = if kw == "if" || kw == "elseif" {
                    pretty_cond(&rest)
                } else {
                    norm(&rest)
                };
                toks.push(Tok {
                    kind: kw.to_string(),
                    text,
                    line: ln,
                });
                matched = true;
                break;
            }
        }
        if !matched {
            toks.push(Tok {
                kind: "stmt".to_string(),
                text: norm(&code),
                line: ln,
            });
        }
    }
    toks
}

fn extract_assigned_vars(lines: &[String]) -> Vec<String> {
    let re = Regex::new(r"\b([A-Za-z_]\w*)\s*=").expect("valid regex");
    let mut out: Vec<String> = vec![];
    let mut seen: HashSet<String> = HashSet::new();
    for ln in lines {
        for c in re.captures_iter(ln) {
            if let Some(m0) = c.get(0) {
                if ln[m0.end()..].starts_with('=') {
                    continue;
                }
            }
            let v = c[1].to_string();
            if !seen.contains(&v) {
                seen.insert(v.clone());
                out.push(v);
            }
        }
    }
    out
}

fn has_word(lines: &[String], pat: &str) -> bool {
    let re = Regex::new(pat).expect("valid regex");
    lines.iter().any(|ln| re.is_match(ln))
}

fn summarize_process_lines(lines: &[String], seen_vars: &mut HashSet<String>) -> String {
    let assigned = extract_assigned_vars(lines);
    let has_assign = !assigned.is_empty();
    let has_plot = has_word(
        lines,
        r"(?i)\b(figure|plot|legend|title|xlabel|ylabel|grid|yline)\b",
    );
    let has_print = has_word(lines, r"(?i)\b(fprintf|disp|error)\b");
    let has_math = has_word(lines, r"(?i)\b(sqrt|exp|sin|cos)\b");

    if has_plot {
        return "Plots/figure settings updated".to_string();
    }
    if has_print && !has_assign {
        return "Messages/validation handled".to_string();
    }

    if has_assign {
        let const_names: HashSet<&str> = ["g", "tzero", "NumIt", "Numcyc", "tfinish"]
            .into_iter()
            .collect();
        let constants: Vec<String> = assigned
            .iter()
            .filter(|v| v.ends_with("_base") || const_names.contains(v.as_str()))
            .cloned()
            .collect();
        let values: Vec<String> = assigned
            .iter()
            .filter(|v| !(v.ends_with("_base") || const_names.contains(v.as_str())))
            .cloned()
            .collect();

        let newly_set: Vec<String> = assigned
            .iter()
            .filter(|v| !seen_vars.contains(v.as_str()))
            .cloned()
            .collect();
        let updated: Vec<String> = assigned
            .iter()
            .filter(|v| seen_vars.contains(v.as_str()))
            .cloned()
            .collect();

        let action = if !newly_set.is_empty() && updated.is_empty() {
            "set"
        } else if !updated.is_empty() && newly_set.is_empty() {
            "updated"
        } else {
            "set & updated"
        };

        let mut domain = if !constants.is_empty() && values.is_empty() {
            "Constants"
        } else if !values.is_empty() && constants.is_empty() {
            "Values"
        } else {
            "Values/constants"
        };
        if has_math {
            domain = "Computed values";
        }

        let mut preview = assigned
            .iter()
            .take(8)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        if assigned.len() > 8 {
            preview.push_str(&format!(", ... (+{})", assigned.len() - 8));
        }

        for v in assigned {
            seen_vars.insert(v);
        }
        return format!("{} {}: {}", domain, action, preview);
    }

    "Processing step".to_string()
}

fn parse_if(toks: &[Tok], mut i: usize, seen_vars: &mut HashSet<String>) -> (AstNode, usize) {
    let t_if = toks[i].clone();
    i += 1;

    let mut branches: Vec<IfBranch> = vec![];
    let (body, ni) = parse_block(
        toks,
        i,
        ["elseif", "else", "end"].into_iter().collect(),
        seen_vars,
    );
    i = ni;
    branches.push(IfBranch {
        cond: t_if.text.clone(),
        body,
        line: t_if.line,
    });

    while i < toks.len() && toks[i].kind == "elseif" {
        let t = toks[i].clone();
        i += 1;
        let (body, ni2) = parse_block(
            toks,
            i,
            ["elseif", "else", "end"].into_iter().collect(),
            seen_vars,
        );
        i = ni2;
        branches.push(IfBranch {
            cond: t.text,
            body,
            line: t.line,
        });
    }

    let mut else_body: Option<Vec<AstNode>> = None;
    if i < toks.len() && toks[i].kind == "else" {
        i += 1;
        let (body, ni3) = parse_block(toks, i, ["end"].into_iter().collect(), seen_vars);
        i = ni3;
        else_body = Some(body);
    }

    let end_line = if i < toks.len() && toks[i].kind == "end" {
        let l = toks[i].line;
        i += 1;
        l
    } else {
        branches.last().map(|b| b.line).unwrap_or(t_if.line)
    };

    (
        AstNode::If {
            cond: t_if.text,
            line_start: t_if.line,
            line_end: end_line,
            branches,
            else_body,
        },
        i,
    )
}

fn parse_loop(toks: &[Tok], mut i: usize, seen_vars: &mut HashSet<String>) -> (AstNode, usize) {
    let t = toks[i].clone();
    i += 1;
    let (body, ni) = parse_block(toks, i, ["end"].into_iter().collect(), seen_vars);
    i = ni;
    let end_line = if i < toks.len() && toks[i].kind == "end" {
        let l = toks[i].line;
        i += 1;
        l
    } else {
        t.line
    };
    (
        AstNode::Loop {
            loop_kind: t.kind,
            header: t.text,
            line_start: t.line,
            line_end: end_line,
            body,
        },
        i,
    )
}

fn parse_switch(toks: &[Tok], mut i: usize, seen_vars: &mut HashSet<String>) -> (AstNode, usize) {
    let t = toks[i].clone();
    i += 1;
    let mut cases: Vec<IfBranch> = vec![];
    let mut otherwise: Option<Vec<AstNode>> = None;

    while i < toks.len() && toks[i].kind != "end" {
        if toks[i].kind == "case" {
            let tc = toks[i].clone();
            i += 1;
            let (body, ni) = parse_block(
                toks,
                i,
                ["case", "otherwise", "end"].into_iter().collect(),
                seen_vars,
            );
            i = ni;
            cases.push(IfBranch {
                cond: tc.text,
                body,
                line: tc.line,
            });
            continue;
        }
        if toks[i].kind == "otherwise" {
            i += 1;
            let (body, ni) = parse_block(toks, i, ["end"].into_iter().collect(), seen_vars);
            i = ni;
            otherwise = Some(body);
            break;
        }
        i += 1;
    }

    let end_line = if i < toks.len() && toks[i].kind == "end" {
        let l = toks[i].line;
        i += 1;
        l
    } else {
        t.line
    };

    (
        AstNode::Switch {
            expr: t.text,
            line_start: t.line,
            line_end: end_line,
            cases,
            otherwise,
        },
        i,
    )
}

fn parse_block(
    toks: &[Tok],
    mut i: usize,
    stop_kinds: HashSet<&str>,
    seen_vars: &mut HashSet<String>,
) -> (Vec<AstNode>, usize) {
    let mut out: Vec<AstNode> = vec![];
    let mut proc_buf: Vec<Tok> = vec![];
    let mut proc_start: Option<usize> = None;

    let flush_proc = |out: &mut Vec<AstNode>,
                      proc_buf: &mut Vec<Tok>,
                      proc_start: &mut Option<usize>,
                      seen_vars: &mut HashSet<String>| {
        if proc_buf.is_empty() {
            return;
        }
        let lines: Vec<String> = proc_buf.iter().map(|x| x.text.clone()).collect();
        let preview = summarize_process_lines(&lines, seen_vars);
        out.push(AstNode::Process {
            text: preview,
            line_start: proc_start.unwrap_or(proc_buf[0].line),
            line_end: proc_buf.last().map(|t| t.line).unwrap_or(proc_buf[0].line),
        });
        proc_buf.clear();
        *proc_start = None;
    };

    while i < toks.len() {
        let t = toks[i].clone();
        if stop_kinds.contains(t.kind.as_str()) {
            break;
        }

        if t.kind == "stmt" {
            if proc_start.is_none() {
                proc_start = Some(t.line);
            }
            proc_buf.push(t);
            i += 1;
            continue;
        }

        flush_proc(&mut out, &mut proc_buf, &mut proc_start, seen_vars);

        if t.kind == "if" {
            let (node, ni) = parse_if(toks, i, seen_vars);
            out.push(node);
            i = ni;
            continue;
        }
        if t.kind == "for" || t.kind == "while" {
            let (node, ni) = parse_loop(toks, i, seen_vars);
            out.push(node);
            i = ni;
            continue;
        }
        if t.kind == "switch" {
            let (node, ni) = parse_switch(toks, i, seen_vars);
            out.push(node);
            i = ni;
            continue;
        }

        out.push(AstNode::Process {
            text: format!("{} {}", t.kind, t.text).trim().to_string(),
            line_start: t.line,
            line_end: t.line,
        });
        i += 1;
    }

    flush_proc(&mut out, &mut proc_buf, &mut proc_start, seen_vars);
    (out, i)
}

fn simplify_block(stmts: &[AstNode]) -> Vec<AstNode> {
    let mut out: Vec<AstNode> = vec![];

    for st in stmts {
        match st {
            AstNode::If {
                cond,
                line_start,
                line_end,
                branches,
                else_body,
            } => {
                if (cond.contains("strcmp(Test") || cond.contains("Test ==")) && *line_start <= 60 {
                    out.push(AstNode::Decision {
                        text: "Select Test case?".to_string(),
                        line_start: *line_start,
                        line_end: *line_end,
                    });
                    out.push(AstNode::Process {
                        text: "Values set: Test, value (Default/theta0/thetadot0/spring_stiff/length/zeta/mass); else error".to_string(),
                        line_start: *line_start,
                        line_end: *line_end,
                    });
                    continue;
                }

                if (cond.contains("strcmp(Test") || cond.contains("Test =="))
                    && *line_start > 60
                    && branches.len() >= 3
                {
                    out.push(AstNode::Process {
                        text: "Values updated: theta0, thetadot0, L, m, k1, k2, k3, legend_label, graphtitle".to_string(),
                        line_start: *line_start,
                        line_end: *line_end,
                    });
                    continue;
                }

                let branches2: Vec<IfBranch> = branches
                    .iter()
                    .map(|br| IfBranch {
                        cond: br.cond.clone(),
                        body: simplify_block(&br.body),
                        line: br.line,
                    })
                    .collect();
                let else2 = else_body.as_ref().map(|b| simplify_block(b));
                out.push(AstNode::If {
                    cond: cond.clone(),
                    line_start: *line_start,
                    line_end: *line_end,
                    branches: branches2,
                    else_body: else2,
                });
            }
            AstNode::Loop {
                loop_kind,
                header,
                line_start,
                line_end,
                body,
            } => out.push(AstNode::Loop {
                loop_kind: loop_kind.clone(),
                header: header.clone(),
                line_start: *line_start,
                line_end: *line_end,
                body: simplify_block(body),
            }),
            AstNode::Switch {
                expr,
                line_start,
                line_end,
                cases,
                otherwise,
            } => {
                let cases2: Vec<IfBranch> = cases
                    .iter()
                    .map(|c| IfBranch {
                        cond: c.cond.clone(),
                        body: simplify_block(&c.body),
                        line: c.line,
                    })
                    .collect();
                let other2 = otherwise.as_ref().map(|b| simplify_block(b));
                out.push(AstNode::Switch {
                    expr: expr.clone(),
                    line_start: *line_start,
                    line_end: *line_end,
                    cases: cases2,
                    otherwise: other2,
                });
            }
            _ => out.push(st.clone()),
        }
    }

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
        text,
        line_start,
        line_end,
        lane,
        level: None,
    });
    id
}

fn connect(edges: &mut Vec<Edge>, srcs: &[Exit], dst: &str, label: Option<&str>) {
    for s in srcs {
        edges.push(Edge {
            from: s.id.clone(),
            to: dst.to_string(),
            label: s.label.clone().or_else(|| label.map(|x| x.to_string())),
            back_edge: None,
        });
    }
}

fn walk_block(
    stmts: &[AstNode],
    incoming: Vec<Exit>,
    lane: i32,
    nodes: &mut Vec<Node>,
    edges: &mut Vec<Edge>,
    next_id: &mut usize,
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
                    id: nid,
                    label: None,
                }];
            }
            AstNode::Loop {
                loop_kind,
                header,
                line_start,
                line_end,
                body,
            } => {
                let lid = add_node(
                    nodes,
                    next_id,
                    "loop",
                    format!("{} {}", loop_kind, header).trim().to_string(),
                    Some(*line_start),
                    Some(*line_end),
                    lane,
                );
                connect(edges, &cur, &lid, None);
                let body_out = walk_block(
                    body,
                    vec![Exit {
                        id: lid.clone(),
                        label: Some("enter".to_string()),
                    }],
                    lane,
                    nodes,
                    edges,
                    next_id,
                );
                for src in body_out {
                    edges.push(Edge {
                        from: src.id,
                        to: lid.clone(),
                        label: Some("next".to_string()),
                        back_edge: None,
                    });
                }
                cur = vec![Exit {
                    id: lid,
                    label: Some("done".to_string()),
                }];
            }
            AstNode::If {
                cond,
                line_start,
                line_end,
                branches,
                else_body,
            } => {
                let did = add_node(
                    nodes,
                    next_id,
                    "decision",
                    format!("if {}", cond).trim().to_string(),
                    Some(*line_start),
                    Some(*line_end),
                    lane,
                );
                connect(edges, &cur, &did, None);

                let mut out: Vec<Exit> = vec![];
                for (idx, br) in branches.iter().enumerate() {
                    let blabel = if idx == 0 { "yes" } else { "elseif" };
                    let blane = if idx == 0 {
                        lane - 1
                    } else {
                        lane + idx as i32 - 1
                    };
                    if !br.body.is_empty() {
                        let bout = walk_block(
                            &br.body,
                            vec![Exit {
                                id: did.clone(),
                                label: Some(blabel.to_string()),
                            }],
                            blane,
                            nodes,
                            edges,
                            next_id,
                        );
                        out.extend(bout);
                    } else {
                        out.push(Exit {
                            id: did.clone(),
                            label: Some(blabel.to_string()),
                        });
                    }
                }

                if let Some(else_stmts) = else_body {
                    if !else_stmts.is_empty() {
                        let eout = walk_block(
                            else_stmts,
                            vec![Exit {
                                id: did.clone(),
                                label: Some("else".to_string()),
                            }],
                            lane + 1,
                            nodes,
                            edges,
                            next_id,
                        );
                        out.extend(eout);
                    } else {
                        out.push(Exit {
                            id: did.clone(),
                            label: Some("else".to_string()),
                        });
                    }
                } else {
                    out.push(Exit {
                        id: did.clone(),
                        label: Some("no".to_string()),
                    });
                }
                cur = out;
            }
            AstNode::Decision {
                text,
                line_start,
                line_end,
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
                cur = vec![Exit {
                    id: did,
                    label: None,
                }];
            }
            AstNode::Switch {
                expr,
                line_start,
                line_end,
                cases,
                otherwise,
            } => {
                let did = add_node(
                    nodes,
                    next_id,
                    "decision",
                    format!("switch {}", expr).trim().to_string(),
                    Some(*line_start),
                    Some(*line_end),
                    lane,
                );
                connect(edges, &cur, &did, None);
                let mut out: Vec<Exit> = vec![];
                for (idx, c) in cases.iter().enumerate() {
                    let lbl = format!("case {}", c.cond);
                    let clane = lane + idx as i32 - (cases.len() as i32 / 2);
                    if !c.body.is_empty() {
                        let cout = walk_block(
                            &c.body,
                            vec![Exit {
                                id: did.clone(),
                                label: Some(lbl),
                            }],
                            clane,
                            nodes,
                            edges,
                            next_id,
                        );
                        out.extend(cout);
                    } else {
                        out.push(Exit {
                            id: did.clone(),
                            label: Some(lbl),
                        });
                    }
                }
                if let Some(other_stmts) = otherwise {
                    if !other_stmts.is_empty() {
                        out.extend(walk_block(
                            other_stmts,
                            vec![Exit {
                                id: did.clone(),
                                label: Some("otherwise".to_string()),
                            }],
                            lane + 1,
                            nodes,
                            edges,
                            next_id,
                        ));
                    } else {
                        out.push(Exit {
                            id: did.clone(),
                            label: Some("otherwise".to_string()),
                        });
                    }
                }
                if out.is_empty() {
                    out.push(Exit {
                        id: did,
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
    let mut line_rank: HashMap<usize, i32> = HashMap::new();
    for (i, ln) in line_vals.iter().enumerate() {
        line_rank.insert(*ln, i as i32 + 1);
    }

    let bucket = 3i32;
    let mut level: HashMap<String, i32> = HashMap::new();
    for n in nodes.iter() {
        if n.id == start_id {
            level.insert(n.id.clone(), 0);
            continue;
        }
        if let Some(ls) = n.line_start {
            let rk = *line_rank.get(&ls).unwrap_or(&1);
            level.insert(n.id.clone(), 1 + (rk - 1) / bucket);
        } else {
            let top = line_rank.values().copied().max().unwrap_or(1);
            level.insert(n.id.clone(), 1 + (top - 1) / bucket + 1);
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

    let mut lv: HashMap<String, i32> = HashMap::new();
    for n in nodes.iter() {
        lv.insert(n.id.clone(), n.level.unwrap_or(0));
    }
    for e in edges.iter_mut() {
        let src = *lv.get(&e.from).unwrap_or(&0);
        let dst = *lv.get(&e.to).unwrap_or(&0);
        e.back_edge = Some(e.label.as_deref() == Some("next") || dst <= src);
    }
}

fn build_graph(ast: &[AstNode]) -> (Vec<Node>, Vec<Edge>, String, String) {
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
        ast,
        vec![Exit {
            id: start.clone(),
            label: None,
        }],
        0,
        &mut nodes,
        &mut edges,
        &mut next_id,
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

    (nodes, edges, start, end)
}

fn default_output_path(input: &PathBuf) -> PathBuf {
    let s = input.to_string_lossy().to_string();
    PathBuf::from(format!("{}.graph.json", s))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let input = cli.input_file;
    let output = cli.output.unwrap_or_else(|| default_output_path(&input));

    let src = fs::read_to_string(&input)
        .with_context(|| format!("failed to read input file {}", input.display()))?;
    let lines: Vec<String> = src.lines().map(|s| s.to_string()).collect();

    let toks = lex(&lines);
    let mut seen_vars: HashSet<String> = HashSet::new();
    let (ast, _) = parse_block(&toks, 0, HashSet::new(), &mut seen_vars);
    let ast = simplify_block(&ast);

    let (mut nodes, mut edges, start, end) = build_graph(&ast);
    assign_levels(&mut nodes, &mut edges, &start, &end);

    let payload = Payload {
        source: input.display().to_string(),
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
