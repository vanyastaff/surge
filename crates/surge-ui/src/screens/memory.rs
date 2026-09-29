//! Memory — the project's notebook, read and written like a vault.
//!
//! Two real sources, nothing invented:
//! - **Notes** — `.surge/memory/*.md` in the project. Every run starts with
//!   them; agents add to them and each note carries the run and step that
//!   wrote it. Read, edit, link (`[[note]]`), tag (`#tag`), follow
//!   backlinks, see the vault as a graph — like an Obsidian vault.
//! - **Learned** — claims Surge recorded from past runs
//!   (`~/.surge/memory.db`), each with its source and whether anything has
//!   verified it. A claim worth keeping becomes a project note in one click.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Editor, EditorState, Input, InputEvent, InputState};
use gpui_kit::component::{Icon, IconName, Sizable, StyledExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use surge_persistence::memory::MemoryStore;

use crate::app_state::AppState;
use crate::memory_vault::{self, EdgeKind, Note};
use crate::theme::{self, Semantic};
use crate::ui;

const GRAPH_W: f32 = 900.0;
const GRAPH_H: f32 = 560.0;

/// What the Memory screen asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum MemoryAction {
    /// Open the mission a note was written by.
    OpenRun(surge_core::RunId),
}

impl EventEmitter<MemoryAction> for MemoryScreen {}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Notes,
    Graph,
    Learned,
}

/// A learned claim, ready to show.
#[derive(Clone)]
struct Claim {
    text: String,
    source: String,
    verified: bool,
    confidence: String,
}

pub struct MemoryScreen {
    state: Entity<AppState>,
    mode: Mode,
    notes: Vec<Note>,
    selected: Option<String>,
    /// Graph positions, recomputed only when the vault changes.
    graph: Vec<(f32, f32)>,
    edges: Vec<(usize, usize, EdgeKind)>,
    search: Option<Entity<InputState>>,
    query: String,
    /// Open editor: (slug being edited — None for a new note, title field, body editor).
    editing: Option<(Option<String>, Entity<InputState>, Entity<EditorState>)>,
    confirm_delete: bool,
    claims: Option<Result<Vec<Claim>, String>>,
    note: Option<String>,
    loaded_root: Option<std::path::PathBuf>,
}

