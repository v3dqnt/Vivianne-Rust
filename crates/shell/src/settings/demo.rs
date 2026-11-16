//! Deterministic demo data for the Settings pages, generated with a small
//! LCG (no `rand` crate). When the settings store arrives this is replaced by
//! whatever it loads; the pages only read from `Values`.

use std::collections::BTreeMap;

use super::values::*;

/// A tiny linear congruential generator. Same seed, same data, every run.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }

    fn below(&mut self, max: u64) -> u64 {
        self.next() % max.max(1)
    }

    /// 0.0 to just under 1.0.
    fn unit(&mut self) -> f64 {
        self.next() as f64 / (1_u64 << 53) as f64
    }
}

/// Fill `values` with the demo world. Called once when Settings opens.
pub fn fill(v: &mut Values) {
    // Appearance defaults matching the old app before its own store runs.
    v.mode = "dark".into();
    v.scale = 1.0;
    v.chat_density = "comfortable".into();
    v.chat_width = "normal".into();
    v.visuals = "auto".into();
    v.read_aloud = false;
    v.robot_scale = 1.0;
    v.robot_move = "busy".into();
    v.face = 0;
    v.sidebar_side = "left".into();

    usage(v);
    providers(v);
    local_models(v);
    reliability(v);
    mods(v);
    connections(v);
    web(v);
    memory(v);
    vault(v);
    skills(v);
    diagnostics(v);
    layout(v);
    components(v);
    updates(v);
}

fn usage(v: &mut Values) {
    let mut rng = Lcg(0x5EED_1111);
    v.usage_days.clear();
    for offset in 0..371_u32 {
        let weekdayish = (offset + 4) % 7;
        let base: u64 = match weekdayish {
            5 | 6 => 0,
            _ => rng.below(90),
        };
        if base == 0 {
            continue;
        }
        let requests = base as u32 + 2;
        let input = requests as u64 * (rng.below(6_000) + 2_000);
        let output = requests as u64 * (rng.below(1_400) + 200);
        // Mostly priced, with a handful of recent unpriced-only days so the
        // partial-cost footnote shows up.
        let cost = if offset < 9 && offset % 3 == 0 {
            None
        } else {
            Some((requests as f64) * 0.021 + (output as f64) * 0.000_045)
        };
        v.usage_days.push(UsageDay {
            offset,
            requests,
            input,
            output,
            cost,
        });
    }
    v.model_usage = vec![
        ModelUsage {
            model: "claude-sonnet-4-5".into(),
            provider: "anthropic".into(),
            requests: 412,
            input: 2_481_203,
            output: 318_740,
            cost: Some(21.84),
        },
        ModelUsage {
            model: "gpt-5.1".into(),
            provider: "openai".into(),
            requests: 187,
            input: 1_204_882,
            output: 141_090,
            cost: Some(9.13),
        },
        ModelUsage {
            model: "gemini-3-flash".into(),
            provider: "google".into(),
            requests: 96,
            input: 512_400,
            output: 66_120,
            cost: Some(1.42),
        },
        ModelUsage {
            model: "qwen3-4b-q8_0 (local)".into(),
            provider: "local".into(),
            requests: 231,
            input: 1_491_022,
            output: 208_503,
            cost: None,
        },
    ];
}

