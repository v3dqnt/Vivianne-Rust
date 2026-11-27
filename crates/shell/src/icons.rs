//! The app's icons: Iconoir (MIT), served under the names the app and the Kit's components ask for, so no caller
//! changes. The files and what each one is are in `assets/iconoir/`.

macro_rules! icon {
    ($name:literal) => {
        (
            $name,
            include_bytes!(concat!("../assets/iconoir/", $name, ".svg")).as_slice(),
        )
    };
}

const ICONS: [(&str, &[u8]); 94] = [
    icon!("arrow-down"),
    icon!("arrow-up"),
    icon!("asterisk"),
    icon!("ban"),
    icon!("bell"),
    icon!("book-open"),
    icon!("bot"),
    icon!("calendar"),
    icon!("case-sensitive"),
    icon!("check"),
    icon!("chevron-down"),
    icon!("chevron-left"),
    icon!("chevron-right"),
    icon!("chevron-up"),
    icon!("chevrons-up-down"),
    icon!("circle-check"),
    icon!("circle-x"),
    icon!("close"),
    icon!("copy"),
    icon!("database"),
    icon!("ellipsis"),
    icon!("external-link"),
    icon!("eye"),
    icon!("eye-off"),
    icon!("file"),
    icon!("file-archive"),
    icon!("file-badge"),
    icon!("file-box"),
    icon!("file-braces"),
    icon!("file-chart-pie"),
    icon!("file-code"),
    icon!("file-cog"),
    icon!("file-diff"),
    icon!("file-image"),
    icon!("file-key"),
    icon!("file-lock"),
    icon!("file-music"),
    icon!("file-spreadsheet"),
    icon!("file-terminal"),
    icon!("file-text"),
    icon!("file-type"),
    icon!("file-video-camera"),
    icon!("folder"),
    icon!("folder-archive"),
    icon!("folder-check"),
    icon!("folder-code"),
    icon!("folder-cog"),
    icon!("folder-git"),
    icon!("folder-git-2"),
    icon!("folder-heart"),
    icon!("folder-open"),
    icon!("folder-tree"),
    icon!("frame"),
    icon!("github"),
    icon!("globe"),
    icon!("heart"),
    icon!("inbox"),
    icon!("info"),
    icon!("inspector"),
    icon!("layout-dashboard"),
    icon!("loader"),
    icon!("maximize"),
    icon!("mic"),
    icon!("minimize"),
    icon!("minus"),
    icon!("network"),
    icon!("palette"),
    icon!("panel-bottom"),
    icon!("panel-bottom-open"),
    icon!("panel-left"),
    icon!("panel-left-close"),
    icon!("panel-left-open"),
    icon!("panel-right"),
    icon!("panel-right-close"),
    icon!("panel-right-open"),
    icon!("pencil"),
    icon!("plus"),
    icon!("refresh-cw"),
    icon!("replace"),
    icon!("search"),
    icon!("settings"),
    icon!("sort-ascending"),
    icon!("sort-descending"),
    icon!("square"),
    icon!("square-terminal"),
    icon!("star"),
    icon!("star-fill"),
    icon!("triangle-alert"),
    icon!("undo-2"),
    icon!("user"),
    icon!("window-close"),
    icon!("window-maximize"),
    icon!("window-minimize"),
    icon!("window-restore"),
];

/// The Iconoir drawing for an asset path such as `icons/search.svg`, when there is one.
#[must_use]
pub fn load(path: &str) -> Option<&'static [u8]> {
    let name = path.strip_prefix("icons/")?.strip_suffix(".svg")?;
    ICONS.iter().find(|(n, _)| *n == name).map(|(_, bytes)| *bytes)
}

#[cfg(test)]
mod tests {
    use gpui_kit::AssetSource as _;

    use super::*;

    #[test]
    fn every_icon_replaces_one_of_the_kits_and_is_what_the_app_gets() {
        let kit = gpui_kit::assets::AllAssets::new("");
        let app = crate::assets::Assets::new();
        for (name, bytes) in ICONS {
            let path = format!("icons/{name}.svg");
            assert!(std::str::from_utf8(bytes).is_ok_and(|t| t.contains("<svg")), "{name}");
            // A name the Kit does not have would never be asked for.
            assert!(kit.load(&path).is_ok_and(|found| found.is_some()), "{name}");
            assert_eq!(app.load(&path).expect("loads").as_deref(), Some(bytes), "{name}");
        }
        assert!(load("icons/no-such-icon.svg").is_none());
        assert!(load("logos/openai.svg").is_none());
    }
}
