//! Parser for Mermaid `erDiagram` sources.
//!
//! Besides standard Mermaid syntax this understands group directives in comments, which
//! Mermaid itself ignores:
//!
//! ```text
//! %% group Billing: INVOICE, PAYMENT, "LINE ITEM"
//! %% describe INVOICE: What the customer is billed. Repeated lines start new paragraphs.
//! ```

use crate::model::{Attribute, Cardinality, Diagram, GroupDef, Relationship};

/// If `src` is Markdown, returns the concatenated ```` ```mermaid ```` blocks that contain an
/// `erDiagram`; otherwise returns `src` unchanged.
pub fn extract_er_source(src: &str) -> String {
    if !src.contains("```") {
        return src.to_owned();
    }
    let mut out = String::new();
    let mut block: Option<String> = None;
    for line in src.lines() {
        let t = line.trim();
        match &mut block {
            None if t.starts_with("```") && t[3..].trim().eq_ignore_ascii_case("mermaid") => {
                block = Some(String::new());
            }
            None => {}
            Some(b) if t.starts_with("```") => {
                if b.lines().any(|l| l.trim_start().starts_with("erDiagram")) {
                    out.push_str(b);
                }
                block = None;
            }
            Some(b) => {
                b.push_str(line);
                b.push('\n');
            }
        }
    }
    if out.is_empty() { src.to_owned() } else { out }
}

/// Takes the `#TAG` words out of a description line: `#` then a letter or `_`, then letters,
/// digits, `_` or `-`, as a whole word (trailing `.,;:` allowed). `C#` and `#3` are not tags.
fn split_tags(text: &str) -> (Vec<String>, String) {
    let tag = |w: &str| {
        let t = w.strip_prefix('#')?.trim_end_matches(['.', ',', ';', ':']);
        let mut cs = t.chars();
        let ok = cs.next().is_some_and(|c| c.is_alphabetic() || c == '_')
            && cs.all(|c| c.is_alphanumeric() || c == '_' || c == '-');
        ok.then(|| t.to_owned())
    };
    if !text.split_whitespace().any(|w| tag(w).is_some()) {
        return (Vec::new(), text.to_owned());
    }
    let mut tags = Vec::new();
    let mut rest = Vec::new();
    for w in text.split_whitespace() {
        match tag(w) {
            Some(t) => tags.push(t),
            None => rest.push(w),
        }
    }
    (tags, rest.join(" "))
}

pub fn parse(src: &str) -> Diagram {
    let mut d = Diagram::default();
    let mut current: Option<usize> = None;
    let mut descriptions: Vec<(String, String)> = Vec::new();
    let mut front_matter = false;
    let mut seen_content = false;

    for (no, raw) in src.lines().enumerate() {
        let line = raw.trim();
        let lineno = no + 1;
        if line.is_empty() {
            continue;
        }
        if line == "---" && (front_matter || !seen_content) {
            front_matter = !front_matter;
            continue;
        }
        seen_content = true;
        if front_matter {
            continue;
        }
        if let Some(rest) = line.strip_prefix("%%") {
            let rest = rest.trim();
            match directive(rest, "describe") {
                Some(body) => match body.split_once(':') {
                    Some((id, text)) => {
                        descriptions.push((unquote(id.trim()).to_owned(), text.trim().to_owned()))
                    }
                    None => d
                        .warnings
                        .push(format!("line {lineno}: describe directive without `:`")),
                },
                None => parse_directive(&mut d, rest),
            }
            continue;
        }

        if let Some(e) = current {
            let (content, closes) = match line.strip_suffix('}') {
                Some(c) => (c.trim(), true),
                None => (line, false),
            };
            if !content.is_empty() {
                match parse_attribute(content) {
                    Some(a) => d.entities[e].attributes.push(a),
                    None => d
                        .warnings
                        .push(format!("line {lineno}: cannot parse attribute `{content}`")),
                }
            }
            if closes {
                current = None;
            }
            continue;
        }

        let first_word = line.split_whitespace().next().unwrap_or("");
        if first_word.starts_with("erDiagram") {
            continue;
        }
        if matches!(
            first_word,
            "direction" | "title" | "accTitle" | "accDescr" | "classDef" | "class" | "style"
        ) || first_word.starts_with("accTitle:")
            || first_word.starts_with("accDescr:")
            || first_word.starts_with("title:")
        {
            continue;
        }

        if parse_relationship(&mut d, line) {
            continue;
        }

        if let Some(brace) = find_outside_quotes(line, '{') {
            let head = line[..brace].trim();
            let tail = line[brace + 1..].trim();
            let Some((id, label)) = parse_entity_ref(head) else {
                d.warnings
                    .push(format!("line {lineno}: cannot parse entity `{head}`"));
                continue;
            };
            let e = d.ensure_entity(&id);
            if label.is_some() {
                d.entities[e].label = label;
            }
            match tail.strip_suffix('}') {
                Some(inner) => {
                    let inner = inner.trim();
                    if !inner.is_empty()
                        && let Some(a) = parse_attribute(inner)
                    {
                        d.entities[e].attributes.push(a);
                    }
                }
                None => {
                    if !tail.is_empty()
                        && let Some(a) = parse_attribute(tail)
                    {
                        d.entities[e].attributes.push(a);
                    }
                    current = Some(e);
                }
            }
            continue;
        }

        // A bare entity declaration.
        let toks = tokenize(line);
        if toks.len() == 1
            && let Some((id, label)) = parse_entity_ref(&toks[0])
        {
            let e = d.ensure_entity(&id);
            if label.is_some() {
                d.entities[e].label = label;
            }
            continue;
        }

        d.warnings.push(format!("line {lineno}: ignored `{line}`"));
    }

    for (id, text) in descriptions {
        let Some(e) = d.entity_index(&id) else {
            d.warnings.push(format!("describe: unknown entity `{id}`"));
            continue;
        };
        let (tags, text) = split_tags(&text);
        for t in tags {
            if !d.tags.contains(&t) {
                d.tags.push(t.clone());
            }
            if !d.entities[e].tags.contains(&t) {
                d.entities[e].tags.push(t);
            }
        }
        if text.is_empty() {
            continue;
        }
        let desc = d.entities[e].description.get_or_insert_with(String::new);
        if !desc.is_empty() {
            desc.push('\n');
        }
        desc.push_str(&text);
    }
    if current.is_some() {
        d.warnings
            .push("unterminated entity block at end of file".to_owned());
    }
    for g in &d.groups {
        for m in &g.members {
            if d.entity_index(m).is_none() {
                d.warnings
                    .push(format!("group `{}`: unknown entity `{m}`", g.name));
            }
        }
    }
    d
}

