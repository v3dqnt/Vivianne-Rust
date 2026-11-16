//! Connections: the apps the agent can sign in to, and the MCP servers its tools come from.

use gpui_kit::base::h_flex;
use gpui_kit::component::IconName;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::{SettingGroup, SettingItem, SettingPage};
use gpui_kit::prelude::*;
use gpui_kit::{Styled, div};

use crate::settings::values::{AppEntry, Shared, AUTH_BOT, AUTH_OAUTH, AUTH_OPEN, AUTH_TOKEN};
use crate::settings::widgets::{keyed_input, mono, muted, pill, strong};

pub fn page(values: &Shared) -> SettingPage {
    SettingPage::new("Connections")
        .icon(IconName::Inbox)
        .description("Apps the agent can use on your behalf, and the servers that give it tools.")
        .group(apps_group(values))
        .group(mcp_group(values))
}

fn apps_group(values: &Shared) -> SettingGroup {
    let values = values.clone();
    SettingGroup::new().title("Apps").item(SettingItem::render(move |_options, window, cx| {
        let search = keyed_input("conn-search", &values, |v| v.conn_search.clone(), |v, x| v.conn_search = x, "Find an app", window, cx);
        let (query, apps) = {
            let v = values.borrow();
            (v.conn_search.to_lowercase(), v.apps.clone())
        };
        let rows = apps
            .into_iter()
            .enumerate()
            .filter(|(_, app)| app_matches(app, &query))
            .map(|(index, app)| app_row(&values, index, &app));
        div().flex().flex_col().gap_3().child(search).children(rows).into_any_element()
    }))
}

fn app_matches(app: &AppEntry, query: &str) -> bool {
    let haystack = format!("{} {} {}", app.name, app.category, app.desc).to_lowercase();
    query.split_whitespace().all(|word| haystack.contains(word))
}

/// How an app signs in, as the pill beside its name.
fn auth_label(auth: u8) -> &'static str {
    match auth {
        AUTH_OAUTH => "Sign in",
        AUTH_OPEN => "No sign-in",
        AUTH_TOKEN => "Token",
        AUTH_BOT => "Bot token",
        _ => "",
    }
}

fn app_row(values: &Shared, index: usize, app: &AppEntry) -> impl IntoElement + use<> {
    let toggle = values.clone();
    let label = if app.connected { "Disconnect" } else { "Connect" };
    let button = Button::new(("app", index)).label(label).small().on_click(move |_, _, cx| {
        let mut v = toggle.borrow_mut();
        let Some(entry) = v.apps.get_mut(index) else { return };
        entry.connected = !entry.connected;
        entry.pending = false;
        cx.refresh_windows();
    });
    let account = app.account.clone().filter(|_| app.connected);
    h_flex()
        .justify_between()
        .items_center()
        .gap_4()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(h_flex().gap_2().items_center().child(strong(app.name.clone())).child(pill(auth_label(app.auth))))
                .child(muted(app.desc.clone()))
                .children(account.map(|a| muted(format!("Connected as {a}")))),
        )
        .child(button)
}

fn mcp_group(values: &Shared) -> SettingGroup {
    let values = values.clone();
    SettingGroup::new()
        .title("MCP servers")
        .description("Programs that offer the agent extra tools.")
        .item(SettingItem::render(move |_options, _window, _cx| {
            let servers = values.borrow().mcp.clone();
            if servers.is_empty() {
                return muted("No servers added.").into_any_element();
            }
            let rows = servers.into_iter().map(|s| {
                let remove = values.clone();
                let id = s.id.clone();
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(h_flex().gap_2().items_center().child(strong(s.name.clone())).child(pill(format!("{} tools", s.tools))))
                            .child(mono(format!("{} · {}", s.kind, s.detail))),
                    )
                    .child(
                        Button::new(gpui_kit::SharedString::from(format!("mcp-remove-{}", s.id)))
                            .label("Remove")
                            .ghost()
                            .small()
                            .on_click(move |_, _, cx| {
                                remove.borrow_mut().mcp.retain(|m| m.id != id);
                                cx.refresh_windows();
                            }),
                    )
            });
            div().flex().flex_col().gap_3().children(rows).into_any_element()
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, category: &str) -> AppEntry {
        AppEntry {
            id: name.into(),
            name: name.into(),
            category: category.into(),
            auth: AUTH_OAUTH,
            account: None,
            connected: false,
            pending: false,
            desc: String::new(),
        }
    }

    #[test]
    fn search_covers_name_and_category() {
        let slack = app("Slack", "Chat");
        assert!(app_matches(&slack, "slack"));
        assert!(app_matches(&slack, "chat"));
        assert!(!app_matches(&slack, "calendar"));
    }

    #[test]
    fn every_sign_in_kind_has_a_label() {
        for kind in [AUTH_OAUTH, AUTH_OPEN, AUTH_TOKEN, AUTH_BOT] {
            assert!(!auth_label(kind).is_empty());
        }
    }
}
