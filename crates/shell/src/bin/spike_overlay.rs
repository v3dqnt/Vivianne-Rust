//! Phase 1 spike (b): a transparent, borderless, always-on-top, click-through
//! window on Windows, like the desktop-control pointer overlay. Prints what the
//! OS reports, including whether `WindowFromPoint` sees through it, then exits.
#![allow(unsafe_code)] // Win32 style calls; the real version lives in one platform module.
#![allow(clippy::pedantic, clippy::expect_used)] // throwaway spike

use std::ffi::c_void;
use std::time::Duration;

use gpui::{
    App, Application, Bounds, Context, Window, WindowBackgroundAppearance, WindowBounds,
    WindowKind, WindowOptions, div, prelude::*, px, rgba, size,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, GetWindowRect, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SetWindowLongPtrW, SetWindowPos, WINDOW_EX_STYLE, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WindowFromPoint,
};

struct Dot;

impl Render for Dot {
    fn render(&mut self, _w: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(div().size(px(80.0)).rounded_full().bg(rgba(0x3b82f6b0)))
    }
}

fn hwnd_of(window: &Window) -> Option<HWND> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(HWND(h.hwnd.get() as *mut c_void)),
        _ => None,
    }
}

/// Make the window click-through and topmost, then report what the OS says.
fn apply_and_check(hwnd: HWND) -> String {
    let extra =
        WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST;
    let mut rect = RECT::default();
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            (WINDOW_EX_STYLE(style) | extra).0 as isize,
        );
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        let _ = GetWindowRect(hwnd, &mut rect);
    }
    let style = WINDOW_EX_STYLE(unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32);
    let centre = POINT {
        x: (rect.left + rect.right) / 2,
        y: (rect.top + rect.bottom) / 2,
    };
    let hit = unsafe { WindowFromPoint(centre) };
    format!(
        "click_through={} topmost={} layered={} hit_test_sees_overlay={}",
        style.contains(WS_EX_TRANSPARENT),
        style.contains(WS_EX_TOPMOST),
        style.contains(WS_EX_LAYERED),
        hit == hwnd,
    )
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(400.0), px(400.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            kind: WindowKind::PopUp,
            focus: false,
            is_movable: false,
            window_background: WindowBackgroundAppearance::Transparent,
            ..Default::default()
        };
        let window = cx
            .open_window(options, |_, cx| cx.new(|_| Dot))
            .expect("open window");

        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_millis(800))
                .await;
            let report = window
                .update(cx, |_, window, _| hwnd_of(window).map(apply_and_check))
                .ok()
                .flatten()
                .unwrap_or_else(|| "no Win32 handle".to_owned());
            println!("{report}");
            // Leave it up a few seconds so it can be looked at.
            cx.background_executor().timer(Duration::from_secs(4)).await;
            cx.update(|cx| cx.quit()).ok();
        })
        .detach();
    });
}