impl MemoryScreen {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |this: &mut Self, _state, cx| {
            let root = this.state.read(cx).project_path.clone();
            if root != this.loaded_root {
                this.reload(cx);
            }
        })
        .detach();
        let mut this = Self {
            state,
            mode: Mode::Notes,
            notes: Vec::new(),
            selected: None,
            graph: Vec::new(),
            edges: Vec::new(),
            search: None,
            query: String::new(),
            editing: None,
            confirm_delete: false,
            claims: None,
            note: None,
            loaded_root: None,
        };
        this.reload(cx);
        this
    }

    fn root(&self, cx: &Context<Self>) -> Option<std::path::PathBuf> {
        self.state.read(cx).project_path.clone()
    }

    /// Re-read the vault (after opening a project or writing a note).
    fn reload(&mut self, cx: &mut Context<Self>) {
        let root = self.root(cx);
        self.loaded_root = root.clone();
        self.notes = root.as_deref().map(memory_vault::load).unwrap_or_default();
        self.edges = memory_vault::edges(&self.notes);
        self.graph = memory_vault::layout(self.notes.len(), &self.edges, GRAPH_W, GRAPH_H);
        if self
            .selected
            .as_ref()
            .is_none_or(|s| !self.notes.iter().any(|n| &n.slug == s))
        {
            self.selected = self
                .notes
                .iter()
                .find(|n| !n.is_index)
                .or(self.notes.first())
                .map(|n| n.slug.clone());
        }
        cx.notify();
    }

    fn load_claims(&mut self) {
        if self.claims.is_some() {
            return;
        }
        let result = MemoryStore::default_path()
            .and_then(|p| MemoryStore::open(&p))
            .and_then(|store| store.list_claims())
            .map(|claims| {
                claims
                    .into_iter()
                    .rev()
                    .map(|c| Claim {
                        text: c.text().to_string(),
                        source: c.provenance().source.clone(),
                        verified: c.provenance().is_verified(),
                        confidence: c.confidence().as_str().replace('_', " "),
                    })
                    .collect()
            })
            .map_err(|e| e.to_string());
        self.claims = Some(result);
    }

    fn selected_note(&self) -> Option<&Note> {
        let slug = self.selected.as_ref()?;
        self.notes.iter().find(|n| &n.slug == slug)
    }

    fn open_note(&mut self, slug: String, cx: &mut Context<Self>) {
        self.selected = Some(slug);
        self.mode = Mode::Notes;
        self.editing = None;
        self.confirm_delete = false;
        self.note = None;
        cx.notify();
    }

    fn start_edit(&mut self, slug: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let (title, body) = match slug
            .as_ref()
            .and_then(|s| self.notes.iter().find(|n| &n.slug == s))
        {
            Some(n) => (n.title.clone(), n.body.clone()),
            None => (String::new(), String::new()),
        };
        let title_input = cx.new(|cx| InputState::new(window, cx).placeholder("Title"));
        title_input.update(cx, |s, cx| s.set_value(title, window, cx));
        let editor = cx.new(|cx| EditorState::new(window, cx).language("markdown"));
        editor.update(cx, |e, cx| e.set_value(&body, window, cx));
        self.editing = Some((slug, title_input, editor));
        self.confirm_delete = false;
        cx.notify();
    }

    fn save_edit(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root(cx) else { return };
        let Some((slug, title_input, editor)) = self.editing.clone() else {
            return;
        };
        let title = title_input.read(cx).value().trim().to_string();
        let body = editor.read(cx).value().to_string();
        if title.is_empty() {
            self.note = Some("Give the note a title first.".into());
            cx.notify();
            return;
        }
        let existing = slug
            .as_ref()
            .and_then(|s| self.notes.iter().find(|n| &n.slug == s))
            .cloned();
        let file = existing.as_ref().map_or_else(
            || memory_vault::free_file_name(&root, &title),
            |n| n.file.clone(),
        );
        let dir = memory_vault::vault_dir(&root);
        let content = memory_vault::compose(
            &title,
            &body,
            existing.as_ref().and_then(|n| n.stamp.as_ref()),
        );
        let result =
            std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(dir.join(&file), content));
        match result {
            Ok(()) => {
                tracing::info!(file = %file, "memory note saved from the UI");
                self.editing = None;
                self.selected = Some(memory_vault::parse_note(&file, "").slug);
                self.note = None;
                self.reload(cx);
            },
            Err(error) => {
                self.note = Some(format!("Could not save {file}: {error}"));
                cx.notify();
            },
        }
    }

    fn delete_selected(&mut self, cx: &mut Context<Self>) {
        let (Some(root), Some(note)) = (self.root(cx), self.selected_note().cloned()) else {
            return;
        };
        let path = memory_vault::vault_dir(&root).join(&note.file);
        match std::fs::remove_file(&path) {
            Ok(()) => {
                tracing::info!(file = %note.file, "memory note deleted from the UI");
                self.selected = None;
                self.confirm_delete = false;
                self.reload(cx);
            },
            Err(error) => {
                self.note = Some(format!("Could not delete {}: {error}", note.file));
                cx.notify();
            },
        }
    }

    /// Turn a learned claim into a project note (unverified, with its source).
    fn keep_claim(&mut self, claim: &Claim, cx: &mut Context<Self>) {
        let Some(root) = self.root(cx) else { return };
        let title: String = crate::ui::headline(&claim.text, 60);
        let file = memory_vault::free_file_name(&root, &title);
        let body = format!(
            "{}\n\nSource: `{}` · {}. Unverified until a check confirms it.\n\n#learned",
            claim.text, claim.source, claim.confidence
        );
        let dir = memory_vault::vault_dir(&root);
        let result = std::fs::create_dir_all(&dir).and_then(|()| {
            std::fs::write(dir.join(&file), memory_vault::compose(&title, &body, None))
        });
        match result {
            Ok(()) => {
                self.selected = Some(memory_vault::parse_note(&file, "").slug);
                self.reload(cx);
                self.mode = Mode::Notes;
            },
            Err(error) => self.note = Some(format!("Could not save the note: {error}")),
        }
        cx.notify();
    }

    // ── panes ───────────────────────────────────────────────────────

    fn render_modes(&self, cx: &mut Context<Self>) -> Div {
        let mut row = div()
            .h_flex()
            .p(px(2.0))
            .gap(px(2.0))
            .rounded(px(ui::R_CONTROL + 1.0))
            .border_1()
            .border_color(theme::hairline())
            .bg(theme::panel_deep());
        for (mode, label, icon) in [
            (Mode::Notes, "Notes", Lucide::ScrollText),
            (Mode::Graph, "Graph", Lucide::Waypoints),
            (Mode::Learned, "Learned", Lucide::Brain),
        ] {
            let active = self.mode == mode;
            row = row.child(
                div()
                    .id(SharedString::from(format!("memory-mode-{label}")))
                    .role(Role::Tab)
                    .h_flex()
                    .gap(px(6.0))
                    .items_center()
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded(px(ui::R_CONTROL))
                    .text_size(px(11.5))
                    .cursor_pointer()
                    .when(active, |el| {
                        el.bg(theme::surface()).text_color(theme::text_primary())
                    })
                    .when(!active, |el| {
                        el.text_color(theme::text_muted())
                            .hover(|s: StyleRefinement| s.text_color(theme::text_primary()))
                    })
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        this.mode = mode;
                        if mode == Mode::Learned {
                            this.load_claims();
                        }
                        cx.notify();
                    }))
                    .child(Icon::new(icon).size(px(12.0)))
                    .child(label),
            );
        }
        row
    }

    fn render_list(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if self.search.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search notes…"));
            cx.subscribe(&input, |this: &mut Self, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.query = input.read(cx).value().to_lowercase();
                    cx.notify();
                }
            })
            .detach();
            self.search = Some(input);
        }
        let q = self.query.clone();
        let rows: Vec<Stateful<Div>> = self
            .notes
            .iter()
            .filter(|n| {
                q.is_empty()
                    || n.title.to_lowercase().contains(&q)
                    || n.body.to_lowercase().contains(&q)
                    || n.tags.iter().any(|t| t.contains(q.trim_start_matches('#')))
            })
            .map(|n| {
                let slug = n.slug.clone();
                let active = self.selected.as_deref() == Some(n.slug.as_str());
                div()
                    .id(SharedString::from(format!("note-{}", n.slug)))
                    .role(Role::Button)
                    .aria_label(n.title.clone())
                    .h_flex()
                    .gap(px(8.0))
                    .items_center()
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(ui::R_CONTROL))
                    .cursor_pointer()
                    .when(active, |el| el.bg(theme::surface()))
                    .hover(|s: StyleRefinement| s.bg(theme::surface()))
                    .on_click(cx.listener(move |this, _e, _w, cx| this.open_note(slug.clone(), cx)))
                    .child(
                        Icon::new(if n.is_index {
                            Lucide::Kanban
                        } else {
                            Lucide::FileText
                        })
                        .size(px(12.0))
                        .text_color(if n.stamp.is_some() {
                            theme::accent()
                        } else {
                            theme::text_dim()
                        }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(12.0))
                            .text_color(if active {
                                theme::text_primary()
                            } else {
                                theme::text_muted()
                            })
                            .truncate()
                            .child(n.title.clone()),
                    )
            })
            .collect();
        ui::panel()
            .w(px(260.0))
            .flex_none()
            .h_full()
            .v_flex()
            .overflow_hidden()
            .child(
                div()
                    .h_flex()
                    .gap(px(8.0))
                    .items_center()
                    .px(px(10.0))
                    .h(px(40.0))
                    .border_b_1()
                    .border_color(theme::hairline())
                    .child(
                        Icon::new(IconName::Search)
                            .size(px(12.0))
                            .text_color(theme::text_dim()),
                    )
                    .child(
                        div().flex_1().child(
                            Input::new(self.search.as_ref().unwrap())
                                .appearance(false)
                                .accessibility_id("memory-search")
                                .aria_label("Search notes"),
                        ),
                    ),
            )
            .child(
                div()
                    .id("memory-notes")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .v_flex()
                    .gap(px(1.0))
                    .p(px(6.0))
                    .children(rows),
            )
            .child(
                div()
                    .p(px(8.0))
                    .border_t_1()
                    .border_color(theme::hairline())
                    .child(
                        Button::new("memory-new-note")
                            .ghost()
                            .small()
                            .w_full()
                            .icon(IconName::Plus)
                            .label("New note")
                            .on_click(cx.listener(|this, _e, window, cx| {
                                this.start_edit(None, window, cx)
                            })),
                    ),
            )
    }

    fn render_reader(&self, note: &Note, cx: &mut Context<Self>) -> Div {
        let stamp_run = note
            .stamp
            .as_ref()
            .and_then(|s| s.run.parse::<surge_core::RunId>().ok());
        let mission = stamp_run.and_then(|run| {
            self.state
                .read(cx)
                .run_prompt(&run)
                .map(|p| ui::headline(p, 60))
        });
        let slug = note.slug.clone();
        div()
            .v_flex()
            .gap(px(14.0))
            .child(
                div()
                    .h_flex()
                    .gap(px(10.0))
                    .items_start()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .v_flex()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_size(px(20.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_primary())
                                    .child(note.title.clone()),
                            )
                            .child(
                                div()
                                    .h_flex()
                                    .flex_wrap()
                                    .gap(px(6.0))
                                    .items_center()
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .text_color(theme::text_dim())
                                            .child(format!(".surge/memory/{}", note.file)),
                                    )
                                    .when_some(note.stamp.clone(), |el, stamp| {
                                        let label = match &mission {
                                            Some(m) => format!(
                                                "written by “{m}” · {}",
                                                stamp.node.replace('_', " ")
                                            ),
                                            None => format!(
                                                "written by a run · {}",
                                                stamp.node.replace('_', " ")
                                            ),
                                        };
                                        el.child(
                                            div()
                                                .id("note-provenance")
                                                .role(Role::Button)
                                                .when_some(stamp_run, |el, run| {
                                                    el.cursor_pointer().on_click(cx.listener(
                                                        move |_this, _e, _w, cx| {
                                                            cx.emit(MemoryAction::OpenRun(run));
                                                        },
                                                    ))
                                                })
                                                .child(ui::role_badge(label, Semantic::Agent)),
                                        )
                                    })
                                    .when(note.stamp.is_none() && !note.is_index, |el| {
                                        el.child(ui::role_badge("written by you", Semantic::You))
                                    })
                                    .when(note.is_index, |el| {
                                        el.child(ui::role_badge(
                                            "index · not sent to runs",
                                            Semantic::External,
                                        ))
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap(px(6.0))
                            .flex_none()
                            .child(
                                Button::new("note-edit")
                                    .outline()
                                    .small()
                                    .icon(Lucide::ScrollText)
                                    .label("Edit")
                                    .on_click(cx.listener(move |this, _e, window, cx| {
                                        this.start_edit(Some(slug.clone()), window, cx)
                                    })),
                            )
                            .child(if self.confirm_delete {
                                Button::new("note-delete-confirm")
                                    .danger()
                                    .small()
                                    .label("Delete for good")
                                    .on_click(
                                        cx.listener(|this, _e, _w, cx| this.delete_selected(cx)),
                                    )
                            } else {
                                Button::new("note-delete")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Delete)
                                    .tooltip("Delete this note")
                                    .on_click(cx.listener(|this, _e, _w, cx| {
                                        this.confirm_delete = true;
                                        cx.notify();
                                    }))
                            }),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.5))
                    .line_height(px(20.0))
                    .text_color(theme::text_primary())
                    .child(crate::markdown::render_markdown(
                        &memory_vault::wikilinks_to_markdown(&note.body),
                    )),
            )
    }

    fn render_editor(&self, cx: &mut Context<Self>) -> Div {
        let Some((slug, title, editor)) = &self.editing else {
            return div();
        };
        let is_new = slug.is_none();
        div()
            .v_flex()
            .gap(px(10.0))
            .h_full()
            .child(
                div()
                    .h_flex()
                    .gap(px(8.0))
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .h(px(36.0))
                            .px(px(10.0))
                            .rounded(px(ui::R_CONTROL))
                            .border_1()
                            .border_color(theme::hairline_strong())
                            .bg(theme::panel_deep())
                            .flex()
                            .items_center()
                            .child(Input::new(title).appearance(false).accessibility_id("note-title").aria_label("Note title")),
                    )
                    .child(
                        Button::new("note-cancel")
                            .ghost()
                            .small()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _e, _w, cx| {
                                this.editing = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("note-save")
                            .primary()
                            .small()
                            .icon(IconName::Check)
                            .label(if is_new { "Create note" } else { "Save" })
                            .on_click(cx.listener(|this, _e, _w, cx| this.save_edit(cx))),
                    ),
            )
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(theme::text_dim())
                    .child("Markdown. Link notes with [[note name]], tag with #tag. Every new run reads these notes."),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(320.0))
                    .rounded(px(ui::R_CONTROL + 2.0))
                    .border_1()
                    .border_color(theme::hairline())
                    .overflow_hidden()
                    .child(Editor::new(editor).aria_label("Note body").h(relative(1.0))),
            )
    }

    fn link_row(
        &self,
        id: String,
        title: String,
        slug: String,
        kind: &'static str,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        div()
            .id(SharedString::from(id))
            .role(Role::Button)
            .h_flex()
            .gap(px(8.0))
            .items_center()
            .px(px(8.0))
            .py(px(5.0))
            .rounded(px(ui::R_CONTROL))
            .cursor_pointer()
            .hover(|s: StyleRefinement| s.bg(theme::surface()))
            .on_click(cx.listener(move |this, _e, _w, cx| this.open_note(slug.clone(), cx)))
            .child(
                Icon::new(IconName::FileText)
                    .size(px(11.0))
                    .text_color(theme::text_dim()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_size(px(11.5))
                    .text_color(theme::text_primary())
                    .child(title),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(theme::text_dim())
                    .child(kind),
            )
    }

    fn render_links(&self, note: &Note, cx: &mut Context<Self>) -> Div {
        let outgoing: Vec<(String, Option<String>)> = note
            .links
            .iter()
            .map(|l| match memory_vault::resolve(&self.notes, l) {
                Some(i) => (
                    self.notes[i].slug.clone(),
                    Some(self.notes[i].title.clone()),
                ),
                None => (l.clone(), None),
            })
            .collect();
        let back: Vec<(String, String)> = memory_vault::backlinks(&self.notes, &note.slug)
            .into_iter()
            .map(|n| (n.slug.clone(), n.title.clone()))
            .collect();
        let related: Vec<(String, String)> = self
            .notes
            .iter()
            .filter(|n| {
                let linked = self.edges.iter().any(|&(a, b, kind)| {
                    kind == EdgeKind::Link
                        && ((self.notes[a].slug == note.slug && self.notes[b].slug == n.slug)
                            || (self.notes[b].slug == note.slug && self.notes[a].slug == n.slug))
                });
                n.slug != note.slug && !linked
            })
            .filter(|n| n.tags.iter().any(|t| note.tags.contains(t)))
            .map(|n| (n.slug.clone(), n.title.clone()))
            .collect();

        let mut panel = div().v_flex().gap(px(14.0));
        let mut out = div()
            .v_flex()
            .gap(px(1.0))
            .child(ui::section_label(format!("Links · {}", outgoing.len())));
        if outgoing.is_empty() {
            out = out.child(
                div()
                    .px(px(8.0))
                    .text_size(px(11.0))
                    .text_color(theme::text_dim())
                    .child("No links. Use [[note name]]."),
            );
        }
        for (i, (slug, title)) in outgoing.into_iter().enumerate() {
            match title {
                Some(t) => out = out.child(self.link_row(format!("out-{i}"), t, slug, "", cx)),
                None => {
                    out = out.child(
                        div()
                            .h_flex()
                            .gap(px(8.0))
                            .px(px(8.0))
                            .py(px(5.0))
                            .child(
                                Icon::new(IconName::FileText)
                                    .size(px(11.0))
                                    .text_color(theme::text_dim()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(theme::text_dim())
                                    .child(slug),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(theme::warning())
                                    .child("not written yet"),
                            ),
                    )
                },
            }
        }
        panel = panel.child(out);
        let mut backs = div()
            .v_flex()
            .gap(px(1.0))
            .child(ui::section_label(format!("Backlinks · {}", back.len())));
        if back.is_empty() {
            backs = backs.child(
                div()
                    .px(px(8.0))
                    .text_size(px(11.0))
                    .text_color(theme::text_dim())
                    .child("Nothing links here yet."),
            );
        }
        for (i, (slug, title)) in back.into_iter().enumerate() {
            backs = backs.child(self.link_row(format!("back-{i}"), title, slug, "", cx));
        }
        panel = panel.child(backs);
        if !note.tags.is_empty() {
            panel = panel.child(
                div()
                    .v_flex()
                    .gap(px(6.0))
                    .child(ui::section_label("Tags"))
                    .child(
                        div().h_flex().flex_wrap().gap(px(5.0)).children(
                            note.tags
                                .iter()
                                .map(|t| ui::role_badge(format!("#{t}"), Semantic::Plan)),
                        ),
                    ),
            );
        }
        if !related.is_empty() {
            let mut rel = div()
                .v_flex()
                .gap(px(1.0))
                .child(ui::section_label("Same tags"));
            for (i, (slug, title)) in related.into_iter().enumerate() {
                rel = rel.child(self.link_row(format!("rel-{i}"), title, slug, "", cx));
            }
            panel = panel.child(rel);
        }
        panel
    }

    fn render_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let list = self.render_list(window, cx);
        let center: AnyElement = if self.editing.is_some() {
            self.render_editor(cx).into_any_element()
        } else if let Some(note) = self.selected_note().cloned() {
            div()
                .flex()
                .gap(px(18.0))
                .items_start()
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(self.render_reader(&note, cx)),
                )
                .child(
                    ui::panel()
                        .w(px(260.0))
                        .flex_none()
                        .p(px(12.0))
                        .child(self.render_links(&note, cx)),
                )
                .into_any_element()
        } else {
            ui::panel()
                .child(
                    ui::empty_state(
                        "✎",
                        "No project notes yet",
                        "Notes in .surge/memory are read by every mission — invariants, gotchas, decisions that must not regress. Agents add to them as they learn; you can too.",
                    )
                    .child(
                        Button::new("memory-empty-new")
                            .primary()
                            .icon(IconName::Plus)
                            .label("Write the first note")
                            .on_click(cx.listener(|this, _e, window, cx| this.start_edit(None, window, cx))),
                    ),
                )
                .into_any_element()
        };
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .gap(px(18.0))
            .child(list)
            .child(
                div()
                    .id("memory-center")
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
                    .overflow_y_scroll()
                    .children(self.note.clone().map(|n| {
                        div()
                            .pb(px(10.0))
                            .text_size(px(11.5))
                            .text_color(theme::error())
                            .child(n)
                    }))
                    .child(center),
            )
    }

    fn render_graph(&self, cx: &mut Context<Self>) -> Div {
        if self.notes.is_empty() {
            return ui::panel().child(ui::empty_state(
                "◌",
                "The graph appears with your notes",
                "Each note is a dot; links and shared tags connect them.",
            ));
        }
        let mut degree = vec![0usize; self.notes.len()];
        for &(a, b, _) in &self.edges {
            degree[a] += 1;
            degree[b] += 1;
        }
        let pos = self.graph.clone();
        let edges = self.edges.clone();
        let link = theme::stroke(theme::accent());
        let tag = theme::graph_line();
        let wires = canvas(
            |_b, _w, _cx| {},
            move |bounds, _p, window, _cx| {
                for &(a, b, kind) in &edges {
                    let (Some(&(x1, y1)), Some(&(x2, y2))) = (pos.get(a), pos.get(b)) else {
                        continue;
                    };
                    let mut path =
                        PathBuilder::stroke(px(if kind == EdgeKind::Link { 1.4 } else { 1.0 }));
                    path.move_to(point(bounds.origin.x + px(x1), bounds.origin.y + px(y1)));
                    path.line_to(point(bounds.origin.x + px(x2), bounds.origin.y + px(y2)));
                    if let Ok(p) = path.build() {
                        window.paint_path(p, if kind == EdgeKind::Link { link } else { tag });
                    }
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        let selected = self.selected.clone();
        let dots: Vec<Stateful<Div>> = self
            .notes
            .iter()
            .enumerate()
            .map(|(i, note)| {
                let (x, y) = self.graph[i];
                let r = 5.0 + (degree[i] as f32).min(6.0) * 1.5;
                let active = selected.as_deref() == Some(note.slug.as_str());
                let color = if note.stamp.is_some() {
                    theme::accent()
                } else {
                    theme::violet()
                };
                let slug = note.slug.clone();
                div()
                    .id(SharedString::from(format!("graph-note-{}", note.slug)))
                    .role(Role::Button)
                    .aria_label(note.title.clone())
                    .absolute()
                    .left(px(x - 70.0))
                    .top(px(y - r))
                    .w(px(140.0))
                    .v_flex()
                    .items_center()
                    .gap(px(4.0))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _e, _w, cx| this.open_note(slug.clone(), cx)))
                    .child(
                        div()
                            .size(px(r * 2.0))
                            .rounded_full()
                            .bg(theme::tint(color))
                            .border_1()
                            .border_color(if active {
                                theme::text_primary()
                            } else {
                                theme::stroke(color)
                            })
                            .child(div().size_full().rounded_full().bg(color.opacity(0.55))),
                    )
                    .child(
                        div()
                            .max_w(px(140.0))
                            .truncate()
                            .text_size(px(10.5))
                            .text_color(if active {
                                theme::text_primary()
                            } else {
                                theme::text_muted()
                            })
                            .child(note.title.clone()),
                    )
            })
            .collect();
        ui::panel()
            .v_flex()
            .overflow_hidden()
            .child(
                div()
                    .h_flex()
                    .gap(px(14.0))
                    .px(px(14.0))
                    .py(px(10.0))
                    .border_b_1()
                    .border_color(theme::hairline())
                    .child(ui::section_label(format!(
                        "{} notes · {} connections",
                        self.notes.len(),
                        self.edges.len()
                    )))
                    .child(div().flex_1())
                    .child(
                        div()
                            .h_flex()
                            .gap(px(6.0))
                            .items_center()
                            .child(div().w(px(16.0)).h(px(2.0)).bg(link))
                            .child(ui::meta("link")),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap(px(6.0))
                            .items_center()
                            .child(div().w(px(16.0)).h(px(1.0)).bg(tag))
                            .child(ui::meta("same tag")),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap(px(6.0))
                            .items_center()
                            .child(ui::status_dot(theme::accent()))
                            .child(ui::meta("written by a run")),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap(px(6.0))
                            .items_center()
                            .child(ui::status_dot(theme::violet()))
                            .child(ui::meta("written by you")),
                    ),
            )
            .child(
                div()
                    .id("memory-graph")
                    .overflow_scroll()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .relative()
                            .w(px(GRAPH_W))
                            .h(px(GRAPH_H))
                            .child(ui::grid_backdrop(24.0))
                            .child(wires)
                            .children(dots),
                    ),
            )
    }

    fn render_learned(&self, cx: &mut Context<Self>) -> Div {
        let body: AnyElement = match &self.claims {
            None => ui::meta("Reading…").into_any_element(),
            Some(Err(e)) => ui::panel().child(ui::empty_state("◌", "Learned memory is unavailable", e.clone())).into_any_element(),
            Some(Ok(claims)) if claims.is_empty() => ui::panel()
                .child(ui::empty_state(
                    "◌",
                    "Nothing learned yet",
                    "When a run fails in a way worth remembering, Surge records it here with its source.",
                ))
                .into_any_element(),
            Some(Ok(claims)) => {
                let has_project = self.state.read(cx).project_path.is_some();
                div()
                    .v_flex()
                    .gap(px(8.0))
                    .children(claims.iter().enumerate().map(|(i, c)| {
                        let claim = c.clone();
                        ui::panel()
                            .v_flex()
                            .gap(px(6.0))
                            .p(px(12.0))
                            .child(
                                div()
                                    .h_flex()
                                    .gap(px(8.0))
                                    .items_start()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .text_size(px(12.0))
                                            .line_height(px(18.0))
                                            .text_color(theme::text_primary())
                                            .child(c.text.clone()),
                                    )
                                    .child(if c.verified {
                                        ui::role_badge("verified", Semantic::Verified)
                                    } else {
                                        ui::role_badge("unverified", Semantic::External)
                                    }),
                            )
                            .child(
                                div()
                                    .h_flex()
                                    .gap(px(8.0))
                                    .items_center()
                                    .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(10.5)).text_color(theme::text_dim()).child(format!("{} · {}", c.confidence, c.source)))
                                    .when(has_project, |el| {
                                        el.child(
                                            Button::new(SharedString::from(format!("claim-keep-{i}")))
                                                .ghost()
                                                .small()
                                                .icon(IconName::Plus)
                                                .label("Keep as project note")
                                                .on_click(cx.listener(move |this, _e, _w, cx| this.keep_claim(&claim, cx))),
                                        )
                                    }),
                            )
                    }))
                    .into_any_element()
            },
        };
        div()
            .v_flex()
            .gap(px(10.0))
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(theme::text_muted())
                    .child("What Surge recorded from past runs, across projects. Unverified until a check confirms it; keep what matters as a project note."),
            )
            .child(body)
    }
}