/// `group …` / `@group …` → the text after the keyword.
fn directive<'a>(text: &'a str, keyword: &str) -> Option<&'a str> {
    let text = text.strip_prefix('@').unwrap_or(text);
    text.strip_prefix(keyword)
        .filter(|r| r.starts_with(char::is_whitespace))
}

fn parse_directive(d: &mut Diagram, text: &str) {
    let Some(rest) = directive(text, "group") else {
        return;
    };
    let Some((name, members)) = rest.split_once(':') else {
        d.warnings
            .push(format!("group directive without `:` — `{text}`"));
        return;
    };
    let name = unquote(name.trim()).to_owned();
    let members: Vec<String> = members
        .split(',')
        .map(|m| unquote(m.trim()).to_owned())
        .filter(|m| !m.is_empty())
        .collect();
    match d.groups.iter_mut().find(|g| g.name == name) {
        Some(g) => g.members.extend(members),
        None => d.groups.push(GroupDef { name, members }),
    }
}

fn parse_relationship(d: &mut Diagram, line: &str) -> bool {
    let (lhs, label) = match find_label_colon(line) {
        Some(i) => (&line[..i], unquote(line[i + 1..].trim()).to_owned()),
        None => (line, String::new()),
    };
    let toks = tokenize(lhs);
    if toks.len() < 3 {
        return false;
    }

    let parsed = if toks.len() == 3 {
        parse_symbolic(&toks[1])
    } else {
        None
    }
    .or_else(|| parse_words(&toks[1..toks.len() - 1].join(" ")));
    let Some((card_a, identifying, card_b)) = parsed else {
        return false;
    };
    let (Some((ida, la)), Some((idb, lb))) = (
        parse_entity_ref(&toks[0]),
        parse_entity_ref(&toks[toks.len() - 1]),
    ) else {
        return false;
    };
    let a = d.ensure_entity(&ida);
    if la.is_some() {
        d.entities[a].label = la;
    }
    let b = d.ensure_entity(&idb);
    if lb.is_some() {
        d.entities[b].label = lb;
    }
    d.relationships.push(Relationship {
        a,
        b,
        card_a,
        card_b,
        identifying,
        label,
    });
    true
}

/// `||--o{`, `}|..|{`, …
fn parse_symbolic(tok: &str) -> Option<(Cardinality, bool, Cardinality)> {
    let (sep, identifying) = if tok.contains("--") {
        ("--", true)
    } else if tok.contains("..") {
        ("..", false)
    } else {
        return None;
    };
    let (l, r) = tok.split_once(sep)?;
    let valid = |s: &str| s.len() == 2 && s.chars().all(|c| matches!(c, '|' | 'o' | '{' | '}'));
    if !valid(l) || !valid(r) {
        return None;
    }
    Some((symbol_card(l), identifying, symbol_card(r)))
}

