//! Connections: the apps the agent can sign in to, and the MCP servers its tools come from.

use gpui_kit::base::{Disableable as _, h_flex};
use gpui_kit::component::IconName;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::{SettingGroup, SettingItem, SettingPage};
use gpui_kit::prelude::*;
use gpui_kit::{Styled, div};

use crate::settings::values::{
    AUTH_BOT, AUTH_OAUTH, AUTH_OPEN, AUTH_TOKEN, AppEntry, McpServer, Shared,
};
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
    SettingGroup::new()
        .title("Apps")
        .item(SettingItem::render(move |_options, window, cx| {
            if values
                .borrow_mut()
                .key_drafts
                .insert("conn-apps-synced".to_owned(), "1".to_owned())
                .is_none()
            {
                {
                    let mut v = values.borrow_mut();
                    if v.apps.is_empty() {
                        v.apps = catalogue();
                    }
                }
                sync_apps(&values, cx);
            }
            let search = keyed_input(
                "conn-search",
                &values,
                |v| v.conn_search.clone(),
                |v, x| v.conn_search = x,
                "Find an app",
                window,
                cx,
            );
            let (query, apps) = {
                let v = values.borrow();
                (v.conn_search.to_lowercase(), v.apps.clone())
            };
            // Cards, a section per category in the order the catalogue lists them.
            let mut categories: Vec<String> = Vec::new();
            for app in &apps {
                if !categories.contains(&app.category) {
                    categories.push(app.category.clone());
                }
            }
            let mut rows: Vec<gpui_kit::AnyElement> = Vec::new();
            for category in categories {
                let cards = apps
                    .iter()
                    .enumerate()
                    .filter(|(_, app)| app.category == category && app_matches(app, &query))
                    .map(|(index, app)| app_card(&values, index, app, window, cx))
                    .collect::<Vec<_>>();
                if cards.is_empty() {
                    continue;
                }
                rows.push(
                    div()
                        .flex()
                        .flex_col()
                        .gap(gpui_kit::px(10.0))
                        .child(
                            div()
                                .text_size(gpui_kit::px(13.0))
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .text_color(crate::theme::color(crate::theme::MUTED))
                                .child(category),
                        )
                        .child(
                            div()
                                .grid()
                                .grid_cols(3)
                                .gap(gpui_kit::px(12.0))
                                .children(cards),
                        )
                        .into_any_element(),
                );
            }
            div()
                .flex()
                .flex_col()
                .gap(gpui_kit::px(18.0))
                .child(search)
                .children(rows)
                .into_any_element()
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

const TOKEN_KEY: &str = "conn-token";
/// Set in the drafts to the id of the app whose token box is open.
const OPEN_KEY: &str = "conn-open";

/// Every app of the catalogue, none connected yet (`sync_apps` reads which are).
pub fn catalogue() -> Vec<AppEntry> {
    use vivianne_tools::connections::catalog::{CATALOG, Kind};
    CATALOG
        .iter()
        .map(|a| AppEntry {
            id: a.id.to_owned(),
            name: a.name.to_owned(),
            category: a.category.to_owned(),
            auth: match a.kind {
                Kind::OAuth { .. } => AUTH_OAUTH,
                Kind::Open { .. } => AUTH_OPEN,
                Kind::Token { .. } => AUTH_TOKEN,
                Kind::Native => AUTH_BOT,
            },
            account: None,
            connected: false,
            pending: false,
            desc: a.description.to_owned(),
            field: a
                .fields
                .first()
                .map(|f| f.label.to_owned())
                .unwrap_or_default(),
            help: a.help.to_owned(),
            help_url: a.help_url.to_owned(),
            error: String::new(),
            domain: a.domain.to_owned(),
        })
        .collect()
}

/// Reads which apps are connected, and as whom, then redraws.
fn sync_apps(values: &Shared, cx: &mut gpui_kit::App) {
    let shared = values.clone();
    crate::backend::run(cx, vivianne_tools::connections::list(), move |list, cx| {
        if let Ok(list) = list {
            let mut v = shared.borrow_mut();
            for view in list {
                if let Some(app) = v.apps.iter_mut().find(|a| a.id == view.id) {
                    app.connected = view.connected;
                    app.account = view.account;
                }
            }
        }
        cx.refresh_windows();
    });
    let domains = values.borrow().apps.iter().map(|a| a.domain.clone()).collect();
    crate::brand::prefetch(cx, domains);
}

fn start_connect(values: &Shared, id: String, token: Option<String>, cx: &mut gpui_kit::App) {
    if let Some(app) = values.borrow_mut().apps.iter_mut().find(|a| a.id == id) {
        app.pending = true;
        app.error.clear();
    }
    let fields = token.map(|t| std::collections::HashMap::from([("token".to_owned(), t)]));
    let after = values.clone();
    let work_id = id.clone();
    crate::backend::run(
        cx,
        async move { vivianne_tools::connections::connect(work_id, fields).await },
        move |result, cx| {
            {
                let mut v = after.borrow_mut();
                if let Some(app) = v.apps.iter_mut().find(|a| a.id == id) {
                    app.pending = false;
                    match result {
                        Ok(done) => {
                            app.connected = true;
                            app.account = done.account;
                        }
                        Err(e) => app.error = e,
                    }
                }
                v.key_drafts.remove(OPEN_KEY);
                v.key_drafts.remove(TOKEN_KEY);
            }
            cx.refresh_windows();
        },
    );
    cx.refresh_windows();
}

fn app_card(
    values: &Shared,
    index: usize,
    app: &AppEntry,
    window: &mut gpui_kit::Window,
    cx: &mut gpui_kit::App,
) -> gpui_kit::AnyElement {
    use crate::theme::{self, color};
    let needs_token = matches!(app.auth, AUTH_TOKEN | AUTH_BOT);
    let open = values
        .borrow()
        .key_drafts
        .get(OPEN_KEY)
        .is_some_and(|o| *o == app.id);
    let (id, handle) = (app.id.clone(), values.clone());
    let label = if app.pending {
        "Connecting…"
    } else if app.connected {
        "Disconnect"
    } else {
        "Connect"
    };
    let connected = app.connected;
    let button = Button::new(("app", index))
        .label(label)
        .small()
        .disabled(app.pending)
        .on_click(move |_, _, cx| {
            if connected {
                let (after, gone) = (handle.clone(), id.clone());
                crate::backend::run(
                    cx,
                    async move { vivianne_tools::connections::disconnect(gone).await },
                    move |_, cx| sync_apps(&after, cx),
                );
                if let Some(a) = handle.borrow_mut().apps.get_mut(index) {
                    a.connected = false;
                }
                cx.refresh_windows();
            } else if needs_token {
                let mut v = handle.borrow_mut();
                v.key_drafts.insert(OPEN_KEY.to_owned(), id.clone());
                v.key_drafts.remove(TOKEN_KEY);
                drop(v);
                cx.refresh_windows();
            } else {
                start_connect(&handle, id.clone(), None, cx);
            }
        });
    let button = if connected { button.outline() } else { button };
    let status = if app.connected {
        Some(
            h_flex()
                .gap(gpui_kit::px(6.0))
                .items_center()
                .child(
                    div()
                        .size(gpui_kit::px(7.0))
                        .rounded_full()
                        .bg(color(theme::OK)),
                )
                .child(muted(
                    app.account
                        .clone()
                        .map_or_else(|| "Connected".to_owned(), |a| format!("Connected as {a}")),
                )),
        )
    } else {
        None
    };
    let mut card = div()
        .flex()
        .flex_col()
        .gap(gpui_kit::px(10.0))
        .p(gpui_kit::px(14.0))
        .min_w_0()
        .rounded(gpui_kit::px(theme::RADIUS_SURFACE))
        .bg(color(theme::SURFACE))
        .child(
            h_flex()
                .gap(gpui_kit::px(10.0))
                .items_center()
                .child(crate::brand::tile_with(&app.id, &app.domain, &app.name, 36.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .child(strong(app.name.clone()))
                        .child(
                            div()
                                .text_size(gpui_kit::px(theme::TEXT_XS))
                                .text_color(color(theme::FAINT))
                                .child(auth_label(app.auth)),
                        ),
                ),
        )
        .child(
            div()
                .flex_1()
                .text_size(gpui_kit::px(theme::TEXT_SM))
                .text_color(color(theme::MUTED))
                .line_clamp(3)
                .child(app.desc.clone()),
        );
    if !app.error.is_empty() {
        card = card.child(
            div()
                .text_size(gpui_kit::px(theme::TEXT_XS))
                .text_color(color(theme::DANGER))
                .child(app.error.clone()),
        );
    }
    if open && needs_token && !app.connected {
        let input = crate::settings::widgets::keyed_secret(
            TOKEN_KEY,
            values,
            |v| v.key_drafts.get(TOKEN_KEY).cloned().unwrap_or_default(),
            |v, x| {
                v.key_drafts.insert(TOKEN_KEY.to_owned(), x);
            },
            "Paste it here",
            window,
            cx,
        );
        let (go, id, cancel) = (values.clone(), app.id.clone(), values.clone());
        card = card
            .child(muted(format!("{}. {}", app.field, app.help)))
            .child(input)
            .child(
                h_flex()
                    .gap(gpui_kit::px(6.0))
                    .child(
                        Button::new(("app-go", index))
                            .small()
                            .label("Connect")
                            .on_click(move |_, _, cx| {
                                let token = go
                                    .borrow()
                                    .key_drafts
                                    .get(TOKEN_KEY)
                                    .cloned()
                                    .unwrap_or_default();
                                start_connect(&go, id.clone(), Some(token), cx);
                            }),
                    )
                    .child(
                        Button::new(("app-cancel", index))
                            .ghost()
                            .small()
                            .label("Cancel")
                            .on_click(move |_, _, cx| {
                                cancel.borrow_mut().key_drafts.remove(OPEN_KEY);
                                cx.refresh_windows();
                            }),
                    ),
            );
    } else {
        card = card.child(
            h_flex()
                .justify_between()
                .items_center()
                .gap(gpui_kit::px(8.0))
                .child(div().min_w_0().children(status))
                .child(button),
        );
    }
    card.into_any_element()
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
                let count = tools
                    .iter()
                    .filter(|(server, _)| *server == config.name)
                    .count();
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
                        McpTransportConfig::Stdio { command, args, .. } => (
                            "Program".to_owned(),
                            format!("{command} {}", args.join(" ")).trim().to_owned(),
                        ),
                        McpTransportConfig::Http { url, .. } => {
                            ("Web address".to_owned(), url.clone())
                        }
                        McpTransportConfig::Native { connector } => {
                            ("Built in".to_owned(), connector.clone())
                        }
                    };
                    McpServer {
                        id: config.name.clone(),
                        name: config.name,
                        kind,
                        detail,
                        tools: u32::try_from(tools).unwrap_or(u32::MAX),
                        env: String::new(),
                    }
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
        McpTransportConfig::Http {
            url: text.to_owned(),
            headers: std::collections::HashMap::new(),
        }
    } else {
        let mut parts = text.split_whitespace();
        let command = parts.next()?.to_owned();
        McpTransportConfig::Stdio {
            command,
            args: parts.map(str::to_owned).collect(),
            env: std::collections::HashMap::new(),
        }
    };
    let name = match &transport {
        McpTransportConfig::Http { url, .. } => url
            .split("//")
            .nth(1)
            .and_then(|rest| rest.split(['/', ':']).next())
            .unwrap_or("server")
            .replace('.', "-"),
        McpTransportConfig::Native { connector } => connector.clone(),
        McpTransportConfig::Stdio { command, args, .. } => {
            // `npx -y @scope/server-name` is named for the package, not for npx.
            let named = args
                .iter()
                .rev()
                .find(|a| !a.starts_with('-'))
                .map_or(command.as_str(), String::as_str);
            named
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(named)
                .trim_start_matches('@')
                .to_owned()
        }
    };
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    (!name.is_empty()).then_some(McpServerConfig {
        name,
        transport,
        timeout_ms: None,
        auth: None,
        allowed_tools: None,
    })
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
            field: String::new(),
            help: String::new(),
            help_url: String::new(),
            error: String::new(),
            domain: String::new(),
        }
    }

    #[test]
    fn a_typed_server_is_a_web_address_or_a_program_named_for_what_it_runs() {
        let web = server_from("https://mcp.example.com/mcp").expect("a server");
        assert!(matches!(web.transport, McpTransportConfig::Http { .. }));
        assert_eq!(web.name, "mcp-example-com");
        let program = server_from("npx -y @modelcontextprotocol/server-memory").expect("a server");
        assert_eq!(program.name, "server-memory");
        assert!(
            matches!(&program.transport, McpTransportConfig::Stdio { command, args, .. } if command == "npx" && args.len() == 2)
        );
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
