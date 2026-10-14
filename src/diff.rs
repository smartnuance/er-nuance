//! Semantic difference between two versions of a diagram.
//!
//! The result is one *merged* [`Diagram`] holding everything from both versions, plus
//! [`DiffMarks`] saying for each entity, attribute and relationship whether it was added,
//! removed or changed. Indices in the marks match the merged diagram, so the scene can lay it
//! out and paint it like any other diagram.

use std::collections::{HashMap, VecDeque};

use crate::model::{Attribute, Diagram, GroupDef, Relationship};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Same,
    Added,
    Removed,
    Changed,
}

impl Status {
    pub fn differs(self) -> bool {
        self != Self::Same
    }

    /// `+`, `−`, `~`, or nothing.
    pub fn sign(self) -> &'static str {
        match self {
            Self::Same => "",
            Self::Added => "+",
            Self::Removed => "−",
            Self::Changed => "~",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct AttrDiff {
    pub status: Status,
    /// The old attribute, when `Changed`.
    pub old: Option<Attribute>,
}

#[derive(Clone, Debug, Default)]
pub struct EntityDiff {
    pub status: Status,
    /// One per attribute of the merged entity.
    pub attrs: Vec<AttrDiff>,
    /// What changed, one line each, for tooltips and the details panel.
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct RelDiff {
    pub status: Status,
    /// What changed, when `Changed`.
    pub note: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
}

impl Counts {
    fn of(statuses: impl Iterator<Item = Status>) -> Self {
        let mut c = Self::default();
        for s in statuses {
            match s {
                Status::Added => c.added += 1,
                Status::Removed => c.removed += 1,
                Status::Changed => c.changed += 1,
                Status::Same => {}
            }
        }
        c
    }

    pub fn is_empty(self) -> bool {
        self == Self::default()
    }

    /// `+2 −1 ~3`, leaving out zeros.
    pub fn badge(self) -> String {
        [("+", self.added), ("−", self.removed), ("~", self.changed)]
            .iter()
            .filter(|(_, n)| *n > 0)
            .map(|(s, n)| format!("{s}{n}"))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Clone, Debug, Default)]
pub struct DiffMarks {
    /// One per entity of the merged diagram.
    pub entities: Vec<EntityDiff>,
    /// One per relationship of the merged diagram.
    pub relationships: Vec<RelDiff>,
}

impl DiffMarks {
    pub fn entity_counts(&self) -> Counts {
        Counts::of(self.entities.iter().map(|e| e.status))
    }

    pub fn relationship_counts(&self) -> Counts {
        Counts::of(self.relationships.iter().map(|r| r.status))
    }

    pub fn counts_of(&self, entities: &[usize]) -> Counts {
        Counts::of(entities.iter().map(|&e| self.entities[e].status))
    }

    pub fn is_identical(&self) -> bool {
        self.entity_counts().is_empty() && self.relationship_counts().is_empty()
    }
}

pub struct DiffResult {
    pub merged: Diagram,
    pub marks: DiffMarks,
}

/// Text of an attribute's type column: `old → new` when the type changed.
pub fn type_text(a: &Attribute, d: Option<&AttrDiff>) -> String {
    match d.and_then(|d| d.old.as_ref()) {
        Some(old) if old.ty != a.ty => format!("{} → {}", old.ty, a.ty),
        _ => a.ty.clone(),
    }
}

/// Text of an attribute's keys column: `old → new` when the keys changed.
pub fn keys_text(a: &Attribute, d: Option<&AttrDiff>) -> String {
    let keys = |a: &Attribute| {
        if a.keys.is_empty() {
            "none".to_owned()
        } else {
            a.keys.join(",")
        }
    };
    match d.and_then(|d| d.old.as_ref()) {
        Some(old) if old.keys != a.keys => format!("{} → {}", keys(old), keys(a)),
        _ => a.keys.join(","),
    }
}

pub fn diff(old: &Diagram, new: &Diagram) -> DiffResult {
    let mut merged = Diagram::default();
    let mut entities = Vec::new();
    let old_groups = group_names(old);
    let new_groups = group_names(new);

    for ne in &new.entities {
        let i = merged.ensure_entity(&ne.id);
        let ed = match old.entity_index(&ne.id).map(|o| &old.entities[o]) {
            None => {
                merged.entities[i] = ne.clone();
                EntityDiff {
                    status: Status::Added,
                    attrs: vec![
                        AttrDiff {
                            status: Status::Added,
                            old: None
                        };
                        ne.attributes.len()
                    ],
                    notes: Vec::new(),
                }
            }
            Some(oe) => {
                let (attributes, attrs) = merge_attributes(&oe.attributes, &ne.attributes);
                merged.entities[i] = crate::model::Entity {
                    attributes,
                    ..ne.clone()
                };
                let mut notes = Vec::new();
                if oe.label != ne.label {
                    notes.push(format!(
                        "name: {} → {}",
                        oe.display_name(),
                        ne.display_name()
                    ));
                }
                if oe.description != ne.description {
                    notes.push(match (&oe.description, &ne.description) {
                        (None, Some(_)) => "description added".to_owned(),
                        (Some(_), None) => "description removed".to_owned(),
                        _ => "description changed".to_owned(),
                    });
                }
                let added: Vec<String> = ne
                    .tags
                    .iter()
                    .filter(|t| !oe.tags.contains(t))
                    .map(|t| format!("+#{t}"))
                    .collect();
                let removed: Vec<String> = oe
                    .tags
                    .iter()
                    .filter(|t| !ne.tags.contains(t))
                    .map(|t| format!("−#{t}"))
                    .collect();
                if !added.is_empty() || !removed.is_empty() {
                    notes.push(format!("tags: {}", [added, removed].concat().join(" ")));
                }
                let (og, ng) = (
                    old_groups.get(ne.id.as_str()),
                    new_groups.get(ne.id.as_str()),
                );
                if og != ng {
                    let name = |g: Option<&&str>| g.map_or("none", |g| g).to_owned();
                    notes.push(format!("group: {} → {}", name(og), name(ng)));
                }
                for (a, d) in merged.entities[i].attributes.iter().zip(&attrs) {
                    match d.status {
                        Status::Added => notes.push(format!("field added: {}", a.name)),
                        Status::Removed => notes.push(format!("field removed: {}", a.name)),
                        Status::Changed => notes.push(format!(
                            "field changed: {}{}",
                            a.name,
                            attr_change(d.old.as_ref().unwrap(), a)
                        )),
                        Status::Same => {}
                    }
                }
                let status = if notes.is_empty() {
                    Status::Same
                } else {
                    Status::Changed
                };
                EntityDiff {
                    status,
                    attrs,
                    notes,
                }
            }
        };
        entities.push(ed);
    }
    for oe in old
        .entities
        .iter()
        .filter(|oe| new.entity_index(&oe.id).is_none())
    {
        let i = merged.ensure_entity(&oe.id);
        merged.entities[i] = oe.clone();
        entities.push(EntityDiff {
            status: Status::Removed,
            attrs: vec![
                AttrDiff {
                    status: Status::Removed,
                    old: None
                };
                oe.attributes.len()
            ],
            notes: Vec::new(),
        });
    }

    // Relationships match on unordered endpoint ids and label; duplicates pair up in order.
    let key = |d: &Diagram, r: &Relationship| {
        let (a, b) = (d.entities[r.a].id.clone(), d.entities[r.b].id.clone());
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        (a, b, r.label.clone())
    };
    let mut unmatched: HashMap<(String, String, String), VecDeque<usize>> = HashMap::new();
    for (i, r) in old.relationships.iter().enumerate() {
        unmatched.entry(key(old, r)).or_default().push_back(i);
    }
    let mut relationships = Vec::new();
    for nr in &new.relationships {
        let remapped = remap(new, &merged, nr);
        let rd = match unmatched
            .get_mut(&key(new, nr))
            .and_then(VecDeque::pop_front)
        {
            None => RelDiff {
                status: Status::Added,
                note: None,
            },
            Some(o) => {
                let or = &old.relationships[o];
                // Orient the old one like the new one before comparing ends.
                let same_way = old.entities[or.a].id == new.entities[nr.a].id;
                let (oca, ocb) = if same_way {
                    (or.card_a, or.card_b)
                } else {
                    (or.card_b, or.card_a)
                };
                let mut changes = Vec::new();
                if (oca, ocb) != (nr.card_a, nr.card_b) {
                    changes.push(format!(
                        "{} : {} → {} : {}",
                        oca.short(),
                        ocb.short(),
                        nr.card_a.short(),
                        nr.card_b.short()
                    ));
                }
                if or.identifying != nr.identifying {
                    let kind = |i: bool| if i { "identifying" } else { "non-identifying" };
                    changes.push(format!(
                        "{} → {}",
                        kind(or.identifying),
                        kind(nr.identifying)
                    ));
                }
                if changes.is_empty() {
                    RelDiff::default()
                } else {
                    RelDiff {
                        status: Status::Changed,
                        note: Some(changes.join(", ")),
                    }
                }
            }
        };
        merged.relationships.push(remapped);
        relationships.push(rd);
    }
    let mut removed: Vec<usize> = unmatched.into_values().flatten().collect();
    removed.sort_unstable();
    for o in removed {
        merged
            .relationships
            .push(remap(old, &merged, &old.relationships[o]));
        relationships.push(RelDiff {
            status: Status::Removed,
            note: None,
        });
    }

    // Groups: the new version's, plus removed entities in their old groups.
    merged.groups = new.groups.clone();
    for oe in old
        .entities
        .iter()
        .filter(|oe| new.entity_index(&oe.id).is_none())
    {
        let Some(&g) = old_groups.get(oe.id.as_str()) else {
            continue;
        };
        match merged.groups.iter_mut().find(|d| d.name == g) {
            Some(d) => d.members.push(oe.id.clone()),
            None => merged.groups.push(GroupDef {
                name: g.to_owned(),
                members: vec![oe.id.clone()],
            }),
        }
    }

    merged.tags = new.tags.clone();
    merged
        .tags
        .extend(old.tags.iter().filter(|t| !new.tags.contains(t)).cloned());

    merged.warnings = old
        .warnings
        .iter()
        .map(|w| format!("old: {w}"))
        .chain(new.warnings.iter().map(|w| format!("new: {w}")))
        .collect();

    DiffResult {
        merged,
        marks: DiffMarks {
            entities,
            relationships,
        },
    }
}

/// The explicit group of each entity id (the first group listing it, as in grouping).
fn group_names(d: &Diagram) -> HashMap<&str, &str> {
    let mut m = HashMap::new();
    for g in &d.groups {
        for id in &g.members {
            m.entry(id.as_str()).or_insert(g.name.as_str());
        }
    }
    m
}

fn remap(from: &Diagram, to: &Diagram, r: &Relationship) -> Relationship {
    let idx = |e: usize| {
        to.entity_index(&from.entities[e].id)
            .expect("merged has every entity")
    };
    Relationship {
        a: idx(r.a),
        b: idx(r.b),
        ..r.clone()
    }
}

/// New attributes in order, each removed one inserted after its old predecessor.
fn merge_attributes(old: &[Attribute], new: &[Attribute]) -> (Vec<Attribute>, Vec<AttrDiff>) {
    let mut out: Vec<(Attribute, AttrDiff)> = new
        .iter()
        .map(|na| {
            let d = match old.iter().find(|oa| oa.name == na.name) {
                None => AttrDiff {
                    status: Status::Added,
                    old: None,
                },
                Some(oa) if oa == na => AttrDiff::default(),
                Some(oa) => AttrDiff {
                    status: Status::Changed,
                    old: Some(oa.clone()),
                },
            };
            (na.clone(), d)
        })
        .collect();
    let mut anchor = 0;
    for oa in old {
        let kept = new.iter().any(|na| na.name == oa.name);
        if kept {
            anchor = out
                .iter()
                .position(|(a, d)| a.name == oa.name && d.status != Status::Removed)
                .unwrap()
                + 1;
        } else {
            out.insert(
                anchor,
                (
                    oa.clone(),
                    AttrDiff {
                        status: Status::Removed,
                        old: None,
                    },
                ),
            );
            anchor += 1;
        }
    }
    out.into_iter().unzip()
}

fn attr_change(old: &Attribute, new: &Attribute) -> String {
    let mut parts = Vec::new();
    if old.ty != new.ty {
        parts.push(format!("type {} → {}", old.ty, new.ty));
    }
    if old.keys != new.keys {
        parts.push(format!(
            "keys {} → {}",
            keys_or_none(old),
            keys_or_none(new)
        ));
    }
    if old.comment != new.comment {
        parts.push("comment".to_owned());
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join(", "))
    }
}

fn keys_or_none(a: &Attribute) -> String {
    if a.keys.is_empty() {
        "none".to_owned()
    } else {
        a.keys.join(",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mermaid::parse;

    fn run(old: &str, new: &str) -> DiffResult {
        diff(&parse(old), &parse(new))
    }

    fn status_of(r: &DiffResult, id: &str) -> Status {
        r.marks.entities[r.merged.entity_index(id).unwrap()].status
    }

    const BASE: &str = "erDiagram
%% group Sales: CUSTOMER, ORDER
CUSTOMER ||--o{ ORDER : places
ORDER }o--|| PRODUCT : refers
CUSTOMER {
  string id PK
  string name
  string email
}
";

    #[test]
    fn identical_versions_have_no_difference() {
        let r = run(BASE, BASE);
        assert!(r.marks.is_identical());
        assert_eq!(r.merged.entities.len(), 3);
    }

    #[test]
    fn reordering_and_whitespace_are_not_differences() {
        let reordered = "erDiagram
    CUSTOMER {
        string id   PK
        string name
        string email
    }

    ORDER }o--|| PRODUCT : refers
    CUSTOMER ||--o{ ORDER : places
    %% group Sales: CUSTOMER, ORDER
";
        assert!(run(BASE, reordered).marks.is_identical());
    }

    #[test]
    fn reversed_relationship_is_the_same_relationship() {
        let reversed = BASE.replace(
            "CUSTOMER ||--o{ ORDER : places",
            "ORDER }o--|| CUSTOMER : places",
        );
        assert!(run(BASE, &reversed).marks.is_identical());
    }

    #[test]
    fn added_and_removed_entities() {
        let new = BASE.replace(
            "ORDER }o--|| PRODUCT : refers",
            "ORDER }o--|| ITEM : refers",
        );
        let r = run(BASE, &new);
        assert_eq!(status_of(&r, "ITEM"), Status::Added);
        assert_eq!(status_of(&r, "PRODUCT"), Status::Removed);
        assert_eq!(status_of(&r, "CUSTOMER"), Status::Same);
        let statuses: Vec<Status> = r.marks.relationships.iter().map(|d| d.status).collect();
        assert_eq!(statuses, [Status::Same, Status::Added, Status::Removed]);
        // The removed relationship still points at the (removed) entity in the merged diagram.
        let removed = &r.merged.relationships[2];
        assert_eq!(r.merged.entities[removed.b].id, "PRODUCT");
        assert_eq!(
            r.marks.entity_counts(),
            Counts {
                added: 1,
                removed: 1,
                changed: 0
            }
        );
    }

    #[test]
    fn attribute_changes_keep_old_values_and_order() {
        let new = BASE
            .replace("string id PK", "int id PK")
            .replace("  string name\n", "")
            .replace("string email", "string email UK");
        let new = new.replace("string email UK\n", "string email UK\n  date born\n");
        let r = run(BASE, &new);
        let c = r.merged.entity_index("CUSTOMER").unwrap();
        let names: Vec<&str> = r.merged.entities[c]
            .attributes
            .iter()
            .map(|a| a.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["id", "name", "email", "born"],
            "removed field stays where it was"
        );
        let d = &r.marks.entities[c];
        let st: Vec<Status> = d.attrs.iter().map(|a| a.status).collect();
        assert_eq!(
            st,
            [
                Status::Changed,
                Status::Removed,
                Status::Changed,
                Status::Added
            ]
        );
        assert_eq!(d.status, Status::Changed);
        let id = &r.merged.entities[c].attributes[0];
        assert_eq!(type_text(id, Some(&d.attrs[0])), "string → int");
        assert_eq!(keys_text(id, Some(&d.attrs[0])), "PK");
        let email = &r.merged.entities[c].attributes[2];
        assert_eq!(keys_text(email, Some(&d.attrs[2])), "none → UK");
        assert!(
            d.notes
                .iter()
                .any(|n| n == "field changed: id (type string → int)"),
            "{:?}",
            d.notes
        );
    }

    #[test]
    fn relationship_cardinality_change() {
        let new = BASE.replace("CUSTOMER ||--o{ ORDER", "CUSTOMER ||..|{ ORDER");
        let r = run(BASE, &new);
        let d = &r.marks.relationships[0];
        assert_eq!(d.status, Status::Changed);
        assert_eq!(
            d.note.as_deref(),
            Some("1 : 0..* → 1 : 1..*, identifying → non-identifying")
        );
    }

    #[test]
    fn group_move_label_and_description_mark_entity_changed() {
        let new = BASE
            .replace(
                "%% group Sales: CUSTOMER, ORDER",
                "%% group Sales: ORDER\n%% group People: CUSTOMER\n%% describe ORDER: New.",
            )
            .replace("ORDER }o--|| PRODUCT", "ORDER }o--|| p[\"Product\"]")
            .replace("PRODUCT : refers", "p : refers");
        let r = run(BASE, &new);
        let notes = |id: &str| {
            r.marks.entities[r.merged.entity_index(id).unwrap()]
                .notes
                .clone()
        };
        assert_eq!(notes("CUSTOMER"), ["group: Sales → People"]);
        assert_eq!(notes("ORDER"), ["description added"]);
        assert_eq!(
            status_of(&r, "PRODUCT"),
            Status::Removed,
            "renamed id is removed + added"
        );
        assert_eq!(status_of(&r, "p"), Status::Added);
    }

    #[test]
    fn tag_changes_are_noted_and_all_tags_kept() {
        let old = "erDiagram\n%% describe A: #x #y Text.\nA { int id }\n";
        let new = "erDiagram\n%% describe A: #y #z Text.\nA { int id }\n";
        let r = run(old, new);
        assert_eq!(r.marks.entities[0].notes, ["tags: +#z −#x"]);
        assert_eq!(r.merged.tags, ["y", "z", "x"]);
        assert!(
            run(old, &old.replace("#x #y", "#y #x"))
                .marks
                .is_identical(),
            "order is not a change"
        );
    }

    #[test]
    fn removed_entities_stay_in_their_old_group() {
        let old =
            "erDiagram\n%% group Gone: A, B\n%% group Kept: C\nA ||--o{ B : x\nB ||--o{ C : y\n";
        let new = "erDiagram\n%% group Kept: C\nC { int id }\n";
        let r = run(old, new);
        let groups: Vec<(&str, Vec<&str>)> = r
            .merged
            .groups
            .iter()
            .map(|g| {
                (
                    g.name.as_str(),
                    g.members.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(groups, [("Kept", vec!["C"]), ("Gone", vec!["A", "B"])]);
        assert_eq!(
            r.marks.relationship_counts(),
            Counts {
                added: 0,
                removed: 2,
                changed: 0
            }
        );
    }

    #[test]
    fn empty_old_side_means_everything_added() {
        let r = diff(&Diagram::default(), &parse(BASE));
        assert_eq!(
            r.marks.entity_counts(),
            Counts {
                added: 3,
                removed: 0,
                changed: 0
            }
        );
        assert_eq!(r.marks.entity_counts().badge(), "+3");
    }
}