fn providers(v: &mut Values) {
    for (id, key) in [
        ("anthropic", "sk-ant-api03-demo-5f2b"),
        ("google", "AIzaSyD-demo-9a1c"),
        ("openrouter", "sk-or-v1-demo-7731"),
    ] {
        v.api_keys.insert(id.into(), key.into());
    }
    let mut pool = BTreeMap::new();
    pool.insert(
        "openai".to_owned(),
        vec![
            PoolKey {
                label: "main".into(),
                key: "sk-proj-demo-1".into(),
                requests: 4_120,
                state: KEY_READY,
            },
            PoolKey {
                label: "backup key".into(),
                key: "sk-proj-demo-2".into(),
                requests: 318,
                state: KEY_READY,
            },
            PoolKey {
                label: "spare".into(),
                key: "sk-proj-demo-3".into(),
                requests: 22,
                state: KEY_RATE,
            },
        ],
    );
    v.pool = pool;
    v.rotation.insert("openai".into(), "fill_first".into());
    v.chatgpt_connected = false;
    v.chatgpt_email = "you@example.com".into();
    v.chatgpt_shared_at_signin = true;
    v.quick_model = "auto".into();
    v.tier_easy = String::new();
    v.tier_medium = String::new();
    v.tier_hard = String::new();
    v.agents = vec![
        agent(
            "claude-code",
            "Claude Code",
            "Anthropic's coding agent",
            "npx",
            AGENT_READY,
        ),
        agent(
            "codex",
            "Codex",
            "OpenAI's coding agent",
            "npx",
            AGENT_READY,
        ),
        agent(
            "gemini-cli",
            "Gemini CLI",
            "Google's coding agent",
            "npx",
            AGENT_READY,
        ),
        agent(
            "qwen-code",
            "Qwen Code",
            "Alibaba's open-weight coding agent",
            "npx",
            AGENT_SETUP,
        ),
        agent(
            "opencode",
            "OpenCode",
            "Open-source terminal coding agent",
            "npx",
            AGENT_SETUP,
        ),
        agent("crush", "Crush", "Charm's coding agent", "npx", AGENT_SETUP),
        agent(
            "roo-code",
            "Roo Code",
            "Community fork of Cline",
            "npx",
            AGENT_SETUP,
        ),
        agent(
            "kilo-code",
            "Kilo Code",
            "Cline fork with its own model plans",
            "npx",
            AGENT_SETUP,
        ),
        agent(
            "cline",
            "Cline",
            "The agent Roo Code forked from",
            "npx",
            AGENT_SETUP,
        ),
        agent("goose", "Goose", "Block's open agent", "uvx", AGENT_MISSING),
        agent(
            "aider",
            "Aider",
            "Pair programming in the terminal",
            "uvx",
            AGENT_MISSING,
        ),
        agent(
            "prowler",
            "Prowler",
            "Security checks on your repo",
            "uvx",
            AGENT_MISSING,
        ),
    ];
    v.custom_providers.push(CustomProvider {
        id: "custom-acme".into(),
        name: "Acme".into(),
        base_url: "https://api.acme.dev/v1".into(),
        format: "openai".into(),
        has_key: true,
        models: vec![CustomModel {
            id: "acme-large".into(),
            ctx: 200_000,
            max_out: Some(8_192),
        }],
    });
}

fn agent(id: &str, name: &str, desc: &str, launcher: &str, state: u8) -> CodingAgent {
    CodingAgent {
        id: id.into(),
        name: name.into(),
        desc: desc.into(),
        launcher: launcher.into(),
        state,
        downloading: None,
    }
}

