//! Assigning entities to groups: from source directives, by name prefix, or by graph clustering.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::model::Diagram;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupingMode {
    /// `%% group` directives in the source.
    File,
    /// `billing_invoice`, `BillingInvoice` → `billing`.
    Prefix,
    /// Communities in the relationship graph (Louvain).
    Clusters,
    None,
}

impl GroupingMode {
    pub const ALL: [Self; 4] = [Self::File, Self::Prefix, Self::Clusters, Self::None];

    pub fn label(self) -> &'static str {
        match self {
            Self::File => "From file (%% group …)",
            Self::Prefix => "By name prefix",
            Self::Clusters => "By relationship clusters",
            Self::None => "No groups",
        }
    }

    /// File directives when present, otherwise automatic clustering.
    pub fn default_for(d: &Diagram) -> Self {
        if !d.groups.is_empty() {
            Self::File
        } else if d.entities.len() > 12 {
            Self::Clusters
        } else {
            Self::None
        }
    }
}

#[derive(Clone, Debug)]
pub struct Group {
    pub name: String,
    pub members: Vec<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct Grouping {
    pub groups: Vec<Group>,
    pub group_of: Vec<Option<usize>>,
}

pub fn compute(d: &Diagram, mode: GroupingMode) -> Grouping {
    let raw: Vec<(String, Vec<usize>)> = match mode {
        GroupingMode::File => {
            let mut taken = vec![false; d.entities.len()];
            d.groups
                .iter()
                .map(|g| {
                    let members = g
                        .members
                        .iter()
                        .filter_map(|m| d.entity_index(m))
                        .filter(|&e| !std::mem::replace(&mut taken[e], true))
                        .collect();
                    (g.name.clone(), members)
                })
                .collect()
        }
        GroupingMode::Prefix => by_prefix(d),
        GroupingMode::Clusters => by_clusters(d),
        GroupingMode::None => Vec::new(),
    };

    let mut grouping = Grouping {
        groups: Vec::new(),
        group_of: vec![None; d.entities.len()],
    };
    for (name, members) in raw {
        if members.is_empty() {
            continue;
        }
        let gi = grouping.groups.len();
        for &m in &members {
            grouping.group_of[m] = Some(gi);
        }
        grouping.groups.push(Group { name, members });
    }
    grouping
}

fn prefix_of(id: &str) -> Option<&str> {
    if let Some(i) = id.find(['_', '-', '.', ' ']) {
        return (i > 0).then(|| &id[..i]);
    }
    // CamelCase: cut before the second uppercase letter that follows a lowercase one.
    let mut prev_lower = false;
    for (i, c) in id.char_indices() {
        if c.is_uppercase() && prev_lower {
            return Some(&id[..i]);
        }
        prev_lower = c.is_lowercase();
    }
    None
}

fn by_prefix(d: &Diagram) -> Vec<(String, Vec<usize>)> {
    let mut map: BTreeMap<String, (String, Vec<usize>)> = BTreeMap::new();
    for (i, e) in d.entities.iter().enumerate() {
        if let Some(p) = prefix_of(&e.id) {
            map.entry(p.to_lowercase())
                .or_insert_with(|| (p.to_owned(), Vec::new()))
                .1
                .push(i);
        }
    }
    map.into_values().filter(|(_, m)| m.len() >= 2).collect()
}

fn by_clusters(d: &Diagram) -> Vec<(String, Vec<usize>)> {
    let n = d.entities.len();
    let edges: Vec<(usize, usize)> = d.relationships.iter().map(|r| (r.a, r.b)).collect();
    let comm = louvain(n, &edges);

    let mut degree = vec![0usize; n];
    for &(a, b) in &edges {
        degree[a] += 1;
        degree[b] += 1;
    }
    let mut clusters: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, &c) in comm.iter().enumerate() {
        clusters.entry(c).or_default().push(i);
    }
    let mut out: Vec<(String, Vec<usize>)> = clusters
        .into_values()
        .filter(|m| m.len() >= 2)
        .map(|m| {
            let hub = *m
                .iter()
                .max_by_key(|&&e| (degree[e], std::cmp::Reverse(e)))
                .unwrap();
            (format!("{} cluster", d.entities[hub].display_name()), m)
        })
        .collect();
    out.sort_by_key(|(_, m)| m[0]);
    out
}

