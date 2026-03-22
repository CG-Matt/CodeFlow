use std::cmp::{max, min};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use ab_glyph::{FontRef, PxScale};
use anyhow::{bail, Context, Result};
use clap::Parser;
use image::{imageops, ImageBuffer, Rgb, RgbImage, Rgba, RgbaImage};
use imageproc::drawing::{
    draw_filled_circle_mut, draw_hollow_rect_mut, draw_line_segment_mut, draw_polygon_mut,
    draw_text_mut, text_size,
};
use imageproc::point::Point;
use imageproc::rect::Rect;
use serde::Deserialize;
use serde_json::json;

#[derive(Parser, Debug)]
#[command(name = "codeflow-render")]
#[command(about = "CodeFlow renderer")]
struct Cli {
    graph_json: PathBuf,

    #[arg(short, long, default_value = "flowchart_rs.png")]
    output: PathBuf,

    #[arg(long, default_value = "24")]
    font_size: f32,

    #[arg(long)]
    strict_report: bool,

    #[arg(long)]
    hide_labels: bool,

    #[arg(long)]
    transparent_bg: bool,

    #[arg(long)]
    separate_layers: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct Node {
    id: String,
    shape: String,
    text: String,
    lane: Option<i32>,
    level: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
struct Edge {
    from: String,
    to: String,
    label: Option<String>,
    back_edge: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct Graph {
    source: Option<String>,
    nodes: Vec<Node>,
    edges: Vec<Edge>,
}

#[derive(Debug, Clone)]
struct EdgePath {
    from: String,
    to: String,
    label: Option<String>,
    path: Vec<(i32, i32)>,
}

fn is_loop_next_done_siblings(a: &EdgePath, b: &EdgePath) -> bool {
    let la = a.label.as_deref().unwrap_or("");
    let lb = b.label.as_deref().unwrap_or("");
    (la == "next" && lb == "done" && a.to == b.from)
        || (la == "done" && lb == "next" && a.from == b.to)
}

#[derive(Debug, Clone)]
struct EdgeMeta {
    edge_idx: usize,
    out_idx: i32,
    out_count: i32,
    src_split_y: i32,
    force_center: bool,
    prefer_side: Option<String>,
    sx_override: (i32, i32),
    tx_override: (i32, i32),
    src_side: String,
    dst_side: String,
    src_port_variants: Vec<(i32, i32)>,
    dst_port_variants: Vec<(i32, i32)>,
    dst_candidates_all: Vec<(i32, i32)>,
    side_lane_x: Option<i32>,
    bounds: (i32, i32),
}

#[derive(Debug, Copy, Clone)]
struct Color(u8, u8, u8);

#[derive(Debug, Clone, Copy)]
struct BBox {
    min_x: i32,
    min_y: i32,
    max_x: i32,
    max_y: i32,
}

impl BBox {
    fn new() -> Self {
        Self {
            min_x: i32::MAX,
            min_y: i32::MAX,
            max_x: i32::MIN,
            max_y: i32::MIN,
        }
    }

    fn include_point(&mut self, x: i32, y: i32) {
        self.min_x = min(self.min_x, x);
        self.min_y = min(self.min_y, y);
        self.max_x = max(self.max_x, x);
        self.max_y = max(self.max_y, y);
    }

    fn include_rect(&mut self, x1: i32, y1: i32, x2: i32, y2: i32) {
        self.include_point(x1, y1);
        self.include_point(x2, y2);
    }

    fn is_valid(&self) -> bool {
        self.min_x <= self.max_x && self.min_y <= self.max_y
    }
}

const STYLE_BG: Color = Color(247, 249, 252);
const STYLE_EDGE: Color = Color(55, 65, 81);
const STYLE_TEXT: Color = Color(17, 24, 39);
const STYLE_PROCESS: Color = Color(219, 234, 254);
const STYLE_DECISION: Color = Color(254, 243, 199);
const STYLE_LOOP: Color = Color(220, 252, 231);
const STYLE_TERM: Color = Color(209, 229, 255);

const MIN_LINE_GAP: i32 = 60;
const MIN_CLOSE_VIOLATION: i32 = 24;
const MIN_EXIT_VERTICAL: i32 = 34;
const MIN_EXIT_HORIZONTAL: i32 = 34;
const TOP_LINE_CLEAR: i32 = 18;
const BOTTOM_LINE_CLEAR: i32 = 26;
const SIDE_LINE_CLEAR: i32 = 18;
const MIN_TOP_ENTRY_LEN: i32 = 52;
const STROKE_WIDTH: i32 = 2;
const BLOCK_OUTLINE_WIDTH: i32 = 2;
const BLOCK_CORNER_WIDTH: i32 = 2;

fn rgb(c: Color) -> Rgb<u8> {
    Rgb([c.0, c.1, c.2])
}

fn rgba(c: Color, a: u8) -> Rgba<u8> {
    Rgba([c.0, c.1, c.2, a])
}

fn output_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("flowchart");
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("png");
    parent.join(format!("{stem}_{suffix}.{ext}"))
}

fn save_rgb_with_optional_transparency(
    img: &RgbImage,
    output: &Path,
    bg: Color,
    transparent_bg: bool,
) -> Result<()> {
    if !transparent_bg {
        img.save(output)
            .with_context(|| format!("failed to save {}", output.display()))?;
        return Ok(());
    }
    let mut out: RgbaImage = ImageBuffer::new(img.width(), img.height());
    for (x, y, p) in img.enumerate_pixels() {
        let is_bg = p.0[0] == bg.0 && p.0[1] == bg.1 && p.0[2] == bg.2;
        if is_bg {
            out.put_pixel(x, y, rgba(bg, 0));
        } else {
            out.put_pixel(x, y, Rgba([p.0[0], p.0[1], p.0[2], 255]));
        }
    }
    out.save(output)
        .with_context(|| format!("failed to save {}", output.display()))?;
    Ok(())
}

fn node_dims(shape: &str) -> (i32, i32) {
    match shape {
        "terminator" => (420, 92),
        "decision" => (680, 180),
        "loop" => (760, 112),
        _ => (820, 132),
    }
}

fn fill_for(shape: &str) -> Color {
    match shape {
        "decision" => STYLE_DECISION,
        "loop" => STYLE_LOOP,
        "terminator" => STYLE_TERM,
        _ => STYLE_PROCESS,
    }
}

fn is_rect_like(shape: &str) -> bool {
    matches!(shape, "process" | "loop" | "terminator")
}

fn snap_y(y: i32) -> i32 {
    let step = 12;
    ((y as f32 / step as f32).round() as i32) * step
}

fn segment_orient_and_span(a: (i32, i32), b: (i32, i32)) -> (char, i32, i32, i32) {
    if a.1 == b.1 {
        let x1 = min(a.0, b.0);
        let x2 = max(a.0, b.0);
        return ('h', a.1, x1, x2);
    }
    if a.0 == b.0 {
        let y1 = min(a.1, b.1);
        let y2 = max(a.1, b.1);
        return ('v', a.0, y1, y2);
    }
    ('x', 0, 0, 0)
}

fn seg_hits_rect(a: (i32, i32), b: (i32, i32), r: (i32, i32, i32, i32)) -> bool {
    let (x1, y1) = a;
    let (x2, y2) = b;
    let (rx1, ry1, rx2, ry2) = r;
    if x1 == x2 {
        if rx1 < x1 && x1 < rx2 {
            let sy1 = min(y1, y2);
            let sy2 = max(y1, y2);
            return sy1 < ry2 && sy2 > ry1;
        }
        return false;
    }
    if y1 == y2 {
        if ry1 < y1 && y1 < ry2 {
            let sx1 = min(x1, x2);
            let sx2 = max(x1, x2);
            return sx1 < rx2 && sx2 > rx1;
        }
        return false;
    }
    true
}

fn seg_too_close_to_rect(a: (i32, i32), b: (i32, i32), r: (i32, i32, i32, i32)) -> bool {
    let (x1, y1) = a;
    let (x2, y2) = b;
    let (rx1, ry1, rx2, ry2) = r;

    if y1 == y2 {
        let y = y1;
        let sx1 = min(x1, x2);
        let sx2 = max(x1, x2);
        let overlap = min(sx2, rx2) - max(sx1, rx1);
        if overlap <= 40 {
            return false;
        }
        if (ry1 - TOP_LINE_CLEAR) < y && y < (ry1 + TOP_LINE_CLEAR) {
            return true;
        }
        if (ry2 - BOTTOM_LINE_CLEAR) < y && y < (ry2 + BOTTOM_LINE_CLEAR) {
            return true;
        }
        return false;
    }

    if x1 == x2 {
        let x = x1;
        let sy1 = min(y1, y2);
        let sy2 = max(y1, y2);
        let overlap = min(sy2, ry2) - max(sy1, ry1);
        if overlap <= 40 {
            return false;
        }
        if (rx1 - SIDE_LINE_CLEAR) < x && x < (rx1 + SIDE_LINE_CLEAR) {
            return true;
        }
        if (rx2 - SIDE_LINE_CLEAR) < x && x < (rx2 + SIDE_LINE_CLEAR) {
            return true;
        }
        return false;
    }

    false
}

fn path_collides(
    path: &[(i32, i32)],
    rects: &HashMap<String, (i32, i32, i32, i32)>,
    src: &str,
    dst: &str,
) -> bool {
    for i in 0..path.len().saturating_sub(1) {
        let a = path[i];
        let b = path[i + 1];
        if a.0 != b.0 && a.1 != b.1 {
            return true;
        }
        for (nid, r) in rects {
            if nid == src || nid == dst {
                continue;
            }
            if seg_hits_rect(a, b, *r) || seg_too_close_to_rect(a, b, *r) {
                return true;
            }
        }
    }
    false
}

fn simplify_path(path: &[(i32, i32)]) -> Vec<(i32, i32)> {
    if path.is_empty() {
        return vec![];
    }
    let mut out = vec![path[0]];
    for p in path.iter().skip(1) {
        if *p != *out.last().unwrap() {
            out.push(*p);
        }
    }
    loop {
        if out.len() < 3 {
            break;
        }
        let mut changed = false;
        let mut i = 1;
        while i < out.len() - 1 {
            let a = out[i - 1];
            let b = out[i];
            let c = out[i + 1];
            if (a.0 == b.0 && b.0 == c.0) || (a.1 == b.1 && b.1 == c.1) {
                out.remove(i);
                changed = true;
            } else {
                i += 1;
            }
        }
        if !changed {
            break;
        }
    }
    out
}

fn human_bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    if n < 1024 {
        format!("{n} B")
    } else if (n as f64) < MB {
        format!("{:.1} KiB", (n as f64) / KB)
    } else {
        format!("{:.2} MiB", (n as f64) / MB)
    }
}

fn seg_contains_point(a: (i32, i32), b: (i32, i32), p: (i32, i32)) -> bool {
    if a.0 == b.0 && p.0 == a.0 {
        let lo = min(a.1, b.1);
        let hi = max(a.1, b.1);
        return lo <= p.1 && p.1 <= hi;
    }
    if a.1 == b.1 && p.1 == a.1 {
        let lo = min(a.0, b.0);
        let hi = max(a.0, b.0);
        return lo <= p.0 && p.0 <= hi;
    }
    false
}

fn path_respects_block_constraints(
    path: &[(i32, i32)],
    rects: &HashMap<String, (i32, i32, i32, i32)>,
    src_id: &str,
    dst_id: &str,
) -> bool {
    if path.len() < 2 {
        return false;
    }
    let src_pt = path[0];
    let dst_pt = *path.last().unwrap();
    let last_seg_idx = path.len() - 2;
    for i in 0..path.len().saturating_sub(1) {
        let a = path[i];
        let b = path[i + 1];
        if a.0 != b.0 && a.1 != b.1 {
            return false;
        }
        for (nid, r) in rects {
            if nid == src_id {
                // Source block may only be touched by the very first segment at the source anchor.
                if seg_hits_rect(a, b, *r) && !(i == 0 && seg_contains_point(a, b, src_pt)) {
                    return false;
                }
                continue;
            }
            if nid == dst_id {
                // Destination block may only be touched by the final segment at destination anchor.
                if seg_hits_rect(a, b, *r)
                    && !(i == last_seg_idx && seg_contains_point(a, b, dst_pt))
                {
                    return false;
                }
                continue;
            }
            if seg_hits_rect(a, b, *r) || seg_too_close_to_rect(a, b, *r) {
                return false;
            }
        }
    }
    true
}

fn path_respects_parallel_constraints(
    path: &[(i32, i32)],
    src_id: &str,
    existing: &[EdgePath],
) -> bool {
    if path.len() < 2 {
        return false;
    }
    let cand_start = path.first().copied().unwrap_or((0, 0));
    let mine = path_segments(path);
    for e in existing {
        let allow_overlap = e.from == src_id && e.path.first().copied() == Some(cand_start);
        let oth = path_segments(&e.path);
        for s1 in &mine {
            for s2 in &oth {
                if s1.0 == 'x' || s2.0 == 'x' || s1.0 != s2.0 {
                    continue;
                }
                if s1.0 == 'h' {
                    let (y1, x1a, x1b) = (s1.1, s1.2, s1.3);
                    let (y2, x2a, x2b) = (s2.1, s2.2, s2.3);
                    let overlap = min(x1b, x2b) - max(x1a, x2a);
                    if overlap > 0 && y1 == y2 && !allow_overlap {
                        return false;
                    }
                    // Treat very-close short runs as effectively intersecting at stroke thickness.
                    if overlap > 0 && (y1 - y2).abs() <= (STROKE_WIDTH * 2 + 1) && !allow_overlap {
                        return false;
                    }
                } else {
                    let (x1, y1a, y1b) = (s1.1, s1.2, s1.3);
                    let (x2, y2a, y2b) = (s2.1, s2.2, s2.3);
                    let overlap = min(y1b, y2b) - max(y1a, y2a);
                    if overlap > 0 && x1 == x2 && !allow_overlap {
                        return false;
                    }
                    if overlap > 0 && (x1 - x2).abs() <= (STROKE_WIDTH * 2 + 1) && !allow_overlap {
                        return false;
                    }
                }
            }
        }
    }
    true
}

fn path_len_and_bends(path: &[(i32, i32)]) -> (i32, i32) {
    let mut plen = 0;
    let mut bends = 0;
    let mut prev: Option<char> = None;
    for i in 0..path.len().saturating_sub(1) {
        let a = path[i];
        let b = path[i + 1];
        plen += (a.0 - b.0).abs() + (a.1 - b.1).abs();
        let cur = if a.1 == b.1 { 'h' } else { 'v' };
        if let Some(p) = prev {
            if p != cur {
                bends += 1;
            }
        }
        prev = Some(cur);
    }
    (plen, bends)
}

fn path_segments(path: &[(i32, i32)]) -> Vec<(char, i32, i32, i32)> {
    let mut out = vec![];
    for i in 0..path.len().saturating_sub(1) {
        out.push(segment_orient_and_span(path[i], path[i + 1]));
    }
    out
}

fn path_segments_count(path: &[(i32, i32)]) -> i64 {
    path_segments(path).iter().filter(|s| s.0 != 'x').count() as i64
}

fn path_spacing_penalty_from_segments(
    path_segs: &[(char, i32, i32, i32)],
    existing_seg_lists: &[Vec<(char, i32, i32, i32)>],
) -> i64 {
    let mut penalty = 0i64;
    for s1 in path_segs {
        let (o1, c1, s1a, s1b) = *s1;
        if o1 == 'x' {
            penalty += 20_000;
            continue;
        }
        for oth in existing_seg_lists {
            for s2 in oth {
                let (o2, c2, s2a, s2b) = *s2;
                if o1 != o2 || o2 == 'x' {
                    continue;
                }
                let overlap = min(s1b, s2b) - max(s1a, s2a);
                if overlap <= 0 {
                    continue;
                }
                let d = (c1 - c2).abs();
                if d == 0 {
                    penalty += 120_000 + (overlap as i64 * 35);
                } else if d < MIN_CLOSE_VIOLATION {
                    penalty += ((MIN_CLOSE_VIOLATION - d) as i64) * 120 + overlap as i64;
                } else if d < MIN_LINE_GAP {
                    penalty += ((MIN_LINE_GAP - d) as i64) * 6;
                }
            }
        }
    }
    penalty
}

fn path_spacing_penalty(path: &[(i32, i32)], existing: &[EdgePath]) -> i64 {
    let mut penalty = 0i64;
    let cand_from = "";
    let cand_start = path.first().copied();
    for i in 0..path.len().saturating_sub(1) {
        let (o1, c1, s1a, s1b) = segment_orient_and_span(path[i], path[i + 1]);
        if o1 == 'x' {
            penalty += 20_000;
            continue;
        }
        for e in existing {
            let allow_same_output_overlap = !cand_from.is_empty()
                && e.from == cand_from
                && e.path.first().copied() == cand_start;
            if allow_same_output_overlap {
                continue;
            }
            for j in 0..e.path.len().saturating_sub(1) {
                let (o2, c2, s2a, s2b) = segment_orient_and_span(e.path[j], e.path[j + 1]);
                if o1 != o2 || o2 == 'x' {
                    continue;
                }
                let overlap = min(s1b, s2b) - max(s1a, s2a);
                if overlap <= 0 {
                    continue;
                }
                let d = (c1 - c2).abs();
                if d == 0 {
                    penalty += 120_000 + (overlap as i64 * 35);
                } else if d < MIN_CLOSE_VIOLATION {
                    penalty += ((MIN_CLOSE_VIOLATION - d) as i64) * 120 + overlap as i64;
                } else if d < MIN_LINE_GAP {
                    penalty += ((MIN_LINE_GAP - d) as i64) * 6;
                }
            }
        }
    }
    penalty
}

fn path_spacing_penalty_for_edge(path: &[(i32, i32)], existing: &[EdgePath], from_id: &str) -> i64 {
    let mut penalty = 0i64;
    let cand_start = path.first().copied();
    for i in 0..path.len().saturating_sub(1) {
        let (o1, c1, s1a, s1b) = segment_orient_and_span(path[i], path[i + 1]);
        if o1 == 'x' {
            penalty += 20_000;
            continue;
        }
        for e in existing {
            let allow_same_output_overlap =
                e.from == from_id && e.path.first().copied() == cand_start;
            if allow_same_output_overlap {
                continue;
            }
            for j in 0..e.path.len().saturating_sub(1) {
                let (o2, c2, s2a, s2b) = segment_orient_and_span(e.path[j], e.path[j + 1]);
                if o1 != o2 || o2 == 'x' {
                    continue;
                }
                let overlap = min(s1b, s2b) - max(s1a, s2a);
                if overlap <= 0 {
                    continue;
                }
                let d = (c1 - c2).abs();
                if d == 0 {
                    penalty += 120_000 + (overlap as i64 * 35);
                } else if d < MIN_CLOSE_VIOLATION {
                    penalty += ((MIN_CLOSE_VIOLATION - d) as i64) * 120 + overlap as i64;
                } else if d < MIN_LINE_GAP {
                    penalty += ((MIN_LINE_GAP - d) as i64) * 6;
                }
            }
        }
    }
    penalty
}

fn choose_best_path(
    cands: Vec<Vec<(i32, i32)>>,
    rects: &HashMap<String, (i32, i32, i32, i32)>,
    src_id: &str,
    dst_id: &str,
    existing: &[EdgePath],
) -> Vec<(i32, i32)> {
    let mut best: Option<Vec<(i32, i32)>> = None;
    let mut best_key: (i64, i64, i64, i64) = (i64::MAX, i64::MAX, i64::MAX, i64::MAX);
    let mut best_relaxed: Option<Vec<(i32, i32)>> = None;
    let mut best_relaxed_key: (i64, i64, i64, i64) = (i64::MAX, i64::MAX, i64::MAX, i64::MAX);

    for c in cands {
        let p = simplify_path(&c);
        let mut coll = 0i64;
        for i in 0..p.len().saturating_sub(1) {
            let a = p[i];
            let b = p[i + 1];
            if a.0 != b.0 && a.1 != b.1 {
                coll += 100_000;
            }
            for (nid, r) in rects {
                if nid == src_id || nid == dst_id {
                    continue;
                }
                if seg_hits_rect(a, b, *r) {
                    coll += 250_000;
                } else if seg_too_close_to_rect(a, b, *r) {
                    coll += 50_000;
                }
            }
        }
        let segs = p.len().saturating_sub(1) as i64;
        let spacing = path_spacing_penalty_for_edge(&p, existing, src_id);
        let (plen, bends) = path_len_and_bends(&p);
        let secondary = bends as i64 * 120 + spacing;
        let key = (coll, segs, plen as i64, secondary);
        if key < best_relaxed_key {
            best_relaxed_key = key;
            best_relaxed = Some(p.clone());
        }
        if !path_respects_block_constraints(&p, rects, src_id, dst_id) {
            continue;
        }
        if !path_respects_parallel_constraints(&p, src_id, existing) {
            continue;
        }
        if key < best_key {
            best_key = key;
            best = Some(p);
        }
    }

    best.or(best_relaxed).unwrap_or_default()
}

fn port_variants(base_pt: (i32, i32), candidates: &[(i32, i32)], limit: usize) -> Vec<(i32, i32)> {
    let mut uniq: Vec<(i32, i32)> = vec![];
    let mut seen: HashSet<(i32, i32)> = HashSet::new();
    for p in std::iter::once(base_pt).chain(candidates.iter().copied()) {
        if seen.insert(p) {
            uniq.push(p);
        }
    }
    uniq.sort_by_key(|p| (p.0 - base_pt.0).abs() + (p.1 - base_pt.1).abs());
    uniq.truncate(max(1, limit as i32) as usize);
    uniq
}

fn unique_ports(candidates: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let mut out = Vec::with_capacity(candidates.len());
    for p in candidates {
        if !out.contains(p) {
            out.push(*p);
        }
    }
    out
}

fn clear_vertical_x(
    mut x: i32,
    y1: i32,
    y2: i32,
    rects: &HashMap<String, (i32, i32, i32, i32)>,
    src_id: &str,
    dst_id: &str,
    direction: i32,
    min_x: i32,
    max_x: i32,
) -> i32 {
    let y_lo = min(y1, y2);
    let y_hi = max(y1, y2);
    for _ in 0..80 {
        let mut blocked = false;
        for (nid, r) in rects {
            if nid == src_id || nid == dst_id {
                continue;
            }
            if seg_hits_rect((x, y_lo), (x, y_hi), *r) {
                blocked = true;
                break;
            }
        }
        if !blocked {
            return x;
        }
        x += direction * MIN_LINE_GAP;
        x = max(min_x, min(max_x, x));
    }
    x
}

fn route_edge_smart(
    src_id: &str,
    dst_id: &str,
    rects: &HashMap<String, (i32, i32, i32, i32)>,
    existing: &[EdgePath],
    out_idx: i32,
    out_count: i32,
    src_split_y: i32,
    force_center: bool,
    prefer_side: Option<&str>,
    sx_override: (i32, i32),
    tx_override: (i32, i32),
    src_side: &str,
    dst_side: &str,
    side_lane_x: Option<i32>,
    bounds: (i32, i32),
) -> Vec<(i32, i32)> {
    let (sx_base, sy_base) = sx_override;
    let (tx, ty) = tx_override;
    let off = ((out_idx as f32 - (out_count as f32 - 1.0) / 2.0) * MIN_LINE_GAP as f32) as i32;
    let exit_h = MIN_EXIT_HORIZONTAL + min(MIN_LINE_GAP, off.abs() / 2);
    let exit_v = MIN_EXIT_VERTICAL + min(MIN_LINE_GAP, off.abs() / 2);
    let jog_off = if out_count > 2 {
        max(-MIN_LINE_GAP, min(MIN_LINE_GAP, off))
    } else {
        0
    };

    let (sx, sy, start_prefix) = match src_side {
        "left" => {
            let sx = sx_base - exit_h;
            let sy = snap_y(sy_base + off / 2);
            let mut p = vec![(sx_base, sy_base), (sx, sy_base)];
            if sy != sy_base {
                p.push((sx, sy));
            }
            (sx, sy, p)
        }
        "right" => {
            let sx = sx_base + exit_h;
            let sy = snap_y(sy_base + off / 2);
            let mut p = vec![(sx_base, sy_base), (sx, sy_base)];
            if sy != sy_base {
                p.push((sx, sy));
            }
            (sx, sy, p)
        }
        "top" => {
            let sy = sy_base - exit_v;
            let sx = sx_base + jog_off;
            let mut p = vec![(sx_base, sy_base), (sx_base, sy)];
            if sx != sx_base {
                p.push((sx, sy));
            }
            (sx, sy, p)
        }
        _ => {
            let sy = sy_base + exit_v;
            let sx = sx_base + jog_off;
            let mut p = vec![(sx_base, sy_base), (sx_base, sy)];
            if sx != sx_base {
                p.push((sx, sy));
            }
            (sx, sy, p)
        }
    };

    let mut split_vals = vec![
        snap_y(src_split_y + off),
        snap_y(src_split_y + off + MIN_LINE_GAP),
        snap_y(src_split_y + off - MIN_LINE_GAP),
        snap_y(src_split_y),
    ];
    split_vals = split_vals.into_iter().map(|s| max(s, sy + 12)).collect();

    let near_t = snap_y(ty - 26);
    let mut near_vals = vec![
        near_t,
        snap_y(near_t - MIN_LINE_GAP),
        snap_y(near_t + MIN_LINE_GAP),
    ];

    let dst_is_side = dst_side == "left" || dst_side == "right";
    if dst_is_side {
        near_vals = vec![ty];
    } else if dst_side == "top" {
        near_vals = near_vals
            .into_iter()
            .filter(|v| *v <= ty - MIN_TOP_ENTRY_LEN)
            .collect();
        if near_vals.is_empty() {
            near_vals = vec![ty - MIN_TOP_ENTRY_LEN];
        }
    } else if dst_side == "bottom" {
        near_vals = near_vals.into_iter().filter(|v| *v > ty).collect();
        if near_vals.is_empty() {
            near_vals = vec![ty + max(12, MIN_LINE_GAP / 2)];
        }
    }

    let (min_x, max_x) = bounds;
    let span = (tx - sx).abs();
    let side_pad = 180 + min(280, span / 2) + off.abs() / 2;

    let mut cands: Vec<Vec<(i32, i32)>> = vec![];
    if sx == tx {
        cands.push(vec![(sx, sy), (tx, ty)]);
    } else {
        for sp in &split_vals {
            for nt in &near_vals {
                if dst_is_side {
                    let join_off = 44;
                    let join_x = if dst_side == "left" {
                        tx - join_off
                    } else {
                        tx + join_off
                    };
                    cands.push(vec![
                        (sx, sy),
                        (sx, *sp),
                        (join_x, *sp),
                        (join_x, *nt),
                        (join_x, ty),
                        (tx, ty),
                    ]);
                    if prefer_side.is_none() {
                        let mid_x = ((sx + join_x) / 2) + off;
                        cands.push(vec![
                            (sx, sy),
                            (sx, *sp),
                            (mid_x, *sp),
                            (mid_x, ty),
                            (tx, ty),
                        ]);
                    }
                } else {
                    cands.push(vec![(sx, sy), (sx, *sp), (tx, *sp), (tx, *nt), (tx, ty)]);
                    if force_center {
                        let my = snap_y((*sp + *nt) / 2);
                        cands.push(vec![(sx, sy), (sx, my), (tx, my), (tx, ty)]);
                    }
                    if prefer_side.is_none() {
                        let mid_x = ((sx + tx) / 2) + off;
                        cands.push(vec![
                            (sx, sy),
                            (sx, *sp),
                            (mid_x, *sp),
                            (mid_x, *nt),
                            (tx, *nt),
                            (tx, ty),
                        ]);
                        cands.push(vec![(sx, sy), (sx, *sp), (mid_x, *sp), (tx, *sp), (tx, ty)]);
                    }
                }
            }
        }
    }

    if let Some(side_lane) = side_lane_x {
        for lane_off in [
            0,
            MIN_LINE_GAP,
            -MIN_LINE_GAP,
            2 * MIN_LINE_GAP,
            -2 * MIN_LINE_GAP,
            3 * MIN_LINE_GAP,
            -3 * MIN_LINE_GAP,
        ] {
            let lane_x = max(min_x, min(max_x, side_lane + lane_off));
            let dir_sign = if lane_x <= min(sx, tx) { -1 } else { 1 };
            for sp in &split_vals {
                for nt in &near_vals {
                    let lx = clear_vertical_x(
                        lane_x, *sp, *nt, rects, src_id, dst_id, dir_sign, min_x, max_x,
                    );
                    if dst_is_side {
                        cands.push(vec![
                            (sx, sy),
                            (sx, *sp),
                            (lx, *sp),
                            (lx, *nt),
                            (lx, ty),
                            (tx, ty),
                        ]);
                    } else {
                        cands.push(vec![
                            (sx, sy),
                            (sx, *sp),
                            (lx, *sp),
                            (lx, *nt),
                            (tx, *nt),
                            (tx, ty),
                        ]);
                    }
                }
            }
        }
    }

    for pad in [side_pad, side_pad + 120, side_pad + 240] {
        let left_x = max(min_x, min(sx, tx) - pad);
        let right_x = min(max_x, max(sx, tx) + pad);
        for sp in &split_vals {
            for nt in &near_vals {
                let lx =
                    clear_vertical_x(left_x, *sp, *nt, rects, src_id, dst_id, -1, min_x, max_x);
                let rx =
                    clear_vertical_x(right_x, *sp, *nt, rects, src_id, dst_id, 1, min_x, max_x);
                let left_c = if dst_is_side {
                    vec![
                        (sx, sy),
                        (sx, *sp),
                        (lx, *sp),
                        (lx, *nt),
                        (lx, ty),
                        (tx, ty),
                    ]
                } else {
                    vec![
                        (sx, sy),
                        (sx, *sp),
                        (lx, *sp),
                        (lx, *nt),
                        (tx, *nt),
                        (tx, ty),
                    ]
                };
                let right_c = if dst_is_side {
                    vec![
                        (sx, sy),
                        (sx, *sp),
                        (rx, *sp),
                        (rx, *nt),
                        (rx, ty),
                        (tx, ty),
                    ]
                } else {
                    vec![
                        (sx, sy),
                        (sx, *sp),
                        (rx, *sp),
                        (rx, *nt),
                        (tx, *nt),
                        (tx, ty),
                    ]
                };
                match prefer_side {
                    Some("left") => {
                        cands.push(left_c);
                        cands.push(right_c);
                    }
                    Some("right") => {
                        cands.push(right_c);
                        cands.push(left_c);
                    }
                    _ => {
                        cands.push(right_c);
                        cands.push(left_c);
                    }
                }
            }
        }
    }

    let mut patched: Vec<Vec<(i32, i32)>> = vec![];
    for p in cands {
        let mut z = start_prefix.clone();
        z.extend_from_slice(&p[1..]);
        patched.push(simplify_path(&z));
    }

    let dir_ok: Vec<Vec<(i32, i32)>> = patched
        .into_iter()
        .filter(|p| terminal_dir_ok_for_side(p, dst_side))
        .collect();

    let use_cands = if dir_ok.is_empty() { vec![] } else { dir_ok };
    let final_cands = if use_cands.is_empty() {
        let fb = if dst_side == "top" {
            let entry_y = min(sy, ty - MIN_TOP_ENTRY_LEN);
            vec![(sx, sy), (sx, entry_y), (tx, entry_y), (tx, ty)]
        } else if dst_side == "bottom" {
            let entry_y = max(sy, ty + max(12, MIN_TOP_ENTRY_LEN / 2));
            vec![(sx, sy), (sx, entry_y), (tx, entry_y), (tx, ty)]
        } else {
            vec![(sx, sy), (tx, sy), (tx, ty)]
        };
        vec![simplify_path(&fb)]
    } else {
        use_cands
    };

    choose_best_path(final_cands, rects, src_id, dst_id, existing)
}

fn side_port_candidates(rect: (i32, i32, i32, i32), shape: &str, side: &str) -> Vec<(i32, i32)> {
    let (x1, y1, x2, y2) = rect;
    let cx = (x1 + x2) / 2;
    let cy = (y1 + y2) / 2;
    let w = x2 - x1;
    match side {
        "top" => {
            let mut pts = vec![(cx, y1)];
            if is_rect_like(shape) {
                pts.push((x1 + w / 4, y1));
                pts.push((x1 + (3 * w) / 4, y1));
            }
            pts
        }
        "bottom" => {
            let mut pts = vec![(cx, y2)];
            if is_rect_like(shape) {
                pts.push((x1 + w / 4, y2));
                pts.push((x1 + (3 * w) / 4, y2));
            }
            pts
        }
        "left" => vec![(x1, cy)],
        "right" => vec![(x2, cy)],
        _ => vec![(cx, cy)],
    }
}

fn output_port_candidates(
    rect: (i32, i32, i32, i32),
    shape: &str,
    prefer_side: Option<&str>,
) -> Vec<(i32, i32)> {
    let mut cands = side_port_candidates(rect, shape, "bottom");
    match prefer_side {
        Some("left") => {
            cands.extend(side_port_candidates(rect, shape, "left"));
            cands.extend(side_port_candidates(rect, shape, "right"));
        }
        Some("right") => {
            cands.extend(side_port_candidates(rect, shape, "right"));
            cands.extend(side_port_candidates(rect, shape, "left"));
        }
        _ => {
            cands.extend(side_port_candidates(rect, shape, "left"));
            cands.extend(side_port_candidates(rect, shape, "right"));
        }
    }
    cands
}

fn input_port_candidates(
    rect: (i32, i32, i32, i32),
    shape: &str,
    prefer_side: Option<&str>,
) -> Vec<(i32, i32)> {
    let top_all = side_port_candidates(rect, shape, "top");
    let mut cands = vec![];
    if let Some(first) = top_all.first() {
        cands.push(*first);
    }
    if top_all.len() > 1 {
        cands.extend_from_slice(&top_all[1..]);
    }
    match prefer_side {
        Some("left") => {
            cands.extend(side_port_candidates(rect, shape, "left"));
            cands.extend(side_port_candidates(rect, shape, "right"));
        }
        Some("right") => {
            cands.extend(side_port_candidates(rect, shape, "right"));
            cands.extend(side_port_candidates(rect, shape, "left"));
        }
        _ => {
            cands.extend(side_port_candidates(rect, shape, "left"));
            cands.extend(side_port_candidates(rect, shape, "right"));
        }
    }
    cands
}

fn extend_unique(dst: &mut Vec<(i32, i32)>, src: Vec<(i32, i32)>) {
    for p in src {
        if !dst.contains(&p) {
            dst.push(p);
        }
    }
}

fn decision_output_side_order(
    src_rect: (i32, i32, i32, i32),
    dst_rect: (i32, i32, i32, i32),
    label_lc: &str,
) -> Vec<&'static str> {
    let src_cx = (src_rect.0 + src_rect.2) / 2;
    let dst_cx = (dst_rect.0 + dst_rect.2) / 2;
    let dx = dst_cx - src_cx;
    let mut sides = if dx < -16 {
        vec!["left", "bottom", "right"]
    } else if dx > 16 {
        vec!["right", "bottom", "left"]
    } else {
        vec!["bottom", "left", "right"]
    };

