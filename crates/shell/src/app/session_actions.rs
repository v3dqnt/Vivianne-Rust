//! What the app does with saved conversations and scheduled tasks: deliver a due task, export the open thread.

use std::path::PathBuf;

use gpui_kit::{Context, Window};
use vivianne_runtime::export::Options;
use vivianne_runtime::export_bundle;
use vivianne_runtime::query::Query;

use super::{AppView, Page};

/// Writes thread `id` to `path`, redacted, as a Markdown transcript or a ZIP. Blocking.
fn write_export(id: &str, path: &std::path::Path, zip: bool) -> Result<(), String> {
    let query = Query::standard().map_err(|e| e.to_string())?;
    let bytes = if zip {
        let read = export_bundle::standard_reader();
        export_bundle::bundle(&query, id, Options::default(), &read)
            .map_err(|e| e.to_string())?
            .0
    } else {
        let thread = query.read_thread(id).map_err(|e| e.to_string())?;
        export_bundle::markdown(&thread, Options::default()).into_bytes()
    };
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

/// A title as a file name: letters, digits and dashes.
fn file_stem(title: &str) -> String {
    let stem: String = title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .take(60)
        .collect();
    let stem = stem
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if stem.is_empty() {
        "conversation".to_owned()
    } else {
        stem
    }
}

impl AppView {
    /// Asks where to save the open thread and writes it there. Secrets are taken out.
    pub(super) fn export_thread(
        &mut self,
        zip: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Page::Thread(index) = self.page else {
            return;
        };
        let Some(thread) = self.threads.get(index) else {
            return;
        };
        let chat = thread.chat.clone();
        let Some(id) = chat.read(cx).thread_id().map(str::to_owned) else {
            return;
        };
        let name = format!(
            "{}.{}",
            file_stem(&chat.read(cx).title()),
            if zip { "zip" } else { "md" }
        );
        let folder = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map_or_else(
                || self.workspace.clone(),
                |home| PathBuf::from(home).join("Downloads"),
            );
        let chosen = cx.prompt_for_new_path(&folder, Some(&name));
        cx.spawn(async move |_app, cx| {
            let Ok(Ok(Some(path))) = chosen.await else {
                return;
            };
            let to = path.clone();
            let written = cx
                .background_executor()
                .spawn(async move { write_export(&id, &to, zip) })
                .await;
            let said = match written {
                Ok(()) => format!("Exported to {} (secrets removed).", path.display()),
                Err(why) => format!("Couldn't export: {why}"),
            };
            chat.update(cx, |chat, cx| chat.notify_user(said, cx));
        })
        .detach();
    }

    /// A scheduled task is due: it is sent to the conversation it names (waiting while that one is busy or still
    /// opening) or to a new one, and only then marked as delivered.
    pub(super) fn run_scheduled(
        &mut self,
        due: vivianne_runtime::schedule::store::Due,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use vivianne_runtime::schedule::{domain, store::Store};
        let task = &due.entry.task;
        let reused = match &task.target {
            domain::Target::Session { session_id } => Some(session_id.clone()),
            domain::Target::NewOnce { session_id, .. } if !session_id.is_empty() => {
                Some(session_id.clone())
            }
            _ => None,
        };
        let index = match reused {
            Some(id) => {
                let open = self
                    .threads
                    .iter()
                    .position(|t| t.chat.read(cx).thread_id() == Some(id.as_str()));
                match open {
                    Some(index) => index,
                    None if vivianne_runtime::store::load(&id).is_some() => {
                        // Opens in the background; the next tick sends it.
                        self.open_saved(&id, window, cx);
                        return;
                    }
                    // The conversation it named is gone: it gets a new one.
                    None => self.add_thread(window, cx),
                }
            }
            None => self.add_thread(window, cx),
        };
        let chat = self.threads[index].chat.clone();
        if chat.read(cx).is_running() {
            return;
        }
        let message = domain::reminder(&task.id, &task.prompt, &domain::iso(due.occurrence_at));
        let (title, empty) = (task.title.clone(), chat.read(cx).is_empty());
        chat.update(cx, |chat, cx| {
            chat.run_scheduled(&title, &message, empty, window, cx);
        });
        let session = chat.read(cx).thread_id().map(str::to_owned);
        if let Some(session) = session {
            let now = i64::try_from(vivianne_runtime::automation::now_ms()).unwrap_or(i64::MAX);
            // A task changed meanwhile keeps its new form; this delivery is simply not recorded.
            let _ = Store::standard().delivered(&due, &session, now);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_becomes_a_safe_file_name() {
        assert_eq!(file_stem("Fix: the/parser \\ now!"), "fix-the-parser-now");
        assert_eq!(file_stem("  ../.. "), "conversation");
        assert!(file_stem(&"x".repeat(200)).len() <= 60);
    }
}
