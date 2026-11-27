//! The Settings pages, one module per page, in the order the old app showed
//! them. `all()` returns them for the dialog's sidebar.

mod account;
mod account_agents;
mod account_profile;
mod appearance;
mod automations;
mod browser;
mod characters;
mod components;
mod computer;
mod connections;
mod diagnostics;
mod layout;
mod local;
mod machines;
mod memory;
mod mods;
mod providers;
mod reliability;
mod skills;
mod updates;
mod vault;
mod web;

use gpui_kit::component::setting::SettingPage;

use super::values::Shared;

/// The pages in sidebar order. Neighbours with the same section sit under one label; the first have none.
pub fn all(values: &Shared) -> Vec<SettingPage> {
    let mut pages = vec![
        account::page(values),
        appearance::page(values),
        characters::page(values),
        layout::page(values),
    ];
    let sections: [(&str, Vec<SettingPage>); 3] = [
        (
            "Agents",
            vec![
                providers::page(values),
                local::page(values),
                reliability::page(values),
                memory::page(values),
                skills::page(values),
                automations::page(values),
            ],
        ),
        (
            "Tools",
            vec![
                computer::page(values),
                web::page(values),
                connections::page(values),
                mods::page(values),
                vault::page(values),
                machines::page(values),
            ],
        ),
        (
            "App",
            vec![
                diagnostics::page(values),
                components::page(values),
                updates::page(values),
            ],
        ),
    ];
    for (name, members) in sections {
        pages.extend(members.into_iter().map(|page| page.section(name)));
    }
    pages
}