    // Keep decision labels as a soft hint, but avoid forcing long cross-overs.
    if matches!(label_lc, "elseif") {
        sides = vec!["bottom", "left", "right"];
    } else if matches!(label_lc, "yes" | "if" | "case") && dx.abs() <= 16 {
        sides = vec!["left", "bottom", "right"];
    } else if matches!(label_lc, "no" | "else" | "default" | "otherwise") && dx.abs() <= 16 {
        sides = vec!["right", "bottom", "left"];
    }
    sides
}

fn top_entry_clearance_available(
    rects: &HashMap<String, (i32, i32, i32, i32)>,
    dst_id: &str,
    dst_rect: (i32, i32, i32, i32),
) -> bool {
    let (x1, y1, x2, _y2) = dst_rect;
    let cx = (x1 + x2) / 2;
    let band_half = 28;
    let y_top = y1 - MIN_TOP_ENTRY_LEN;
    for (nid, r) in rects {
        if nid == dst_id {
            continue;
        }
        let x_overlap = min(cx + band_half, r.2) - max(cx - band_half, r.0);
        if x_overlap <= 0 {
            continue;
        }
        if r.3 > y_top && r.1 < y1 {
            return false;
        }
    }
    true
}

fn choose_input_port_prioritize_top(
    used_in: &mut HashMap<String, HashSet<(i32, i32)>>,
    used_out: &mut HashMap<String, HashSet<(i32, i32)>>,
    node_id: &str,
    rects: &HashMap<String, (i32, i32, i32, i32)>,
    dst_rect: (i32, i32, i32, i32),
    candidates: &[(i32, i32)],
    prefer_point: Option<(i32, i32)>,
) -> (i32, i32) {
    let top_center = ((dst_rect.0 + dst_rect.2) / 2, dst_rect.1);
    if candidates.contains(&top_center)
        && top_entry_clearance_available(rects, node_id, dst_rect)
        && !used_in
            .get(node_id)
            .map(|s| s.contains(&top_center))
            .unwrap_or(false)
        && !used_out
            .get(node_id)
            .map(|s| s.contains(&top_center))
            .unwrap_or(false)
    {
        used_in
            .entry(node_id.to_string())
            .or_default()
            .insert(top_center);
        return top_center;
    }
    choose_port_role(used_in, used_out, node_id, candidates, "in", prefer_point)
}

