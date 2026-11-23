//! Choosing the part of a picture to use as the photo or the banner. A dialog shows the picture with a frame of the
//! right shape (a circle for the photo, a wide strip for the banner); the frame is dragged to where it should be and
//! made smaller or larger with the zoom buttons or the mouse wheel. Saving cuts that part out, scales it to a sensible
//! size and stores it as the picture. The maths is plain functions so it is tested without a window.

use std::path::{Path, PathBuf};

use gpui_kit::component::Sizable as _;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    Context, Entity, Hsla, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ObjectFit, Render, ScrollWheelEvent, Styled, Window, div, img, px,
};

use super::profile::{self, Kind};
use crate::theme::{self, color};

/// The preview area, in pixels.
const PREVIEW_W: f32 = 560.0;
const PREVIEW_H: f32 = 340.0;
const MAX_ZOOM: f32 = 8.0;
const ZOOM_STEP: f32 = 1.2;

/// The shape and the size the saved picture gets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shape {
    /// Width over height of the frame.
    pub aspect: f32,
    /// The saved picture's width in pixels.
    pub out_width: u32,
    pub round: bool,
}

impl Shape {
    #[must_use]
    pub fn of(kind: Kind) -> Self {
        match kind {
            Kind::Avatar => Self {
                aspect: 1.0,
                out_width: 512,
                round: true,
            },
            // The banner is drawn about six times as wide as it is high.
            Kind::Banner => Self {
                aspect: 6.4,
                out_width: 1920,
                round: false,
            },
        }
    }
}

/// A rectangle in the picture's own pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// The frame for a picture of `size`, of the given `aspect`, `zoom` times smaller than the biggest that fits,
/// centred on `(cx, cy)` as far as the picture allows (it never leaves the picture).
#[must_use]
pub fn frame(size: (f32, f32), aspect: f32, zoom: f32, cx: f32, cy: f32) -> Frame {
    let (iw, ih) = size;
    let (mut w, mut h) = if iw / ih > aspect {
        (ih * aspect, ih)
    } else {
        (iw, iw / aspect)
    };
    let zoom = zoom.clamp(1.0, MAX_ZOOM);
    w /= zoom;
    h /= zoom;
    Frame {
        x: (cx - w / 2.0).clamp(0.0, (iw - w).max(0.0)),
        y: (cy - h / 2.0).clamp(0.0, (ih - h).max(0.0)),
        w,
        h,
    }
}

/// How the picture is drawn in the preview: the scale, and where its top-left corner sits.
#[must_use]
pub fn fit(size: (f32, f32), area: (f32, f32)) -> (f32, (f32, f32)) {
    let scale = (area.0 / size.0).min(area.1 / size.1);
    (
        scale,
        (
            (area.0 - size.0 * scale) / 2.0,
            (area.1 - size.1 * scale) / 2.0,
        ),
    )
}

/// Cuts `frame` out of the picture at `source`, scales it to `shape`, and writes it as a PNG to `dest`.
pub fn cut(source: &Path, frame: Frame, shape: Shape, dest: &Path) -> Result<(), String> {
    let picture =
        image::open(source).map_err(|e| format!("That picture could not be read: {e}"))?;
    let crop_w = (frame.w.round() as u32).max(1);
    let crop_h = (frame.h.round() as u32).max(1);
    let cropped = image::imageops::crop_imm(
        &picture,
        frame.x.round() as u32,
        frame.y.round() as u32,
        crop_w,
        crop_h,
    )
    .to_image();
    let out_h = ((shape.out_width as f32 / shape.aspect).round() as u32).max(1);
    // Never made bigger than it was: a small picture stays sharp instead of blurred up.
    let (out_w, out_h) = if crop_w < shape.out_width {
        (
            crop_w,
            ((crop_w as f32 / shape.aspect).round() as u32).max(1),
        )
    } else {
        (shape.out_width, out_h)
    };
    let scaled = image::imageops::resize(
        &cropped,
        out_w,
        out_h,
        image::imageops::FilterType::Lanczos3,
    );
    scaled
        .save_with_format(dest, image::ImageFormat::Png)
        .map_err(|e| e.to_string())
}

