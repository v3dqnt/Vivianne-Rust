//! The top of Account & usage: a banner, a borderless round photo that overlaps it, and the person's name. The
//! photo and banner are pictures the person picks (copied into `~/.viv/profile/`); without them there is the
//! initials in the accent colour and a quiet gradient. Nothing shows a button: hovering the photo or the banner
//! shows a pencil over it and clicking it picks a picture, and a pencil beside the name edits the name in place.
//! Everything stays in this app.

use gpui_kit::base::h_flex;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::SettingItem;
use gpui_kit::component::{Icon, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    FontWeight, Hsla, IntoElement, ObjectFit, PathPromptOptions, StatefulInteractiveElement as _,
    Styled, div, img, linear_color_stop, linear_gradient, px,
};

use crate::settings::crop;
use crate::settings::profile::{self, Kind};
use crate::settings::values::Shared;
use crate::settings::widgets::keyed_input;
use crate::theme::{self, color};

const BANNER_HEIGHT: f32 = 136.0;
const AVATAR: f32 = 92.0;
const NAME_KEY: &str = "profile-name";
/// Set in the drafts while the name is being edited.
const EDITING_KEY: &str = "profile-editing";

/// Asks for a picture, then opens the dialog to choose the part of it to use.
fn pick(kind: Kind, window: &mut gpui_kit::Window, cx: &mut gpui_kit::App) {
    let chosen = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some("Use".into()),
    });
    window
        .spawn(cx, async move |cx| {
            if let Ok(Ok(Some(paths))) = chosen.await
                && let Some(path) = paths.into_iter().next()
            {
                // A file the picture cannot be read from is refused before the dialog opens.
                if let Err(why) = profile::check_image(&path) {
                    vivianne_core::debuglog::log("app", format!("profile picture: {why}"));
                    return;
                }
                cx.update(|window, cx| crop::open(kind, path, window, cx))
                    .ok();
            }
        })
        .detach();
}

/// A pencil on a dimmed layer over a picture, shown only while the pointer is over it.
fn pencil_overlay(group: &'static str, round: bool) -> impl IntoElement {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .when(round, gpui_kit::Styled::rounded_full)
        .bg(Hsla {
            h: 0.0,
            s: 0.0,
            l: 0.0,
            a: 0.5,
        })
        .opacity(0.0)
        .group_hover(group, |style| style.opacity(1.0))
        .child(
            Icon::empty()
                .path("icons/pencil.svg")
                .size(px(if round { 22.0 } else { 20.0 }))
                .text_color(color(theme::ON_ACCENT)),
        )
}

fn banner(p: &profile::Profile) -> impl IntoElement {
    let base = div()
        .id("profile-banner")
        .group("profile-banner")
        .relative()
        .w_full()
        .h(px(BANNER_HEIGHT))
        .rounded(px(theme::RADIUS_SURFACE))
        .overflow_hidden()
        .cursor_pointer()
        // Without a picture: the accent fading into the surface, so the page has a top without a box.
        .bg(linear_gradient(
            135.0,
            linear_color_stop(color(theme::ACCENT_LINE), 0.0),
            linear_color_stop(color(theme::SURFACE), 1.0),
        ))
        .on_click(|_e, window, cx| pick(Kind::Banner, window, cx));
    let base = match &p.banner {
        Some(path) => base.child(
            img(path.clone())
                .size_full()
                .rounded(px(theme::RADIUS_SURFACE))
                .object_fit(ObjectFit::Cover),
        ),
        None => base,
    };
    base.child(pencil_overlay("profile-banner", false))
}

