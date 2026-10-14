//! Size-aware force layout and overlap removal for boxes.

use egui::{Pos2, Vec2, pos2, vec2};

/// Fruchterman–Reingold variant that treats nodes as boxes of the given sizes.
/// Returns box centers, roughly centered on the origin. Deterministic.
pub fn force_layout(sizes: &[Vec2], edges: &[(usize, usize)], gap: f32) -> Vec<Pos2> {
    let n = sizes.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![Pos2::ZERO];
    }
    let radius: Vec<f32> = sizes.iter().map(|s| 0.5 * s.length()).collect();
    let avg = radius.iter().sum::<f32>() / n as f32;
    let k = avg * 0.8 + 40.0;

    let mut adj = vec![Vec::new(); n];
    for &(a, b) in edges {
        if a != b {
            adj[a].push(b);
            adj[b].push(a);
        }
    }

    // Seed positions along a golden-angle spiral in BFS order, so neighbours start close together.
    let mut order = Vec::with_capacity(n);
    let mut seen = vec![false; n];
    let mut by_degree: Vec<usize> = (0..n).collect();
    by_degree.sort_by_key(|&i| std::cmp::Reverse(adj[i].len()));
    for &start in &by_degree {
        if seen[start] {
            continue;
        }
        seen[start] = true;
        let mut queue = std::collections::VecDeque::from([start]);
        while let Some(v) = queue.pop_front() {
            order.push(v);
            for &w in &adj[v] {
                if !std::mem::replace(&mut seen[w], true) {
                    queue.push_back(w);
                }
            }
        }
    }
    let mut pos = vec![Pos2::ZERO; n];
    let golden = std::f32::consts::PI * (3.0 - 5f32.sqrt());
    for (rank, &v) in order.iter().enumerate() {
        let r = (avg + k) * 0.6 * (rank as f32).sqrt();
        let a = rank as f32 * golden;
        pos[v] = pos2(r * a.cos(), r * a.sin());
    }

    let iterations = 400;
    let mut temp = (avg + k) * (n as f32).sqrt() * 0.5;
    let cooling = temp / iterations as f32;
    let mut disp = vec![Vec2::ZERO; n];
    for _ in 0..iterations {
        disp.fill(Vec2::ZERO);
        for i in 0..n {
            for j in i + 1..n {
                let mut d = pos[i] - pos[j];
                if d.length_sq() < 1e-6 {
                    d = vec2(((i * 7 + j) % 5) as f32 - 2.0, 1.0);
                }
                let len = d.length();
                let gap = (len - radius[i] - radius[j]).max(k * 0.1);
                let f = d / len * (k * k / gap);
                disp[i] += f;
                disp[j] -= f;
            }
        }
        for &(a, b) in edges {
            if a == b {
                continue;
            }
            let d = pos[a] - pos[b];
            let len = d.length().max(1e-3);
            let gap = (len - radius[a] - radius[b]).max(0.0);
            let f = d / len * (gap * gap / k);
            disp[a] -= f;
            disp[b] += f;
        }
        for i in 0..n {
            // Gentle gravity keeps disconnected components together.
            disp[i] -= pos[i].to_vec2() * 0.08;
            let len = disp[i].length();
            if len > 0.0 {
                pos[i] += disp[i] / len * len.min(temp);
            }
        }
        temp = (temp - cooling).max(1.0);
    }

    let pinned = vec![false; n];
    remove_overlaps(&mut pos, sizes, &pinned, gap);
    compact(&mut pos, sizes, gap);
    let c = centroid(&pos);
    pos.iter().map(|p| *p - c.to_vec2()).collect()
}

/// Pushes overlapping boxes apart along their axis of least penetration. Pinned boxes never move.
pub fn remove_overlaps(centers: &mut [Pos2], sizes: &[Vec2], pinned: &[bool], gap: f32) {
    let n = centers.len();
    for _ in 0..1000 {
        let mut any = false;
        for i in 0..n {
            for j in i + 1..n {
                let d = centers[j] - centers[i];
                let ox = (sizes[i].x + sizes[j].x) * 0.5 + gap - d.x.abs();
                let oy = (sizes[i].y + sizes[j].y) * 0.5 + gap - d.y.abs();
                if ox <= 0.0 || oy <= 0.0 {
                    continue;
                }
                let (mi, mj) = match (pinned[i], pinned[j]) {
                    (true, true) => continue,
                    (true, false) => (0.0, 1.0),
                    (false, true) => (1.0, 0.0),
                    (false, false) => (0.5, 0.5),
                };
                any = true;
                let sign = |v: f32| if v < 0.0 { -1.0 } else { 1.0 };
                if ox < oy {
                    let s = sign(d.x) * (ox + 0.5);
                    centers[i].x -= s * mi;
                    centers[j].x += s * mj;
                } else {
                    let s = sign(d.y) * (oy + 0.5);
                    centers[i].y -= s * mi;
                    centers[j].y += s * mj;
                }
            }
        }
        if !any {
            break;
        }
    }
}