fn choose_port_role(
    used_in: &mut HashMap<String, HashSet<(i32, i32)>>,
    used_out: &mut HashMap<String, HashSet<(i32, i32)>>,
    node_id: &str,
    candidates: &[(i32, i32)],
    role: &str,
    prefer_point: Option<(i32, i32)>,
) -> (i32, i32) {
    let (own_used, other_used) = if role == "in" {
        let own = used_in.entry(node_id.to_string()).or_default().clone();
        let other = used_out.entry(node_id.to_string()).or_default().clone();
        (own, other)
    } else {
        let own = used_out.entry(node_id.to_string()).or_default().clone();
        let other = used_in.entry(node_id.to_string()).or_default().clone();
        (own, other)
    };

    let mut avail: Vec<(i32, i32)> = candidates
        .iter()
        .copied()
        .filter(|p| !own_used.contains(p) && !other_used.contains(p))
        .collect();

    if !avail.is_empty() {
        let p = if let Some((px, py)) = prefer_point {
            avail.sort_by_key(|q| ((q.0 - px).abs() + (q.1 - py).abs(), (q.0 - px).abs()));
            avail[0]
        } else {
            avail[0]
        };
        if role == "in" {
            used_in.entry(node_id.to_string()).or_default().insert(p);
        } else {
            used_out.entry(node_id.to_string()).or_default().insert(p);
        }
        return p;
    }

    let mut avail2: Vec<(i32, i32)> = candidates
        .iter()
        .copied()
        .filter(|p| !other_used.contains(p))
        .collect();
    if !avail2.is_empty() {
        let p = if let Some((px, py)) = prefer_point {
            avail2.sort_by_key(|q| ((q.0 - px).abs() + (q.1 - py).abs(), (q.0 - px).abs()));
            avail2[0]
        } else {
            avail2[0]
        };
        if role == "in" {
            used_in.entry(node_id.to_string()).or_default().insert(p);
        } else {
            used_out.entry(node_id.to_string()).or_default().insert(p);
        }
        return p;
    }

    let p = candidates[0];
    if role == "in" {
        used_in.entry(node_id.to_string()).or_default().insert(p);
    } else {
        used_out.entry(node_id.to_string()).or_default().insert(p);
    }
    p
}

fn port_side_from_point(rect: (i32, i32, i32, i32), pt: (i32, i32)) -> &'static str {
    let (x1, y1, x2, y2) = rect;
    if pt.0 == x1 {
        "left"
    } else if pt.0 == x2 {
        "right"
    } else if pt.1 == y1 {
        "top"
    } else if pt.1 == y2 {
        "bottom"
    } else {
        "center"
    }
}

fn incoming_dir_valid(path: &[(i32, i32)], dst_rect: (i32, i32, i32, i32)) -> (bool, String) {
    if path.len() < 2 {
        return (true, "path too short".to_string());
    }
    let a = path[path.len() - 2];
    let b = path[path.len() - 1];
    let side = port_side_from_point(dst_rect, b);
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    match side {
        "top" => (
            dx == 0 && dy >= MIN_TOP_ENTRY_LEN,
            format!(
                "top expects downward entry with min {MIN_TOP_ENTRY_LEN}px, got {:?}->{:?}",
                a, b
            ),
        ),
        "bottom" => (
            dx == 0 && dy < 0,
            format!("bottom expects upward entry, got {:?}->{:?}", a, b),
        ),
        "left" => (
            dy == 0 && dx > 0,
            format!("left expects rightward entry, got {:?}->{:?}", a, b),
        ),
        "right" => (
            dy == 0 && dx < 0,
            format!("right expects leftward entry, got {:?}->{:?}", a, b),
        ),
        _ => (false, format!("endpoint not on node boundary: {:?}", b)),
    }
}

fn terminal_dir_ok_for_side(path: &[(i32, i32)], dst_side: &str) -> bool {
    if path.len() < 2 {
        return true;
    }
    let a = path[path.len() - 2];
    let b = path[path.len() - 1];
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    match dst_side {
        "top" => dx == 0 && dy >= MIN_TOP_ENTRY_LEN,
        "bottom" => dx == 0 && dy < 0,
        "left" => dy == 0 && dx > 0,
        "right" => dy == 0 && dx < 0,
        _ => true,
    }
}

fn edge_conflict_score(
    i: usize,
    edge_paths: &[EdgePath],
    rects: &HashMap<String, (i32, i32, i32, i32)>,
) -> i64 {
    let e = &edge_paths[i];
    let path = &e.path;
    let mut score: i64 = 0;

    for k in 0..path.len().saturating_sub(1) {
        let a = path[k];
        let b = path[k + 1];
        if a.0 != b.0 && a.1 != b.1 {
            score += 2000;
        }
        for (nid, r) in rects {
            if nid == &e.from || nid == &e.to {
                continue;
            }
            if seg_hits_rect(a, b, *r) {
                score += 3000;
            } else if seg_too_close_to_rect(a, b, *r) {
                score += 350;
            }
        }
    }

    let my = path_segments(path);
    let e_anchor = e.path.first().copied().unwrap_or((0, 0));
    for (j, o) in edge_paths.iter().enumerate() {
        if j == i {
            continue;
        }
        let o_anchor = o.path.first().copied().unwrap_or((0, 0));
        let allow_same_output_overlap = e.from == o.from && e_anchor == o_anchor;
        let oth = path_segments(&o.path);
        for s1 in &my {
            for s2 in &oth {
                if s1.0 == 'x' || s2.0 == 'x' || s1.0 != s2.0 {
                    continue;
                }
                if s1.0 == 'h' {
                    let (y1, x1a, x1b) = (s1.1, s1.2, s1.3);
                    let (y2, x2a, x2b) = (s2.1, s2.2, s2.3);
                    let overlap = min(x1b, x2b) - max(x1a, x2a);
                    if overlap <= 0 {
                        continue;
                    }
                    if y1 == y2 {
                        if allow_same_output_overlap {
                            continue;
                        }
                        score += 2600 + overlap as i64 * 6;
                        if is_loop_next_done_siblings(e, o) {
                            score += 600;
                        }
                    } else if (y1 - y2).abs() < MIN_CLOSE_VIOLATION && overlap > 0 {
                        score += 260 + overlap as i64;
                    }
                } else {
                    let (x1, y1a, y1b) = (s1.1, s1.2, s1.3);
                    let (x2, y2a, y2b) = (s2.1, s2.2, s2.3);
                    let overlap = min(y1b, y2b) - max(y1a, y2a);
                    if overlap <= 0 {
                        continue;
                    }
                    if x1 == x2 {
                        if allow_same_output_overlap {
                            continue;
                        }
                        score += 2600 + overlap as i64 * 6;
                        if is_loop_next_done_siblings(e, o) {
                            score += 600;
                        }
                    } else if (x1 - x2).abs() < MIN_CLOSE_VIOLATION && overlap > 0 {
                        score += 260 + overlap as i64;
                    }
                }
            }
        }
    }
    score
}

