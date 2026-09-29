//! Native read-only review of a run checkout.

use std::path::Path;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Editor, EditorState, TextDecoration, TextDecorationCollection};
use gpui_kit::component::{Icon, IconName, Sizable, StyledExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use surge_core::RunId;

use crate::theme;

const MAX_FILES: usize = 200;
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_PATCH_BYTES: usize = 256 * 1024;
const MAX_TOTAL_BYTES: usize = 2 * 1024 * 1024;

/// What happened to a file, read from its patch header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileChange {
    Added,
    Modified,
    Deleted,
}

/// Lines added / removed and the change kind, counted once when the diff
/// is read (never per frame).
fn patch_stats(patch: &str) -> (usize, usize, FileChange) {
    let mut added = 0;
    let mut removed = 0;
    let mut kind = FileChange::Modified;
    for line in patch.lines() {
        if line.starts_with("new file mode") {
            kind = FileChange::Added;
        } else if line.starts_with("deleted file mode") {
            kind = FileChange::Deleted;
        } else if line.starts_with('+') && !line.starts_with("+++") {
            added += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            removed += 1;
        }
    }
    (added, removed, kind)
}

struct ChangedFile {
    path: String,
    patch: String,
    added: usize,
    removed: usize,
    change: FileChange,
    /// Surge's own working file (plan, contract artifact, agent settings),
    /// not part of the application being built.
    working: bool,
}

/// Paths Surge itself writes into a run checkout, derived from data: every
/// artifact path a profile contract declares (plans, specs, verification
/// reports) and every agent startup file a registry entry declares, plus
/// Surge's `.surge/` directory. The Changes view separates these from the
/// application's own changes instead of mixing them together.
fn surge_working_paths() -> std::collections::BTreeSet<String> {
    let mut paths = std::collections::BTreeSet::new();
    if let Ok(profiles) = surge_orchestrator::profile_loader::ProfileRegistry::load() {
        for entry in profiles.list() {
            for outcome in &entry.profile.outcomes {
                paths.extend(outcome.required_artifacts.iter().cloned());
                paths.extend(outcome.produced_artifacts.iter().map(|a| a.path.clone()));
            }
        }
    }
    for entry in surge_acp::Registry::builtin().list() {
        paths.extend(entry.settings_files.iter().map(|f| f.path.clone()));
    }
    paths
}

/// What "Keep app changes" did.
#[derive(Debug, PartialEq, Eq)]
enum KeepOutcome {
    /// Committed on the run branch and fast-forwarded into the project.
    Merged { branch: String, target: String },
    /// Committed on the run branch; the project could not be fast-forwarded
    /// safely, so it was left untouched.
    Committed { branch: String, reason: String },
}