fn local_models(v: &mut Values) {
    v.local_tab = 1;
    v.hardware = Some(Hardware {
        ram_gb: 64,
        threads: 24,
        gpus: vec![("NVIDIA GeForce RTX 4080".into(), 12)],
        unified: false,
        os: "Windows".into(),
    });
    v.llama_ready = true;
    v.endpoints.push(LocalEndpoint {
        id: "qwen3-4b".into(),
        name: "Qwen3 4B".into(),
        engine: "llamaCpp".into(),
        file: "qwen3-4b-q8_0.gguf".into(),
        port: 8317,
        ctx: 32_768,
        ngl: 999,
        kv: "q8_0".into(),
        device: String::new(),
        threads: String::new(),
        reuse: String::new(),
        speculative: "n-gram".into(),
        cache_ram: String::new(),
        idle: String::new(),
        mlock: false,
        extra: String::new(),
        state: EP_STOPPED,
        more_open: false,
        more: BTreeMap::new(),
    });
    v.hf_results = vec![
        HfRepo {
            repo: "deepseek-ai/DeepSeek-R1-Distill-Qwen-7B-GGUF".into(),
            downloads: 1_482_301,
            files: vec![
                ("deepseek-r1-qwen-7b-q4_k_m.gguf".into(), 4.7),
                ("deepseek-r1-qwen-7b-q8_0.gguf".into(), 8.1),
            ],
        },
        HfRepo {
            repo: "Qwen/Qwen3-Coder-30B-A3B-GGUF".into(),
            downloads: 903_544,
            files: vec![("qwen3-coder-30b-a3b-q4_k_m.gguf".into(), 18.6)],
        },
    ];
    v.ollama_models = vec!["qwen3:8b".into(), "llama3.2:3b".into(), "phi4-mini".into()];
    v.sd_ready = false;
    v.image_models = vec![
        ImageModel {
            id: "sd-turbo".into(),
            name: "SD 1.5 Turbo".into(),
            gb: 4.1,
            note: "Quick sketches, a few seconds a picture.".into(),
            owned: false,
        },
        ImageModel {
            id: "sdxl-base".into(),
            name: "SDXL Base".into(),
            gb: 6.9,
            note: "The general-purpose pick; slower but sharper.".into(),
            owned: false,
        },
        ImageModel {
            id: "flux-schnell".into(),
            name: "FLUX.1 schnell".into(),
            gb: 11.8,
            note: "The best looking of the three; needs most of a 12 GB card.".into(),
            owned: false,
        },
    ];
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

fn mods(v: &mut Values) {
    v.guides_repo = "https://github.com/v3dqnt/vivianne-guides".into();
    v.mods = vec![
        ModEntry {
            id: "brief".into(),
            name: "brief".into(),
            desc: "Answers in three sentences or fewer, no preamble.".into(),
            enabled: true,
        },
        ModEntry {
            id: "tests-first".into(),
            name: "tests-first".into(),
            desc: "Writes the failing test before touching source code.".into(),
            enabled: false,
        },
        ModEntry {
            id: "no-commits".into(),
            name: "no-commits".into(),
            desc: "Never runs git commit; leaves that to you.".into(),
            enabled: true,
        },
        ModEntry {
            id: "pr-title".into(),
            name: "pr-title".into(),
            desc: "Titles pull requests in the repo's own convention.".into(),
            enabled: false,
        },
    ];
    let tool = |id: &str, label: &str, enabled: bool, group: &str| ToolEntry {
        id: id.into(),
        label: label.into(),
        enabled,
        group: group.into(),
    };
    v.tools = vec![
        tool("read_file", "Read file", true, "Files"),
        tool("write_file", "Write file", true, "Files"),
        tool("edit_file", "Edit file", true, "Files"),
        tool("list_dir", "List directory", true, "Files"),
        tool("run_command", "Run command", true, "Shell"),
        tool("run_sandbox", "Run in sandbox", true, "Shell"),
        tool("web_search", "Web search", true, "Web"),
        tool("web_fetch", "Web fetch", true, "Web"),
        tool("remember", "Remember", true, "Memory"),
        tool("recall", "Recall", true, "Memory"),
        tool("open_app", "Open app", true, "Computer"),
        tool("screenshot", "Screenshot", false, "Computer"),
    ];
    v.custom_tools = vec![
        UsedTool {
            name: "clean_builds".into(),
            uses: 14,
            last: "2 days ago".into(),
        },
        UsedTool {
            name: "sort_imports".into(),
            uses: 3,
            last: "3 weeks ago".into(),
        },
    ];
}

fn connections(v: &mut Values) {
    let app = |id: &str, name: &str, cat: &str, auth: u8, connected: bool, desc: &str| AppEntry {
        id: id.into(),
        name: name.into(),
        category: cat.into(),
        auth,
        account: connected.then(|| "you@example.com".into()),
        connected,
        pending: false,
        desc: desc.into(),
    };
    v.apps = vec![
        app(
            "github",
            "GitHub",
            "Dev",
            AUTH_TOKEN,
            true,
            "Repositories, issues and pull requests.",
        ),
        app(
            "linear",
            "Linear",
            "Dev",
            AUTH_OAUTH,
            false,
            "Issues and projects from the agent.",
        ),
        app(
            "sentry",
            "Sentry",
            "Dev",
            AUTH_OAUTH,
            false,
            "Reads recent errors and stack traces.",
        ),
        app(
            "slack",
            "Slack",
            "Comms",
            AUTH_BOT,
            false,
            "Reads channels and sends messages.",
        ),
        app(
            "discord",
            "Discord",
            "Comms",
            AUTH_BOT,
            false,
            "Servers the bot has been added to.",
        ),
        app(
            "gmail",
            "Gmail",
            "Comms",
            AUTH_OAUTH,
            false,
            "Searches mail and drafts replies.",
        ),
        app(
            "notion",
            "Notion",
            "Docs",
            AUTH_OAUTH,
            false,
            "Pages and databases.",
        ),
        app(
            "figma",
            "Figma",
            "Docs",
            AUTH_OAUTH,
            false,
            "Design files and comments.",
        ),
        app(
            "drive",
            "Google Drive",
            "Docs",
            AUTH_OAUTH,
            false,
            "Documents and spreadsheets.",
        ),
        app(
            "stripe",
            "Stripe",
            "Data",
            AUTH_TOKEN,
            false,
            "Payments, invoices and balances.",
        ),
        app(
            "postgres",
            "PostgreSQL",
            "Data",
            AUTH_OPEN,
            false,
            "Read-only queries against your database.",
        ),
        app(
            "supabase",
            "Supabase",
            "Data",
            AUTH_TOKEN,
            false,
            "Tables, storage and logs.",
        ),
    ];
    let mcp = |id: &str, name: &str, kind: &str, detail: &str, tools: u32, env: &str| McpServer {
        id: id.into(),
        name: name.into(),
        kind: kind.into(),
        detail: detail.into(),
        tools,
        env: env.into(),
    };
    v.mcp = vec![
        mcp(
            "playwright",
            "Playwright",
            "stdio",
            "npx @playwright/mcp@latest",
            21,
            "{}",
        ),
        mcp(
            "filesystem",
            "Filesystem bridge",
            "http",
            "http://localhost:9314/mcp",
            8,
            "{}",
        ),
    ];
}

fn web(v: &mut Values) {
    v.search_provider = "auto".into();
    v.search_active = "DuckDuckGo".into();
    v.browser_connected = false;
    v.browsers = vec![
        Browser {
            name: "Chrome".into(),
            state: BROWSER_NO_EXT,
        },
        Browser {
            name: "Edge".into(),
            state: BROWSER_CONNECTED,
        },
        Browser {
            name: "Firefox".into(),
            state: BROWSER_NOT_INSTALLED,
        },
        Browser {
            name: "Arc".into(),
            state: BROWSER_NOT_OPEN,
        },
    ];
}

fn memory(v: &mut Values) {
    v.remember_across_sessions = true;
    v.auto_consolidate = true;
    v.auto_extract = true;
    v.refuse_secrets = true;
    v.memory_entries = 412;
    v.memory_tokens = 68_400;
    v.memory_mb = 2.4;
    v.memory_pages = vec![
        MemoryPage {
            name: "Coding habits".into(),
            question: "How does this person like their code written?".into(),
        },
        MemoryPage {
            name: "This project".into(),
            question: "What matters in the repo I am working in?".into(),
        },
    ];
    v.cloud_prefer = "local".into();
    v.cloud_provider = "openai".into();
    v.cloud_model = "text-embedding-3-small".into();
    v.mem_results = vec![
        MemoryHit {
            text: "Prefers tabs over spaces in the C# projects; spaces in Rust.".into(),
            date: "3 days ago".into(),
        },
        MemoryHit {
            text: "The old frontend lives in packages/frontend — frozen, do not modify.".into(),
            date: "Last week".into(),
        },
        MemoryHit {
            text: "Meeting notes live in docs/meetings; ask before scheduling on Fridays.".into(),
            date: "2 weeks ago".into(),
        },
    ];
}

fn vault(v: &mut Values) {
    let entry =
        |id: &str, label: &str, kind: u8, site: &str, username: &str, hint: &str, used: &str| {
            VaultEntry {
                id: id.into(),
                label: label.into(),
                kind,
                site: site.into(),
                username: username.into(),
                hint: hint.into(),
                used: used.into(),
            }
        };
    v.vault = vec![
        entry(
            "v1",
            "GitHub token",
            VAULT_SECRET,
            "github.com",
            "",
            "ghp_…",
            "used today",
        ),
        entry(
            "v2",
            "Work sign-in",
            VAULT_LOGIN,
            "portal.corp",
            "v3dqn",
            "P***w0rd!",
            "used 2 days ago",
        ),
        entry(
            "v3",
            "Company card",
            VAULT_CARD,
            "portal.corp",
            "",
            "•••• 4821",
            "used last week",
        ),
    ];
}

fn skills(v: &mut Values) {
    v.skills = vec![
        Skill {
            name: "release-checklist".into(),
            desc: "Runs the steps this repo expects before tagging a release.".into(),
            path: "~/.viv/skills/release-checklist".into(),
        },
        Skill {
            name: "sql-review".into(),
            desc: "Reviews migrations for locking behaviour and backfills.".into(),
            path: "~/.viv/skills/sql-review".into(),
        },
        Skill {
            name: "commit-style".into(),
            desc: "Matches this team's commit message conventions.".into(),
            path: "~/.viv/skills/commit-style".into(),
        },
    ];
    v.hub_source = "nvidia".into();
    let hub =
        |name: &str, dir: &str, desc: &str, files: u32, installed: bool, modified: bool| HubSkill {
            name: name.into(),
            dir: dir.into(),
            desc: desc.into(),
            files,
            installed,
            modified,
        };
    v.hub = vec![
        hub(
            "CUDA development",
            "cuda-development",
            "Build, debug and profile CUDA kernels.",
            8,
            true,
            false,
        ),
        hub(
            "CUDA debugger",
            "cuda-debugger",
            "printf and memory debugging in kernels.",
            4,
            false,
            false,
        ),
        hub(
            "Kernel optimization",
            "kernel-optimization",
            "Make CUDA kernels faster, safely.",
            11,
            false,
            false,
        ),
        hub(
            "C++ to CUDA",
            "c-to-cuda",
            "Port C++ compute code to CUDA.",
            6,
            false,
            false,
        ),
        hub(
            "CUTLASS",
            "cutlass",
            "Work with CUTLASS templates.",
            9,
            false,
            false,
        ),
        hub(
            "cuBLAS",
            "cublas",
            "The cuBLAS library and its gotchas.",
            5,
            false,
            false,
        ),
        hub(
            "CUDA debugger rules",
            "cuda-development-rules",
            "Ground rules for kernel work.",
            3,
            false,
            true,
        ),
        hub(
            "NVTX",
            "nvtx",
            "Trace annotations for profiling.",
            3,
            false,
            false,
        ),
        hub(
            "Nsight",
            "nsight",
            "Read nsight-compute reports.",
            6,
            false,
            false,
        ),
        hub(
            "RAPIDS",
            "rapids",
            "cuDF and cuML on the GPU.",
            7,
            false,
            false,
        ),
        hub(
            "Python",
            "python",
            "NVIDIA's Python style and tooling.",
            12,
            false,
            false,
        ),
        hub(
            "RAG FAQ",
            "rag-faq",
            "FAQ-style retrieval blueprint.",
            10,
            false,
            false,
        ),
        hub(
            "RAG semantic",
            "rag-semantic",
            "Semantic search end to end.",
            14,
            false,
            false,
        ),
        hub(
            "Agentic RAG",
            "agentic-rag",
            "Retrieval inside an agent loop.",
            12,
            false,
            false,
        ),
        hub(
            "NeMo scripts",
            "nemo-minimal",
            "Run and fine-tune NeMo models.",
            18,
            false,
            false,
        ),
    ];
}

/// How many hub entries a catalog page shows.
pub const HUB_PAGE: usize = 40;

fn diagnostics(v: &mut Values) {
    v.share_diagnostics = false;
    v.diag_events = 48_219;
    v.diag_since = "Nov 2".into();
    v.diag_kinds = vec![
        ("model.request".into(), 21_880),
        ("tool.call".into(), 14_302),
        ("thread.turn".into(), 6_411),
        ("memory.write".into(), 3_205),
        ("web.search".into(), 1_884),
        ("vault.read".into(), 537),
    ];
    v.crashes = vec![
        Crash {
            time: "Nov 14, 09:12".into(),
            kind: "panicked at local_models: llama-server exited (code 134)".into(),
            trace: vec![
                "0: llama_server_proc::spawn".into(),
                "1: local_models::Endpoint::start".into(),
                "2: agent::turn::run".into(),
            ],
        },
        Crash {
            time: "Nov 9, 22:41".into(),
            kind: "panicked at terminal: failed to decode PTY chunk".into(),
            trace: vec!["0: terminal::feed".into(), "1: shell::Session::pump".into()],
        },
        Crash {
            time: "Oct 31, 17:03".into(),
            kind: "panicked at settings: store write raced a reload".into(),
            trace: vec![
                "0: settings::store::save".into(),
                "1: settings::open".into(),
            ],
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
            true,
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

fn updates(v: &mut Values) {
    let rel = |version: &str, date: &str, notes: &[&str]| Release {
        version: version.into(),
        date: date.into(),
        notes: notes.iter().map(|s| (*s).into()).collect(),
    };
    v.releases = vec![
        rel(
            "0.9.0",
            "Nov 4",
            &[
                "Settings, rebuilt as real pages",
                "The vault keeps values out of the transcript",
                "Faster first paint on cold start",
            ],
        ),
        rel(
            "0.8.2",
            "Oct 21",
            &[
                "Fixed the overlay losing focus after a Windows update",
                "Shell sandbox now clears PATH quietly",
            ],
        ),
        rel(
            "0.8.0",
            "Oct 6",
            &[
                "Local models on llama.cpp or mistral.rs",
                "Key pools with cooldowns",
                "Mods replace prompts-in-a-config-file",
            ],
        ),
    ];
}

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