/// The state of the dialog.
pub struct CropView {
    kind: Kind,
    source: PathBuf,
    size: (f32, f32),
    shape: Shape,
    zoom: f32,
    /// The frame's centre, in the picture's pixels.
    center: (f32, f32),
    /// While the mouse is down on the frame: where it went down, and the centre then.
    drag: Option<((f32, f32), (f32, f32))>,
    error: Option<String>,
    saving: bool,
}

impl CropView {
    fn new(kind: Kind, source: PathBuf, size: (f32, f32)) -> Self {
        Self {
            kind,
            source,
            size,
            shape: Shape::of(kind),
            zoom: 1.0,
            center: (size.0 / 2.0, size.1 / 2.0),
            drag: None,
            error: None,
            saving: false,
        }
    }

    fn current(&self) -> Frame {
        frame(
            self.size,
            self.shape.aspect,
            self.zoom,
            self.center.0,
            self.center.1,
        )
    }

    fn zoom_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        self.zoom = (self.zoom * factor).clamp(1.0, MAX_ZOOM);
        // Keep the frame inside the picture at its new size.
        let f = self.current();
        self.center = (f.x + f.w / 2.0, f.y + f.h / 2.0);
        cx.notify();
    }

    fn save(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        self.saving = true;
        self.error = None;
        let (source, f, shape, kind) = (self.source.clone(), self.current(), self.shape, self.kind);
        let view = cx.entity().downgrade();
        let dialog_window = window.window_handle();
        cx.notify();
        let work = async move {
            // Decoding and scaling a large picture is not quick, so it is not done on the window's thread.
            tokio::task::spawn_blocking(move || {
                let temp =
                    std::env::temp_dir().join(format!("viv-crop-{}.png", uuid::Uuid::new_v4()));
                let result =
                    cut(&source, f, shape, &temp).and_then(|()| profile::set_image(kind, &temp));
                // The temporary file is ours; the profile has its own copy.
                let _ = std::fs::remove_file(&temp);
                result
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()))
        };
        crate::backend::run(cx, work, move |result, cx| {
            let result_ok = result.is_ok();
            view.update(cx, |this, cx| {
                this.saving = false;
                match &result {
                    Ok(()) => {}
                    Err(why) => this.error = Some(why.clone()),
                }
                cx.notify();
            })
            .ok();
            if result_ok {
                // Saved: the dialog closes and the page draws the new picture.
                let _ = dialog_window.update(cx, |_view, window, cx| window.close_dialog(cx));
            }
            cx.refresh_windows();
        });
    }
}

