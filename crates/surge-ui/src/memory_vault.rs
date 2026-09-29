//! The project's memory vault: `.surge/memory/*.md`, read like a notebook.
//!
//! Every run starts with these notes (see `docs/conventions/project-memory.md`);
//! agents add to them and the engine stamps each note with the run and step
//! that wrote it (`<!-- surge:memory run=… node=… -->`). Here they are parsed
//! into notes with titles, links (`[[note]]`, `[text](note.md)`), tags
//! (`#tag`) and provenance, so the screen can show a note, what it links to,
//! what links back, and the whole vault as a graph.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Where the vault lives inside a project.
pub fn vault_dir(project_root: &Path) -> PathBuf {
    project_root.join(".surge").join("memory")
}

/// Which run and step wrote a note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub run: String,
    pub node: String,
}

/// One note.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    /// File name, e.g. `auth.md`.
    pub file: String,
    /// Link key: the file stem, lowercase (`auth`).
    pub slug: String,
    pub title: String,
    /// Markdown without the provenance stamp or the title heading.
    pub body: String,
    pub stamp: Option<Stamp>,
    /// Slugs this note links to.
    pub links: Vec<String>,
    pub tags: Vec<String>,
    pub modified_ms: Option<i64>,
    /// `MEMORY.md` — the human index, not fed to runs.
    pub is_index: bool,
}

/// `Auth invariant!` → `auth-invariant`.
pub fn slugify(title: &str) -> String {
    let mut slug = String::new();
    let mut dash = false;
    for c in title.trim().chars() {
        if c.is_alphanumeric() {
            slug.extend(c.to_lowercase());
            dash = false;
        } else if !dash && !slug.is_empty() {
            slug.push('-');
            dash = true;
        }
    }
    slug.trim_end_matches('-').to_string()
}

fn parse_stamp(line: &str) -> Option<Stamp> {
    let inner = line
        .trim()
        .strip_prefix("<!--")?
        .strip_suffix("-->")?
        .trim();
    let rest = inner.strip_prefix("surge:memory")?;
    let mut run = None;
    let mut node = None;
    for part in rest.split_whitespace() {
        if let Some(v) = part.strip_prefix("run=") {
            run = Some(v.to_string());
        } else if let Some(v) = part.strip_prefix("node=") {
            node = Some(v.to_string());
        }
    }
    Some(Stamp {
        run: run?,
        node: node.unwrap_or_default(),
    })
}

/// Link targets in a line: `[[target]]`, `[[target|alias]]`, `[x](target.md)`.
fn links_in(line: &str, out: &mut Vec<String>) {
    let mut rest = line;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else { break };
        let target = after[..end].split('|').next().unwrap_or_default();
        let slug = slugify(target.trim_end_matches(".md"));
        if !slug.is_empty() {
            out.push(slug);
        }
        rest = &after[end + 2..];
    }
    let mut rest = line;
    while let Some(start) = rest.find("](") {
        let after = &rest[start + 2..];
        let Some(end) = after.find(')') else { break };
        let target = &after[..end];
        let is_note = target.ends_with(".md") && !target.contains("://");
        if is_note {
            let stem = Path::new(target)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            let slug = slugify(stem);
            if !slug.is_empty() {
                out.push(slug);
            }
        }
        rest = &after[end + 1..];
    }
}

fn tags_in(line: &str, out: &mut Vec<String>) {
    for word in line.split_whitespace() {
        let Some(tag) = word.strip_prefix('#') else {
            continue;
        };
        let tag: String = tag
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_' || *c == '/')
            .collect();
        if tag.chars().next().is_some_and(char::is_alphabetic) {
            out.push(tag.to_lowercase());
        }
    }
}

