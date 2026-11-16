//! Appearance: theme, interface size, how the chat looks, and the companion robot.

use gpui_kit::component::IconName;
use gpui_kit::component::setting::{NumberFieldOptions, SettingGroup, SettingItem, SettingPage};

use crate::settings::values::Shared;
use crate::settings::widgets::{field_dropdown, field_number, field_switch};

const MODES: [(&str, &str); 3] = [("dark", "Dark"), ("light", "Light"), ("system", "Match the system")];
const DENSITIES: [(&str, &str); 2] = [("comfortable", "Comfortable"), ("compact", "Compact")];
const WIDTHS: [(&str, &str); 3] = [("narrow", "Narrow"), ("normal", "Normal"), ("wide", "Wide")];
const VISUALS: [(&str, &str); 3] = [
    ("auto", "When it helps"),
    ("always", "Always"),
    ("never", "Never"),
];
const ROBOT_MOVES: [(&str, &str); 3] = [
    ("busy", "While the agent works"),
    ("always", "Always"),
    ("never", "Never"),
];
const FACES: [(&str, &str); 4] = [("0", "Round"), ("1", "Square"), ("2", "Sleepy"), ("3", "Spark")];

const SCALE: NumberFieldOptions = NumberFieldOptions { min: 0.8, max: 1.4, step: 0.05 };
const ROBOT_SCALE: NumberFieldOptions = NumberFieldOptions { min: 0.5, max: 2.0, step: 0.1 };

pub fn page(values: &Shared) -> SettingPage {
    SettingPage::new("Appearance")
        .icon(IconName::Palette)
        .description("How Vivianne looks and reads.")
        .group(theme_group(values))
        .group(chat_group(values))
        .group(robot_group(values))
}

fn theme_group(values: &Shared) -> SettingGroup {
    SettingGroup::new().title("Theme").items([
        SettingItem::new("Mode", field_dropdown(values, &MODES, |v| v.mode.clone(), |v, x| v.mode = x))
            .description("Dark, light, or whatever your system is set to."),
        SettingItem::new("Interface size", field_number(values, SCALE, |v| f64::from(v.scale), |v, x| v.scale = x as f32))
            .description("Scales text and controls everywhere. 1.0 is the default."),
    ])
}

fn chat_group(values: &Shared) -> SettingGroup {
    SettingGroup::new().title("Chat").items([
        SettingItem::new(
            "Density",
            field_dropdown(values, &DENSITIES, |v| v.chat_density.clone(), |v, x| v.chat_density = x),
        )
        .description("How much room there is between messages."),
        SettingItem::new(
            "Width",
            field_dropdown(values, &WIDTHS, |v| v.chat_width.clone(), |v, x| v.chat_width = x),
        )
        .description("How wide the conversation column is."),
        SettingItem::new(
            "Visual replies",
            field_dropdown(values, &VISUALS, |v| v.visuals.clone(), |v, x| v.visuals = x),
        )
        .description("Charts, diagrams and previews drawn in the chat instead of described."),
        SettingItem::new("Read replies aloud", field_switch(values, |v| v.read_aloud, |v, on| v.read_aloud = on))
            .description("Speak each reply when it finishes."),
    ])
}

fn robot_group(values: &Shared) -> SettingGroup {
    SettingGroup::new().title("Companion").items([
        SettingItem::new(
            "Size",
            field_number(values, ROBOT_SCALE, |v| f64::from(v.robot_scale), |v, x| v.robot_scale = x as f32),
        ),
        SettingItem::new(
            "Moves",
            field_dropdown(values, &ROBOT_MOVES, |v| v.robot_move.clone(), |v, x| v.robot_move = x),
        ),
        SettingItem::new(
            "Face",
            field_dropdown(values, &FACES, |v| v.face.to_string(), |v, x| v.face = x.parse().unwrap_or(0)),
        ),
    ])
}
