//! GPUI Kit spike: does it build on Windows, and do its text input and
//! markdown view work for chat? Throwaway; findings go in docs/spikes.md.
#![allow(clippy::pedantic, clippy::expect_used)]

use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::text::TextView;
use gpui_kit::*;

const REPLY: &str = "\
I'll check the project first, then run the tests.

- **Fast:** one pass over the crates
- `cargo test` for the gate

```rust
fn main() {
    println!(\"hello\");
}
```

| crate | role |
|---|---|
| `core` | domain types |
| `runtime` | agent loop |
";

struct Spike {
    input: Entity<TextareaState>,
}

impl Spike {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 6));
        Self { input }
    }
}

impl Render for Spike {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .p(px(24.0))
            .child(TextView::markdown("reply", REPLY))
            .child(Textarea::new(&self.input))
    }
}

fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| Spike::new(window, cx))
        })
        .expect("open window");
    });
}
