//! Coding agents: what Claude Code, Codex and the other agents connected over ACP have used, and which model inside
//! each did the work. The usage store keeps an agent's turns under `harness/<agent>/<model>` (an agent that has not
//! said which model it runs is kept under `harness/<agent>`), and this page folds those keys back into agents with
//! their models, for the same range as the By model list.

use std::collections::BTreeMap;

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::setting::{SettingGroup, SettingItem};
use gpui_kit::prelude::*;
use gpui_kit::{FontWeight, IntoElement, Styled, div, px, relative};

use crate::settings::logos;
use crate::settings::values::{ModelUsage, Shared};
use crate::theme::{self, color};

/// The provider the usage store files a coding agent's turns under.
pub const PROVIDER: &str = "harness";
const UNKNOWN_MODEL: &str = "Model not reported";

/// One model used inside an agent.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelShare {
    pub model: String,
    pub requests: u32,
    pub tokens: u64,
}

/// One agent and everything used through it.
#[derive(Debug, Clone, PartialEq)]
pub struct Agent {
    pub id: String,
    pub requests: u32,
    pub tokens: u64,
    /// Most tokens first.
    pub models: Vec<ModelShare>,
}

/// A name for an agent id: the well-known ones by their product name, others made readable.
#[must_use]
pub fn agent_name(id: &str) -> String {
    match id {
        "claude-acp" | "claude" | "claude-code" => "Claude Code".to_owned(),
        "codex-acp" | "codex" => "Codex".to_owned(),
        "gemini" | "gemini-cli" => "Gemini CLI".to_owned(),
        "opencode" => "OpenCode".to_owned(),
        other => {
            let base = other.strip_suffix("-acp").unwrap_or(other);
            let mut words = base.split(['-', '_']).filter(|w| !w.is_empty()).map(|w| {
                let mut c = w.chars();
                c.next().map_or_else(String::new, |f| {
                    f.to_uppercase().collect::<String>() + c.as_str()
                })
            });
            let name = words.by_ref().collect::<Vec<_>>().join(" ");
            if name.is_empty() {
                other.to_owned()
            } else {
                name
            }
        }
    }
}

/// Folds the per-model rows into agents with the models used inside them. Rows of other providers are skipped.
#[must_use]
pub fn fold(rows: &[ModelUsage]) -> Vec<Agent> {
    let mut agents: BTreeMap<String, Agent> = BTreeMap::new();
    for r in rows.iter().filter(|r| r.provider == PROVIDER) {
        let (id, model) = r
            .model
            .split_once('/')
            .map_or((r.model.as_str(), UNKNOWN_MODEL), |(a, m)| (a, m));
        let agent = agents.entry(id.to_owned()).or_insert_with(|| Agent {
            id: id.to_owned(),
            requests: 0,
            tokens: 0,
            models: Vec::new(),
        });
        let tokens = r.input + r.output;
        agent.requests += r.requests;
        agent.tokens += tokens;
        match agent.models.iter_mut().find(|m| m.model == model) {
            Some(m) => {
                m.requests += r.requests;
                m.tokens += tokens;
            }
            None => agent.models.push(ModelShare {
                model: model.to_owned(),
                requests: r.requests,
                tokens,
            }),
        }
    }
    let mut out: Vec<Agent> = agents.into_values().collect();
    for a in &mut out {
        a.models
            .sort_by(|x, y| y.tokens.cmp(&x.tokens).then_with(|| x.model.cmp(&y.model)));
    }
    out.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.id.cmp(&b.id)));
    out
}

fn bar(share: f32) -> impl IntoElement {
    div()
        .h(px(4.0))
        .w_full()
        .rounded_full()
        .bg(color(theme::RAISED))
        .child(
            div()
                .h_full()
                .w(relative(share.clamp(0.0, 1.0)))
                .rounded_full()
                .bg(color(theme::ACCENT)),
        )
}