/// Commit only `app_paths` in the run checkout, then fast-forward the
/// project's checked-out branch to that commit when — and only when — the
/// project has no local changes and the move is a pure fast-forward
/// (`git merge --ff-only`). Checkout is `safe`, never `force`: nothing of
/// the operator's is overwritten. Surge's working files are never committed.
fn keep_app_changes(
    run_checkout: &Path,
    project: &Path,
    app_paths: &[String],
    message: &str,
) -> Result<KeepOutcome, String> {
    use git2::{Repository, Signature, Status, StatusOptions};
    let fail = |error: git2::Error| error.message().to_string();
    if app_paths.is_empty() {
        return Err("There are no app changes to keep.".into());
    }
    let run = Repository::open(run_checkout).map_err(fail)?;
    let branch = run
        .head()
        .map_err(fail)?
        .shorthand()
        .unwrap_or("HEAD")
        .to_string();
    let mut index = run.index().map_err(fail)?;
    for path in app_paths {
        let relative = Path::new(path);
        if run_checkout.join(relative).exists() {
            index.add_path(relative).map_err(fail)?;
        } else {
            index.remove_path(relative).map_err(fail)?;
        }
    }
    index.write().map_err(fail)?;
    let tree = run
        .find_tree(index.write_tree().map_err(fail)?)
        .map_err(fail)?;
    let parent = run.head().and_then(|h| h.peel_to_commit()).map_err(fail)?;
    let signature = Signature::now("Surge", "surge@localhost").map_err(fail)?;
    let commit = run
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &[&parent],
        )
        .map_err(fail)?;

    let repo = Repository::open(project).map_err(fail)?;
    let head = repo.head().map_err(fail)?;
    let Some(target_ref) = head.name().filter(|_| head.is_branch()).map(str::to_string) else {
        return Ok(KeepOutcome::Committed {
            branch,
            reason: "the project is not on a branch".into(),
        });
    };
    let target = head.shorthand().unwrap_or("HEAD").to_string();
    let mut options = StatusOptions::new();
    options.include_untracked(false);
    let dirty = repo
        .statuses(Some(&mut options))
        .map_err(fail)?
        .iter()
        .any(|entry| entry.status() != Status::CURRENT && !entry.status().is_ignored());
    if dirty {
        return Ok(KeepOutcome::Committed {
            branch,
            reason: format!("the project has uncommitted changes on {target}"),
        });
    }
    let current = head.peel_to_commit().map_err(fail)?.id();
    if !(current == commit || repo.graph_descendant_of(commit, current).map_err(fail)?) {
        return Ok(KeepOutcome::Committed {
            branch,
            reason: format!("{target} moved since this run started"),
        });
    }
    // Update the working tree first (safe: refuses to overwrite anything the
    // operator changed), then move the branch — `git merge --ff-only` order.
    let new_tree = repo.find_commit(commit).map_err(fail)?.into_object();
    repo.checkout_tree(&new_tree, Some(git2::build::CheckoutBuilder::new().safe()))
        .map_err(fail)?;
    repo.reference(
        &target_ref,
        commit,
        true,
        &format!("surge: keep app changes from {branch}"),
    )
    .map_err(fail)?;
    Ok(KeepOutcome::Merged { branch, target })
}

fn is_working_file(path: &str, working: &std::collections::BTreeSet<String>) -> bool {
    path.starts_with(".surge/") || working.contains(path)
}

struct Changes {
    base_label: String,
    files: Vec<ChangedFile>,
    truncated: bool,
}

fn refreshed_selection(path: Option<&str>, files: &[ChangedFile]) -> usize {
    path.and_then(|path| files.iter().position(|file| file.path == path))
        .unwrap_or(0)
}

fn read_changes(path: &Path, base: Option<git2::Oid>) -> Result<Changes, String> {
    let repo = git2::Repository::open(path).map_err(|error| error.to_string())?;
    let (oid, base_label) = match base {
        Some(base) => (
            Some(base),
            format!("Run changes against captured base {base}"),
        ),
        None => match repo.head() {
            Ok(head) => {
                let oid = head
                    .peel_to_commit()
                    .map_err(|error| error.to_string())?
                    .id();
                (
                    Some(oid),
                    format!("Working changes against HEAD {oid} · committed run changes excluded"),
                )
            },
            Err(error) if error.code() == git2::ErrorCode::UnbornBranch => {
                (None, "Working changes before first commit".into())
            },
            Err(error) => return Err(error.to_string()),
        },
    };
    let tree = oid
        .map(|oid| repo.find_commit(oid).and_then(|commit| commit.tree()))
        .transpose()
        .map_err(|error| error.to_string())?;
    let mut options = git2::DiffOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(true)
        .max_size(MAX_FILE_BYTES as i64);
    let diff = repo
        .diff_tree_to_workdir_with_index(tree.as_ref(), Some(&mut options))
        .map_err(|error| error.to_string())?;
    let mut files = Vec::new();
    let mut total = 0;
    let mut truncated = diff.deltas().len() > MAX_FILES;
    for (index, delta) in diff.deltas().take(MAX_FILES).enumerate() {
        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .map_or_else(
                || "(unknown path)".into(),
                |path| path.to_string_lossy().into_owned(),
            );
        let patch = render_patch(&diff, index)?;
        if total + patch.len() > MAX_TOTAL_BYTES {
            truncated = true;
            break;
        }
        total += patch.len();
        let (added, removed, change) = patch_stats(&patch);
        files.push(ChangedFile {
            path,
            patch,
            added,
            removed,
            change,
            working: false,
        });
    }
    Ok(Changes {
        base_label,
        files,
        truncated,
    })
}