fn input_port_usage(edge_paths: &[EdgePath]) -> HashMap<(String, (i32, i32)), Vec<usize>> {
    let mut usage: HashMap<(String, (i32, i32)), Vec<usize>> = HashMap::new();
    for (i, e) in edge_paths.iter().enumerate() {
        if e.path.is_empty() {
            continue;
        }
        let dst_pt = *e.path.last().unwrap();
        usage.entry((e.to.clone(), dst_pt)).or_default().push(i);
    }
    usage
}

fn validate_layout(
    rects: &HashMap<String, (i32, i32, i32, i32)>,
    edge_paths: &[EdgePath],
) -> serde_json::Value {
    let mut issues: Vec<String> = vec![];
    let mut overlap_count = 0;
    let mut cross_count = 0;
    let mut non_orth_count = 0;
    let mut edge_overlap_count = 0;
    let mut parallel_overlap_count = 0;
    let mut line_block_clearance_violations = 0;
    let mut bad_entry_dir_count = 0;
    let mut shared_input_port_count = 0;
    let mut close_parallel_count = 0;
    let mut micro_overlap_risk_count = 0;
    let mut loop_next_done_overlap_count = 0;
    let mut loop_next_done_close_count = 0;
    let mut parallel_overlap_samples: Vec<String> = vec![];
    let mut allowed_parallel_overlap_count = 0;
    let mut allowed_parallel_overlap_samples: Vec<String> = vec![];

    let keys: Vec<&String> = rects.keys().collect();
    for i in 0..keys.len() {
        for j in (i + 1)..keys.len() {
            let a = rects[keys[i]];
            let b = rects[keys[j]];
            if a.0 < b.2 && a.2 > b.0 && a.1 < b.3 && a.3 > b.1 {
                issues.push(format!("Node overlap: {} with {}", keys[i], keys[j]));
                overlap_count += 1;
            }
        }
    }

    for e in edge_paths {
        let (ok_entry, why) = incoming_dir_valid(&e.path, rects[&e.to]);
        if !ok_entry {
            bad_entry_dir_count += 1;
            issues.push(format!(
                "Edge {}->{} bad final-entry direction: {}",
                e.from, e.to, why
            ));
        }
        for i in 0..e.path.len().saturating_sub(1) {
            let a = e.path[i];
            let b = e.path[i + 1];
            if a.0 != b.0 && a.1 != b.1 {
                issues.push(format!(
                    "Non-orth segment on edge {}->{}: {:?}->{:?}",
                    e.from, e.to, a, b
                ));
                non_orth_count += 1;
            }
            for (nid, r) in rects {
                if nid == &e.from || nid == &e.to {
                    continue;
                }
                if seg_hits_rect(a, b, *r) {
                    issues.push(format!("Edge {}->{} crosses node {}", e.from, e.to, nid));
                    cross_count += 1;
                    break;
                }
                if seg_too_close_to_rect(a, b, *r) {
                    line_block_clearance_violations += 1;
                    issues.push(format!(
                        "Edge {}->{} too close to node {}",
                        e.from, e.to, nid
                    ));
                    break;
                }
            }
        }
    }

    let mut in_use: HashMap<(String, (i32, i32)), i32> = HashMap::new();
    for e in edge_paths {
        if let Some(last) = e.path.last() {
            let k = (e.to.clone(), *last);
            *in_use.entry(k).or_insert(0) += 1;
        }
    }
    for ((dst, pt), cnt) in in_use {
        if cnt > 1 {
            shared_input_port_count += cnt - 1;
            issues.push(format!(
                "Input port reused on {} at {:?}: {} edges",
                dst, pt, cnt
            ));
        }
    }

    let mut horiz: Vec<(i32, i32, i32, (String, String), Option<String>, (i32, i32))> = vec![];
    let mut vert: Vec<(i32, i32, i32, (String, String), Option<String>, (i32, i32))> = vec![];
    for e in edge_paths {
        let owner = (e.from.clone(), e.to.clone());
        let lbl = e.label.clone();
        let src_anchor = e.path.first().copied().unwrap_or((0, 0));
        for i in 0..e.path.len().saturating_sub(1) {
            let a = e.path[i];
            let b = e.path[i + 1];
            if a.1 == b.1 {
                horiz.push((
                    a.1,
                    min(a.0, b.0),
                    max(a.0, b.0),
                    owner.clone(),
                    lbl.clone(),
                    src_anchor,
                ));
            } else if a.0 == b.0 {
                vert.push((
                    a.0,
                    min(a.1, b.1),
                    max(a.1, b.1),
                    owner.clone(),
                    lbl.clone(),
                    src_anchor,
                ));
            }
        }
    }
    for i in 0..horiz.len() {
        let (y1, x1a, x2a, o1, l1, s1) = &horiz[i];
        for j in (i + 1)..horiz.len() {
            let (y2, x1b, x2b, o2, l2, s2) = &horiz[j];
            let loop_siblings = (l1.as_deref() == Some("next")
                && l2.as_deref() == Some("done")
                && o1.1 == o2.0)
                || (l1.as_deref() == Some("done") && l2.as_deref() == Some("next") && o1.0 == o2.1);
            let allow_overlap = o1.0 == o2.0 && s1 == s2;
            if o1 == o2 {
                continue;
            }
            let overlap_len = min(*x2a, *x2b) - max(*x1a, *x1b);
            if *y1 == *y2 && overlap_len > 0 {
                if allow_overlap {
                    allowed_parallel_overlap_count += 1;
                    if allowed_parallel_overlap_samples.len() < 8 {
                        allowed_parallel_overlap_samples.push(format!(
                            "H y={} overlap={} edges {}->{} vs {}->{}",
                            y1, overlap_len, o1.0, o1.1, o2.0, o2.1
                        ));
                    }
                    continue;
                }
                parallel_overlap_count += 1;
                edge_overlap_count += 1;
                if parallel_overlap_samples.len() < 8 {
                    parallel_overlap_samples.push(format!(
                        "H y={} overlap={} edges {}->{} vs {}->{}",
                        y1, overlap_len, o1.0, o1.1, o2.0, o2.1
                    ));
                }
                if loop_siblings {
                    loop_next_done_overlap_count += 1;
                }
                continue;
            }
            // Detect short very-close horizontal runs that can visually merge at stroke width.
            if !allow_overlap && overlap_len > 0 && (*y1 - *y2).abs() <= (STROKE_WIDTH * 2 + 1) {
                micro_overlap_risk_count += 1;
            }
            if (*y1 - *y2).abs() < MIN_CLOSE_VIOLATION && overlap_len > 80 {
                close_parallel_count += 1;
                continue;
            }
            if loop_siblings && (*y1 - *y2).abs() < MIN_CLOSE_VIOLATION && overlap_len > 24 {
                close_parallel_count += 1;
                loop_next_done_close_count += 1;
            }
        }
    }
    for i in 0..vert.len() {
        let (x1, y1a, y2a, o1, l1, s1) = &vert[i];
        for j in (i + 1)..vert.len() {
            let (x2, y1b, y2b, o2, l2, s2) = &vert[j];
            let loop_siblings = (l1.as_deref() == Some("next")
                && l2.as_deref() == Some("done")
                && o1.1 == o2.0)
                || (l1.as_deref() == Some("done") && l2.as_deref() == Some("next") && o1.0 == o2.1);
            let allow_overlap = o1.0 == o2.0 && s1 == s2;
            if o1 == o2 {
                continue;
            }
            let overlap_len = min(*y2a, *y2b) - max(*y1a, *y1b);
            if *x1 == *x2 && overlap_len > 0 {
                if allow_overlap {
                    allowed_parallel_overlap_count += 1;
                    if allowed_parallel_overlap_samples.len() < 8 {
                        allowed_parallel_overlap_samples.push(format!(
                            "V x={} overlap={} edges {}->{} vs {}->{}",
                            x1, overlap_len, o1.0, o1.1, o2.0, o2.1
                        ));
                    }
                    continue;
                }
                parallel_overlap_count += 1;
                edge_overlap_count += 1;
                if parallel_overlap_samples.len() < 8 {
                    parallel_overlap_samples.push(format!(
                        "V x={} overlap={} edges {}->{} vs {}->{}",
                        x1, overlap_len, o1.0, o1.1, o2.0, o2.1
                    ));
                }
                if loop_siblings {
                    loop_next_done_overlap_count += 1;
                }
                continue;
            }
            // Detect short very-close vertical runs that can visually merge at stroke width.
            if !allow_overlap && overlap_len > 0 && (*x1 - *x2).abs() <= (STROKE_WIDTH * 2 + 1) {
                micro_overlap_risk_count += 1;
            }
            if (*x1 - *x2).abs() < MIN_CLOSE_VIOLATION && overlap_len > 80 {
                close_parallel_count += 1;
                continue;
            }
            if loop_siblings && (*x1 - *x2).abs() < MIN_CLOSE_VIOLATION && overlap_len > 24 {
                close_parallel_count += 1;
                loop_next_done_close_count += 1;
            }
        }
    }

    if parallel_overlap_count > 0 {
        issues.push(format!(
            "Parallel segments overlap: {}",
            parallel_overlap_count
        ));
    }
    if close_parallel_count > 0 {
        issues.push(format!(
            "Parallel segments too close: {}",
            close_parallel_count
        ));
    }
    if micro_overlap_risk_count > 0 {
        issues.push(format!(
            "Micro-overlap visual risk (very close short parallel runs): {}",
            micro_overlap_risk_count
        ));
    }
    if loop_next_done_overlap_count > 0 {
        issues.push(format!(
            "Loop next/done siblings overlap: {}",
            loop_next_done_overlap_count
        ));
    }
    if loop_next_done_close_count > 0 {
        issues.push(format!(
            "Loop next/done siblings too close: {}",
            loop_next_done_close_count
        ));
    }
    let issue_count = issues.len();
    let issue_samples: Vec<String> = issues.into_iter().take(30).collect();

    json!({
        "node_overlaps": overlap_count,
        "edge_node_crossings": cross_count,
        "line_block_clearance_violations": line_block_clearance_violations,
        "shared_input_ports": shared_input_port_count,
        "bad_entry_direction_edges": bad_entry_dir_count,
        "non_orth_segments": non_orth_count,
        "edge_edge_overlaps": edge_overlap_count,
        "parallel_overlap_segments": parallel_overlap_count,
        "parallel_overlap_samples": parallel_overlap_samples,
        "allowed_parallel_overlap_segments": allowed_parallel_overlap_count,
        "allowed_parallel_overlap_samples": allowed_parallel_overlap_samples,
        "close_parallel_segments": close_parallel_count,
        "micro_overlap_risk_segments": micro_overlap_risk_count,
        "loop_next_done_overlap_segments": loop_next_done_overlap_count,
        "loop_next_done_close_segments": loop_next_done_close_count,
        "issue_count": issue_count,
        "issue_samples": issue_samples,
        "nodes": rects.len(),
        "edges": edge_paths.len(),
    })
}

fn wrap_text_approx(text: &str, max_chars: usize) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut cur = String::new();
    for w in text.split_whitespace() {
        if cur.is_empty() {
            cur.push_str(w);
        } else if cur.len() + 1 + w.len() <= max_chars {
            cur.push(' ');
            cur.push_str(w);
        } else {
            out.push(cur);
            cur = w.to_string();
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn truncate_ellipsis(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    if max_chars <= 3 {
        return "...".to_string();
    }
    let mut out = String::new();
    for ch in s.chars().take(max_chars - 3) {
        out.push(ch);
    }
    out.push_str("...");
    out
}

fn text_size_px(font: &FontRef<'_>, font_size: f32, text: &str) -> (i32, i32) {
    let (w, h) = text_size(PxScale::from(font_size), font, text);
    (w as i32, h as i32)
}

fn line_height_px(font: &FontRef<'_>, font_size: f32) -> i32 {
    let (_w, h) = text_size_px(font, font_size, "Ag");
    max(1, h + 4)
}

fn wrap_text_by_width(
    font: &FontRef<'_>,
    font_size: f32,
    text: &str,
    max_w_px: i32,
) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return vec![String::new()];
    }
    let mut lines: Vec<String> = vec![words[0].to_string()];
    for w in words.iter().skip(1) {
        let cur = lines.last().cloned().unwrap_or_default();
        let cand = format!("{cur} {w}");
        if text_size_px(font, font_size, &cand).0 <= max_w_px {
            let last = lines.len() - 1;
            lines[last] = cand;
        } else {
            lines.push((*w).to_string());
        }
    }
    lines
}

fn fit_text_lines(
    font: &FontRef<'_>,
    text: &str,
    max_w_px: i32,
    max_h_px: i32,
    font_size: f32,
) -> (Vec<String>, i32) {
    let lh = line_height_px(font, font_size);
    let max_lines = max(1, max_h_px / max(1, lh)) as usize;
    let mut lines = wrap_text_by_width(font, font_size, text, max_w_px.max(20));
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        // Ensure last visible line fits after ellipsis.
        let mut last = lines.last().cloned().unwrap_or_default();
        while !last.is_empty()
            && text_size_px(font, font_size, &(last.clone() + "...")).0 > max_w_px
        {
            last.pop();
        }
        lines[max_lines - 1] = if last.is_empty() {
            "...".to_string()
        } else {
            format!("{last}...")
        };
    }
    (lines, lh)
}