impl Render for MemoryScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let notes = self.notes.iter().filter(|n| !n.is_index).count();
        let written_by_runs = self.notes.iter().filter(|n| n.stamp.is_some()).count();
        let subtitle = Some(SharedString::from(if notes == 0 {
            "What every mission knows about this project before it starts.".to_string()
        } else {
            format!(
                "{notes} notes · {written_by_runs} written by missions · read by every new mission"
            )
        }));
        let body: AnyElement = match self.mode {
            Mode::Notes => self.render_notes(window, cx).into_any_element(),
            Mode::Graph => div()
                .id("memory-graph-scroll")
                .flex_1()
                .overflow_y_scroll()
                .child(self.render_graph(cx))
                .into_any_element(),
            Mode::Learned => div()
                .id("memory-learned")
                .flex_1()
                .overflow_y_scroll()
                .child(self.render_learned(cx))
                .into_any_element(),
        };
        div()
            .size_full()
            .v_flex()
            .gap(px(16.0))
            .bg(theme::background())
            .px(px(24.0))
            .pt(px(22.0))
            .pb(px(20.0))
            .child(ui::page_header(
                "Memory",
                subtitle,
                div()
                    .h_flex()
                    .gap(px(8.0))
                    .child(self.render_modes(cx))
                    .child(
                        Button::new("memory-reload")
                            .ghost()
                            .small()
                            .icon(IconName::RefreshCw)
                            .tooltip("Re-read the notes from disk")
                            .on_click(cx.listener(|this, _e, _w, cx| {
                                this.claims = None;
                                if this.mode == Mode::Learned {
                                    this.load_claims();
                                }
                                this.reload(cx);
                            })),
                    ),
            ))
            .child(body)
    }
}