/// Parse a note from its file name and contents.
pub fn parse_note(file: &str, raw: &str) -> Note {
    let stem = Path::new(file)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(file);
    let mut stamp = None;
    let mut title = None;
    let mut body_lines = Vec::new();
    let mut links = Vec::new();
    let mut tags = Vec::new();
    let mut in_fence = false;
    for line in raw.lines() {
        let trimmed = line.trim();
        if stamp.is_none()
            && let Some(s) = parse_stamp(trimmed)
        {
            stamp = Some(s);
            continue;
        }
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
        }
        if title.is_none()
            && !in_fence
            && let Some(h) = trimmed.strip_prefix("# ")
        {
            title = Some(h.trim().to_string());
            continue;
        }
        if !in_fence {
            links_in(line, &mut links);
            // `# Heading` is a heading; `#tag` at the start of a line is a tag.
            let heading = trimmed.starts_with("# ") || trimmed.starts_with("##");
            if !heading {
                tags_in(line, &mut tags);
            }
        }
        body_lines.push(line);
    }
    let slug = slugify(stem);
    links.retain(|l| l != &slug);
    let mut seen = BTreeSet::new();
    links.retain(|l| seen.insert(l.clone()));
    let mut seen = BTreeSet::new();
    tags.retain(|t| seen.insert(t.clone()));
    Note {
        file: file.to_string(),
        slug,
        title: title.unwrap_or_else(|| stem.replace(['-', '_'], " ")),
        body: body_lines.join("\n").trim().to_string(),
        stamp,
        links,
        tags,
        modified_ms: None,
        is_index: file.eq_ignore_ascii_case("MEMORY.md"),
    }
}

/// Every note in the vault, index first, then by title.
pub fn load(project_root: &Path) -> Vec<Note> {
    let dir = vault_dir(project_root);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut notes: Vec<Note> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
        .filter_map(|e| {
            let file = e.file_name().to_string_lossy().into_owned();
            let raw = std::fs::read_to_string(e.path()).ok()?;
            let mut note = parse_note(&file, &raw);
            note.modified_ms = e
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64);
            Some(note)
        })
        .collect();
    notes.sort_by(|a, b| {
        b.is_index
            .cmp(&a.is_index)
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
    notes
}

/// The file content for a note, keeping its provenance stamp first.
pub fn compose(title: &str, body: &str, stamp: Option<&Stamp>) -> String {
    let mut out = String::new();
    if let Some(s) = stamp {
        out.push_str(&format!(
            "<!-- surge:memory run={} node={} -->\n",
            s.run, s.node
        ));
    }
    out.push_str(&format!("# {}\n\n{}\n", title.trim(), body.trim()));
    out
}

/// A free `<slug>.md` file name in the vault for `title`.
pub fn free_file_name(project_root: &Path, title: &str) -> String {
    let base = match slugify(title) {
        s if s.is_empty() => "note".to_string(),
        s => s,
    };
    let dir = vault_dir(project_root);
    let mut name = format!("{base}.md");
    let mut n = 2;
    while dir.join(&name).exists() {
        name = format!("{base}-{n}.md");
        n += 1;
    }
    name
}

/// `[[Target|Alias]]` → `[Alias](target)` so the renderer shows it as a
/// link (Obsidian-style) instead of raw brackets.
pub fn wikilinks_to_markdown(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find("[[") {
        let Some(end) = rest[start + 2..].find("]]") else {
            break;
        };
        let inner = &rest[start + 2..start + 2 + end];
        let (target, label) = inner.split_once('|').unwrap_or((inner, inner));
        out.push_str(&rest[..start]);
        out.push_str(&format!("[{}]({})", label.trim(), slugify(target)));
        rest = &rest[start + 2 + end + 2..];
    }
    out.push_str(rest);
    out
}

/// How two notes are related.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    Link,
    SharedTag,
}

/// Edges between notes by index: explicit links, then shared tags
/// (only where no link already connects the pair).
/// Resolve a link key to a note: by file name first, then by title — people
/// write `[[Timer ticks once per second]]` as often as `[[timer-ticks]]`.
pub fn resolve(notes: &[Note], key: &str) -> Option<usize> {
    notes
        .iter()
        .position(|n| n.slug == key)
        .or_else(|| notes.iter().position(|n| slugify(&n.title) == key))
}