fn rows(values: &Shared) -> SettingItem {
    let values = values.clone();
    SettingItem::render(move |_options, _window, _cx| {
        let v = values.borrow();
        let agents = fold(&v.model_usage);
        if agents.is_empty() {
            return div()
                .text_xs()
                .text_color(color(theme::MUTED))
                .child(if v.usage_loaded {
                    "No coding agent has been used in this range. Connect Claude Code, Codex or another agent in Providers, and what it uses shows here, model by model."
                } else {
                    "Reading your usage…"
                })
                .into_any_element();
        }
        let top = agents
            .iter()
            .flat_map(|a| a.models.iter().map(move |m| (a, m)))
            .max_by_key(|(_, m)| m.tokens);
        let busiest = agents.iter().map(|a| a.tokens).max().unwrap_or(1).max(1);
        let mut list = v_flex().gap_4();
        if let Some((agent, model)) = top {
            list = list.child(
                div()
                    .text_sm()
                    .text_color(color(theme::MUTED))
                    .child(format!(
                        "Most used: {} in {}",
                        model.model,
                        agent_name(&agent.id)
                    )),
            );
        }
        for a in &agents {
            let busiest_model = a.models.iter().map(|m| m.tokens).max().unwrap_or(1).max(1);
            let mut block = v_flex().gap_2().child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .items_baseline()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(color(theme::TEXT))
                                    .child(agent_name(&a.id)),
                            )
                            .child(
                                div()
                                    .font_family(theme::MONO)
                                    .text_xs()
                                    .text_color(color(theme::MUTED))
                                    .child(format!(
                                        "{} tokens · {} requests",
                                        super::account::compact(a.tokens),
                                        a.requests
                                    )),
                            ),
                    )
                    .child(bar(a.tokens as f32 / busiest as f32)),
            );
            for m in &a.models {
                block = block.child(
                    v_flex()
                        .pl(px(14.0))
                        .gap_1()
                        .child(
                            h_flex()
                                .items_baseline()
                                .gap_2()
                                .child(crate::brand::tile(
                                    logos::model_company(&m.model).unwrap_or(""),
                                    &m.model,
                                    18.0,
                                ))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_sm()
                                        .text_color(color(if m.model == UNKNOWN_MODEL {
                                            theme::MUTED
                                        } else {
                                            theme::TEXT
                                        }))
                                        .child(m.model.clone()),
                                )
                                .child(
                                    div()
                                        .font_family(theme::MONO)
                                        .text_xs()
                                        .text_color(color(theme::MUTED))
                                        .child(format!(
                                            "{} · {} requests",
                                            super::account::compact(m.tokens),
                                            m.requests
                                        )),
                                ),
                        )
                        .child(bar(m.tokens as f32 / busiest_model as f32)),
                );
            }
            list = list.child(block);
        }
        list.into_any_element()
    })
}

pub(super) fn group(values: &Shared) -> SettingGroup {
    SettingGroup::new()
        .title("Coding agents")
        .description(
            "Usage through the agents you connected, and which model inside each did the work.",
        )
        .item(super::account::range_item(values, false))
        .item(rows(values))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(provider: &str, model: &str, requests: u32, input: u64, output: u64) -> ModelUsage {
        ModelUsage {
            model: model.to_owned(),
            provider: provider.to_owned(),
            requests,
            input,
            output,
            cost: None,
            unpriced: true,
        }
    }

    #[test]
    fn agents_are_folded_from_their_models_and_other_providers_are_left_out() {
        let rows = vec![
            row("harness", "claude-acp/Opus 4.5", 3, 900, 100),
            row("harness", "claude-acp/Sonnet 4.5", 5, 400, 100),
            row("harness", "claude-acp", 1, 10, 0),
            row("harness", "codex-acp/GPT-5", 2, 50, 50),
            row("openrouter", "some/model", 9, 9_000_000, 0),
        ];
        let agents = fold(&rows);
        assert_eq!(agents.len(), 2);
        let claude = &agents[0];
        assert_eq!(
            (claude.id.as_str(), claude.requests, claude.tokens),
            ("claude-acp", 9, 1510)
        );
        assert_eq!(
            claude.models[0].model, "Opus 4.5",
            "the most used model comes first"
        );
        assert!(
            claude
                .models
                .iter()
                .any(|m| m.model == "Model not reported"),
            "an agent that did not say its model is shown as such"
        );
        assert_eq!(agents[1].id, "codex-acp");
    }

    #[test]
    fn agent_names_are_the_products_name_or_a_readable_version_of_the_id() {
        assert_eq!(agent_name("claude-acp"), "Claude Code");
        assert_eq!(agent_name("codex-acp"), "Codex");
        assert_eq!(agent_name("my-cool-agent-acp"), "My Cool Agent");
        assert_eq!(agent_name("x"), "X");
    }
}
