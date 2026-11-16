//! Local models: servers running on this machine, models to download, Ollama
//! and image models.

use gpui_kit::base::{Disableable as _, h_flex};
use gpui_kit::component::IconName;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::{SettingGroup, SettingItem, SettingPage};
use gpui_kit::prelude::*;
use gpui_kit::{SharedString, Styled, div};

use crate::settings::values::{EP_CRASHED, EP_RUNNING, EP_STARTING, EP_STOPPED, Hardware, HfRepo, ImageModel, LocalEndpoint, Shared};
use crate::settings::widgets::{keyed_input, mono, muted, note, pill, strong};

pub fn page(values: &Shared) -> SettingPage {
    SettingPage::new("Local models")
        .icon(IconName::Frame)
        .description("Run models on this computer instead of a provider.")
        .group(hardware_group(values))
        .group(endpoints_group(values))
        .group(download_group(values))
        .group(ollama_group(values))
        .group(images_group(values))
}

fn hardware_text(hw: &Hardware) -> String {
    let gpus = if hw.gpus.is_empty() {
        "no GPU".to_owned()
    } else {
        hw.gpus.iter().map(|(name, gb)| format!("{name} ({gb} GB)")).collect::<Vec<_>>().join(", ")
    };
    let memory = if hw.unified { "shared memory" } else { "RAM" };
    format!("{} · {} GB {memory} · {} threads · {gpus}", hw.os, hw.ram_gb, hw.threads)
}

fn hardware_group(values: &Shared) -> SettingGroup {
    let values = values.clone();
    SettingGroup::new().title("This computer").item(SettingItem::render(move |_options, _window, _cx| {
        match values.borrow().hardware.as_ref() {
            Some(hw) => muted(hardware_text(hw)).into_any_element(),
            None => muted("Hardware not checked yet.").into_any_element(),
        }
    }))
}

fn endpoint_state(state: u8) -> &'static str {
    match state {
        EP_RUNNING => "Running",
        EP_STARTING => "Starting",
        EP_CRASHED => "Crashed",
        _ => "Stopped",
    }
}

/// What the start/stop button does next.
fn toggled(state: u8) -> u8 {
    if state == EP_STOPPED || state == EP_CRASHED { EP_RUNNING } else { EP_STOPPED }
}

fn endpoints_group(values: &Shared) -> SettingGroup {
    let values = values.clone();
    SettingGroup::new()
        .title("Servers")
        .description("Each one serves a downloaded model on a port of its own.")
        .item(SettingItem::render(move |_options, _window, _cx| {
            let endpoints = values.borrow().endpoints.clone();
            if endpoints.is_empty() {
                return muted("No servers set up.").into_any_element();
            }
            let rows = endpoints.into_iter().enumerate().map(|(index, e)| endpoint_row(&values, index, &e));
            div().flex().flex_col().gap_3().children(rows).into_any_element()
        }))
}

fn endpoint_row(values: &Shared, index: usize, e: &LocalEndpoint) -> impl IntoElement + use<> {
    let (flip, remove) = (values.clone(), values.clone());
    let label = if toggled(e.state) == EP_RUNNING { "Start" } else { "Stop" };
    h_flex()
        .justify_between()
        .items_center()
        .gap_4()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(h_flex().gap_2().items_center().child(strong(e.name.clone())).child(pill(endpoint_state(e.state))))
                .child(mono(format!("{} · port {} · {} tokens of context", e.file, e.port, e.ctx))),
        )
        .child(
            h_flex()
                .gap_2()
                .child(Button::new(("ep-toggle", index)).label(label).small().on_click(move |_, _, cx| {
                    if let Some(ep) = flip.borrow_mut().endpoints.get_mut(index) {
                        ep.state = toggled(ep.state);
                    }
                    cx.refresh_windows();
                }))
                .child(Button::new(("ep-remove", index)).label("Remove").small().ghost().on_click(move |_, _, cx| {
                    let mut v = remove.borrow_mut();
                    if index < v.endpoints.len() {
                        v.endpoints.remove(index);
                    }
                    cx.refresh_windows();
                })),
        )
}

fn download_group(values: &Shared) -> SettingGroup {
    let values = values.clone();
    SettingGroup::new()
        .title("Get models")
        .description("Search Hugging Face for models in GGUF form.")
        .item(SettingItem::render(move |_options, window, cx| {
            let search = keyed_input("hf-query", &values, |v| v.hf_query.clone(), |v, x| v.hf_query = x, "Search models", window, cx);
            let (query, repos, pulled) = {
                let v = values.borrow();
                (v.hf_query.to_lowercase(), v.hf_results.clone(), v.pulled.clone())
            };
            let rows = repos
                .into_iter()
                .filter(|r| repo_matches(r, &query))
                .enumerate()
                .map(|(i, r)| repo_row(&values, i, &r, &pulled));
            div().flex().flex_col().gap_3().child(search).children(rows).into_any_element()
        }))
}

