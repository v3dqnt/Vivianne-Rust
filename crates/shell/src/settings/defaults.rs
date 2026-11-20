//! What Settings starts from: the app's own defaults and catalogs (shortcuts, limits, optional
//! components, coding agents, starter models). Nothing here pretends to be the user's data; what
//! the user has (keys, memory, skills...) is read from where it is kept, and is empty until then.

use super::values::*;

/// Fill `values` with the defaults. Called once when Settings opens.
pub fn fill(v: &mut Values) {
    v.mode = "dark".into();
    v.scale = 1.0;
    v.chat_density = "comfortable".into();
    v.chat_width = "normal".into();
    v.visuals = "auto".into();
    v.robot_scale = 1.0;
    v.robot_move = "busy".into();
    v.sidebar_side = "left".into();
    v.search_provider = "auto".into();
    v.guides_repo = "https://github.com/v3dqnt/vivianne-guides".into();
    v.remember_across_sessions = true;
    v.auto_consolidate = true;
    v.auto_extract = true;
    v.refuse_secrets = true;
    v.cloud_prefer = "local".into();
    v.usage_range = super::usage_data::DEFAULT_RANGE.into();
    reliability(v);
    layout(v);
    components(v);
}

fn reliability(v: &mut Values) {
    v.retry_transient = true;
    v.limits = vec![
        Limit {
            id: "shellTimeoutSecs".into(),
            label: "Shell command timeout".into(),
            unit: "seconds".into(),
            min: 5.0,
            max: 600.0,
            default: 120.0,
            value: 120.0,
            hint: String::new(),
        },
        Limit {
            id: "maxActiveSubagents".into(),
            label: "Subagents at once".into(),
            unit: "agents".into(),
            min: 1.0,
            max: 16.0,
            default: 8.0,
            value: 8.0,
            hint: "How many subagents may work at the same time. Each is a full agent \
                   run, so more of them use more of your plan and your machine."
                .into(),
        },
    ];
}

fn layout(v: &mut Values) {
    v.sidebar_side = "left".into();
    let sc = |id: &str, label: &str, group: &str, keys: &str| Shortcut {
        id: id.into(),
        label: label.into(),
        group: group.into(),
        default_keys: keys.into(),
        keys: keys.into(),
    };
    v.shortcuts = vec![
        sc("sidebar.toggle", "Show or hide the sidebar", "App", "Mod+B"),
        sc("settings.open", "Open Settings", "App", "Mod+,"),
        sc("chat.new", "New chat", "Chat", "Mod+Shift+N"),
        sc("chat.focus", "Focus the message box", "Chat", "Mod+L"),
        sc("chat.stop", "Stop the agent", "Chat", "Mod+Shift+."),
        sc("project.1", "Switch to project 1", "Projects", "Mod+1"),
        sc("project.2", "Switch to project 2", "Projects", "Mod+2"),
        sc("project.3", "Switch to project 3", "Projects", "Mod+3"),
        sc("project.4", "Switch to project 4", "Projects", "Mod+4"),
        sc("project.5", "Switch to project 5", "Projects", "Mod+5"),
        sc("project.6", "Switch to project 6", "Projects", "Mod+6"),
        sc("project.7", "Switch to project 7", "Projects", "Mod+7"),
        sc("project.8", "Switch to project 8", "Projects", "Mod+8"),
        sc("project.9", "Switch to project 9", "Projects", "Mod+9"),
    ];
}

fn components(v: &mut Values) {
    let comp = |id: &str, name: &str, size: &str, desc: &str, installed: bool| ComponentEntry {
        id: id.into(),
        name: name.into(),
        size: size.into(),
        desc: desc.into(),
        installed,
        running: false,
        progress: None,
        note: None,
    };
    v.components = vec![
        comp(
            "camoufox",
            "Browser engine (Camoufox)",
            "~300 MB",
            "for web browsing, previews, and search",
            false,
        ),
        comp(
            "docker",
            "Docker Desktop",
            "~500 MB",
            "for running untrusted code in a sandbox",
            false,
        ),
        comp(
            "parakeet",
            "Parakeet TDT 0.6B",
            "~628 MB",
            "Viv Instant's most accurate voice model, on-device",
            false,
        ),
    ];
}

/// How many catalog entries a page shows.
pub const HUB_PAGE: usize = 40;

/// Open models worth starting with, mirroring the old app's STARTERS list.
pub struct StarterFamily {
    pub name: String,
    pub note: String,
    pub models: Vec<StarterModel>,
}

pub struct StarterModel {
    pub label: String,
    pub size_gb: f64,
}

/// The curated starter list, family by family.
pub fn starters() -> Vec<StarterFamily> {
    let m = |label: &str, size: f64| StarterModel {
        label: label.into(),
        size_gb: size,
    };
    vec![
        StarterFamily {
            name: "Qwen3".into(),
            note: "Strong all-rounder; thinks before answering".into(),
            models: vec![
                m("Qwen3 0.6B Q8_0", 0.64),
                m("Qwen3 1.7B", 1.8),
                m("Qwen3 4B", 2.5),
                m("Qwen3 8B", 5.0),
                m("Qwen3 14B", 9.0),
                m("Qwen3 32B", 19.8),
            ],
        },
        StarterFamily {
            name: "Gemma 3".into(),
            note: "Google's open model; sees images".into(),
            models: vec![
                m("Gemma 3 4B", 3.1),
                m("Gemma 3 12B", 8.1),
                m("Gemma 3 27B", 17.2),
            ],
        },
        StarterFamily {
            name: "gpt-oss".into(),
            note: "OpenAI's open-weight reasoning model".into(),
            models: vec![m("gpt-oss 20B mxfp4", 12.1)],
        },
        StarterFamily {
            name: "Llama".into(),
            note: "Meta's widely used open models".into(),
            models: vec![m("Llama 3.2 3B", 2.0), m("Llama 3.1 8B", 4.9)],
        },
        StarterFamily {
            name: "Phi-4 mini".into(),
            note: "Microsoft's small, quick model".into(),
            models: vec![m("Phi-4 mini 3.8B", 2.5)],
        },
        StarterFamily {
            name: "Mistral Nemo".into(),
            note: "Good at writing and many languages".into(),
            models: vec![m("Mistral Nemo 12B", 7.5)],
        },
    ]
}
