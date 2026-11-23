//! Vault: logins, cards and secrets the agent can use without ever seeing them.

use gpui_kit::base::h_flex;
use gpui_kit::component::IconName;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::{SettingGroup, SettingItem, SettingPage};
use gpui_kit::prelude::*;
use gpui_kit::{Styled, div};

use crate::settings::values::{Shared, VAULT_CARD, VAULT_LOGIN, VAULT_SECRET, VaultEntry};
use crate::settings::widgets::{muted, note, pill, strong};

pub fn page(values: &Shared) -> SettingPage {
    SettingPage::new("Vault")
        .icon(IconName::Eye)
        .description("Saved logins and secrets, kept out of the conversation.")
        .group(
            SettingGroup::new()
                .item(note("The agent asks for an entry by name and the value goes straight to where it is used. It never appears in a chat."))
                .item(list(values)),
        )
}

fn list(values: &Shared) -> SettingItem {
    let values = values.clone();
    SettingItem::render(move |_options, _window, _cx| {
        let entries = values.borrow().vault.clone();
        if entries.is_empty() {
            return muted("Nothing saved yet.").into_any_element();
        }
        let rows = entries.into_iter().map(|entry| row(&values, &entry));
        div()
            .flex()
            .flex_col()
            .gap_3()
            .children(rows)
            .into_any_element()
    })
}

fn row(values: &Shared, entry: &VaultEntry) -> impl IntoElement + use<> {
    let (remove, id) = (values.clone(), entry.id.clone());
    let detail = detail_line(entry);
    h_flex()
        .justify_between()
        .items_center()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(strong(entry.label.clone()))
                        .child(pill(kind_name(entry.kind))),
                )
                .child(muted(detail)),
        )
        .child(
            Button::new(gpui_kit::SharedString::from(format!(
                "vault-remove-{}",
                entry.id
            )))
            .label("Remove")
            .ghost()
            .small()
            .on_click(move |_, _, cx| {
                remove.borrow_mut().vault.retain(|e| e.id != id);
                cx.refresh_windows();
            }),
        )
}

fn kind_name(kind: u8) -> &'static str {
    match kind {
        VAULT_LOGIN => "Login",
        VAULT_CARD => "Card",
        VAULT_SECRET => "Secret",
        _ => "Entry",
    }
}

/// Site and user for a login, the hint for a card or secret, then when it was last used.
fn detail_line(entry: &VaultEntry) -> String {
    let who = match entry.kind {
        VAULT_LOGIN => format!("{} · {}", entry.site, entry.username),
        _ => entry.hint.clone(),
    };
    if entry.used.is_empty() {
        who
    } else {
        format!("{who} · used {}", entry.used)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: u8, used: &str) -> VaultEntry {
        VaultEntry {
            id: "e".into(),
            label: "L".into(),
            kind,
            site: "github.com".into(),
            username: "viv".into(),
            hint: "ends 4242".into(),
            used: used.into(),
        }
    }

    #[test]
    fn a_login_shows_site_and_user_and_a_card_its_hint() {
        assert_eq!(detail_line(&entry(VAULT_LOGIN, "")), "github.com · viv");
        assert_eq!(
            detail_line(&entry(VAULT_CARD, "today")),
            "ends 4242 · used today"
        );
    }
}