fn symbol_card(s: &str) -> Cardinality {
    let many = s.contains('{') || s.contains('}');
    let zero = s.contains('o');
    match (many, zero) {
        (true, true) => Cardinality::ZeroOrMany,
        (true, false) => Cardinality::OneOrMany,
        (false, true) => Cardinality::ZeroOrOne,
        (false, false) => Cardinality::ExactlyOne,
    }
}

/// `one or more to zero or many`, `1 optionally to 0+`, …
fn parse_words(text: &str) -> Option<(Cardinality, bool, Cardinality)> {
    let text = text.to_ascii_lowercase();
    let (l, r, identifying) = if let Some((l, r)) = text.split_once(" optionally to ") {
        (l, r, false)
    } else {
        let (l, r) = text.split_once(" to ")?;
        (l, r, true)
    };
    Some((word_card(l.trim())?, identifying, word_card(r.trim())?))
}

fn word_card(s: &str) -> Option<Cardinality> {
    Some(match s {
        "one or zero" | "zero or one" => Cardinality::ZeroOrOne,
        "only one" | "1" | "one" => Cardinality::ExactlyOne,
        "zero or more" | "zero or many" | "many(0)" | "0+" => Cardinality::ZeroOrMany,
        "one or more" | "one or many" | "many(1)" | "1+" => Cardinality::OneOrMany,
        _ => return None,
    })
}

/// `type name PK, FK "comment"`
fn parse_attribute(text: &str) -> Option<Attribute> {
    let toks = tokenize(text);
    if toks.len() < 2 {
        return None;
    }
    let mut a = Attribute {
        ty: toks[0].clone(),
        name: unquote(&toks[1]).to_owned(),
        ..Default::default()
    };
    for t in &toks[2..] {
        if t.starts_with('"') {
            a.comment = Some(unquote(t).to_owned());
        } else {
            a.keys.extend(
                t.split(',')
                    .map(str::trim)
                    .filter(|k| !k.is_empty())
                    .map(str::to_owned),
            );
        }
    }
    Some(a)
}

/// `NAME`, `"Name with spaces"`, `p["Person"]`, `p[Person]`, `NAME:::cssClass`.
fn parse_entity_ref(tok: &str) -> Option<(String, Option<String>)> {
    let tok = tok.trim();
    let tok = match tok.find(":::") {
        Some(i) if !tok.starts_with('"') => &tok[..i],
        _ => tok,
    };
    if tok.is_empty() {
        return None;
    }
    if tok.starts_with('"') {
        return Some((unquote(tok).to_owned(), None));
    }
    if let Some(open) = tok.find('[') {
        let id = &tok[..open];
        let alias = tok[open + 1..].strip_suffix(']')?;
        return Some((id.to_owned(), Some(unquote(alias.trim()).to_owned())));
    }
    if tok
        .chars()
        .any(|c| matches!(c, '{' | '}' | '|' | ':' | '"'))
    {
        return None;
    }
    Some((tok.to_owned(), None))
}