fn render_patch(diff: &git2::Diff<'_>, index: usize) -> Result<String, String> {
    if diff.get_delta(index).is_some_and(|delta| {
        delta.old_file().size() > MAX_FILE_BYTES || delta.new_file().size() > MAX_FILE_BYTES
    }) {
        return Ok("File larger than 1 MiB; content omitted.".into());
    }
    let Some(mut patch) = git2::Patch::from_diff(diff, index).map_err(|error| error.to_string())?
    else {
        return Ok("Binary change or file larger than 1 MiB; content omitted.".into());
    };
    let mut text = String::new();
    let mut truncated = false;
    let printed = patch.print(&mut |_delta, _hunk, line| {
        let content = String::from_utf8_lossy(line.content());
        if text.len() + content.len() + 1 > MAX_PATCH_BYTES {
            truncated = true;
            return false;
        }
        if matches!(line.origin(), '+' | '-' | ' ') {
            text.push(line.origin());
        }
        text.push_str(&content);
        true
    });
    if truncated {
        text.push_str("\n[Patch truncated at 256 KiB]\n");
    } else {
        printed.map_err(|error| error.to_string())?;
    }
    if text.is_empty() {
        text.push_str("File metadata changed; no text patch.");
    }
    Ok(text)
}

fn diff_decorations(text: &str) -> Vec<TextDecoration> {
    let mut offset = 0;
    let mut in_hunk = false;
    let mut decorations = Vec::new();
    for line in text.split_inclusive('\n') {
        let color = if line.starts_with("@@") {
            in_hunk = true;
            Some(theme::accent())
        } else if in_hunk && line.starts_with('+') {
            Some(theme::success())
        } else if in_hunk && line.starts_with('-') {
            Some(theme::error())
        } else {
            None
        };
        if let Some(color) = color {
            decorations.push(TextDecoration::new(
                offset..offset + line.len(),
                HighlightStyle {
                    color: Some(color),
                    ..Default::default()
                },
            ));
        }
        offset += line.len();
    }
    decorations
}

pub(super) struct ChangesView {
    run_id: RunId,
    generation: u64,
    loading: bool,
    note: String,
    files: Vec<ChangedFile>,
    selected: usize,
    editor: Entity<EditorState>,
    decorations: TextDecorationCollection,
    editor_dirty: bool,
    /// Result of the last "Keep app changes".
    keep_note: Option<String>,
    /// Surge's own files (plans, reports) are folded away until asked for.
    show_working: bool,
}

impl ChangesView {
    pub(super) fn new(run_id: RunId, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| EditorState::new(window, cx).language("diff"));
        let decorations = editor.update(cx, |editor, cx| {
            editor.create_decorations_collection(Vec::new(), cx)
        });
        let mut view = Self {
            run_id,
            generation: 0,
            loading: false,
            note: String::new(),
            files: Vec::new(),
            selected: 0,
            editor,
            decorations,
            editor_dirty: true,
            keep_note: None,
            show_working: false,
        };
        view.refresh(cx);
        view
    }

    fn keep(&mut self, cx: &mut Context<Self>) {
        let run_id = self.run_id;
        let app_paths: Vec<String> = self
            .files
            .iter()
            .filter(|f| !f.working)
            .map(|f| f.path.clone())
            .collect();
        self.keep_note = Some("Keeping app changes…".into());
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = async {
                let home = surge_core::home::surge_home_dir().ok_or("Surge home unavailable")?;
                let checkout = super::load_result_folder(home.clone(), run_id).await?;
                let capture =
                    surge_persistence::runs::Storage::inspect_existing_run_capture(home.clone(), run_id)
                        .await
                        .map_err(|e| e.to_string())?
                        .ok_or("This run has no recorded project to keep changes into")?;
                let project = capture.fields().repository.clone();
                let prompt = surge_persistence::runs::Storage::inspect_existing_run_events(
                    home.join("runs"),
                    run_id,
                )
                .await
                .ok()
                .and_then(|events| {
                    events.iter().find_map(|e| match &e.payload.payload {
                        surge_core::EventPayload::RunStarted { initial_prompt, .. } => {
                            Some(initial_prompt.clone())
                        },
                        _ => None,
                    })
                })
                .unwrap_or_default();
                let message = format!(
                    "{}\n\nBuilt by Surge (run {run_id}).",
                    crate::ui::headline(&prompt, 72)
                );
                tokio::task::spawn_blocking(move || {
                    keep_app_changes(&checkout, &project, &app_paths, &message)
                })
                .await
                .map_err(|e| e.to_string())?
            }
            .await;
            cx.update(|cx| {
                let _ = this.update(cx, |view, cx| {
                    view.keep_note = Some(match result {
                        Ok(KeepOutcome::Merged { branch, target }) => {
                            format!("Kept: committed on {branch} and fast-forwarded {target}.")
                        },
                        Ok(KeepOutcome::Committed { branch, reason }) => format!(
                            "Committed on {branch}. Not merged because {reason} — run `git merge {branch}` when ready."
                        ),
                        Err(error) => format!("Could not keep changes: {error}"),
                    });
                    view.refresh(cx);
                });
            });
        })
        .detach();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let run_id = self.run_id;
        self.loading = true;
        self.note = "Reading changes…".into();
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = load_changes(run_id).await;
            cx.update(|cx| {
                let _ = this.update(cx, |view, cx| {
                    if generation != view.generation {
                        return;
                    }
                    view.loading = false;
                    let selected_path = view.files.get(view.selected).map(|file| file.path.clone());
                    view.selected = 0;
                    view.files.clear();
                    view.note = match result {
                        Ok(changes) => {
                            view.selected =
                                refreshed_selection(selected_path.as_deref(), &changes.files);
                            view.files = changes.files;
                            format!(
                                "{}{}",
                                changes.base_label,
                                if changes.truncated {
                                    " · Partial view: 200 files / 2 MiB limit reached"
                                } else {
                                    ""
                                }
                            )
                        },
                        Err(error) => format!("Cannot load changes: {error}"),
                    };
                    view.editor_dirty = true;
                    cx.notify();
                });
            });
        })
        .detach();
    }
}

