//! Characters: the mini characters that can stand for agents, each playing the chosen animation. They are drawn
//! in this app from the models in `assets/characters/` (Kenney, CC0) the first time they are shown.

use gpui_kit::base::h_flex;
use gpui_kit::component::IconName;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::{SettingGroup, SettingItem, SettingPage};
use gpui_kit::prelude::*;
use gpui_kit::{Styled, div, px};

use crate::characters::{ANIMATIONS, CharacterView, NAMES};
use crate::settings::values::Shared;
use crate::settings::widgets::muted;
use crate::theme::{self, color};

const ANIM_KEY: &str = "char-anim";
const CARD: f32 = 120.0;

pub fn page(values: &Shared) -> SettingPage {
    let values = values.clone();
    SettingPage::new("Characters")
        .icon(IconName::User)
        .description("The little people that can stand for your agents. Pick an animation to see them all do it.")
        .group(
            SettingGroup::new()
                .title("Animation")
                .item(SettingItem::render({
                    let values = values.clone();
                    move |_options, _window, _cx| {
                        let current = current(&values);
                        let chips = ANIMATIONS.iter().enumerate().map(|(i, name)| {
                            let handle = values.clone();
                            let name = (*name).to_owned();
                            let on = name == current;
                            let label = name.replace('-', " ");
                            let button = Button::new(("char-anim", i)).small().label(label);
                            let button = if on { button } else { button.ghost() };
                            button.on_click(move |_, _, cx| {
                                handle
                                    .borrow_mut()
                                    .key_drafts
                                    .insert(ANIM_KEY.to_owned(), name.clone());
                                cx.refresh_windows();
                            })
                        });
                        h_flex()
                            .flex_wrap()
                            .gap(px(6.0))
                            .children(chips)
                            .into_any_element()
                    }
                })),
        )
        .group(SettingGroup::new().title("Cast").item(SettingItem::render(
            move |_options, window, cx| {
                let animation = current(&values);
                let cards = NAMES.iter().map(|name| {
                    let key = format!("char-{name}-{animation}");
                    let (n, a) = ((*name).to_owned(), animation.clone());
                    let view = window.use_keyed_state(
                        gpui_kit::SharedString::from(key),
                        cx,
                        move |_window, cx| CharacterView::new(&n, &a, CARD, cx),
                    );
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(4.0))
                        .w(px(CARD))
                        .child(view)
                        .child(
                            div()
                                .text_size(px(theme::TEXT_XS))
                                .text_color(color(theme::MUTED))
                                .child((*name).to_owned()),
                        )
                });
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(div().flex().flex_wrap().gap(px(12.0)).children(cards))
                    .child(muted("Characters by Kenney (kenney.nl), public domain (CC0)."))
                    .into_any_element()
            },
        )))
}

fn current(values: &Shared) -> String {
    values
        .borrow()
        .key_drafts
        .get(ANIM_KEY)
        .cloned()
        .unwrap_or_else(|| "idle".to_owned())
}