fn draw_rounded_rect_mut(
    img: &mut RgbImage,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    radius: i32,
    fill: Rgb<u8>,
    outline: Rgb<u8>,
) {
    let r = max(0, min(radius, min((x2 - x1) / 2, (y2 - y1) / 2)));
    if r <= 1 {
        let rw = (x2 - x1).max(1) as u32;
        let rh = (y2 - y1).max(1) as u32;
        for yy in y1..y2 {
            draw_line_segment_mut(img, (x1 as f32, yy as f32), (x2 as f32, yy as f32), fill);
        }
        draw_hollow_rect_mut(img, Rect::at(x1, y1).of_size(rw, rh), outline);
        return;
    }

    // Fill center and side bands.
    for yy in (y1 + r)..(y2 - r) {
        draw_line_segment_mut(img, (x1 as f32, yy as f32), (x2 as f32, yy as f32), fill);
    }
    for yy in y1..(y1 + r) {
        draw_line_segment_mut(
            img,
            ((x1 + r) as f32, yy as f32),
            ((x2 - r) as f32, yy as f32),
            fill,
        );
    }
    for yy in (y2 - r)..y2 {
        draw_line_segment_mut(
            img,
            ((x1 + r) as f32, yy as f32),
            ((x2 - r) as f32, yy as f32),
            fill,
        );
    }

    // Fill quarter circles.
    draw_filled_circle_mut(img, (x1 + r, y1 + r), r, fill);
    draw_filled_circle_mut(img, (x2 - r, y1 + r), r, fill);
    draw_filled_circle_mut(img, (x1 + r, y2 - r), r, fill);
    draw_filled_circle_mut(img, (x2 - r, y2 - r), r, fill);

    // Outline: edge lines + corner arcs.
    draw_thick_line_mut(
        img,
        (x1 + r, y1),
        (x2 - r, y1),
        outline,
        BLOCK_OUTLINE_WIDTH,
    );
    draw_thick_line_mut(
        img,
        (x1 + r, y2),
        (x2 - r, y2),
        outline,
        BLOCK_OUTLINE_WIDTH,
    );
    draw_thick_line_mut(
        img,
        (x1, y1 + r),
        (x1, y2 - r),
        outline,
        BLOCK_OUTLINE_WIDTH,
    );
    draw_thick_line_mut(
        img,
        (x2, y1 + r),
        (x2, y2 - r),
        outline,
        BLOCK_OUTLINE_WIDTH,
    );
    draw_quarter_arc_mut(img, (x1 + r, y1 + r), r, BLOCK_CORNER_WIDTH, "tl", outline);
    draw_quarter_arc_mut(img, (x2 - r, y1 + r), r, BLOCK_CORNER_WIDTH, "tr", outline);
    draw_quarter_arc_mut(img, (x1 + r, y2 - r), r, BLOCK_CORNER_WIDTH, "bl", outline);
    draw_quarter_arc_mut(img, (x2 - r, y2 - r), r, BLOCK_CORNER_WIDTH, "br", outline);
}

fn edge_label_text(raw: &Option<String>) -> Option<String> {
    let Some(s) = raw else { return None };
    let t = s.trim().to_ascii_lowercase();
    if t.is_empty() || t == "enter" {
        return None;
    }
    if t == "done" {
        return Some("loop done".to_string());
    }
    if t == "next" {
        return Some("loop back".to_string());
    }
    Some(s.clone())
}

fn pick_label_anchor(path: &[(i32, i32)], prefer_early: bool, prefer_late: bool) -> (i32, i32) {
    if prefer_early {
        for i in 0..path.len().saturating_sub(1) {
            let a = path[i];
            let b = path[i + 1];
            if a.1 == b.1 && (a.0 - b.0).abs() >= 40 {
                return ((a.0 + b.0) / 2, a.1 - 14);
            }
        }
        for i in 0..path.len().saturating_sub(1) {
            let a = path[i];
            let b = path[i + 1];
            if a.0 == b.0 && (a.1 - b.1).abs() >= 36 {
                return (a.0 + 10, (a.1 + b.1) / 2 - 10);
            }
        }
    }
    if prefer_late {
        for i in (0..path.len().saturating_sub(1)).rev() {
            let a = path[i];
            let b = path[i + 1];
            if a.1 == b.1 && (a.0 - b.0).abs() >= 40 {
                let t = 0.72f32;
                let mx = (a.0 as f32 * (1.0 - t) + b.0 as f32 * t) as i32;
                return (mx, a.1 - 16);
            }
        }
        for i in (0..path.len().saturating_sub(1)).rev() {
            let a = path[i];
            let b = path[i + 1];
            if a.0 == b.0 && (a.1 - b.1).abs() >= 36 {
                let t = 0.72f32;
                let my = (a.1 as f32 * (1.0 - t) + b.1 as f32 * t) as i32;
                return (a.0 + 10, my - 10);
            }
        }
    }
    let mut best: Option<(i32, i32)> = None;
    let mut best_len = -1;
    for i in 0..path.len().saturating_sub(1) {
        let a = path[i];
        let b = path[i + 1];
        if a.1 == b.1 {
            let seg_len = (a.0 - b.0).abs();
            if seg_len > best_len {
                best_len = seg_len;
                best = Some(((a.0 + b.0) / 2, a.1 - 14));
            }
        }
    }
    if let Some(p) = best {
        return p;
    }
    if path.len() >= 2 {
        let a = path[0];
        let b = path[1];
        return ((a.0 + b.0) / 2 + 8, (a.1 + b.1) / 2 - 12);
    }
    (0, 0)
}

fn common_prefix_points(a: &[(i32, i32)], b: &[(i32, i32)]) -> usize {
    let n = min(a.len(), b.len());
    let mut k = 0usize;
    while k < n && a[k] == b[k] {
        k += 1;
    }
    k
}

#[derive(Clone, Copy)]
struct SpecialLabelAnchor {
    x: i32,
    y: i32,
    axis: char, // 'h' means keep x near segment center, move in y. 'v' means keep y, move in x.
}

fn draw_arrow_head(draw: &mut RgbImage, from: (i32, i32), to: (i32, i32), color: Rgb<u8>) {
    let (x1, y1) = (from.0 as f32, from.1 as f32);
    let (x2, y2) = (to.0 as f32, to.1 as f32);
    let ang = (y2 - y1).atan2(x2 - x1);
    let s = 18.0f32;
    let p1 = Point::new(x2 as i32, y2 as i32);
    let p2 = Point::new(
        (x2 - s * (ang - 0.5).cos()) as i32,
        (y2 - s * (ang - 0.5).sin()) as i32,
    );
    let p3 = Point::new(
        (x2 - s * (ang + 0.5).cos()) as i32,
        (y2 - s * (ang + 0.5).sin()) as i32,
    );
    draw_polygon_mut(draw, &[p1, p2, p3], color);
}

fn draw_thick_line_mut(
    img: &mut RgbImage,
    a: (i32, i32),
    b: (i32, i32),
    color: Rgb<u8>,
    width: i32,
) {
    if width <= 1 {
        draw_line_segment_mut(
            img,
            (a.0 as f32, a.1 as f32),
            (b.0 as f32, b.1 as f32),
            color,
        );
        return;
    }
    let r = width / 2;
    for ox in -r..=r {
        for oy in -r..=r {
            if ox * ox + oy * oy > r * r + 1 {
                continue;
            }
            draw_line_segment_mut(
                img,
                ((a.0 + ox) as f32, (a.1 + oy) as f32),
                ((b.0 + ox) as f32, (b.1 + oy) as f32),
                color,
            );
        }
    }
}

fn draw_quarter_arc_mut(
    img: &mut RgbImage,
    center: (i32, i32),
    radius: i32,
    width: i32,
    quadrant: &str,
    color: Rgb<u8>,
) {
    let (cx, cy) = center;
    let (start_deg, end_deg) = match quadrant {
        "tl" => (180.0f32, 270.0f32),
        "tr" => (270.0f32, 360.0f32),
        "bl" => (90.0f32, 180.0f32),
        "br" => (0.0f32, 90.0f32),
        _ => return,
    };
    // Draw arc as many tiny thick line segments to match straight-segment stroke width.
    let steps = max(36, radius * 6);
    let mut prev: Option<(i32, i32)> = None;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let deg = start_deg + (end_deg - start_deg) * t;
        let rad = deg.to_radians();
        let p = (
            (cx as f32 + radius as f32 * rad.cos()).round() as i32,
            (cy as f32 + radius as f32 * rad.sin()).round() as i32,
        );
        if let Some(pp) = prev {
            draw_thick_line_mut(img, pp, p, color, width);
        }
        prev = Some(p);
    }
}