async fn load_changes(run_id: RunId) -> Result<Changes, String> {
    let home = surge_core::home::surge_home_dir().ok_or("Surge home unavailable")?;
    let path = super::load_result_folder(home.clone(), run_id).await?;
    let capture = surge_persistence::runs::Storage::inspect_existing_run_capture(home, run_id)
        .await
        .map_err(|error| error.to_string())?;
    tokio::task::spawn_blocking(move || {
        let working = surge_working_paths();
        let base = if let Some(capture) = capture {
            let fields = capture.fields();
            let expected = if fields.planning_run == run_id {
                &fields.planning_worktree
            } else if fields.implementation_run == run_id {
                &fields.implementation_worktree
            } else {
                return Err("Captured run identity does not match".into());
            };
            let canonical = |path: &Path| path.canonicalize().map_err(|error| error.to_string());
            let repo = git2::Repository::open(&path).map_err(|error| error.to_string())?;
            if canonical(&path)? != canonical(expected)?
                || canonical(repo.commondir())? != canonical(&fields.git_common_dir)?
            {
                return Err("Run checkout no longer matches its captured repository".into());
            }
            Some(git2::Oid::from_str(&fields.base_commit).map_err(|error| error.to_string())?)
        } else {
            None
        };
        let mut changes = read_changes(&path, base)?;
        for file in &mut changes.files {
            file.working = is_working_file(&file.path, &working);
        }
        // Application changes first; Surge's working files after them.
        changes.files.sort_by_key(|file| file.working);
        Ok(changes)
    })
    .await
    .map_err(|error| error.to_string())?
}