impl Render for CropView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (scale, (ox, oy)) = fit(self.size, (PREVIEW_W, PREVIEW_H));
        let f = self.current();
        // The frame in the preview's own pixels.
        let (fx, fy, fw, fh) = (ox + f.x * scale, oy + f.y * scale, f.w * scale, f.h * scale);
        let dim = Hsla {
            h: 0.0,
            s: 0.0,
            l: 0.0,
            a: 0.62,
        };
        let shade = move |x: f32, y: f32, w: f32, h: f32| {
            div()
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(w.max(0.0)))
                .h(px(h.max(0.0)))
                .bg(dim)
        };
        let round = self.shape.round;
        let frame_edge = div()
            .absolute()
            .left(px(fx))
            .top(px(fy))
            .w(px(fw))
            .h(px(fh))
            .border_2()
            .border_color(color(theme::ON_ACCENT))
            .cursor_move()
            .when(round, gpui_kit::Styled::rounded_full);
        // A circle is shown by its outline; what is outside the square is dimmed, and the corners of the square
        // inside the circle are left as they are (the saved picture is square, and drawn round).
        let canvas = div()
            .relative()
            .w(px(PREVIEW_W))
            .h(px(PREVIEW_H))
            .overflow_hidden()
            .bg(color(theme::CODE_BG))
            .rounded(px(theme::RADIUS_SURFACE))
            .child(
                img(self.source.clone())
                    .absolute()
                    .left(px(ox))
                    .top(px(oy))
                    .w(px(self.size.0 * scale))
                    .h(px(self.size.1 * scale))
                    .object_fit(ObjectFit::Fill),
            )
            .child(shade(0.0, 0.0, PREVIEW_W, fy))
            .child(shade(0.0, fy + fh, PREVIEW_W, PREVIEW_H - fy - fh))
            .child(shade(0.0, fy, fx, fh))
            .child(shade(fx + fw, fy, PREVIEW_W - fx - fw, fh))
            .child(frame_edge)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, _w, cx| {
                    this.drag = Some((
                        (f32::from(e.position.x), f32::from(e.position.y)),
                        this.center,
                    ));
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |this, e: &MouseMoveEvent, _w, cx| {
                let Some((start, from)) = this.drag else {
                    return;
                };
                if e.pressed_button != Some(MouseButton::Left) {
                    this.drag = None;
                    return;
                }
                // Mouse movement in the preview is movement in the picture, scaled back up.
                let (dx, dy) = (
                    (f32::from(e.position.x) - start.0) / scale,
                    (f32::from(e.position.y) - start.1) / scale,
                );
                let moved = frame(
                    this.size,
                    this.shape.aspect,
                    this.zoom,
                    from.0 - dx,
                    from.1 - dy,
                );
                this.center = (moved.x + moved.w / 2.0, moved.y + moved.h / 2.0);
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _e: &MouseUpEvent, _w, cx| {
                    this.drag = None;
                    cx.notify();
                }),
            )
            .on_scroll_wheel(cx.listener(|this, e: &ScrollWheelEvent, _w, cx| {
                let dy = f32::from(e.delta.pixel_delta(px(16.0)).y);
                if dy != 0.0 {
                    this.zoom_by(if dy < 0.0 { ZOOM_STEP } else { 1.0 / ZOOM_STEP }, cx);
                }
            }));

        let zoom_text = format!("{:.1}×", self.zoom);
        div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(canvas)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                Button::new("crop-out")
                                    .outline()
                                    .small()
                                    .label("−")
                                    .on_click(cx.listener(|this, _e, _w, cx| {
                                        this.zoom_by(1.0 / ZOOM_STEP, cx)
                                    })),
                            )
                            .child(
                                div()
                                    .w(px(44.0))
                                    .text_center()
                                    .text_size(px(theme::TEXT_SM))
                                    .text_color(color(theme::MUTED))
                                    .child(zoom_text),
                            )
                            .child(
                                Button::new("crop-in")
                                    .outline()
                                    .small()
                                    .label("+")
                                    .on_click(
                                        cx.listener(|this, _e, _w, cx| this.zoom_by(ZOOM_STEP, cx)),
                                    ),
                            )
                            .child(
                                div()
                                    .pl(px(8.0))
                                    .text_size(px(theme::TEXT_SM))
                                    .text_color(color(theme::MUTED))
                                    .child("Drag the frame, scroll to zoom"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(
                                Button::new("crop-cancel")
                                    .ghost()
                                    .small()
                                    .label("Cancel")
                                    .on_click(|_e, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                Button::new("crop-save")
                                    .primary()
                                    .small()
                                    .label(if self.saving { "Saving…" } else { "Use this" })
                                    .on_click(
                                        cx.listener(|this, _e, window, cx| this.save(window, cx)),
                                    ),
                            ),
                    ),
            )
            .children(self.error.clone().map(|e| {
                div()
                    .text_size(px(theme::TEXT_SM))
                    .text_color(color(theme::DANGER))
                    .child(e)
            }))
    }
}

