//! Connections: the apps the agent can sign in to, and the MCP servers its tools come from.

use gpui_kit::base::h_flex;
use gpui_kit::component::IconName;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::{SettingGroup, SettingItem, SettingPage};
use gpui_kit::prelude::*;
use gpui_kit::{Styled, div};

use crate::settings::values::{AppEntry, McpServer, Shared, AUTH_BOT, AUTH_OAUTH, AUTH_OPEN, AUTH_TOKEN};
use crate::settings::widgets::{btn, keyed_input, mono, muted, pill, strong};
use vivianne_tools::mcp::{self, McpServerConfig, McpTransportConfig};

const MCP_ADD: &str = "mcp-add";
const MCP_ERROR: &str = "mcp-error";
const MCP_SYNCED: &str = "mcp-synced";

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

/// Reads the connected servers and how many tools each offers, then redraws.
fn sync_servers(values: &Shared, cx: &mut gpui_kit::App) {
    let values = values.clone();
    let work = async {
        let configs = mcp::list_servers().await;
        let tools = mcp::list_all_tools().await;
        configs
            .into_iter()
            .map(|config| {
                let count = tools.iter().filter(|(server, _)| *server == config.name).count();
                (config, count)
            })
            .collect::<Vec<_>>()
    };
    {
        crate::backend::run(cx, work, move |servers, cx| {
            values.borrow_mut().mcp = servers
                .into_iter()
                .map(|(config, tools)| {
                    let (kind, detail) = match &config.transport {
                        McpTransportConfig::Stdio { command, args, .. } => ("Program".to_owned(), format!("{command} {}", args.join(" ")).trim().to_owned()),
                        McpTransportConfig::Http { url, .. } => ("Web address".to_owned(), url.clone()),
                    };
                    McpServer { id: config.name.clone(), name: config.name, kind, detail, tools: u32::try_from(tools).unwrap_or(u32::MAX), env: String::new() }
                })
                .collect();
            cx.refresh_windows();
        });
    }
}

/// What was typed as a server: a web address, or a program with its arguments. The name comes from it.
fn server_from(text: &str) -> Option<McpServerConfig> {
    let text = text.trim();
    let transport = if text.starts_with("http://") || text.starts_with("https://") {
        McpTransportConfig::Http { url: text.to_owned(), headers: std::collections::HashMap::new() }
    } else {
        let mut parts = text.split_whitespace();
        let command = parts.next()?.to_owned();
        McpTransportConfig::Stdio { command, args: parts.map(str::to_owned).collect(), env: std::collections::HashMap::new() }
    };
    let name = match &transport {
        McpTransportConfig::Http { url, .. } => url.split("//").nth(1).and_then(|rest| rest.split(['/', ':']).next()).unwrap_or("server").replace('.', "-"),
        McpTransportConfig::Stdio { command, args, .. } => {
            // `npx -y @scope/server-name` is named for the package, not for npx.
            let named = args.iter().rev().find(|a| !a.starts_with('-')).map_or(command.as_str(), String::as_str);
            named.rsplit(['/', '\\']).next().unwrap_or(named).trim_start_matches('@').to_owned()
        }
    };
    let name: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    (!name.is_empty()).then_some(McpServerConfig { name, transport, timeout_ms: None, auth: None })
}

fn mcp_group(values: &Shared) -> SettingGroup {
    let values = values.clone();
    let add = values.clone();
    SettingGroup::new()
        .title("MCP servers")
        .description("Programs that offer the agent extra tools. Add a web address, or a program to run, such as npx -y @modelcontextprotocol/server-memory.")
        .item(SettingItem::render(move |_options, window, cx| {
            let values = add.clone();
            // The connected servers are read the first time the page is drawn (and again after each change).
            if values.borrow_mut().key_drafts.insert(MCP_SYNCED.to_owned(), "1".to_owned()).is_none() {
                sync_servers(&values, cx);
            }
            let input = keyed_input(MCP_ADD, &values, |v| v.key_drafts.get(MCP_ADD).cloned().unwrap_or_default(), |v, x| {
                v.key_drafts.insert(MCP_ADD.to_owned(), x);
            }, "web address, or a program and its arguments", window, cx);
            let problem = values.borrow().key_drafts.get(MCP_ERROR).cloned().filter(|e| !e.is_empty());
            let pressed = values.clone();
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(h_flex().gap_2().child(input).child(btn("mcp-add-button", "Add", move |_, _, cx| {
                    let draft = pressed.borrow_mut().key_drafts.remove(MCP_ADD).unwrap_or_default();
                    let Some(config) = server_from(&draft) else { return };
                    pressed.borrow_mut().key_drafts.insert(MCP_ERROR.to_owned(), format!("Connecting to {}…", config.name));
                    let after = pressed.clone();
                    crate::backend::run(cx, async move { mcp::add_server(config).await.map(|tools| tools.len()) }, move |result, cx| {
                        after.borrow_mut().key_drafts.insert(MCP_ERROR.to_owned(), result.err().unwrap_or_default());
                        sync_servers(&after, cx);
                        cx.refresh_windows();
                    });
                    cx.refresh_windows();
                })))
                .children(problem.map(muted))
                .into_any_element()
        }))
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
                                let (name, after) = (id.clone(), remove.clone());
                                remove.borrow_mut().mcp.retain(|m| m.id != id);
                                crate::backend::run(cx, async move { mcp::remove_server(&name).await }, move |_, cx| {
                                    sync_servers(&after, cx);
                                    cx.refresh_windows();
                                });
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
    fn a_typed_server_is_a_web_address_or_a_program_named_for_what_it_runs() {
        let web = server_from("https://mcp.example.com/mcp").expect("a server");
        assert!(matches!(web.transport, McpTransportConfig::Http { .. }));
        assert_eq!(web.name, "mcp-example-com");
        let program = server_from("npx -y @modelcontextprotocol/server-memory").expect("a server");
        assert_eq!(program.name, "server-memory");
        assert!(matches!(&program.transport, McpTransportConfig::Stdio { command, args, .. } if command == "npx" && args.len() == 2));
        assert!(server_from("   ").is_none());
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