impl Render for ChangesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.editor_dirty {
            let text = self
                .files
                .get(self.selected)
                .map_or("No file selected", |file| file.patch.as_str());
            self.editor
                .update(cx, |editor, cx| editor.set_value(text, window, cx));
            self.decorations.set(diff_decorations(text), cx);
            self.editor_dirty = false;
        }
        let app_count = self.files.iter().filter(|f| !f.working).count();
        let working_count = self.files.len() - app_count;
        let (app_added, app_removed) = self
            .files
            .iter()
            .filter(|f| !f.working)
            .fold((0, 0), |(a, r), f| (a + f.added, r + f.removed));

        let mut list = div()
            .id("changed-files")
            .w(px(260.0))
            .flex_shrink_0()
            .overflow_y_scroll()
            .v_flex()
            .gap(px(1.0))
            .pr(px(4.0));
        if app_count > 0 {
            list = list.child(
                div()
                    .px(px(8.0))
                    .pb(px(4.0))
                    .child(crate::ui::section_label(format!("App · {app_count}"))),
            );
        }
        for (index, file) in self.files.iter().enumerate() {
            if file.working && (index == 0 || !self.files[index - 1].working) {
                let open = self.show_working;
                list = list.child(
                    div()
                        .id("toggle-working-files")
                        .role(Role::Button)
                        .h_flex()
                        .gap(px(6.0))
                        .items_center()
                        .px(px(8.0))
                        .pt(px(12.0))
                        .pb(px(4.0))
                        .cursor_pointer()
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.show_working = !view.show_working;
                            cx.notify();
                        }))
                        .child(
                            Icon::new(if open {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .size(px(11.0))
                            .text_color(theme::text_dim()),
                        )
                        .child(crate::ui::section_label(format!(
                            "Surge files · {working_count}"
                        ))),
                );
            }
            if file.working && !self.show_working {
                continue;
            }
            list = list.child(self.render_file_row(index, file, cx));
        }

        div()
            .flex_1()
            .min_h_0()
            .v_flex()
            .gap(px(10.0))
            .px(px(20.0))
            .py(px(12.0))
            .child(
                div()
                    .h_flex()
                    .gap(px(10.0))
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .v_flex()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .h_flex()
                                    .gap(px(8.0))
                                    .items_center()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::text_primary())
                                    .child(format!(
                                        "{app_count} app file{}",
                                        if app_count == 1 { "" } else { "s" }
                                    ))
                                    .child(
                                        div()
                                            .text_color(theme::success())
                                            .child(format!("+{app_added}")),
                                    )
                                    .child(
                                        div()
                                            .text_color(theme::error())
                                            .child(format!("−{app_removed}")),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(theme::text_dim())
                                    .truncate()
                                    .child(self.note.clone()),
                            ),
                    )
                    .children(self.keep_note.clone().map(|note| {
                        div()
                            .max_w(px(360.0))
                            .text_size(px(11.0))
                            .text_color(theme::accent())
                            .child(note)
                    }))
                    .child(
                        Button::new("refresh-changes")
                            .ghost()
                            .small()
                            .icon(IconName::RefreshCw)
                            .tooltip("Refresh")
                            .on_click(cx.listener(|view, _, _, cx| view.refresh(cx))),
                    )
                    .when(app_count > 0, |el| {
                        el.child(
                            Button::new("keep-app-changes")
                                .primary()
                                .small()
                                .icon(IconName::Check)
                                .label("Keep app changes")
                                .tooltip("Commit the app files and bring them into your project")
                                .on_click(cx.listener(|view, _, _, cx| view.keep(cx))),
                        )
                    }),
            )
            .child(if self.files.is_empty() {
                crate::ui::empty_state(
                    "±",
                    if self.loading {
                        "Loading changes…"
                    } else if self.note.starts_with("Cannot") {
                        "Changes unavailable"
                    } else {
                        "No changes"
                    },
                    if self.loading {
                        ""
                    } else {
                        "Nothing differs from the base this run started from."
                    },
                )
                .flex_1()
                .into_any_element()
            } else {
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap(px(10.0))
                    .child(list)
                    .child(
                        div()
                            .debug_selector(|| "run-diff-editor".into())
                            .flex_1()
                            .min_w_0()
                            .rounded(px(crate::ui::R_CONTROL + 2.0))
                            .border_1()
                            .border_color(theme::hairline())
                            .overflow_hidden()
                            .child(
                                Editor::new(&self.editor)
                                    .readonly(true)
                                    .aria_label("Run file diff")
                                    .h(relative(1.0)),
                            ),
                    )
                    .into_any_element()
            })
    }
}