/// Opens the crop dialog for a picture. The picture's size is read first; one that cannot be read says so in a log
/// line and nothing opens.
pub fn open(kind: Kind, source: PathBuf, window: &mut Window, cx: &mut gpui_kit::App) {
    let Ok((w, h)) = image::image_dimensions(&source) else {
        vivianne_core::debuglog::log(
            "app",
            format!("{} is not a picture this app can read", source.display()),
        );
        return;
    };
    let entity: Entity<CropView> = cx.new(|_| CropView::new(kind, source, (w as f32, h as f32)));
    let title = if kind == Kind::Avatar {
        "Crop your photo"
    } else {
        "Crop your banner"
    };
    window.open_dialog(cx, move |dialog, _window, _cx| {
        dialog
            .title(title)
            .w(px(PREVIEW_W + 48.0))
            .child(entity.clone())
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_biggest_frame_fits_the_picture_and_zoom_makes_it_smaller() {
        let wide = frame((2000.0, 1000.0), 1.0, 1.0, 1000.0, 500.0);
        assert_eq!(
            (wide.w, wide.h),
            (1000.0, 1000.0),
            "a square frame is as tall as a wide picture"
        );
        let tall = frame((1000.0, 2000.0), 1.0, 1.0, 500.0, 1000.0);
        assert_eq!((tall.w, tall.h), (1000.0, 1000.0));
        let strip = frame((2000.0, 1000.0), 6.4, 1.0, 1000.0, 500.0);
        assert_eq!(
            (strip.w, strip.h),
            (2000.0, 312.5),
            "a banner frame is as wide as the picture"
        );
        let zoomed = frame((2000.0, 1000.0), 1.0, 2.0, 1000.0, 500.0);
        assert_eq!((zoomed.w, zoomed.h), (500.0, 500.0));
        // Zoom is bounded.
        assert_eq!(
            frame((2000.0, 1000.0), 1.0, 100.0, 0.0, 0.0).w,
            1000.0 / MAX_ZOOM
        );
    }

    #[test]
    fn the_frame_never_leaves_the_picture() {
        let f = frame((2000.0, 1000.0), 1.0, 2.0, -5000.0, 9000.0);
        assert!(
            f.x >= 0.0 && f.y >= 0.0 && f.x + f.w <= 2000.0 && f.y + f.h <= 1000.0,
            "{f:?}"
        );
        let g = frame((2000.0, 1000.0), 1.0, 2.0, 9000.0, -9000.0);
        assert_eq!((g.x + g.w, g.y), (2000.0, 0.0));
    }

    #[test]
    fn the_picture_is_fitted_and_centred_in_the_preview() {
        let (scale, (x, y)) = fit((2000.0, 1000.0), (560.0, 340.0));
        assert_eq!(scale, 0.28);
        assert_eq!((x, y), (0.0, 30.0));
    }

    #[test]
    fn cutting_makes_a_picture_of_the_frames_shape_and_never_blows_a_small_one_up() {
        let dir = std::env::temp_dir().join(format!("viv-crop-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("creates");
        let source = dir.join("in.png");
        image::RgbaImage::from_pixel(300, 200, image::Rgba([200, 30, 30, 255]))
            .save(&source)
            .expect("writes");
        let dest = dir.join("out.png");
        cut(
            &source,
            Frame {
                x: 50.0,
                y: 0.0,
                w: 200.0,
                h: 200.0,
            },
            Shape::of(Kind::Avatar),
            &dest,
        )
        .expect("cuts");
        let out = image::open(&dest).expect("reads");
        assert_eq!(
            (out.width(), out.height()),
            (200, 200),
            "a small crop keeps its own size"
        );
        assert!(
            cut(
                &dir.join("missing.png"),
                Frame {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0
                },
                Shape::of(Kind::Avatar),
                &dest
            )
            .is_err()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