fn repo_matches(repo: &HfRepo, query: &str) -> bool {
    let name = repo.repo.to_lowercase();
    query.split_whitespace().all(|word| name.contains(word))
}

fn repo_row(values: &Shared, repo_index: usize, repo: &HfRepo, pulled: &[String]) -> impl IntoElement + use<> {
    let files = repo.files.iter().enumerate().map(|(file_index, (file, gb))| {
        let have = pulled.contains(file);
        let (add, file) = (values.clone(), file.clone());
        let name = file.clone();
        h_flex()
            .justify_between()
            .items_center()
            .child(mono(format!("{name} · {gb:.1} GB")))
            .child(
                Button::new(SharedString::from(format!("hf-{repo_index}-{file_index}")))
                    .label(if have { "Downloaded" } else { "Download" })
                    .small()
                    .disabled(have)
                    .on_click(move |_, _, cx| {
                        add.borrow_mut().pulled.push(file.clone());
                        cx.refresh_windows();
                    }),
            )
    });
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(h_flex().gap_2().items_center().child(strong(repo.repo.clone())).child(pill(format!("{} downloads", repo.downloads))))
        .children(files)
}

fn ollama_group(values: &Shared) -> SettingGroup {
    let values = values.clone();
    SettingGroup::new()
        .title("Ollama")
        .description("Models already installed in Ollama.")
        .item(SettingItem::render(move |_options, _window, _cx| {
            let (models, used) = {
                let v = values.borrow();
                (v.ollama_models.clone(), v.ollama_used.clone())
            };
            if models.is_empty() {
                return muted("Ollama has no models, or isn't running.").into_any_element();
            }
            let rows = models.into_iter().enumerate().map(|(index, name)| {
                let on = used.contains(&name);
                let flip = values.clone();
                let key = name.clone();
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(mono(name))
                    .child(Button::new(("ollama", index)).label(if on { "Stop using" } else { "Use" }).small().on_click(move |_, _, cx| {
                        let mut v = flip.borrow_mut();
                        if on {
                            v.ollama_used.retain(|m| *m != key);
                        } else {
                            v.ollama_used.push(key.clone());
                        }
                        cx.refresh_windows();
                    }))
            });
            div().flex().flex_col().gap_2().children(rows).into_any_element()
        }))
}

fn images_group(values: &Shared) -> SettingGroup {
    let values = values.clone();
    SettingGroup::new()
        .title("Image models")
        .item(note("Make images on this computer. Each model is a large download."))
        .item(SettingItem::render(move |_options, _window, _cx| {
            let (models, pulled) = {
                let v = values.borrow();
                (v.image_models.clone(), v.pulled.clone())
            };
            let rows = models.into_iter().enumerate().map(|(index, m)| image_row(&values, index, &m, &pulled));
            div().flex().flex_col().gap_3().children(rows).into_any_element()
        }))
}

fn image_row(values: &Shared, index: usize, model: &ImageModel, pulled: &[String]) -> impl IntoElement + use<> {
    let have = pulled.contains(&model.id);
    let (flip, id) = (values.clone(), model.id.clone());
    h_flex()
        .justify_between()
        .items_center()
        .gap_4()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(h_flex().gap_2().items_center().child(strong(model.name.clone())).child(pill(format!("{:.1} GB", model.gb))))
                .child(muted(model.note.clone())),
        )
        .child(Button::new(("image", index)).label(if have { "Remove" } else { "Download" }).small().on_click(move |_, _, cx| {
            let mut v = flip.borrow_mut();
            if have {
                v.pulled.retain(|p| *p != id);
            } else {
                v.pulled.push(id.clone());
            }
            cx.refresh_windows();
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_and_stop_swap() {
        assert_eq!(toggled(EP_STOPPED), EP_RUNNING);
        assert_eq!(toggled(EP_CRASHED), EP_RUNNING);
        assert_eq!(toggled(EP_RUNNING), EP_STOPPED);
        assert_eq!(toggled(EP_STARTING), EP_STOPPED);
    }

    #[test]
    fn hardware_reads_as_one_line() {
        let hw = Hardware { ram_gb: 64, threads: 24, gpus: vec![("RTX".into(), 12)], unified: false, os: "Windows".into() };
        assert_eq!(hardware_text(&hw), "Windows · 64 GB RAM · 24 threads · RTX (12 GB)");
    }

    #[test]
    fn a_repo_search_needs_every_word() {
        let repo = HfRepo { repo: "Qwen/Qwen3-Coder-GGUF".into(), downloads: 1, files: vec![] };
        assert!(repo_matches(&repo, "qwen coder"));
        assert!(!repo_matches(&repo, "llama"));
    }
}
