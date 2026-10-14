//! Format-independent ER model. Parsers (currently only Mermaid) produce a [`Diagram`].

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cardinality {
    ZeroOrOne,
    ExactlyOne,
    ZeroOrMany,
    OneOrMany,
}

impl Cardinality {
    pub fn is_many(self) -> bool {
        matches!(self, Self::ZeroOrMany | Self::OneOrMany)
    }

    pub fn is_optional(self) -> bool {
        matches!(self, Self::ZeroOrOne | Self::ZeroOrMany)
    }

    /// Short textual form, e.g. `0..1`, `1..*`.
    pub fn short(self) -> &'static str {
        match self {
            Self::ZeroOrOne => "0..1",
            Self::ExactlyOne => "1",
            Self::ZeroOrMany => "0..*",
            Self::OneOrMany => "1..*",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Attribute {
    pub ty: String,
    pub name: String,
    /// `PK`, `FK`, `UK`, …
    pub keys: Vec<String>,
    pub comment: Option<String>,
}

impl Attribute {
    /// The comment as an option list, if it is one: `"viewer | analyst | admin"`. Every option
    /// must be a single token (`...` allowed), so prose like `"5m | 15m for measurement"` is not.
    pub fn enum_options(&self) -> Option<String> {
        let c = self.comment.as_deref()?;
        let parts: Vec<&str> = c.split('|').map(str::trim).collect();
        let token = |p: &&str| !p.is_empty() && !p.contains(char::is_whitespace);
        (parts.len() >= 2 && parts.iter().all(token)).then(|| parts.join(" | "))
    }
}

#[derive(Clone, Debug, Default)]
pub struct Entity {
    pub id: String,
    /// Mermaid alias: `p["Person"]`.
    pub label: Option<String>,
    /// Free text from `%% describe ENTITY: …` lines. `*italic*` and `` `code` `` are rendered.
    pub description: Option<String>,
    /// `#TAG`s from describe lines (without `#`), in order, no duplicates.
    pub tags: Vec<String>,
    pub attributes: Vec<Attribute>,
}

impl Entity {
    pub fn display_name(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.id)
    }
}

#[derive(Clone, Debug)]
pub struct Relationship {
    pub a: usize,
    pub b: usize,
    pub card_a: Cardinality,
    pub card_b: Cardinality,
    /// Solid line (`--`) vs. dashed (`..`).
    pub identifying: bool,
    pub label: String,
}

/// A group declared in the source, by entity id.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupDef {
    pub name: String,
    pub members: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Diagram {
    pub entities: Vec<Entity>,
    pub relationships: Vec<Relationship>,
    pub groups: Vec<GroupDef>,
    /// Every tag, in order of first appearance; a tag's position picks its color.
    pub tags: Vec<String>,
    pub warnings: Vec<String>,
    index: HashMap<String, usize>,
}

impl Diagram {
    pub fn entity_index(&self, id: &str) -> Option<usize> {
        self.index.get(id).copied()
    }

    /// Returns the index of entity `id`, creating it if needed.
    pub fn ensure_entity(&mut self, id: &str) -> usize {
        if let Some(&i) = self.index.get(id) {
            return i;
        }
        let i = self.entities.len();
        self.entities.push(Entity {
            id: id.to_owned(),
            ..Default::default()
        });
        self.index.insert(id.to_owned(), i);
        i
    }

    pub fn tag_index(&self, tag: &str) -> Option<usize> {
        self.tags.iter().position(|t| t == tag)
    }

    /// Relationship indices touching entity `e`.
    pub fn relationships_of(&self, e: usize) -> impl Iterator<Item = usize> + '_ {
        self.relationships
            .iter()
            .enumerate()
            .filter(move |(_, r)| r.a == e || r.b == e)
            .map(|(i, _)| i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attr(comment: &str) -> Attribute {
        Attribute {
            comment: Some(comment.to_owned()),
            ..Default::default()
        }
    }

    #[test]
    fn recognises_option_lists() {
        assert_eq!(
            attr("viewer|analyst | admin").enum_options().as_deref(),
            Some("viewer | analyst | admin")
        );
        assert_eq!(
            attr("ct_log | cert_san | ...").enum_options().as_deref(),
            Some("ct_log | cert_san | ...")
        );
        assert_eq!(attr("read | act, group list").enum_options(), None);
        assert_eq!(attr("5m | 15m | 1h for measurement").enum_options(), None);
        assert_eq!(attr("the name").enum_options(), None);
        assert_eq!(Attribute::default().enum_options(), None);
    }
}
