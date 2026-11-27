//! The app's picture files: Iconoir icons over Kit's icon set (see `icons`), plus the company and app logos for the Providers page. The logos are
//! single-colour marks (from the open Simple Icons set, CC0) kept in `assets/logos/` and compiled in; they are drawn
//! in the text colour, so they follow the theme. A name with no mark gets a monogram instead (see `logo`).

use std::borrow::Cow;

use gpui_kit::{AssetSource, SharedString};

/// Every logo, by the name a caller asks for.
const LOGOS: [(&str, &[u8]); 16] = [
    (
        "alibabacloud",
        include_bytes!("../assets/logos/alibabacloud.svg"),
    ),
    ("anthropic", include_bytes!("../assets/logos/anthropic.svg")),
    ("claude", include_bytes!("../assets/logos/claude.svg")),
    (
        "cloudflare",
        include_bytes!("../assets/logos/cloudflare.svg"),
    ),
    ("github", include_bytes!("../assets/logos/github.svg")),
    (
        "githubcopilot",
        include_bytes!("../assets/logos/githubcopilot.svg"),
    ),
    ("google", include_bytes!("../assets/logos/google.svg")),
    (
        "googlegemini",
        include_bytes!("../assets/logos/googlegemini.svg"),
    ),
    (
        "huggingface",
        include_bytes!("../assets/logos/huggingface.svg"),
    ),
    ("meta", include_bytes!("../assets/logos/meta.svg")),
    ("nvidia", include_bytes!("../assets/logos/nvidia.svg")),
    ("ollama", include_bytes!("../assets/logos/ollama.svg")),
    ("openai", include_bytes!("../assets/logos/openai.svg")),
    (
        "perplexity",
        include_bytes!("../assets/logos/perplexity.svg"),
    ),
    ("windsurf", include_bytes!("../assets/logos/windsurf.svg")),
    ("x", include_bytes!("../assets/logos/x.svg")),
];

/// The app's typeface: Allo (Josh Puckett, SIL Open Font License 1.1; the licence is beside the files in
/// `assets/fonts/`). Five weights, all of the one family name `Allo`.
const FONTS: [&[u8]; 5] = [
    include_bytes!("../assets/fonts/Allo-Light.otf"),
    include_bytes!("../assets/fonts/Allo-Regular.otf"),
    include_bytes!("../assets/fonts/Allo-Medium.otf"),
    include_bytes!("../assets/fonts/Allo-SemiBold.otf"),
    include_bytes!("../assets/fonts/Allo-Bold.otf"),
];

/// The family name the fonts are registered under.
pub const FONT_FAMILY: &str = "Allo";

/// The font files, for the text system to register at start.
#[must_use]
pub fn fonts() -> Vec<Cow<'static, [u8]>> {
    FONTS.iter().map(|b| Cow::Borrowed(*b)).collect()
}

/// The path a logo is loaded by, or `None` when there is no mark for that name.
#[must_use]
pub fn logo_path(name: &str) -> Option<String> {
    LOGOS
        .iter()
        .any(|(n, _)| *n == name)
        .then(|| format!("logos/{name}.svg"))
}

/// The icons and the logos.
pub struct Assets {
    kit: gpui_kit::assets::AllAssets,
}

impl Assets {
    #[must_use]
    pub fn new() -> Self {
        Self {
            kit: gpui_kit::assets::AllAssets::new(""),
        }
    }
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        if let Some(name) = path
            .strip_prefix("logos/")
            .and_then(|p| p.strip_suffix(".svg"))
        {
            return Ok(LOGOS
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, bytes)| Cow::Borrowed(*bytes)));
        }
        if let Some(bytes) = crate::icons::load(path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        self.kit.load(path)
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut all = self.kit.list(path)?;
        all.extend(
            LOGOS
                .iter()
                .map(|(n, _)| format!("logos/{n}.svg"))
                .filter(|p| p.starts_with(path))
                .map(SharedString::from),
        );
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_typeface_is_five_fonts_that_all_belong_to_one_family() {
        assert_eq!(fonts().len(), 5);
        for bytes in FONTS {
            // An OpenType file starts with its tag, and names its family in the name table (as UTF-16).
            assert_eq!(&bytes[..4], b"OTTO");
            let family: Vec<u8> = FONT_FAMILY
                .encode_utf16()
                .flat_map(u16::to_be_bytes)
                .collect();
            assert!(bytes.windows(family.len()).any(|w| w == family.as_slice()));
        }
    }

    #[test]
    fn every_logo_is_an_svg_and_is_found_by_its_path_and_an_unknown_one_is_not() {
        for (name, bytes) in LOGOS {
            assert!(
                std::str::from_utf8(bytes).is_ok_and(|t| t.contains("<svg")),
                "{name}"
            );
            assert_eq!(
                logo_path(name).as_deref(),
                Some(format!("logos/{name}.svg").as_str())
            );
        }
        assert!(logo_path("not-a-company").is_none());
        let assets = Assets::new();
        assert!(assets.load("logos/openai.svg").expect("loads").is_some());
        assert!(assets.load("logos/nope.svg").expect("loads").is_none());
    }
}