pub fn edges(notes: &[Note]) -> Vec<(usize, usize, EdgeKind)> {
    let mut out: Vec<(usize, usize, EdgeKind)> = Vec::new();
    let mut linked = BTreeSet::new();
    for (i, note) in notes.iter().enumerate() {
        for target in &note.links {
            if let Some(j) = resolve(notes, target).filter(|j| *j != i) {
                let key = (i.min(j), i.max(j));
                if linked.insert(key) {
                    out.push((i, j, EdgeKind::Link));
                }
            }
        }
    }
    for i in 0..notes.len() {
        for j in i + 1..notes.len() {
            if linked.contains(&(i, j)) {
                continue;
            }
            if notes[i].tags.iter().any(|t| notes[j].tags.contains(t)) {
                out.push((i, j, EdgeKind::SharedTag));
            }
        }
    }
    out
}

/// Notes that link to `slug`.
pub fn backlinks<'a>(notes: &'a [Note], slug: &str) -> Vec<&'a Note> {
    let Some(me) = notes.iter().position(|n| n.slug == slug) else {
        return Vec::new();
    };
    notes
        .iter()
        .enumerate()
        .filter(|(i, n)| *i != me && n.links.iter().any(|l| resolve(notes, l) == Some(me)))
        .map(|(_, n)| n)
        .collect()
}