/// Louvain community detection on an unweighted undirected graph. Deterministic.
pub fn louvain(n: usize, edges: &[(usize, usize)]) -> Vec<usize> {
    let mut adj: Vec<BTreeMap<usize, f64>> = vec![BTreeMap::new(); n];
    for &(a, b) in edges {
        if a == b {
            continue;
        }
        *adj[a].entry(b).or_default() += 1.0;
        *adj[b].entry(a).or_default() += 1.0;
    }
    let mut node_comm: Vec<usize> = (0..n).collect();
    while let Some(comm) = one_level(&adj) {
        for c in node_comm.iter_mut() {
            *c = comm[*c];
        }
        let k = comm.iter().max().map_or(0, |m| m + 1);
        let mut next: Vec<BTreeMap<usize, f64>> = vec![BTreeMap::new(); k];
        for (i, row) in adj.iter().enumerate() {
            for (&j, &w) in row {
                *next[comm[i]].entry(comm[j]).or_default() += w;
            }
        }
        adj = next;
    }
    node_comm
}

/// One local-moving pass. Returns compacted community ids, or `None` if nothing moved.
fn one_level(adj: &[BTreeMap<usize, f64>]) -> Option<Vec<usize>> {
    let n = adj.len();
    let k: Vec<f64> = adj.iter().map(|r| r.values().sum()).collect();
    let m2: f64 = k.iter().sum();
    if m2 == 0.0 {
        return None;
    }
    let mut comm: Vec<usize> = (0..n).collect();
    let mut tot = k.clone();
    let mut improved = false;
    for _ in 0..100 {
        let mut moved = false;
        for i in 0..n {
            let ci = comm[i];
            let mut neigh: BTreeMap<usize, f64> = BTreeMap::new();
            for (&j, &w) in &adj[i] {
                if j != i {
                    *neigh.entry(comm[j]).or_default() += w;
                }
            }
            tot[ci] -= k[i];
            let gain = |c: usize, w: f64| w - tot[c] * k[i] / m2;
            let mut best = ci;
            let mut best_gain = gain(ci, neigh.get(&ci).copied().unwrap_or(0.0));
            for (&c, &w) in &neigh {
                let g = gain(c, w);
                if g > best_gain + 1e-12 {
                    best = c;
                    best_gain = g;
                }
            }
            tot[best] += k[i];
            if best != ci {
                comm[i] = best;
                moved = true;
                improved = true;
            }
        }
        if !moved {
            break;
        }
    }
    if !improved {
        return None;
    }
    let mut remap = HashMap::new();
    Some(
        comm.iter()
            .map(|c| {
                let next = remap.len();
                *remap.entry(*c).or_insert(next)
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mermaid::parse;

    #[test]
    fn prefixes() {
        assert_eq!(prefix_of("billing_invoice"), Some("billing"));
        assert_eq!(prefix_of("LINE-ITEM"), Some("LINE"));
        assert_eq!(prefix_of("OrderItem"), Some("Order"));
        assert_eq!(prefix_of("ORDER"), None);
        assert_eq!(prefix_of("order"), None);
    }

    #[test]
    fn louvain_finds_two_cliques() {
        // Two triangles joined by one edge.
        let edges = [(0, 1), (1, 2), (2, 0), (3, 4), (4, 5), (5, 3), (2, 3)];
        let c = louvain(6, &edges);
        assert_eq!(c[0], c[1]);
        assert_eq!(c[1], c[2]);
        assert_eq!(c[3], c[4]);
        assert_eq!(c[4], c[5]);
        assert_ne!(c[0], c[3]);
    }

    #[test]
    fn file_grouping_first_group_wins() {
        let d = parse(
            "erDiagram\n%% group A: X, Y\n%% group B: Y, Z\nX ||--|| Y : r\nZ ||--|| Y : s\n",
        );
        let g = compute(&d, GroupingMode::File);
        assert_eq!(g.groups[0].members, [0, 1]);
        assert_eq!(g.groups[1].members, [2]);
    }
}
