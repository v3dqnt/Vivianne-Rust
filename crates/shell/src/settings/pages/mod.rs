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

pub fn all(values: &Shared) -> Vec<SettingPage> {
    vec![
        account::page(values),
        providers::page(values),
        local::page(values),
        reliability::page(values),
        mods::page(values),
        connections::page(values),
        machines::page(values),
        computer::page(values),
        browser::page(values),
        web::page(values),
        memory::page(values),
        vault::page(values),
        skills::page(values),
        automations::page(values),
        diagnostics::page(values),
        appearance::page(values),
        characters::page(values),
        layout::page(values),
        components::page(values),
        updates::page(values),
    ]
}
