//! The mini characters (Kenney, CC0) that stand for agents. Each is a skinned 3D model with animations; `render`
//! draws it in software, and the pictures of one loop of an animation are kept as PNG files under
//! `~/.viv/characters/` so a character is drawn once. `CharacterView` plays a loop; ask for one with
//! [`CharacterView::new`] wherever an agent needs a face.

mod render;

use std::path::PathBuf;

use gpui_kit::prelude::*;
use gpui_kit::{Context, Window, div, img, px};

/// Bump when the drawing changes, so old pictures are not reused.
const VERSION: u32 = 1;
const COLORMAP: &[u8] = include_bytes!("../../assets/characters/colormap.png");
/// Pictures are this many pixels square.
const SIZE: usize = 192;
const FPS: f32 = 12.0;

macro_rules! characters {
    ($($name:literal),* $(,)?) => {
        /// Every character, by name.
        pub const NAMES: &[&str] = &[$($name),*];
        fn model_bytes(name: &str) -> Option<&'static [u8]> {
            match name {
                $($name => Some(include_bytes!(concat!("../../assets/characters/character-", $name, ".glb")) as &[u8]),)*
                _ => None,
            }
        }
    };
}

characters!(
    "male-a", "male-b", "male-c", "male-d", "male-e", "male-f", "male-g", "female-a", "female-b",
    "female-c", "female-d", "female-e", "female-f", "alien", "knight", "raider", "robot",
    "soldier", "worker", "zombie",
);

/// The animations worth offering for an agent, by the model's own names.
pub const ANIMATIONS: &[&str] = &[
    "idle", "walk", "sprint", "jump", "emote-yes", "emote-no", "pick-up", "interact-right",
    "sit", "crouch", "attack-melee-right", "die",
];

/// One loop, drawn: its pictures and how long each shows.
#[derive(Debug, Clone)]
pub struct Frames {
    pub paths: Vec<PathBuf>,
    pub ms: u64,
}

fn folder(name: &str, animation: &str) -> PathBuf {
    vivianne_ai::paths::viv_home_path(&["characters", &format!("v{VERSION}"), name, animation])
}

/// The pictures of `animation` for `name`, drawing them first if they are not kept yet. Slow the first time.
pub fn ensure(name: &str, animation: &str) -> Result<Frames, String> {
    let dir = folder(name, animation);
    let meta = dir.join("meta.json");
    if let Some(done) = std::fs::read_to_string(&meta)
        .ok()
        .and_then(|t| serde_json::from_str::<(usize, u64)>(&t).ok())
        && (0..done.0).all(|i| dir.join(format!("{i}.png")).is_file())
    {
        return Ok(Frames {
            paths: (0..done.0).map(|i| dir.join(format!("{i}.png"))).collect(),
            ms: done.1,
        });
    }
    let bytes = model_bytes(name).ok_or_else(|| format!("no character called {name}"))?;
    let model = render::Model::load(bytes, COLORMAP)?;
    let seconds = model.duration(animation);
    let count = ((seconds * FPS).round() as usize).clamp(1, 48);
    let pictures = model.render_loop(animation, count, SIZE);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut paths = Vec::with_capacity(count);
    for (i, rgba) in pictures.into_iter().enumerate() {
        let path = dir.join(format!("{i}.png"));
        image::save_buffer(&path, &rgba, SIZE as u32, SIZE as u32, image::ColorType::Rgba8)
            .map_err(|e| e.to_string())?;
        paths.push(path);
    }
    let ms = if seconds > 0.0 {
        (seconds * 1000.0 / count as f32) as u64
    } else {
        1000
    };
    let _ = std::fs::write(
        &meta,
        serde_json::to_string(&(count, ms)).unwrap_or_default(),
    );
    Ok(Frames { paths, ms })
}

/// A character playing one animation, `size` points square. Draws nothing until its pictures are ready.
pub struct CharacterView {
    frames: Option<Frames>,
    at: usize,
    size: f32,
    problem: Option<String>,
}

impl CharacterView {
    pub fn new(name: &str, animation: &str, size: f32, cx: &mut Context<Self>) -> Self {
        let (name, animation) = (name.to_owned(), animation.to_owned());
        cx.spawn(async move |this, cx| {
            let Some(done) = crate::backend::spawn(async move {
                tokio::task::spawn_blocking(move || ensure(&name, &animation))
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r)
            }) else {
                return;
            };
            let frames = match done.await {
                Ok(Ok(frames)) => frames,
                Ok(Err(e)) => {
                    let _ = this.update(cx, |v, cx| {
                        v.problem = Some(e);
                        cx.notify();
                    });
                    return;
                }
                Err(_) => return,
            };
            let (count, ms) = (frames.paths.len(), frames.ms.max(40));
            if this
                .update(cx, |v, cx| {
                    v.frames = Some(frames);
                    cx.notify();
                })
                .is_err()
            {
                return;
            }
            // Many loops are one picture (a held pose): nothing to play.
            if count < 2 {
                return;
            }
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(ms))
                    .await;
                if this
                    .update(cx, |v, cx| {
                        v.at = (v.at + 1) % count;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Self {
            frames: None,
            at: 0,
            size,
            problem: None,
        }
    }
}

impl Render for CharacterView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let holder = div().size(px(self.size)).flex_none();
        match (&self.frames, &self.problem) {
            (Some(frames), _) => holder.child(img(frames.paths[self.at % frames.paths.len()].clone()).size_full()),
            (None, Some(why)) => holder.child(
                div()
                    .text_size(px(10.0))
                    .text_color(crate::theme::color(crate::theme::DANGER))
                    .child(why.clone()),
            ),
            (None, None) => holder,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_character_draws_something_in_every_listed_animation() {
        for name in NAMES {
            let model =
                render::Model::load(model_bytes(name).expect("model"), COLORMAP).expect("loads");
            let names = model.animation_names();
            for animation in ANIMATIONS {
                assert!(names.iter().any(|n| n == animation), "{name} lacks {animation}");
            }
            let frame = &model.render_loop("idle", 2, 64)[0];
            assert!(frame.chunks(4).any(|p| p[3] > 0), "{name} drew nothing");
        }
    }

    #[test]
    fn a_picture_for_looking_at() {
        let model = render::Model::load(model_bytes("male-a").expect("model"), COLORMAP).expect("loads");
        for (i, anim) in ["idle", "walk"].iter().enumerate() {
            let frames = model.render_loop(anim, 4, 192);
            image::save_buffer(
                std::env::temp_dir().join(format!("viv-char-{i}.png")),
                &frames[1],
                192,
                192,
                image::ColorType::Rgba8,
            )
            .expect("saves");
        }
    }
}