/// Whitespace split that keeps quoted sections (including their quotes) together.
fn tokenize(s: &str) -> Vec<String> {
    let mut toks = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in s.chars() {
        if c == '"' {
            quoted = !quoted;
            cur.push(c);
        } else if c.is_whitespace() && !quoted {
            if !cur.is_empty() {
                toks.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        toks.push(cur);
    }
    toks
}

fn unquote(s: &str) -> &str {
    s.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(s)
}

fn find_outside_quotes(s: &str, needle: char) -> Option<usize> {
    let mut quoted = false;
    for (i, c) in s.char_indices() {
        if c == '"' {
            quoted = !quoted;
        } else if c == needle && !quoted {
            return Some(i);
        }
    }
    None
}

/// The `:` separating a relationship from its label, skipping `:::class` markers.
fn find_label_colon(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut quoted = false;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => quoted = !quoted,
            b':' if !quoted => {
                if b[i..].starts_with(b":::") {
                    i += 3;
                    continue;
                }
                return Some(i);
            }
            _ => {}
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use Cardinality::*;

    const SAMPLE: &str = r#"
---
title: Shop
---
erDiagram
    %% group Sales: CUSTOMER, ORDER
    %% group "Catalog": PRODUCT
    CUSTOMER ||--o{ ORDER : places
    ORDER ||--|{ LINE-ITEM : contains
    PRODUCT }|..o| LINE-ITEM : "is in"
    CUSTOMER {
        string name PK "the name"
        string email UK
        int addressId FK, UK
    }
    p["Person Alias"] {
        varchar(255) title
    }
    "Spaced Name" {}
    CAR 1 to zero or more DRIVER : allows
    CAR one or more optionally to 1 GARAGE : parks
    LONELY
"#;

    #[test]
    fn parses_entities_relationships_and_groups() {
        let d = parse(SAMPLE);
        assert!(d.warnings.is_empty(), "{:?}", d.warnings);
        let names: Vec<_> = d.entities.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(
            names,
            [
                "CUSTOMER",
                "ORDER",
                "LINE-ITEM",
                "PRODUCT",
                "p",
                "Spaced Name",
                "CAR",
                "DRIVER",
                "GARAGE",
                "LONELY"
            ]
        );
        assert_eq!(d.entities[4].display_name(), "Person Alias");

        let r = &d.relationships;
        assert_eq!(r.len(), 5);
        assert_eq!(
            (r[0].card_a, r[0].card_b, r[0].identifying),
            (ExactlyOne, ZeroOrMany, true)
        );
        assert_eq!(r[1].card_b, OneOrMany);
        assert_eq!(
            (r[2].card_a, r[2].card_b, r[2].identifying),
            (OneOrMany, ZeroOrOne, false)
        );
        assert_eq!(r[2].label, "is in");
        assert_eq!((r[3].card_a, r[3].card_b), (ExactlyOne, ZeroOrMany));
        assert_eq!(
            (r[4].card_a, r[4].card_b, r[4].identifying),
            (OneOrMany, ExactlyOne, false)
        );

        let c = &d.entities[0].attributes;
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].comment.as_deref(), Some("the name"));
        assert_eq!(c[0].keys, ["PK"]);
        assert_eq!(c[2].keys, ["FK", "UK"]);
        assert_eq!(d.entities[4].attributes[0].ty, "varchar(255)");

        assert_eq!(d.groups.len(), 2);
        assert_eq!(d.groups[0].members, ["CUSTOMER", "ORDER"]);
        assert_eq!(d.groups[1].name, "Catalog");
    }

    #[test]
    fn descriptions_attach_to_entities() {
        let d = parse(
            "erDiagram\n%% describe A: First: with colon.\n%% describe A: Second.\n%% @describe \"B\": *b*\n%% describe NOPE: x\nA ||--|| B : r\n",
        );
        assert_eq!(
            d.entities[0].description.as_deref(),
            Some("First: with colon.\nSecond.")
        );
        assert_eq!(d.entities[1].description.as_deref(), Some("*b*"));
        assert_eq!(d.warnings.len(), 1);
    }

    #[test]
    fn tags_come_out_of_descriptions() {
        let d = parse(
            "erDiagram\n%% describe A: #CONSUMER A consumer of #events, see C# and issue #3.\n%% describe B: #ops #CONSUMER\n%% describe A: #ops\nA ||--|| B : r\n",
        );
        assert_eq!(d.entities[0].tags, ["CONSUMER", "events", "ops"]);
        assert_eq!(
            d.entities[0].description.as_deref(),
            Some("A consumer of see C# and issue #3.")
        );
        assert_eq!(d.entities[1].tags, ["ops", "CONSUMER"]);
        assert_eq!(
            d.entities[1].description, None,
            "a line of only tags adds no text"
        );
        assert_eq!(d.tags, ["CONSUMER", "events", "ops"]);
        assert_eq!(d.tag_index("ops"), Some(2));
        assert_eq!(
            split_tags("x #a-b_1: y"),
            (vec!["a-b_1".to_owned()], "x y".to_owned())
        );
        assert_eq!(split_tags("keep  spacing # here").1, "keep  spacing # here");
    }

    #[test]
    fn warns_on_unknown_group_members() {
        let d = parse("erDiagram\n%% group G: A, NOPE\nA ||--|| B : x\n");
        assert_eq!(d.warnings.len(), 1);
    }

    #[test]
    fn extracts_mermaid_from_markdown() {
        let md = "# Doc\n```mermaid\nflowchart LR\nA-->B\n```\ntext\n```mermaid\nerDiagram\nA ||--|| B : x\n```\n";
        let d = parse(&extract_er_source(md));
        assert_eq!(d.entities.len(), 2);
        assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    }

    #[test]
    fn class_shorthand_is_ignored() {
        let d = parse("erDiagram\nA:::foo ||--o{ B:::bar : rel\n");
        assert_eq!(d.entities[0].id, "A");
        assert_eq!(d.relationships[0].label, "rel");
    }
}