/// Shrinks the layout towards its centroid while that still reduces its area once overlaps are
/// resolved again. Keeps the overall shape but removes the slack force layouts tend to leave.
pub fn compact(pos: &mut [Pos2], sizes: &[Vec2], gap: f32) {
    let free = vec![false; pos.len()];
    let area = |p: &[Pos2]| {
        let r = p.iter().zip(sizes).fold(egui::Rect::NOTHING, |r, (p, s)| {
            r.union(egui::Rect::from_center_size(*p, *s))
        });
        r.width() * r.height()
    };
    let mut best = area(pos);
    for _ in 0..40 {
        let c = centroid(pos);
        let mut trial: Vec<Pos2> = pos.iter().map(|p| c + (*p - c) * 0.88).collect();
        remove_overlaps(&mut trial, sizes, &free, gap);
        let a = area(&trial);
        if a > best * 0.99 {
            break;
        }
        best = a;
        pos.copy_from_slice(&trial);
    }
}

/// Places nodes whose position is `None` next to their placed neighbours (or beside the existing
/// layout), keeping known positions fixed. Falls back to a full layout if nothing is known.
pub fn place_new(
    known: &[Option<Pos2>],
    sizes: &[Vec2],
    edges: &[(usize, usize)],
    gap: f32,
) -> Vec<Pos2> {
    if known.iter().all(Option::is_none) {
        return force_layout(sizes, edges, gap);
    }
    let n = known.len();
    let placed: Vec<Pos2> = known.iter().flatten().copied().collect();
    let right = placed
        .iter()
        .zip(
            known
                .iter()
                .zip(sizes)
                .filter(|(k, _)| k.is_some())
                .map(|(_, s)| s),
        )
        .map(|(p, s)| p.x + s.x * 0.5)
        .fold(f32::MIN, f32::max);
    let mid_y = centroid(&placed).y;

    let mut pos: Vec<Pos2> = known.iter().map(|p| p.unwrap_or(Pos2::ZERO)).collect();
    let mut fallback_y = mid_y;
    for i in 0..n {
        if known[i].is_some() {
            continue;
        }
        let neighbours: Vec<Pos2> = edges
            .iter()
            .filter_map(|&(a, b)| match (a == i, b == i) {
                (true, false) => known[b],
                (false, true) => known[a],
                _ => None,
            })
            .collect();
        pos[i] = if neighbours.is_empty() {
            let p = pos2(right + 60.0 + sizes[i].x * 0.5, fallback_y);
            fallback_y += sizes[i].y + 30.0;
            p
        } else {
            centroid(&neighbours) + vec2(0.0, 1.0)
        };
    }
    let pinned: Vec<bool> = known.iter().map(Option::is_some).collect();
    remove_overlaps(&mut pos, sizes, &pinned, gap);
    pos
}

pub fn centroid(points: &[Pos2]) -> Pos2 {
    if points.is_empty() {
        return Pos2::ZERO;
    }
    let sum = points.iter().fold(Vec2::ZERO, |acc, p| acc + p.to_vec2());
    (sum / points.len() as f32).to_pos2()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlaps(pos: &[Pos2], sizes: &[Vec2]) -> bool {
        for i in 0..pos.len() {
            for j in i + 1..pos.len() {
                let d = pos[j] - pos[i];
                if d.x.abs() < (sizes[i].x + sizes[j].x) * 0.5
                    && d.y.abs() < (sizes[i].y + sizes[j].y) * 0.5
                {
                    return true;
                }
            }
        }
        false
    }

    #[test]
    fn force_layout_has_no_overlaps() {
        let sizes: Vec<Vec2> = (0..40)
            .map(|i| vec2(80.0 + (i % 5) as f32 * 30.0, 26.0 + (i % 3) as f32 * 60.0))
            .collect();
        let edges: Vec<(usize, usize)> = (1..40).map(|i| (i, i / 3)).collect();
        let pos = force_layout(&sizes, &edges, 30.0);
        assert!(!overlaps(&pos, &sizes));
        // Connected nodes end up closer than the layout's diameter.
        let span = pos.iter().map(|p| p.to_vec2().length()).fold(0.0, f32::max);
        assert!((pos[1] - pos[0]).length() < span);
    }

    #[test]
    fn remove_overlaps_respects_pins() {
        let mut pos = vec![Pos2::ZERO, pos2(10.0, 0.0), pos2(20.0, 5.0)];
        let sizes = vec![vec2(300.0, 300.0), vec2(50.0, 20.0), vec2(50.0, 20.0)];
        remove_overlaps(&mut pos, &sizes, &[true, false, false], 10.0);
        assert_eq!(pos[0], Pos2::ZERO);
        assert!(!overlaps(&pos, &sizes));
    }

    #[test]
    fn place_new_keeps_known() {
        let known = vec![Some(Pos2::ZERO), None, Some(pos2(500.0, 0.0))];
        let sizes = vec![vec2(100.0, 30.0); 3];
        let pos = place_new(&known, &sizes, &[(1, 2)], 30.0);
        assert_eq!(pos[0], Pos2::ZERO);
        assert_eq!(pos[2], pos2(500.0, 0.0));
        assert!((pos[1] - pos[2]).length() < 200.0);
    }
}