fn run_native(cli: &Cli) -> Result<()> {
    let data = fs::read_to_string(&cli.graph_json)
        .with_context(|| format!("failed to read {}", cli.graph_json.display()))?;
    let graph: Graph = serde_json::from_str(&data).context("failed to parse graph JSON")?;

    let node_by_id: HashMap<String, Node> = graph
        .nodes
        .iter()
        .map(|n| (n.id.clone(), n.clone()))
        .collect();

    let mut lanes: Vec<i32> = graph.nodes.iter().map(|n| n.lane.unwrap_or(0)).collect();
    lanes.sort_unstable();
    lanes.dedup();
    let mut levels: Vec<i32> = graph.nodes.iter().map(|n| n.level.unwrap_or(0)).collect();
    levels.sort_unstable();
    levels.dedup();

    let lane_to_idx: HashMap<i32, i32> = lanes
        .iter()
        .enumerate()
        .map(|(i, l)| (*l, i as i32))
        .collect();
    let level_to_idx: HashMap<i32, i32> = levels
        .iter()
        .enumerate()
        .map(|(i, l)| (*l, i as i32))
        .collect();

    let x_step = 1300;
    let y_step = 280;
    let left_pad = 300;
    let top_pad = 140;
    let mut w = left_pad * 2 + max(1, lanes.len() as i32 - 1) * x_step + 1800;
    let mut h = top_pad * 2 + max(1, levels.len() as i32) * y_step + 360;

    let mut img: RgbImage = ImageBuffer::from_pixel(w as u32, h as u32, rgb(STYLE_BG));
    let mut blocks_layer: Option<RgbImage> = if cli.separate_layers {
        Some(ImageBuffer::from_pixel(w as u32, h as u32, rgb(STYLE_BG)))
    } else {
        None
    };
    let mut arrows_layer: Option<RgbImage> = if cli.separate_layers {
        Some(ImageBuffer::from_pixel(w as u32, h as u32, rgb(STYLE_BG)))
    } else {
        None
    };
    let mut text_layer: Option<RgbaImage> = if cli.separate_layers {
        Some(ImageBuffer::from_pixel(
            w as u32,
            h as u32,
            rgba(STYLE_BG, 0),
        ))
    } else {
        None
    };

    let font_data = [
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
    ]
    .iter()
    .find_map(|p| fs::read(p).ok())
    .context("Could not load DejaVuSans font from system paths")?;
    let font = FontRef::try_from_slice(&font_data).context("invalid font bytes")?;

    let source = graph
        .source
        .as_deref()
        .and_then(|s| {
            Path::new(s)
                .file_name()
                .and_then(|f| f.to_str())
                .map(|x| x.to_string())
        })
        .unwrap_or_else(|| cli.graph_json.to_string_lossy().to_string());
    let title = format!("Graph-driven Flowchart: {}", source);
    let (tw, th) = text_size_px(&font, 38.0, &title);
    let mut content_bbox = BBox::new();
    let title_x = w / 2 - tw / 2;
    let title_y = 30;
    content_bbox.include_rect(title_x, title_y, title_x + tw, title_y + th);
    draw_text_mut(
        &mut img,
        rgb(STYLE_TEXT),
        title_x as i32,
        title_y,
        PxScale::from(38.0),
        &font,
        &title,
    );
    if let Some(tl) = text_layer.as_mut() {
        draw_text_mut(
            tl,
            rgba(STYLE_TEXT, 255),
            title_x as i32,
            title_y,
            PxScale::from(38.0),
            &font,
            &title,
        );
    }

    let mut bucket_sizes: HashMap<(i32, i32), i32> = HashMap::new();
    for n in &graph.nodes {
        let key = (n.lane.unwrap_or(0), n.level.unwrap_or(0));
        *bucket_sizes.entry(key).or_insert(0) += 1;
    }
    let mut bucket_seen: HashMap<(i32, i32), i32> = HashMap::new();

    let mut level_node_count: HashMap<i32, i32> = HashMap::new();
    for n in &graph.nodes {
        let lv = n.level.unwrap_or(0);
        *level_node_count.entry(lv).or_insert(0) += 1;
    }
    let mut level_stack_pressure: HashMap<i32, i32> = HashMap::new();
    for ((_, lv), cnt) in &bucket_sizes {
        if *cnt > 1 {
            let e = level_stack_pressure.entry(*lv).or_insert(0);
            *e = max(*e, *cnt - 1);
        }
    }
    let mut level_edge_touch: HashMap<i32, i32> = levels.iter().map(|lv| (*lv, 0)).collect();
    let mut level_cross_pressure: HashMap<i32, i32> = levels.iter().map(|lv| (*lv, 0)).collect();
    let mut level_minlen_pressure: HashMap<i32, i32> = levels.iter().map(|lv| (*lv, 0)).collect();
    for e in &graph.edges {
        let sl = node_by_id[&e.from].level.unwrap_or(0);
        let dl = node_by_id[&e.to].level.unwrap_or(0);
        let is_back = e.back_edge.unwrap_or(false);
        *level_edge_touch.entry(sl).or_insert(0) += 1;
        *level_edge_touch.entry(dl).or_insert(0) += 1;
        let (lo, hi) = if sl <= dl { (sl, dl) } else { (dl, sl) };
        if hi - lo > 1 {
            for lv in &levels {
                if lo < *lv && *lv <= hi {
                    *level_cross_pressure.entry(*lv).or_insert(0) += 1;
                }
            }
        }
        if !is_back && (dl - sl).abs() <= 1 {
            let key_lv = max(sl, dl);
            *level_minlen_pressure.entry(key_lv).or_insert(0) += 1;
        }
    }
    let mut level_extra: HashMap<i32, i32> = HashMap::new();
    let mut accum = 0;
    for lv in &levels {
        level_extra.insert(*lv, accum);
        let score = level_node_count.get(lv).copied().unwrap_or(0) as f32
            + 0.30 * level_edge_touch.get(lv).copied().unwrap_or(0) as f32
            + 0.35 * level_cross_pressure.get(lv).copied().unwrap_or(0) as f32
            + 0.45 * level_minlen_pressure.get(lv).copied().unwrap_or(0) as f32;
        if score >= 8.0 {
            accum += 120;
        } else if score >= 6.0 {
            accum += 70;
        }
        let stack_extra = level_stack_pressure.get(lv).copied().unwrap_or(0);
        if stack_extra > 0 {
            accum += stack_extra * 260;
        }
    }

    let mut rect: HashMap<String, (i32, i32, i32, i32)> = HashMap::new();
    let mut pos: HashMap<String, (i32, i32)> = HashMap::new();
    for n in &graph.nodes {
        let lane = *lane_to_idx.get(&n.lane.unwrap_or(0)).unwrap_or(&0);
        let lvl = *level_to_idx.get(&n.level.unwrap_or(0)).unwrap_or(&0);
        let key = (n.lane.unwrap_or(0), n.level.unwrap_or(0));
        let idx = *bucket_seen.get(&key).unwrap_or(&0);
        bucket_seen.insert(key, idx + 1);
        let vertical_stagger = idx * 260;

        let cx = left_pad + lane * x_step + 600;
        let lv = n.level.unwrap_or(0);
        let cy = top_pad
            + lvl * y_step
            + level_extra.get(&lv).copied().unwrap_or(0)
            + 120
            + vertical_stagger;
        let (nw, nh) = node_dims(&n.shape);
        let x1 = cx - nw / 2;
        let y1 = cy - nh / 2;
        let x2 = cx + nw / 2;
        let y2 = cy + nh / 2;

        rect.insert(n.id.clone(), (x1, y1, x2, y2));
        pos.insert(n.id.clone(), (cx, cy));
    }
    let base_node_y: HashMap<String, i32> = pos.iter().map(|(k, v)| (k.clone(), v.1)).collect();

    // Enforce additional vertical clearance between overlapping columns of blocks so
    // ingress/egress stubs and nearby lines have room without visually merging.
    let min_block_gap = max(
        MIN_EXIT_VERTICAL + MIN_TOP_ENTRY_LEN + MIN_LINE_GAP + 20,
        180,
    );
    let mut vertical_shift_passes = 0i32;
    let mut vertical_shift_applied_px = 0i32;
    for _ in 0..10 {
        let mut shift_from_level: HashMap<i32, i32> = HashMap::new();
        for a in &graph.nodes {
            for b in &graph.nodes {
                let la = a.level.unwrap_or(0);
                let lb = b.level.unwrap_or(0);
                if lb <= la {
                    continue;
                }
                let ra = rect[&a.id];
                let rb = rect[&b.id];
                let x_overlap = min(ra.2, rb.2) - max(ra.0, rb.0);
                if x_overlap < 100 {
                    continue;
                }
                let gap = rb.1 - ra.3;
                if gap < min_block_gap {
                    let need = min_block_gap - gap;
                    let e = shift_from_level.entry(lb).or_insert(0);
                    if need > *e {
                        *e = need;
                    }
                }
            }
        }
        if shift_from_level.is_empty() {
            break;
        }
        vertical_shift_passes += 1;

        let mut levels_sorted: Vec<i32> = shift_from_level.keys().copied().collect();
        levels_sorted.sort_unstable();
        for from_lv in levels_sorted {
            let delta = shift_from_level.get(&from_lv).copied().unwrap_or(0);
            if delta <= 0 {
                continue;
            }
            vertical_shift_applied_px += delta;
            for n in &graph.nodes {
                let lv = n.level.unwrap_or(0);
                if lv >= from_lv {
                    if let Some(r) = rect.get_mut(&n.id) {
                        r.1 += delta;
                        r.3 += delta;
                    }
                    if let Some(p) = pos.get_mut(&n.id) {
                        p.1 += delta;
                    }
                }
            }
        }
    }
    let mut vertical_shift_nodes = 0i32;
    let mut vertical_shift_total_px = 0i32;
    let mut vertical_shift_max_px = 0i32;
    for n in &graph.nodes {
        let y0 = base_node_y.get(&n.id).copied().unwrap_or(0);
        let y1 = pos.get(&n.id).copied().unwrap_or((0, y0)).1;
        let d = max(0, y1 - y0);
        if d > 0 {
            vertical_shift_nodes += 1;
            vertical_shift_total_px += d;
            vertical_shift_max_px = max(vertical_shift_max_px, d);
        }
    }

    // Ensure the working canvas is large enough for stacked nodes before routing/drawing.
    let needed_w = rect.values().map(|r| r.2 + 260).max().unwrap_or(w);
    let needed_h = rect.values().map(|r| r.3 + 320).max().unwrap_or(h);
    if needed_w > w || needed_h > h {
        w = max(w, needed_w);
        h = max(h, needed_h);
        img = ImageBuffer::from_pixel(w as u32, h as u32, rgb(STYLE_BG));
        if cli.separate_layers {
            blocks_layer = Some(ImageBuffer::from_pixel(w as u32, h as u32, rgb(STYLE_BG)));
            arrows_layer = Some(ImageBuffer::from_pixel(w as u32, h as u32, rgb(STYLE_BG)));
            text_layer = Some(ImageBuffer::from_pixel(
                w as u32,
                h as u32,
                rgba(STYLE_BG, 0),
            ));
        } else {
            blocks_layer = None;
            arrows_layer = None;
            text_layer = None;
        }
        content_bbox = BBox::new();
        content_bbox.include_rect(title_x, title_y, title_x + tw, title_y + th);
        draw_text_mut(
            &mut img,
            rgb(STYLE_TEXT),
            title_x as i32,
            title_y,
            PxScale::from(38.0),
            &font,
            &title,
        );
        if let Some(tl) = text_layer.as_mut() {
            draw_text_mut(
                tl,
                rgba(STYLE_TEXT, 255),
                title_x as i32,
                title_y,
                PxScale::from(38.0),
                &font,
                &title,
            );
        }
    }

    let min_rx = rect.values().map(|r| r.0).min().unwrap_or(0);
    let max_rx = rect.values().map(|r| r.2).max().unwrap_or(w);
    let center_x = (min_rx + max_rx) / 2;

    let mut out_map: HashMap<String, Vec<usize>> = HashMap::new();
    let mut in_map: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, e) in graph.edges.iter().enumerate() {
        out_map.entry(e.from.clone()).or_default().push(i);
        in_map.entry(e.to.clone()).or_default().push(i);
    }

    let mut src_split_offset: HashMap<String, i32> = HashMap::new();
    let mut row_sources: BTreeMap<i32, Vec<String>> = BTreeMap::new();
    for src in out_map.keys() {
        if let Some(r) = rect.get(src) {
            row_sources.entry(r.3).or_default().push(src.clone());
        }
    }
    for (_y, mut srcs) in row_sources {
        srcs.sort();
        let mid = (srcs.len() as f32 - 1.0) / 2.0;
        for (i, s) in srcs.iter().enumerate() {
            src_split_offset.insert(s.clone(), ((i as f32 - mid) * 18.0) as i32);
        }
    }

    let mut out_order: HashMap<(String, String, Option<String>), i32> = HashMap::new();
    let mut out_count: HashMap<String, i32> = HashMap::new();
    let mut in_order: HashMap<(String, String, Option<String>), i32> = HashMap::new();
    let mut in_count: HashMap<String, i32> = HashMap::new();
    for (src, elist) in &out_map {
        for (idx, edge_idx) in elist.iter().enumerate() {
            let e = &graph.edges[*edge_idx];
            out_order.insert((e.from.clone(), e.to.clone(), e.label.clone()), idx as i32);
            out_count.insert(src.clone(), elist.len() as i32);
        }
    }
    for (dst, elist) in &in_map {
        for (idx, edge_idx) in elist.iter().enumerate() {
            let e = &graph.edges[*edge_idx];
            in_order.insert((e.from.clone(), e.to.clone(), e.label.clone()), idx as i32);
            in_count.insert(dst.clone(), elist.len() as i32);
        }
    }

    let mut used_in_ports: HashMap<String, HashSet<(i32, i32)>> = HashMap::new();
    let mut used_out_ports: HashMap<String, HashSet<(i32, i32)>> = HashMap::new();
    let mut left_source_lane: HashMap<String, i32> = HashMap::new();
    let mut right_source_lane: HashMap<String, i32> = HashMap::new();
    let outer_step = 130;

    let mut edge_order: Vec<usize> = (0..graph.edges.len()).collect();
    edge_order.sort_by_key(|i| {
        let e = &graph.edges[*i];
        let src = &node_by_id[&e.from];
        let dst = &node_by_id[&e.to];
        let s_lvl = src.level.unwrap_or(0);
        let d_lvl = dst.level.unwrap_or(0);
        let lbl = e
            .label
            .clone()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let is_back = e.back_edge.unwrap_or(d_lvl <= s_lvl || lbl == "next");
        let src_cx = (rect[&e.from].0 + rect[&e.from].2) / 2;
        let dst_cx = (rect[&e.to].0 + rect[&e.to].2) / 2;
        let from_above = (s_lvl < d_lvl) && !is_back;
        (
            e.to.clone(),
            if from_above { 0 } else { 1 },
            (src_cx - dst_cx).abs(),
            *i,
        )
    });

    let mut edge_meta_by_idx: Vec<Option<EdgeMeta>> = vec![None; graph.edges.len()];
    for idx in edge_order {
        let e = &graph.edges[idx];
        let key = (e.from.clone(), e.to.clone(), e.label.clone());
        let oi = *out_order.get(&key).unwrap_or(&0);
        let oc = *out_count.get(&e.from).unwrap_or(&1);
        let _ii = *in_order.get(&key).unwrap_or(&0);
        let _ic = *in_count.get(&e.to).unwrap_or(&1);
        let src_r = rect[&e.from];
        let dst_r = rect[&e.to];
        let src = &node_by_id[&e.from];
        let dst = &node_by_id[&e.to];
        let force_center = dst.shape == "terminator" && dst.text.trim().eq_ignore_ascii_case("end");
        let src_level = src.level.unwrap_or(0);
        let dst_level = dst.level.unwrap_or(0);
        let lbl = e
            .label
            .clone()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let is_back = e.back_edge.unwrap_or(dst_level <= src_level);
        let split_base = src_r.3 + 28;
        let split_y = split_base + src_split_offset.get(&e.from).copied().unwrap_or(0);
        let src_cx = (src_r.0 + src_r.2) / 2;
        let is_end = dst.shape == "terminator" && dst.text.trim().eq_ignore_ascii_case("end");
        let mut prefer_side: Option<String> = if is_back || lbl == "next" || is_end {
            if src_cx <= center_x {
                Some("left".to_string())
            } else {
                Some("right".to_string())
            }
        } else {
            None
        };
        let mut side_lane_x: Option<i32> = None;
        if prefer_side.as_deref() == Some("left") {
            if !left_source_lane.contains_key(&e.from) {
                let lane_x = min_rx - 220 - (left_source_lane.len() as i32) * outer_step;
                left_source_lane.insert(e.from.clone(), lane_x);
            }
            side_lane_x = left_source_lane.get(&e.from).copied();
        } else if prefer_side.as_deref() == Some("right") {
            if !right_source_lane.contains_key(&e.from) {
                let lane_x = max_rx + 220 + (right_source_lane.len() as i32) * outer_step;
                right_source_lane.insert(e.from.clone(), lane_x);
            }
            side_lane_x = right_source_lane.get(&e.from).copied();
        }
        let dst_cx = (dst_r.0 + dst_r.2) / 2;
        if src.shape == "decision" {
            let dx = dst_cx - src_cx;
            prefer_side = if dx < -16 {
                Some("left".to_string())
            } else if dx > 16 {
                Some("right".to_string())
            } else {
                None
            };
        }
        let approach_side = if src_cx < dst_cx {
            Some("left")
        } else {
            Some("right")
        };
        let mut src_candidates = output_port_candidates(src_r, &src.shape, prefer_side.as_deref());
        let mut src_prefer_pt = Some(((dst_r.0 + dst_r.2) / 2, (dst_r.1 + dst_r.3) / 2));
        if src.shape == "decision" {
            let mut ordered: Vec<(i32, i32)> = vec![];
            let side_order = decision_output_side_order(src_r, dst_r, &lbl);
            for side in side_order {
                extend_unique(&mut ordered, side_port_candidates(src_r, &src.shape, side));
            }
            if !ordered.is_empty() {
                src_candidates = ordered;
                // Preserve semantic ordering for decision branches.
                src_prefer_pt = None;
            }
        }
        let dst_candidates = input_port_candidates(dst_r, &dst.shape, approach_side);
        let src_pt = choose_port_role(
            &mut used_in_ports,
            &mut used_out_ports,
            &e.from,
            &src_candidates,
            "out",
            src_prefer_pt,
        );
        let src_center = ((src_r.0 + src_r.2) / 2, (src_r.1 + src_r.3) / 2);
        let dst_pref_pt = if prefer_side.as_deref() == Some("left") {
            (dst_r.0, src_center.1)
        } else if prefer_side.as_deref() == Some("right") {
            (dst_r.2, src_center.1)
        } else {
            src_center
        };
        let dst_pt = choose_input_port_prioritize_top(
            &mut used_in_ports,
            &mut used_out_ports,
            &e.to,
            &rect,
            dst_r,
            &dst_candidates,
            Some(dst_pref_pt),
        );
        let src_side = port_side_from_point(src_r, src_pt).to_string();
        let dst_side = port_side_from_point(dst_r, dst_pt).to_string();
        // Evaluate all legal ports for this edge in the subsequent routing phase.
        // Keep deduped order from candidate generation (side preference encoded there).
        let src_var = unique_ports(&src_candidates);
        let dst_var = unique_ports(&dst_candidates);
        if std::env::var("CODEFLOW_DEBUG_PORTS").is_ok() {
            eprintln!(
                "[ports] {} -> {} label={:?} src_side={} src_pt={:?} dst_side={} dst_pt={:?}",
                e.from, e.to, e.label, src_side, src_pt, dst_side, dst_pt
            );
        }
        let mut dst_candidates_all = vec![];
        for p in &dst_candidates {
            if !dst_candidates_all.contains(p) {
                dst_candidates_all.push(*p);
            }
        }
        edge_meta_by_idx[idx] = Some(EdgeMeta {
            edge_idx: idx,
            out_idx: oi,
            out_count: oc,
            src_split_y: split_y,
            force_center,
            prefer_side,
            sx_override: src_pt,
            tx_override: dst_pt,
            src_side,
            dst_side,
            src_port_variants: src_var,
            dst_port_variants: dst_var,
            dst_candidates_all,
            side_lane_x,
            bounds: (40, w - 40),
        });
    }

    let mut edge_paths: Vec<EdgePath> = vec![];
    for idx in 0..graph.edges.len() {
        let m = edge_meta_by_idx[idx].as_mut().unwrap();
        let e = &graph.edges[m.edge_idx];
        let existing_seg_lists: Vec<Vec<(char, i32, i32, i32)>> = edge_paths
            .iter()
            .map(|ep| path_segments(&ep.path))
            .collect();
        let mut best: Option<Vec<(i32, i32)>> = None;
        let mut best_key: (i64, i64, i64) = (i64::MAX, i64::MAX, i64::MAX);
        let mut best_meta: Option<((i32, i32), (i32, i32), String, String)> = None;

        for sp in m.src_port_variants.clone() {
            for tp in m.dst_port_variants.clone() {
                let ss = port_side_from_point(rect[&e.from], sp).to_string();
                let ds = port_side_from_point(rect[&e.to], tp).to_string();
                let cand = route_edge_smart(
                    &e.from,
                    &e.to,
                    &rect,
                    &edge_paths,
                    m.out_idx,
                    m.out_count,
                    m.src_split_y,
                    m.force_center,
                    m.prefer_side.as_deref(),
                    sp,
                    tp,
                    &ss,
                    &ds,
                    m.side_lane_x,
                    m.bounds,
                );
                let (plen, bends) = path_len_and_bends(&cand);
                let segs = path_segments_count(&cand);
                let secondary = bends as i64 * 120
                    + path_spacing_penalty_from_segments(
                        &path_segments(&cand),
                        &existing_seg_lists,
                    );
                let key = (segs, plen as i64, secondary);
                if best.is_none() || key < best_key {
                    best = Some(cand);
                    best_key = key;
                    best_meta = Some((sp, tp, ss, ds));
                }
            }
        }
        if let Some((sp, tp, ss, ds)) = best_meta {
            m.sx_override = sp;
            m.tx_override = tp;
            m.src_side = ss;
            m.dst_side = ds;
            if std::env::var("CODEFLOW_DEBUG_PORTS").is_ok() {
                eprintln!(
                    "[ports-final] {} -> {} label={:?} src_side={} src_pt={:?} dst_side={} dst_pt={:?}",
                    e.from, e.to, e.label, m.src_side, m.sx_override, m.dst_side, m.tx_override
                );
            }
        }
        edge_paths.push(EdgePath {
            from: e.from.clone(),
            to: e.to.clone(),
            label: e.label.clone(),
            path: best.unwrap_or_default(),
        });
    }

    for pass_idx in 0..6 {
        let mut improved = false;
        let mut ranking: Vec<(i64, usize)> = (0..edge_paths.len())
            .map(|i| (edge_conflict_score(i, &edge_paths, &rect), i))
            .collect();
        ranking.sort_by(|a, b| b.cmp(a));
        for (score, i) in ranking {
            if score <= 0 {
                continue;
            }
            let m = edge_meta_by_idx[i].as_ref().unwrap().clone();
            let e = &graph.edges[m.edge_idx];
            let old = edge_paths[i].path.clone();
            let old_score = edge_conflict_score(i, &edge_paths, &rect);
            let others: Vec<EdgePath> = edge_paths
                .iter()
                .enumerate()
                .filter_map(|(j, ep)| if j != i { Some(ep.clone()) } else { None })
                .collect();
            let new_path = route_edge_smart(
                &e.from,
                &e.to,
                &rect,
                &others,
                m.out_idx,
                m.out_count,
                m.src_split_y + ((pass_idx + 1) as i32 % 2) * 0,
                m.force_center,
                m.prefer_side.as_deref(),
                m.sx_override,
                m.tx_override,
                &m.src_side,
                &m.dst_side,
                m.side_lane_x,
                m.bounds,
            );
            edge_paths[i].path = new_path;
            let new_score = edge_conflict_score(i, &edge_paths, &rect);
            if new_score < old_score {
                improved = true;
            } else {
                edge_paths[i].path = old;
            }
        }
        if !improved {
            break;
        }
    }

    let mut incoming_idx: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, e) in edge_paths.iter().enumerate() {
        incoming_idx.entry(e.to.clone()).or_default().push(i);
    }
    for (dst_id, idxs) in incoming_idx {
        if idxs.len() != 1 {
            continue;
        }
        let i = idxs[0];
        let e = edge_paths[i].clone();
        let m = edge_meta_by_idx[i].as_ref().unwrap().clone();
        let dst_r = rect[&dst_id];
        let top_center = ((dst_r.0 + dst_r.2) / 2, dst_r.1);
        if e.path.last().copied() == Some(top_center) {
            continue;
        }
        let old_path = e.path.clone();
        let old_score = edge_conflict_score(i, &edge_paths, &rect);
        let others: Vec<EdgePath> = edge_paths
            .iter()
            .enumerate()
            .filter_map(|(j, ep)| if j != i { Some(ep.clone()) } else { None })
            .collect();
        let cand = route_edge_smart(
            &e.from,
            &e.to,
            &rect,
            &others,
            m.out_idx,
            m.out_count,
            m.src_split_y,
            true,
            m.prefer_side.as_deref(),
            m.sx_override,
            top_center,
            &m.src_side,
            "top",
            m.side_lane_x,
            m.bounds,
        );
        let (ok_dir, _) = incoming_dir_valid(&cand, dst_r);
        if !ok_dir {
            continue;
        }
        edge_paths[i].path = cand;
        let new_score = edge_conflict_score(i, &edge_paths, &rect);
        if new_score <= old_score {
            if let Some(mm) = edge_meta_by_idx[i].as_mut() {
                mm.tx_override = top_center;
                mm.dst_side = "top".to_string();
            }
        } else {
            edge_paths[i].path = old_path;
        }
    }

    let mut outgoing_idx: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, e) in edge_paths.iter().enumerate() {
        outgoing_idx.entry(e.from.clone()).or_default().push(i);
    }
    for (src_id, idxs) in outgoing_idx {
        if idxs.len() != 1 {
            continue;
        }
        let i = idxs[0];
        let e = edge_paths[i].clone();
        let m = edge_meta_by_idx[i].as_ref().unwrap().clone();
        let src_r = rect[&src_id];
        let bottom_center = ((src_r.0 + src_r.2) / 2, src_r.3);
        if e.path.first().copied() == Some(bottom_center) {
            continue;
        }
        let old_path = e.path.clone();
        let old_score = edge_conflict_score(i, &edge_paths, &rect);
        let others: Vec<EdgePath> = edge_paths
            .iter()
            .enumerate()
            .filter_map(|(j, ep)| if j != i { Some(ep.clone()) } else { None })
            .collect();
        let cand = route_edge_smart(
            &e.from,
            &e.to,
            &rect,
            &others,
            m.out_idx,
            m.out_count,
            m.src_split_y,
            m.force_center,
            m.prefer_side.as_deref(),
            bottom_center,
            m.tx_override,
            "bottom",
            &m.dst_side,
            m.side_lane_x,
            m.bounds,
        );
        let (ok_dir, _) = incoming_dir_valid(&cand, rect[&e.to]);
        if !ok_dir {
            continue;
        }
        edge_paths[i].path = cand;
        let new_score = edge_conflict_score(i, &edge_paths, &rect);
        if new_score <= old_score {
            if let Some(mm) = edge_meta_by_idx[i].as_mut() {
                mm.sx_override = bottom_center;
                mm.src_side = "bottom".to_string();
            }
        } else {
            edge_paths[i].path = old_path;
        }
    }

    for _ in 0..6 {
        let usage = input_port_usage(&edge_paths);
        let conflicts: Vec<((String, (i32, i32)), Vec<usize>)> =
            usage.into_iter().filter(|(_, v)| v.len() > 1).collect();
        if conflicts.is_empty() {
            break;
        }
        let mut changed = false;
        for ((dst_id, _pt), idxs) in conflicts {
            let keep_idx = *idxs.iter().min().unwrap_or(&idxs[0]);
            let mut sorted_idxs = idxs.clone();
            sorted_idxs.sort_by(|a, b| b.cmp(a));
            for i in sorted_idxs {
                if i == keep_idx {
                    continue;
                }
                let e = edge_paths[i].clone();
                let m = edge_meta_by_idx[i].as_ref().unwrap().clone();
                let old_path = e.path.clone();
                let old_score = edge_conflict_score(i, &edge_paths, &rect);

                let usage_now = input_port_usage(&edge_paths);
                let mut used_by_others: HashSet<(i32, i32)> = HashSet::new();
                for ((d, p), arr) in usage_now {
                    if d == dst_id && arr.iter().any(|j| *j != i) {
                        used_by_others.insert(p);
                    }
                }

                let src_x = old_path.first().map(|p| p.0).unwrap_or(m.sx_override.0);
                let mut candidates = m.dst_candidates_all.clone();
                candidates.sort_by_key(|p| (p.0 - src_x).abs());
                let mut rerouted = false;
                for tp in candidates {
                    if used_by_others.contains(&tp) {
                        continue;
                    }
                    let ds = port_side_from_point(rect[&dst_id], tp).to_string();
                    let others: Vec<EdgePath> = edge_paths
                        .iter()
                        .enumerate()
                        .filter_map(|(j, ep)| if j != i { Some(ep.clone()) } else { None })
                        .collect();
                    let cand = route_edge_smart(
                        &e.from,
                        &e.to,
                        &rect,
                        &others,
                        m.out_idx,
                        m.out_count,
                        m.src_split_y,
                        ds == "top",
                        m.prefer_side.as_deref(),
                        m.sx_override,
                        tp,
                        &m.src_side,
                        &ds,
                        m.side_lane_x,
                        m.bounds,
                    );
                    let (ok_dir, _) = incoming_dir_valid(&cand, rect[&dst_id]);
                    if !ok_dir {
                        continue;
                    }
                    edge_paths[i].path = cand;
                    let new_score = edge_conflict_score(i, &edge_paths, &rect);
                    if new_score <= old_score {
                        if let Some(mm) = edge_meta_by_idx[i].as_mut() {
                            mm.tx_override = tp;
                            mm.dst_side = ds;
                        }
                        changed = true;
                        rerouted = true;
                        break;
                    }
                    edge_paths[i].path = old_path.clone();
                }
                if !rerouted {
                    edge_paths[i].path = old_path;
                }
            }
        }
        if !changed {
            break;
        }
    }

    // Resolve left/right input-port inversions for the same destination:
    // if an edge coming from the left is mapped to a further-right port than an
    // edge coming from the right, try swapping their target ports.
    for _ in 0..4 {
        let mut changed = false;
        let mut by_dst: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, ep) in edge_paths.iter().enumerate() {
            by_dst.entry(ep.to.clone()).or_default().push(i);
        }
        for (_dst, idxs) in by_dst {
            if idxs.len() < 2 {
                continue;
            }
            for a in 0..idxs.len() {
                for b in (a + 1)..idxs.len() {
                    let i = idxs[a];
                    let j = idxs[b];
                    let mi = match edge_meta_by_idx[i].as_ref() {
                        Some(m) => m.clone(),
                        None => continue,
                    };
                    let mj = match edge_meta_by_idx[j].as_ref() {
                        Some(m) => m.clone(),
                        None => continue,
                    };
                    if mi.dst_side != mj.dst_side {
                        continue;
                    }
                    let src_i = edge_paths[i]
                        .path
                        .first()
                        .map(|p| p.0)
                        .unwrap_or(mi.sx_override.0);
                    let src_j = edge_paths[j]
                        .path
                        .first()
                        .map(|p| p.0)
                        .unwrap_or(mj.sx_override.0);
                    let tx_i = mi.tx_override.0;
                    let tx_j = mj.tx_override.0;
                    let inverted = (src_i < src_j && tx_i > tx_j) || (src_i > src_j && tx_i < tx_j);
                    if !inverted {
                        continue;
                    }

                    let old_i = edge_paths[i].path.clone();
                    let old_j = edge_paths[j].path.clone();
                    let old_score = edge_conflict_score(i, &edge_paths, &rect)
                        + edge_conflict_score(j, &edge_paths, &rect);

                    let others_i: Vec<EdgePath> = edge_paths
                        .iter()
                        .enumerate()
                        .filter_map(|(k, ep)| if k != i { Some(ep.clone()) } else { None })
                        .collect();
                    let cand_i = route_edge_smart(
                        &edge_paths[i].from,
                        &edge_paths[i].to,
                        &rect,
                        &others_i,
                        mi.out_idx,
                        mi.out_count,
                        mi.src_split_y,
                        mj.dst_side == "top",
                        mi.prefer_side.as_deref(),
                        mi.sx_override,
                        mj.tx_override,
                        &mi.src_side,
                        &mj.dst_side,
                        mi.side_lane_x,
                        mi.bounds,
                    );
                    let (ok_i, _) = incoming_dir_valid(&cand_i, rect[&edge_paths[i].to]);
                    if !ok_i {
                        continue;
                    }
                    edge_paths[i].path = cand_i;

                    let others_j: Vec<EdgePath> = edge_paths
                        .iter()
                        .enumerate()
                        .filter_map(|(k, ep)| if k != j { Some(ep.clone()) } else { None })
                        .collect();
                    let cand_j = route_edge_smart(
                        &edge_paths[j].from,
                        &edge_paths[j].to,
                        &rect,
                        &others_j,
                        mj.out_idx,
                        mj.out_count,
                        mj.src_split_y,
                        mi.dst_side == "top",
                        mj.prefer_side.as_deref(),
                        mj.sx_override,
                        mi.tx_override,
                        &mj.src_side,
                        &mi.dst_side,
                        mj.side_lane_x,
                        mj.bounds,
                    );
                    let (ok_j, _) = incoming_dir_valid(&cand_j, rect[&edge_paths[j].to]);
                    if !ok_j {
                        edge_paths[i].path = old_i;
                        continue;
                    }
                    edge_paths[j].path = cand_j;

                    let new_score = edge_conflict_score(i, &edge_paths, &rect)
                        + edge_conflict_score(j, &edge_paths, &rect);
                    // Enforce non-inverted left/right mapping as a higher priority than
                    // local score optimality, as long as the reroutes are valid.
                    if new_score <= old_score || inverted {
                        if let Some(mm) = edge_meta_by_idx[i].as_mut() {
                            mm.tx_override = mj.tx_override;
                            mm.dst_side = mj.dst_side.clone();
                        }
                        if let Some(mm) = edge_meta_by_idx[j].as_mut() {
                            mm.tx_override = mi.tx_override;
                            mm.dst_side = mi.dst_side.clone();
                        }
                        changed = true;
                    } else {
                        edge_paths[i].path = old_i;
                        edge_paths[j].path = old_j;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }

    // Precompute special label anchors for sibling edges sharing a trunk
    // (same source node and same source output anchor), to place each label
    // on its own diverging segment.
    let mut branch_label_anchor: HashMap<usize, SpecialLabelAnchor> = HashMap::new();
    for i in 0..edge_paths.len() {
        for j in (i + 1)..edge_paths.len() {
            let ei = &edge_paths[i];
            let ej = &edge_paths[j];
            if ei.from != ej.from {
                continue;
            }
            if ei.path.first() != ej.path.first() {
                continue;
            }
            let cp = common_prefix_points(&ei.path, &ej.path);
            if cp < 2 {
                continue;
            }
            if cp >= ei.path.len() || cp >= ej.path.len() {
                continue;
            }
            let ai0 = ei.path[cp - 1];
            let ai1 = ei.path[cp];
            let aj0 = ej.path[cp - 1];
            let aj1 = ej.path[cp];
            let offset = 22;

            let a_anchor = if ai0.1 == ai1.1 {
                SpecialLabelAnchor {
                    x: (ai0.0 + ai1.0) / 2,
                    y: ai0.1 - offset,
                    axis: 'h',
                }
            } else {
                SpecialLabelAnchor {
                    x: ai0.0 + offset,
                    y: (ai0.1 + ai1.1) / 2 - 8,
                    axis: 'v',
                }
            };
            let b_anchor = if aj0.1 == aj1.1 {
                SpecialLabelAnchor {
                    x: (aj0.0 + aj1.0) / 2,
                    y: aj0.1 + offset,
                    axis: 'h',
                }
            } else {
                SpecialLabelAnchor {
                    x: aj0.0 - offset,
                    y: (aj0.1 + aj1.1) / 2 - 8,
                    axis: 'v',
                }
            };

            branch_label_anchor.insert(i, a_anchor);
            branch_label_anchor.insert(j, b_anchor);
        }
    }

    // Draw edges.
    let mut used_label_boxes: Vec<(i32, i32, i32, i32)> = vec![];
    let mut arrow_segments: Vec<((i32, i32), (i32, i32))> = vec![];
    for (ep_idx, ep) in edge_paths.iter().enumerate() {
        for i in 0..ep.path.len().saturating_sub(1) {
            let a = ep.path[i];
            let b = ep.path[i + 1];
            draw_thick_line_mut(&mut img, a, b, rgb(STYLE_EDGE), STROKE_WIDTH);
            if let Some(al) = arrows_layer.as_mut() {
                draw_thick_line_mut(al, a, b, rgb(STYLE_EDGE), STROKE_WIDTH);
            }
            let pad = max(2, STROKE_WIDTH + 2);
            content_bbox.include_rect(
                min(a.0, b.0) - pad,
                min(a.1, b.1) - pad,
                max(a.0, b.0) + pad,
                max(a.1, b.1) + pad,
            );
        }
        if ep.path.len() >= 2 {
            arrow_segments.push((ep.path[ep.path.len() - 2], ep.path[ep.path.len() - 1]));
        }

        if !cli.hide_labels {
            if let Some(lbl) = edge_label_text(&ep.label) {
                let lbl_l = lbl.trim().to_ascii_lowercase();
                let prefer_early = matches!(
                    lbl_l.as_str(),
                    "yes"
                        | "no"
                        | "elseif"
                        | "else"
                        | "case"
                        | "default"
                        | "loop back"
                        | "loop done"
                );
                let prefer_late = lbl_l == "elseif";
                let special_anchor = branch_label_anchor.get(&ep_idx).copied();
                let (mx, mut my) = if let Some(p) = special_anchor {
                    (p.x, p.y)
                } else {
                    pick_label_anchor(&ep.path, prefer_early, prefer_late)
                };
                if lbl_l == "elseif" {
                    my += max(12, MIN_TOP_ENTRY_LEN / 3);
                }
                let fs = 22.0f32;
                let (lw, lh) = text_size_px(&font, fs, &lbl);
                let mut bx1 = mx - lw / 2 - 2;
                let mut by1 = my - 2;
                let mut bx2 = mx + lw / 2 + 2;
                let mut by2 = my + lh + 2;

                if bx1 < 8 {
                    let dx = 8 - bx1;
                    bx1 += dx;
                    bx2 += dx;
                }
                if bx2 > w - 8 {
                    let dx = bx2 - (w - 8);
                    bx1 -= dx;
                    bx2 -= dx;
                }
                if by1 < 8 {
                    let dy = 8 - by1;
                    by1 += dy;
                    by2 += dy;
                }
                if by2 > h - 8 {
                    let dy = by2 - (h - 8);
                    by1 -= dy;
                    by2 -= dy;
                }

                let overlaps_any = |ax1: i32, ay1: i32, ax2: i32, ay2: i32| -> bool {
                    for (ux1, uy1, ux2, uy2) in &used_label_boxes {
                        if ax1 < *ux2 && ax2 > *ux1 && ay1 < *uy2 && ay2 > *uy1 {
                            return true;
                        }
                    }
                    for (rx1, ry1, rx2, ry2) in rect.values() {
                        if ax1 < *rx2 && ax2 > *rx1 && ay1 < *ry2 && ay2 > *ry1 {
                            return true;
                        }
                    }
                    false
                };

                if let Some(sa) = special_anchor {
                    let mut placed = false;
                    let deltas = [0, -12, 12, -24, 24, -36, 36, -48, 48];
                    for d in deltas {
                        let (cx, cy) = if sa.axis == 'h' {
                            (sa.x, sa.y + d)
                        } else {
                            (sa.x + d, sa.y)
                        };
                        let mut tx1 = cx - lw / 2 - 2;
                        let mut ty1 = cy - 2;
                        let mut tx2 = cx + lw / 2 + 2;
                        let mut ty2 = cy + lh + 2;
                        if tx1 < 8 {
                            let dd = 8 - tx1;
                            tx1 += dd;
                            tx2 += dd;
                        }
                        if tx2 > w - 8 {
                            let dd = tx2 - (w - 8);
                            tx1 -= dd;
                            tx2 -= dd;
                        }
                        if ty1 < 8 {
                            let dd = 8 - ty1;
                            ty1 += dd;
                            ty2 += dd;
                        }
                        if ty2 > h - 8 {
                            let dd = ty2 - (h - 8);
                            ty1 -= dd;
                            ty2 -= dd;
                        }
                        if !overlaps_any(tx1, ty1, tx2, ty2) {
                            bx1 = tx1;
                            by1 = ty1;
                            bx2 = tx2;
                            by2 = ty2;
                            placed = true;
                            break;
                        }
                    }
                    if !placed {
                        // Keep close to anchor even if we must overlap a bit.
                        bx1 = sa.x - lw / 2 - 2;
                        by1 = sa.y - 2;
                        bx2 = sa.x + lw / 2 + 2;
                        by2 = sa.y + lh + 2;
                    }
                } else {
                    for _ in 0..20 {
                        if !overlaps_any(bx1, by1, bx2, by2) {
                            break;
                        }
                        by1 += 14;
                        by2 += 14;
                        if by2 > h - 8 {
                            by2 = h - 8;
                            by1 = by2 - (lh + 8);
                        }
                    }
                }

                // imageproc/ab_glyph rasterization sits slightly up-right vs Pillow for this font.
                let is_branch_label = matches!(
                    lbl_l.as_str(),
                    "yes"
                        | "no"
                        | "elseif"
                        | "else"
                        | "case"
                        | "default"
                        | "loop back"
                        | "loop done"
                );
                let tx = if is_branch_label { bx1 - 2 } else { bx1 };
                let ty = if is_branch_label { by1 + 3 } else { by1 + 2 };
                for ox in -3..=3 {
                    for oy in -3..=3 {
                        if ox == 0 && oy == 0 {
                            continue;
                        }
                        if ox * ox + oy * oy > 10 {
                            continue;
                        }
                        draw_text_mut(
                            &mut img,
                            rgb(STYLE_BG),
                            tx + ox,
                            ty + oy,
                            PxScale::from(fs),
                            &font,
                            &lbl,
                        );
                        if let Some(tl) = text_layer.as_mut() {
                            draw_text_mut(
                                tl,
                                rgba(STYLE_BG, 255),
                                tx + ox,
                                ty + oy,
                                PxScale::from(fs),
                                &font,
                                &lbl,
                            );
                        }
                    }
                }
                draw_text_mut(
                    &mut img,
                    rgb(STYLE_TEXT),
                    tx,
                    ty,
                    PxScale::from(fs),
                    &font,
                    &lbl,
                );
                if let Some(tl) = text_layer.as_mut() {
                    draw_text_mut(
                        tl,
                        rgba(STYLE_TEXT, 255),
                        tx,
                        ty,
                        PxScale::from(fs),
                        &font,
                        &lbl,
                    );
                }
                used_label_boxes.push((bx1, by1, bx2, by2));
                content_bbox.include_rect(bx1 - 4, by1 - 4, bx2 + 4, by2 + 4);
            }
        }
    }

    // Draw nodes.
    for n in &graph.nodes {
        let (x1, y1, x2, y2) = rect[&n.id];
        let (cx, cy) = pos[&n.id];
        content_bbox.include_rect(x1 - 4, y1 - 4, x2 + 4, y2 + 4);
        if n.shape == "decision" {
            let pts = [
                Point::new(cx, y1),
                Point::new(x2, cy),
                Point::new(cx, y2),
                Point::new(x1, cy),
            ];
            draw_polygon_mut(&mut img, &pts, rgb(fill_for(&n.shape)));
            if let Some(bl) = blocks_layer.as_mut() {
                draw_polygon_mut(bl, &pts, rgb(fill_for(&n.shape)));
            }
            for i in 0..4 {
                let a = pts[i];
                let b = pts[(i + 1) % 4];
                draw_thick_line_mut(
                    &mut img,
                    (a.x, a.y),
                    (b.x, b.y),
                    rgb(STYLE_EDGE),
                    STROKE_WIDTH,
                );
                if let Some(bl) = blocks_layer.as_mut() {
                    draw_thick_line_mut(bl, (a.x, a.y), (b.x, b.y), rgb(STYLE_EDGE), STROKE_WIDTH);
                }
            }
        } else {
            let rad = if n.shape == "terminator" { 20 } else { 14 };
            draw_rounded_rect_mut(
                &mut img,
                x1,
                y1,
                x2,
                y2,
                rad,
                rgb(fill_for(&n.shape)),
                rgb(STYLE_EDGE),
            );
            if let Some(bl) = blocks_layer.as_mut() {
                draw_rounded_rect_mut(
                    bl,
                    x1,
                    y1,
                    x2,
                    y2,
                    rad,
                    rgb(fill_for(&n.shape)),
                    rgb(STYLE_EDGE),
                );
            }
        }

        let max_w = if n.shape == "decision" {
            ((x2 - x1) as f32 * 0.56) as i32
        } else {
            (x2 - x1) - 100
        };
        let box_h = if n.shape == "decision" {
            (y2 - y1) - 60
        } else {
            (y2 - y1) - 46
        };
        let (lines, line_h) =
            fit_text_lines(&font, &n.text, max_w.max(80), box_h.max(24), cli.font_size);
        let mut y = max(y1 + 10, cy - (lines.len() as i32 * line_h) / 2);
        for line in lines.into_iter().take(6) {
            let (line_w, _line_h_exact) = text_size_px(&font, cli.font_size, &line);
            let x = cx - line_w / 2;
            if y + line_h > y2 - 8 {
                break;
            }
            draw_text_mut(
                &mut img,
                rgb(STYLE_TEXT),
                x,
                y,
                PxScale::from(cli.font_size),
                &font,
                &line,
            );
            if let Some(tl) = text_layer.as_mut() {
                draw_text_mut(
                    tl,
                    rgba(STYLE_TEXT, 255),
                    x,
                    y,
                    PxScale::from(cli.font_size),
                    &font,
                    &line,
                );
            }
            y += line_h;
        }
    }

    // Draw arrowheads last.
    for (a, b) in arrow_segments {
        draw_arrow_head(&mut img, a, b, rgb(STYLE_EDGE));
        if let Some(al) = arrows_layer.as_mut() {
            draw_arrow_head(al, a, b, rgb(STYLE_EDGE));
        }
        content_bbox.include_rect(b.0 - 24, b.1 - 24, b.0 + 24, b.1 + 24);
    }

    if content_bbox.is_valid() {
        let pad = 72;
        let x1 = max(0, content_bbox.min_x - pad);
        let y1 = max(0, content_bbox.min_y - pad);
        let x2 = min(w - 1, content_bbox.max_x + pad);
        let y2 = min(h - 1, content_bbox.max_y + pad);
        if x2 > x1 && y2 > y1 {
            let cw = (x2 - x1 + 1) as u32;
            let ch = (y2 - y1 + 1) as u32;
            img = imageops::crop_imm(&img, x1 as u32, y1 as u32, cw, ch).to_image();
            if let Some(bl) = blocks_layer.take() {
                blocks_layer =
                    Some(imageops::crop_imm(&bl, x1 as u32, y1 as u32, cw, ch).to_image());
            }
            if let Some(al) = arrows_layer.take() {
                arrows_layer =
                    Some(imageops::crop_imm(&al, x1 as u32, y1 as u32, cw, ch).to_image());
            }
            if let Some(tl) = text_layer.take() {
                text_layer = Some(imageops::crop_imm(&tl, x1 as u32, y1 as u32, cw, ch).to_image());
            }
        }
    }

    // Re-center title on final image width after crop.
    let (final_w, _final_h) = img.dimensions();
    let clear_h = title_y + th + 10;
    let y_end = min(clear_h, img.height() as i32);
    for yy in 0..y_end {
        for xx in 0..img.width() as i32 {
            img.put_pixel(xx as u32, yy as u32, rgb(STYLE_BG));
        }
    }
    let final_title_x = final_w as i32 / 2 - tw / 2;
    draw_text_mut(
        &mut img,
        rgb(STYLE_TEXT),
        final_title_x,
        title_y,
        PxScale::from(38.0),
        &font,
        &title,
    );
    if let Some(tl) = text_layer.as_mut() {
        let y_end_t = min(clear_h, tl.height() as i32);
        for yy in 0..y_end_t {
            for xx in 0..tl.width() as i32 {
                tl.put_pixel(xx as u32, yy as u32, rgba(STYLE_BG, 0));
            }
        }
        draw_text_mut(
            tl,
            rgba(STYLE_TEXT, 255),
            final_title_x,
            title_y,
            PxScale::from(38.0),
            &font,
            &title,
        );
    }

    save_rgb_with_optional_transparency(&img, &cli.output, STYLE_BG, cli.transparent_bg)?;
    if cli.separate_layers {
        if let Some(bl) = blocks_layer.as_ref() {
            let p = output_with_suffix(&cli.output, "blocks");
            save_rgb_with_optional_transparency(bl, &p, STYLE_BG, true)?;
        }
        if let Some(al) = arrows_layer.as_ref() {
            let p = output_with_suffix(&cli.output, "arrows");
            save_rgb_with_optional_transparency(al, &p, STYLE_BG, true)?;
        }
        if let Some(tl) = text_layer.as_ref() {
            let p = output_with_suffix(&cli.output, "text");
            tl.save(&p)
                .with_context(|| format!("failed to save {}", p.display()))?;
        }
    }

    let mut report = validate_layout(&rect, &edge_paths);
    if let Some(obj) = report.as_object_mut() {
        obj.insert("min_block_gap_enforced".to_string(), json!(min_block_gap));
        obj.insert(
            "vertical_shift_passes".to_string(),
            json!(vertical_shift_passes),
        );
        obj.insert(
            "vertical_shift_applied_px".to_string(),
            json!(vertical_shift_applied_px),
        );
        obj.insert(
            "vertical_shift_nodes".to_string(),
            json!(vertical_shift_nodes),
        );
        obj.insert(
            "vertical_shift_total_px".to_string(),
            json!(vertical_shift_total_px),
        );
        obj.insert(
            "vertical_shift_max_px".to_string(),
            json!(vertical_shift_max_px),
        );
    }
    if cli.strict_report {
        println!("STRICT_REPORT {}", report);
        if report
            .get("issue_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            > 0
        {
            let lines = report
                .get("issue_samples")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join("\n- ")
                })
                .unwrap_or_else(|| "Unknown validation issue".to_string());
            bail!("Layout validation failed:\n- {}", lines);
        }
    }
    println!("Wrote {}", cli.output.display());
    if let Ok(meta) = fs::metadata(&cli.output) {
        println!(
            "Output stats: {}x{} px, {} ({})",
            img.width(),
            img.height(),
            meta.len(),
            human_bytes(meta.len())
        );
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    run_native(&cli)
}
