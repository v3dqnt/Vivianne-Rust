//! Web & browser: which search the agent uses, its keys, and the browsers it can drive.

use gpui_kit::base::h_flex;
use gpui_kit::component::IconName;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};
use gpui_kit::prelude::*;
use gpui_kit::{SharedString, Styled, div};

use crate::settings::values::{
    Shared, Values, BROWSER_CONNECTED, BROWSER_NOT_INSTALLED, BROWSER_NOT_OPEN, BROWSER_NO_EXT,
};
use crate::settings::widgets::{btn, field_dropdown, muted, pill, strong};

const PROVIDERS: [(&str, &str); 5] = [
    ("auto", "Automatic"),
    ("duckduckgo", "DuckDuckGo (no key)"),
    ("tavily", "Tavily"),
    ("brave", "Brave Search"),
    ("exa", "Exa"),
];

/// Search providers that need a key, with the name the key is saved under.
const KEYED: [(&str, &str); 3] = [("tavily", "Tavily"), ("brave", "Brave Search"), ("exa", "Exa")];

pub fn page(values: &Shared) -> SettingPage {
    SettingPage::new("Web & browser")
        .icon(IconName::Globe)
        .description("How the agent searches the web and drives a browser.")
        .group(search_group(values))
        .group(keys_group(values))
        .group(browsers_group(values))
}

fn search_group(values: &Shared) -> SettingGroup {
    let test = values.clone();
    SettingGroup::new()
        .title("Search")
        .item(
            SettingItem::new(
                "Search provider",
                field_dropdown(values, &PROVIDERS, |v| v.search_provider.clone(), |v, x| v.search_provider = x),
            )
            .description("Automatic uses the first provider that has a key, then DuckDuckGo."),
        )
        .item(SettingItem::render(move |_options, _window, _cx| {
            let (active, result) = {
                let v = test.borrow();
                (v.search_active.clone(), v.search_test.clone())
            };
            let values = test.clone();
            h_flex()
                .gap_3()
                .items_center()
                .child(muted(format!("Searching with {active}.")))
                .child(btn("search-test", "Test search", move |_, _, cx| {
                    let mut v = values.borrow_mut();
                    v.search_test = Some(format!("{} answered.", v.search_active));
                    cx.refresh_windows();
                }))
                .children(result.map(muted))
                .into_any_element()
        }))
}

fn keys_group(values: &Shared) -> SettingGroup {
    let items = KEYED.iter().map(|&(id, name)| {
        let (read, write) = (values.clone(), values.clone());
        let field = SettingField::input(
            move |_| SharedString::from(read.borrow().web_keys.get(id).cloned().unwrap_or_default()),
            move |key, cx| {
                write.borrow_mut().web_keys.insert(id.to_owned(), key.to_string());
                cx.refresh_windows();
            },
        );
        SettingItem::new(format!("{name} key"), field)
    });
    SettingGroup::new().title("Keys").items(items)
}

fn browsers_group(values: &Shared) -> SettingGroup {
    let list = values.clone();
    SettingGroup::new()
        .title("Browsers")
        .description("The agent drives a browser through its extension. Connect one to let it open pages as you.")
        .item(SettingItem::render(move |_options, _window, _cx| {
            let browsers = list.borrow().browsers.clone();
            let rows = browsers.into_iter().enumerate().map(|(index, browser)| {
                let values = list.clone();
                let (label, action) = state_text(browser.state);
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(h_flex().gap_2().items_center().child(strong(browser.name.clone())).child(pill(label)))
                    .children(action.map(|text| {
                        Button::new(("browser-action", index)).label(text).small().on_click(move |_, _, cx| {
                            connect(&mut values.borrow_mut(), index);
                            cx.refresh_windows();
                        })
                    }))
            });
            div().flex().flex_col().gap_2().children(rows).into_any_element()
        }))
}

/// What a browser's state says, and the button that moves it on (if any).
fn state_text(state: u8) -> (&'static str, Option<&'static str>) {
    match state {
        BROWSER_CONNECTED => ("Connected", None),
        BROWSER_NO_EXT => ("Open, extension missing", Some("Add extension")),
        BROWSER_NOT_OPEN => ("Installed, not open", Some("Open")),
        BROWSER_NOT_INSTALLED => ("Not installed", None),
        _ => ("Unknown", None),
    }
}

/// Take a browser to the next step of connecting; the last step is connected.
fn connect(values: &mut Values, index: usize) {
    let Some(browser) = values.browsers.get_mut(index) else { return };
    browser.state = match browser.state {
        BROWSER_NOT_OPEN => BROWSER_NO_EXT,
        BROWSER_NO_EXT => BROWSER_CONNECTED,
        other => other,
    };
    values.browser_connected = values.browsers.iter().any(|b| b.state == BROWSER_CONNECTED);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::values::Browser;

    #[test]
    fn a_browser_steps_from_closed_to_connected() {
        let mut v = Values {
            browsers: vec![Browser { name: "Arc".into(), state: BROWSER_NOT_OPEN }],
            ..Values::default()
        };
        connect(&mut v, 0);
        assert_eq!(v.browsers[0].state, BROWSER_NO_EXT);
        assert!(!v.browser_connected);
        connect(&mut v, 0);
        assert_eq!(v.browsers[0].state, BROWSER_CONNECTED);
        assert!(v.browser_connected);
    }

    #[test]
    fn only_steppable_states_have_a_button() {
        assert!(state_text(BROWSER_NOT_INSTALLED).1.is_none());
        assert!(state_text(BROWSER_CONNECTED).1.is_none());
        assert!(state_text(BROWSER_NO_EXT).1.is_some());
    }
}