impl ChangesView {
    fn render_file_row(
        &self,
        index: usize,
        file: &ChangedFile,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let selected = index == self.selected;
        let (dir, name) = match file.path.rsplit_once('/') {
            Some((dir, name)) => (format!("{dir}/"), name.to_string()),
            None => (String::new(), file.path.clone()),
        };
        let (letter, tone) = match file.change {
            FileChange::Added => ("A", theme::success()),
            FileChange::Modified => ("M", theme::warning()),
            FileChange::Deleted => ("D", theme::error()),
        };
        div()
            .id(SharedString::from(format!("changed-file-{index}")))
            .role(Role::Button)
            .aria_label(file.path.clone())
            .h_flex()
            .gap(px(8.0))
            .items_center()
            .px(px(8.0))
            .py(px(5.0))
            .rounded(px(crate::ui::R_CONTROL))
            .cursor_pointer()
            .when(selected, |el| el.bg(theme::surface()))
            .hover(|s: StyleRefinement| s.bg(theme::surface()))
            .on_click(cx.listener(move |view, _, _, cx| {
                view.selected = index;
                view.editor_dirty = true;
                cx.notify();
            }))
            .child(
                div()
                    .w(px(12.0))
                    .flex_none()
                    .text_size(px(10.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(tone)
                    .child(letter),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .h_flex()
                    .overflow_hidden()
                    .text_size(px(11.5))
                    .child(
                        div()
                            .text_color(if selected {
                                theme::text_primary()
                            } else {
                                theme::text_muted()
                            })
                            .flex_none()
                            .child(name),
                    )
                    .child(
                        div()
                            .pl(px(6.0))
                            .text_color(theme::text_dim())
                            .truncate()
                            .child(dir),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .h_flex()
                    .gap(px(4.0))
                    .text_size(px(10.0))
                    .when(file.added > 0, |el| {
                        el.child(
                            div()
                                .text_color(theme::success())
                                .child(format!("+{}", file.added)),
                        )
                    })
                    .when(file.removed > 0, |el| {
                        el.child(
                            div()
                                .text_color(theme::error())
                                .child(format!("−{}", file.removed)),
                        )
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .expect("git")
            .success();
        assert!(ok, "git {args:?}");
    }

    fn project_with_run_worktree() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("app");
        std::fs::create_dir_all(&project).unwrap();
        git(&project, &["init", "-q", "-b", "main"]);
        std::fs::write(project.join("README.md"), "app\n").unwrap();
        git(&project, &["add", "."]);
        git(
            &project,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "base",
            ],
        );
        let run = tmp.path().join("run");
        git(
            &project,
            &[
                "worktree",
                "add",
                "-q",
                run.to_str().unwrap(),
                "-b",
                "surge/run-x",
            ],
        );
        std::fs::write(run.join("index.html"), "<p>timer</p>\n").unwrap();
        std::fs::write(run.join("spec.md"), "# spec\n").unwrap();
        (tmp, project, run)
    }

    #[test]
    fn keep_commits_only_app_files_and_fast_forwards_a_clean_project() {
        let (_tmp, project, run) = project_with_run_worktree();
        let outcome =
            super::keep_app_changes(&run, &project, &["index.html".into()], "Timer").unwrap();
        assert_eq!(
            outcome,
            super::KeepOutcome::Merged {
                branch: "surge/run-x".into(),
                target: "main".into()
            }
        );
        assert!(
            project.join("index.html").is_file(),
            "app file reached the project"
        );
        assert!(!project.join("spec.md").exists(), "working file stayed out");
    }

    #[test]
    fn keep_never_touches_a_project_with_local_changes() {
        let (_tmp, project, run) = project_with_run_worktree();
        std::fs::write(project.join("README.md"), "operator edit\n").unwrap();
        let outcome =
            super::keep_app_changes(&run, &project, &["index.html".into()], "Timer").unwrap();
        assert!(
            matches!(outcome, super::KeepOutcome::Committed { .. }),
            "{outcome:?}"
        );
        assert_eq!(
            std::fs::read_to_string(project.join("README.md")).unwrap(),
            "operator edit\n"
        );
        assert!(!project.join("index.html").exists());
    }

    #[test]
    fn working_files_come_from_profile_contracts_and_agent_settings() {
        let working = super::surge_working_paths();
        for path in [
            "spec.md",
            "flow.toml",
            "verification-report.toml",
            ".claude/settings.json",
        ] {
            assert!(
                super::is_working_file(path, &working),
                "{path} is a Surge working file"
            );
        }
        assert!(super::is_working_file(".surge/plan.md", &working));
        for path in ["index.html", "timer.js", "test.js", "README.md"] {
            assert!(
                !super::is_working_file(path, &working),
                "{path} is application code"
            );
        }
    }

    #[test]
    fn refresh_retains_selected_path_after_reordering_and_falls_back_if_removed() {
        let files = ["app.js", "README.md", "styles.css"].map(|path| super::ChangedFile {
            path: path.into(),
            patch: String::new(),
            added: 0,
            removed: 0,
            change: super::FileChange::Modified,
            working: false,
        });
        assert_eq!(super::refreshed_selection(Some("README.md"), &files), 1);
        assert_eq!(super::refreshed_selection(Some("removed"), &files), 0);
        assert_eq!(super::refreshed_selection(None, &files), 0);
        assert_eq!(super::refreshed_selection(Some("README.md"), &[]), 0);
    }

    #[test]
    fn patch_stats_count_content_lines_not_headers() {
        let patch = "diff --git a/x b/x\nnew file mode 100644\n--- /dev/null\n+++ b/x\n@@ -0,0 +1,2 @@\n+one\n+two\n";
        assert_eq!(super::patch_stats(patch), (2, 0, super::FileChange::Added));
        let patch = "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new\n context\n";
        assert_eq!(
            super::patch_stats(patch),
            (1, 1, super::FileChange::Modified)
        );
        assert_eq!(
            super::patch_stats("deleted file mode 100644\n--- a/x\n+++ /dev/null\n-gone\n").2,
            super::FileChange::Deleted
        );
    }

    #[test]
    fn unborn_repository_shows_files_before_first_commit() {
        let root = tempfile::tempdir().unwrap();
        git2::Repository::init(root.path()).unwrap();
        std::fs::write(root.path().join("new.txt"), "first content\n").unwrap();
        let changes = super::read_changes(root.path(), None).unwrap();
        assert_eq!(changes.base_label, "Working changes before first commit");
        assert_eq!(changes.files[0].path, "new.txt");
        assert!(changes.files[0].patch.contains("+first content"));
        assert!(super::read_changes(root.path(), Some(git2::Oid::zero())).is_err());
    }

    #[test]
    fn diff_colors_use_utf8_ranges_and_distinguish_headers_from_hunk_content() {
        crate::theme::init();
        let text = "--- a/file\n+++ b/file\n@@ -1 +1 @@\n-старое\n+++новое\n context\n";
        let decorations = super::diff_decorations(text);
        assert_eq!(decorations.len(), 3);
        assert_eq!(&text[decorations[0].range.clone()], "@@ -1 +1 @@\n");
        assert_eq!(&text[decorations[1].range.clone()], "-старое\n");
        assert_eq!(&text[decorations[2].range.clone()], "+++новое\n");
        assert_eq!(decorations[1].style.color, Some(crate::theme::error()));
        assert_eq!(decorations[2].style.color, Some(crate::theme::success()));
    }

    #[test]
    fn native_diff_editor_renders_readonly_patch() {
        use gpui_kit::{AppContext as _, Modifiers, TestAppContext};
        let mut cx = TestAppContext::single();
        cx.update(gpui_kit::init);
        crate::theme::init();
        let (view, window) = cx.add_window_view(|window, cx| {
            let editor = cx.new(|cx| super::EditorState::new(window, cx).language("diff"));
            let decorations = editor.update(cx, |editor, cx| {
                editor.create_decorations_collection(Vec::new(), cx)
            });
            super::ChangesView {
                run_id: surge_core::RunId::new(),
                generation: 0,
                loading: false,
                note: "Run changes against captured base".into(),
                files: vec![super::ChangedFile {
                    path: "app.rs".into(),
                    patch: "+addition\n-deletion\n".into(),
                    added: 1,
                    removed: 1,
                    change: super::FileChange::Modified,
                    working: false,
                }],
                selected: 0,
                decorations,
                editor,
                editor_dirty: true,
                keep_note: None,
                show_working: false,
            }
        });
        window.update(|window, cx| window.draw(cx).clear(cx));
        let bounds = window.debug_bounds("run-diff-editor").unwrap();
        window.simulate_click(bounds.center(), Modifiers::default());
        window.simulate_input("must not modify diff");
        view.update(window, |view, cx| {
            assert_eq!(
                view.editor.read(cx).value().as_ref(),
                "+addition\n-deletion\n"
            );
        });
    }

    fn commit(repo: &git2::Repository) -> git2::Oid {
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let signature = git2::Signature::now("test", "test@example.com").unwrap();
        let parent = repo.head().ok().map(|head| head.peel_to_commit().unwrap());
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            "fixture",
            &tree,
            &parent.iter().collect::<Vec<_>>(),
        )
        .unwrap()
    }

    #[test]
    fn changes_include_committed_staged_unstaged_and_untracked_files() {
        let root = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(root.path()).unwrap();
        std::fs::write(root.path().join("tracked"), "base\n").unwrap();
        let base = commit(&repo);
        std::fs::write(root.path().join("committed"), "committed content\n").unwrap();
        commit(&repo);
        std::fs::write(root.path().join("staged"), "staged content\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("staged")).unwrap();
        index.write().unwrap();
        std::fs::write(root.path().join("tracked"), "working content\n").unwrap();
        std::fs::write(root.path().join("untracked"), "new content\n").unwrap();
        let result = super::read_changes(root.path(), Some(base)).unwrap();
        assert_eq!(
            result
                .files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            ["committed", "staged", "tracked", "untracked"]
        );
        assert!(result.files[2].patch.contains("+working content"));
        assert!(result.files[2].patch.contains("-base"));
    }

    #[test]
    fn changes_show_deletions() {
        let root = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(root.path()).unwrap();
        std::fs::write(root.path().join("deleted"), "removed line\n").unwrap();
        let base = commit(&repo);
        std::fs::remove_file(root.path().join("deleted")).unwrap();
        let result = super::read_changes(root.path(), Some(base)).unwrap();
        assert_eq!(result.files[0].path, "deleted");
        assert!(result.files[0].patch.contains("-removed line"));
        assert!(result.files[0].patch.contains("deleted file mode"));
    }

    #[cfg(unix)]
    #[test]
    fn changes_show_executable_mode_changes() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(root.path()).unwrap();
        let path = root.path().join("script");
        std::fs::write(&path, "echo hello\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let base = commit(&repo);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let result = super::read_changes(root.path(), Some(base)).unwrap();
        assert!(result.files[0].patch.contains("old mode 100644"));
        assert!(result.files[0].patch.contains("new mode 100755"));
    }

    #[test]
    fn changes_bound_large_files_and_patch_output() {
        let root = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(root.path()).unwrap();
        std::fs::write(root.path().join("tracked"), "base\n").unwrap();
        let base = commit(&repo);
        std::fs::write(
            root.path().join("large"),
            vec![b'x'; super::MAX_FILE_BYTES as usize + 1],
        )
        .unwrap();
        std::fs::write(
            root.path().join("patch"),
            "line of added text\n".repeat(30_000),
        )
        .unwrap();
        let result = super::read_changes(root.path(), Some(base)).unwrap();
        assert!(result.files[0].patch.contains("omitted"));
        assert!(result.files[1].patch.contains("Patch truncated"));
        assert!(result.files[1].patch.len() < super::MAX_PATCH_BYTES + 100);
        let fallback = super::read_changes(root.path(), None).unwrap();
        assert!(
            fallback
                .base_label
                .contains("committed run changes excluded")
        );
    }

    #[test]
    fn changes_mark_file_limit_as_partial() {
        let root = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(root.path()).unwrap();
        std::fs::write(root.path().join("tracked"), "base\n").unwrap();
        let base = commit(&repo);
        for index in 0..=super::MAX_FILES {
            std::fs::write(root.path().join(format!("new-{index:03}")), "new\n").unwrap();
        }
        let result = super::read_changes(root.path(), Some(base)).unwrap();
        assert!(result.truncated);
        assert_eq!(result.files.len(), super::MAX_FILES);
    }

    #[test]
    fn changes_explain_binary_and_empty_results() {
        let root = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(root.path()).unwrap();
        std::fs::write(root.path().join("tracked"), "base\n").unwrap();
        let base = commit(&repo);
        assert!(
            super::read_changes(root.path(), Some(base))
                .unwrap()
                .files
                .is_empty()
        );
        std::fs::write(root.path().join("binary"), [0, 1, 2, 0]).unwrap();
        let result = super::read_changes(root.path(), Some(base)).unwrap();
        assert!(result.files[0].patch.contains("Binary"));
        assert!(super::read_changes(&root.path().join("missing"), Some(base)).is_err());
    }
}