fn avatar(p: &profile::Profile) -> impl IntoElement {
    // Borderless: the photo is cut to a circle and sits on the banner with nothing drawn around it.
    let base = div()
        .id("profile-avatar")
        .group("profile-avatar")
        .absolute()
        .left(px(20.0))
        .top(px(BANNER_HEIGHT - AVATAR / 2.0))
        .size(px(AVATAR))
        .rounded_full()
        .overflow_hidden()
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .on_click(|_e, window, cx| pick(Kind::Avatar, window, cx));
    let base = match &p.avatar {
        Some(path) => base.child(
            img(path.clone())
                .size_full()
                .rounded_full()
                .object_fit(ObjectFit::Cover),
        ),
        None => base
            .bg(linear_gradient(
                160.0,
                linear_color_stop(color(theme::ACCENT_HOVER), 0.0),
                linear_color_stop(color(theme::ACCENT), 1.0),
            ))
            .text_size(px(34.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(color(theme::ON_ACCENT))
            .child(profile::initials(&p.name)),
    };
    base.child(pencil_overlay("profile-avatar", true))
}

pub(super) fn item(values: &Shared) -> SettingItem {
    let values = values.clone();
    SettingItem::render(move |_options, window, cx| {
        let p = profile::current();
        let editing = values.borrow().key_drafts.contains_key(EDITING_KEY);
        let has_pictures = p.avatar.is_some() || p.banner.is_some();

        let name_row = if editing {
            let input = keyed_input(
                NAME_KEY,
                &values,
                |v| {
                    v.key_drafts
                        .get(NAME_KEY)
                        .cloned()
                        .unwrap_or_else(|| profile::current().name)
                },
                |v, x| {
                    v.key_drafts.insert(NAME_KEY.to_owned(), x);
                },
                "Your name",
                window,
                cx,
            );
            let (save, cancel, clear) = (values.clone(), values.clone(), values.clone());
            h_flex()
                .gap_2()
                .items_center()
                .child(input)
                .child(Button::new("profile-save").small().label("Save").on_click(
                    move |_e, window, cx| {
                        let text = save
                            .borrow()
                            .key_drafts
                            .get(NAME_KEY)
                            .cloned()
                            .unwrap_or_default();
                        let _ = profile::set_name(&text);
                        {
                            let mut v = save.borrow_mut();
                            v.key_drafts.remove(NAME_KEY);
                            v.key_drafts.remove(EDITING_KEY);
                        }
                        window.refresh();
                        cx.refresh_windows();
                    },
                ))
                .child(
                    Button::new("profile-cancel")
                        .ghost()
                        .small()
                        .label("Cancel")
                        .on_click(move |_e, window, cx| {
                            {
                                let mut v = cancel.borrow_mut();
                                v.key_drafts.remove(NAME_KEY);
                                v.key_drafts.remove(EDITING_KEY);
                            }
                            window.refresh();
                            cx.refresh_windows();
                        }),
                )
                .when(has_pictures, |row| {
                    row.child(
                        Button::new("profile-clear")
                            .ghost()
                            .small()
                            .label("Remove pictures")
                            .on_click(move |_e, window, cx| {
                                let _ = profile::clear_image(Kind::Avatar);
                                let _ = profile::clear_image(Kind::Banner);
                                clear.borrow_mut().key_drafts.remove(EDITING_KEY);
                                window.refresh();
                                cx.refresh_windows();
                            }),
                    )
                })
                .into_any_element()
        } else {
            let start = values.clone();
            h_flex()
                .gap(px(8.0))
                .items_center()
                .child(
                    div()
                        .text_size(px(theme::TEXT_TITLE))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color(theme::TEXT))
                        .child(p.name.clone()),
                )
                .child(
                    Button::new("profile-edit-name")
                        .ghost()
                        .small()
                        .icon(Icon::empty().path("icons/pencil.svg"))
                        .tooltip("Edit name")
                        .on_click(move |_e, window, cx| {
                            start
                                .borrow_mut()
                                .key_drafts
                                .insert(EDITING_KEY.to_owned(), "1".to_owned());
                            window.refresh();
                            cx.refresh_windows();
                        }),
                )
                .into_any_element()
        };

        div()
            .relative()
            .w_full()
            .child(banner(&p))
            .child(avatar(&p))
            // The name sits to the right of the photo, under the banner.
            .child(
                div()
                    .pl(px(20.0 + AVATAR + 16.0))
                    .pt(px(12.0))
                    .child(name_row),
            )
            .into_any_element()
    })
}