/// Force-directed positions in a `w × h` box, deterministic: nodes start on
/// a circle, repel each other, and edges pull their ends together.
pub fn layout(count: usize, edges: &[(usize, usize, EdgeKind)], w: f32, h: f32) -> Vec<(f32, f32)> {
    if count == 0 {
        return Vec::new();
    }
    let (cx, cy) = (w / 2.0, h / 2.0);
    let radius = w.min(h) * 0.35;
    let mut pos: Vec<(f32, f32)> = (0..count)
        .map(|i| {
            let a = i as f32 / count as f32 * std::f32::consts::TAU;
            (cx + radius * a.cos(), cy + radius * a.sin())
        })
        .collect();
    if count == 1 {
        return vec![(cx, cy)];
    }
    let ideal = (w * h / count as f32).sqrt() * 0.55;
    for step in 0..300 {
        let temp = (1.0 - step as f32 / 300.0) * ideal * 0.3 + 0.5;
        let mut force = vec![(0.0_f32, 0.0_f32); count];
        for i in 0..count {
            for j in 0..count {
                if i == j {
                    continue;
                }
                let dx = pos[i].0 - pos[j].0;
                let dy = pos[i].1 - pos[j].1;
                let d = (dx * dx + dy * dy).sqrt().max(1.0);
                // Fruchterman–Reingold: repulsion ideal²/d, attraction d²/ideal.
                let push = ideal * ideal / d;
                force[i].0 += dx / d * push;
                force[i].1 += dy / d * push;
            }
        }
        for &(a, b, kind) in edges {
            let dx = pos[a].0 - pos[b].0;
            let dy = pos[a].1 - pos[b].1;
            let d = (dx * dx + dy * dy).sqrt().max(1.0);
            let pull = d * d / ideal * if kind == EdgeKind::Link { 1.0 } else { 0.4 };
            force[a].0 -= dx / d * pull;
            force[a].1 -= dy / d * pull;
            force[b].0 += dx / d * pull;
            force[b].1 += dy / d * pull;
        }
        for (i, p) in pos.iter_mut().enumerate() {
            // Gentle gravity keeps loose notes on screen.
            force[i].0 += (cx - p.0) * 0.05;
            force[i].1 += (cy - p.1) * 0.05;
            let len = (force[i].0 * force[i].0 + force[i].1 * force[i].1)
                .sqrt()
                .max(0.001);
            let step_len = len.min(temp);
            // Room for the 140px label centred on the dot.
            p.0 = (p.0 + force[i].0 / len * step_len).clamp(80.0, w - 80.0);
            p.1 = (p.1 + force[i].1 / len * step_len).clamp(30.0, h - 40.0);
        }
    }
    pos
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUTH: &str = "<!-- surge:memory run=run-01ABC node=impl_task -->\n# Auth invariant\n\nTokens are always HS256 — see [[Migrations]] and [keys](keys.md). #security #auth\n\n```\n# not a title [[ignored]]\n```\n";

    #[test]
    fn a_note_keeps_its_title_stamp_links_and_tags() {
        let note = parse_note("auth.md", AUTH);
        assert_eq!(note.title, "Auth invariant");
        assert_eq!(note.slug, "auth");
        assert_eq!(
            note.stamp,
            Some(Stamp {
                run: "run-01ABC".into(),
                node: "impl_task".into()
            })
        );
        assert_eq!(note.links, ["migrations", "keys"]);
        assert_eq!(note.tags, ["security", "auth"]);
        assert!(!note.body.contains("surge:memory"));
        assert!(!note.body.starts_with("# Auth"));
    }

    #[test]
    fn untitled_notes_fall_back_to_their_file_name() {
        let note = parse_note("rate-limits.md", "Backoff doubles.");
        assert_eq!(note.title, "rate limits");
        assert!(note.stamp.is_none());
    }

    #[test]
    fn compose_round_trips_and_keeps_the_stamp_first() {
        let note = parse_note("auth.md", AUTH);
        let text = compose(&note.title, &note.body, note.stamp.as_ref());
        assert!(
            text.starts_with(
                "<!-- surge:memory run=run-01ABC node=impl_task -->\n# Auth invariant\n"
            )
        );
        assert_eq!(parse_note("auth.md", &text).links, note.links);
    }

    #[test]
    fn edges_prefer_links_and_backlinks_find_the_source() {
        let notes = vec![
            parse_note("auth.md", AUTH),
            parse_note(
                "migrations.md",
                "# Migrations\nAlways reversible.\n#security",
            ),
            parse_note("ui.md", "# UI\n#design"),
        ];
        let edges = edges(&notes);
        assert_eq!(edges, [(0, 1, EdgeKind::Link)]);
        let back: Vec<&str> = backlinks(&notes, "migrations")
            .iter()
            .map(|n| n.slug.as_str())
            .collect();
        assert_eq!(back, ["auth"]);
    }

    #[test]
    fn a_tag_at_line_start_is_a_tag_not_a_heading() {
        let note = parse_note("x.md", "# Title\n#constraint\n## Section #not-a-tag");
        assert_eq!(note.tags, ["constraint"]);
    }

    #[test]
    fn links_resolve_by_file_name_or_title() {
        let notes = vec![
            parse_note(
                "timer-ticks.md",
                "# Timer ticks once per second\nSee [[phase-banner]].",
            ),
            parse_note(
                "phase-banner.md",
                "# Phase banner\nSee [[Timer ticks once per second]].",
            ),
        ];
        assert_eq!(resolve(&notes, "timer-ticks-once-per-second"), Some(0));
        assert_eq!(edges(&notes), [(0, 1, EdgeKind::Link)]);
        assert_eq!(backlinks(&notes, "timer-ticks").len(), 1);
    }

    #[test]
    fn wikilinks_render_as_links() {
        assert_eq!(
            wikilinks_to_markdown("See [[Phase banner]] and [[auth|the auth rule]]."),
            "See [Phase banner](phase-banner) and [the auth rule](auth)."
        );
    }

    #[test]
    fn slugs_are_file_safe() {
        assert_eq!(slugify("  Auth invariant!  "), "auth-invariant");
        assert_eq!(slugify("C++ / FFI"), "c-ffi");
    }

    #[test]
    fn layout_stays_in_bounds_and_separates_nodes() {
        let edges = vec![(0, 1, EdgeKind::Link), (1, 2, EdgeKind::Link)];
        let pos = layout(5, &edges, 800.0, 500.0);
        assert_eq!(pos.len(), 5);
        for (x, y) in &pos {
            assert!((80.0..=720.0).contains(x) && (30.0..=460.0).contains(y));
        }
        for i in 0..pos.len() {
            for j in i + 1..pos.len() {
                let d = ((pos[i].0 - pos[j].0).powi(2) + (pos[i].1 - pos[j].1).powi(2)).sqrt();
                assert!(d > 90.0, "nodes {i} and {j} are crowded ({d})");
            }
        }
    }
}
