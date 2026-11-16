//! Port of `src/types.ts`, phase 1.
//!
//! The TS file starts with `export * from "@oh-my-pi/pi-catalog/effort"` and `.../types`, so the
//! catalog items it re-exports that this phase needs (`Effort`, `Api`, `Provider`, `Usage`,
//! `ThinkingBudgets`, `ModelKind`, `ThinkingConfig`, a first slice of `Model`) live in the
//! "catalog re-exports" section below.
//!
//! Not ported yet in this file (they need modules that are still stubs): `ApiOptionsMap` /
//! `OptionsForApi` (one options struct per provider), the Cursor exec handler types
//! (`CursorExecHandlers`, `CursorMcpCall`, ... need the cursor proto), `AnthropicSlowModeHooks`,
//! `ApiKey`, `OAuthRequestIdentity`, `FallbackParam`, `MessageCreateParamsStreaming`, the
//! `kStreamingPartialJson` symbol, and the catalog `compat`/`pricing` structs. Where a field needs
//! one of them it is typed `serde_json::Value` and marked `// TODO(port-phase)`.

// @generated:mods (keep this block; add code below)
// @end-generated

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Catalog re-exports (`@oh-my-pi/pi-catalog/effort` and `/types`)
// ---------------------------------------------------------------------------

/// User-facing thinking levels, ordered least to most intensive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Minimal,
    Low,
    Medium,
    High,
    #[serde(rename = "xhigh")]
    XHigh,
    Max,
}

pub const THINKING_EFFORTS: [Effort; 6] = [
    Effort::Minimal,
    Effort::Low,
    Effort::Medium,
    Effort::High,
    Effort::XHigh,
    Effort::Max,
];

/// `KnownApi | (string & {})`: any API id; the known ids are listed in [`KNOWN_APIS`].
pub type Api = String;

pub const KNOWN_APIS: [&str; 16] = [
    "openai-completions",
    "openai-responses",
    "openrouter",
    "openai-codex-responses",
    "azure-openai-responses",
    "anthropic-messages",
    "bedrock-converse-stream",
    "google-generative-ai",
    "google-gemini-cli",
    "google-vertex",
    "ollama-chat",
    "cursor-agent",
    "factory-droid-agent",
    "gitlab-duo-agent",
    "devin-agent",
    "apple-foundation-models",
];

/// Any provider-id string.
pub type Provider = String;

/// Catalog kinds used to isolate role-specific runners from session chat models.
pub const MODEL_KINDS: [&str; 10] = [
    "chat", "tiny", "image", "tts", "stt", "search", "judge", "embedding", "rerank", "video",
];

/// Technical capability of a catalog model; absent model kinds mean chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelKind {
    Chat,
    Tiny,
    Image,
    Tts,
    Stt,
    Search,
    Judge,
    Embedding,
    Rerank,
    Video,
}

/// Kinds a provider maps to a runner transport through `kind-apis` in its KDL.
pub const KIND_API_KINDS: [&str; 6] = ["image", "tts", "stt", "embedding", "rerank", "video"];

/// Grounding transport available to chat models selected by the web role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebSearchGrounding {
    Gemini,
    Anthropic,
    Codex,
    Xai,
    Openrouter,
    Openai,
}

/// Non-chat runner protocols accepted by catalog seeds, outside the chat dispatch union.
pub const RUNNER_APIS: [&str; 15] = [
    "local-inference",
    "web-search",
    "typesafe",
    "openrouter-decisions",
    "openai-decisions",
    "workers-ai-systemone",
    "liquid-systemone",
    "openai-images",
    "openrouter-images",
    "xai-tts",
    "openai-speech",
    "openai-embeddings",
    "openrouter-rerank",
    "openrouter-video",
    "openai-transcriptions",
];

/// Kind each single-purpose runner API serves; `local-inference` hosts several kinds.
pub const RUNNER_API_KINDS: [(&str, ModelKind); 14] = [
    ("web-search", ModelKind::Search),
    ("typesafe", ModelKind::Judge),
    ("openrouter-decisions", ModelKind::Judge),
    ("openai-decisions", ModelKind::Judge),
    ("workers-ai-systemone", ModelKind::Judge),
    ("liquid-systemone", ModelKind::Judge),
    ("openai-images", ModelKind::Image),
    ("openrouter-images", ModelKind::Image),
    ("xai-tts", ModelKind::Tts),
    ("openai-speech", ModelKind::Tts),
    ("openai-embeddings", ModelKind::Embedding),
    ("openrouter-rerank", ModelKind::Rerank),
    ("openrouter-video", ModelKind::Video),
    ("openai-transcriptions", ModelKind::Stt),
];

/// Kind a runner API serves; `None` for chat transports and multi-kind `local-inference`.
pub fn runner_api_kind(api: &str) -> Option<ModelKind> {
    RUNNER_API_KINDS
        .iter()
        .find(|(name, _)| *name == api)
        .map(|(_, kind)| *kind)
}

/// Catalog APIs `generate_image` runs through a pi-ai image client.
pub const IMAGE_GENERATION_APIS: [&str; 7] = [
    "openai-images",
    "openrouter-images",
    "google-generative-ai",
    "google-gemini-cli",
    "openai-responses",
    "openai-codex-responses",
    "factory-droid-agent",
];

const CHAT_TRANSPORT_KINDS: [ModelKind; 2] = [ModelKind::Chat, ModelKind::Tiny];
const IMAGE_CHAT_TRANSPORT_KINDS: [ModelKind; 3] = [ModelKind::Chat, ModelKind::Tiny, ModelKind::Image];

/// Kinds a model on `api` may declare: a runner api serves its own kind; a chat transport serves
/// `chat` and `tiny`, plus `image` when `generate_image` runs it. `None` for `local-inference`.
pub fn served_kinds(api: &str) -> Option<Vec<ModelKind>> {
    if api == "local-inference" {
        return None;
    }
    if let Some(kind) = runner_api_kind(api) {
        return Some(vec![kind]);
    }
    if IMAGE_GENERATION_APIS.contains(&api) {
        Some(IMAGE_CHAT_TRANSPORT_KINDS.to_vec())
    } else {
        Some(CHAT_TRANSPORT_KINDS.to_vec())
    }
}

/// Whether a model of `kind` can run on `api`.
pub fn api_serves_kind(api: &str, kind: ModelKind) -> bool {
    served_kinds(api).map_or(true, |kinds| kinds.contains(&kind))
}

/// Canonical thinking transport used by a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThinkingControlMode {
    #[serde(rename = "effort")]
    Effort,
    #[serde(rename = "budget")]
    Budget,
    #[serde(rename = "google-level")]
    GoogleLevel,
    #[serde(rename = "anthropic-adaptive")]
    AnthropicAdaptive,
    #[serde(rename = "anthropic-budget-effort")]
    AnthropicBudgetEffort,
}

/// Per-model thinking capabilities used to clamp and map user-facing effort levels.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingConfig {
    /// Provider-specific transport used to encode the selected effort.
    pub mode: Option<ThinkingControlMode>,
    /// Supported user-facing efforts, ordered least to most intensive. Never empty.
    pub efforts: Vec<Effort>,
    /// Optional default effort applied when this model is selected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_level: Option<Effort>,
    /// Effort to provider wire-value remap, baked at build time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort_map: Option<HashMap<Effort, String>>,
    /// Adaptive thinking accepts the `display` field (Opus 4.7+, Fable/Mythos 5).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_display: Option<bool>,
    /// Thinking signatures bind each block to its preceding conversation prefix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix_binding: Option<bool>,
    /// Per-effort upstream wire-id routing for collapsed effort-tier variants (`"off"` too).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort_routing: Option<HashMap<String, String>>,
    /// Per-effort thinking budget in tokens, for `mode: "budget"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort_budgets: Option<HashMap<Effort, u64>>,
    /// A thinking-off request MUST explicitly suppress thinking on the wire.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suppress_when_off: Option<bool>,
    /// Reasoning is mandatory upstream: thinking-off clamps to the lowest supported effort.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_effort: Option<bool>,
    /// The catalog has no evidence this model reasons; the ladder is offered to opt into.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unverified: Option<bool>,
}

/// Token budgets for each thinking level (token-based providers only).
pub type ThinkingBudgets = HashMap<Effort, u64>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageOrchestration {
    /// Non-cached orchestration input tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<u64>,
    /// Orchestration tokens read from provider-side cache.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<u64>,
    /// Orchestration output tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<u64>,
}

/// Cache-write TTL breakdown (Anthropic only). Components sum to `cache_write`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageCacheTtl {
    #[serde(rename = "ephemeral5m", skip_serializing_if = "Option::is_none")]
    pub ephemeral_5m: Option<u64>,
    #[serde(rename = "ephemeral1h", skip_serializing_if = "Option::is_none")]
    pub ephemeral_1h: Option<u64>,
}

/// Server-side tool invocations made during a turn. Counts requests, not tokens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageServer {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_search: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_fetch: Option<u64>,
}

/// Provider-reported credit meter values for credit-based billing APIs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageCredits {
    /// Uncommitted per-response credit cost, when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    /// Final committed credit cost, when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub committed_cost: Option<f64>,
    /// Final committed ACU cost, when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acu_cost: Option<f64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageCost {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub total: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    /// Non-cached conversation input tokens (matches the bucket the provider bills as new input).
    pub input: u64,
    /// Total conversation output tokens for the turn, including thinking, text and tool-call args.
    pub output: u64,
    /// Conversation tokens read from the prompt cache.
    pub cache_read: u64,
    /// Conversation tokens written to the prompt cache (cache creation).
    pub cache_write: u64,
    /// Sum of input + output + cacheRead + cacheWrite plus provider-side orchestration tokens.
    pub total_tokens: u64,
    /// Provider-reported occupied context tokens when authoritative but not a billable bucket.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<u64>,
    /// Provider-side orchestration tokens, billed but not part of the conversation buckets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orchestration: Option<UsageOrchestration>,
    /// Copilot premium-request counter, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub premium_requests: Option<f64>,
    /// Reasoning/thinking tokens included in `output`. `None` means unknown, NOT zero.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    /// Cache-write TTL breakdown (Anthropic only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cttl: Option<UsageCacheTtl>,
    /// Server-side tool invocations made during this turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<UsageServer>,
    /// Provider-reported credit meter values for credit-based billing APIs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credits: Option<UsageCredits>,
    pub cost: UsageCost,
}

/// `Model<Api>` from the catalog. The catalog's `Model` struct (about 90 fields, with the compat
/// records) is not ported yet, so a model travels as its JSON until it is.
pub type Model = Value; // TODO(port-phase): catalog Model

/// `FetchImpl` from pi-utils: a replaceable HTTP client. Providers route every request through it.
pub type FetchImpl = Arc<reqwest::Client>;

// ---------------------------------------------------------------------------
// types.ts proper
// ---------------------------------------------------------------------------

/// Ceiling on the output-token count omp requests from any OpenAI-family endpoint
/// (openai-responses, azure/xai responses, and openai-completions).
///
/// Catalog `maxTokens` frequently reflects a model's context window rather than a given
/// upstream's real per-request output cap. OpenRouter, for instance, advertises 131072 output
/// tokens for `z-ai/glm-4.7`, but the Cerebras upstream only allows ~131072 tokens total, so
/// requesting the full ceiling overflows with a 400. Requested output is clamped to this value
/// (and to `model.maxTokens`).
pub const OPENAI_MAX_OUTPUT_TOKENS: u64 = 64000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenTaskBudget {
    #[serde(rename = "type")]
    pub kind: TokenTaskBudgetType,
    pub total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TokenTaskBudgetType {
    Tokens,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageAttribution {
    User,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum NativeToolMarker {
    Computer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolChoice {
    Keyword(ToolChoiceKeyword),
    Tagged(ToolChoiceObject),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolChoiceKeyword {
    Auto,
    None,
    Any,
    Required,
}

/// The object forms of `ToolChoice`:
/// `{type:"function",name}`, `{type:"function",function:{name}}`, `{type:"tool",name}`, `{type:"computer"}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ToolChoiceObject {
    Function(ToolChoiceFunction),
    Tool { name: String },
    Computer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolChoiceFunction {
    Named { name: String },
    Nested { function: ToolChoiceFunctionName },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolChoiceFunctionName {
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheRetention {
    None,
    Short,
    Long,
}

/// Service tier hint for processing priority / cost control. These are the values providers
/// consume on the wire:
///
/// - OpenAI / OpenAI-Codex: sent verbatim as the `service_tier` field
///   (`flex`/`scale`/`priority`/`ultrafast`). `ultrafast` is a separate low-latency serving path:
///   sent to the OpenAI API as-is (preview access is per project), and to Codex only for models
///   whose discovery advertises it.
/// - Google (Gemini API + Vertex AI): sent as the top-level `serviceTier` field (`flex`/`priority`).
/// - OpenRouter: passed through as `service_tier`; OpenRouter realizes it for the OpenAI- and
///   Google-family upstreams it supports and ignores it otherwise.
/// - Direct Anthropic: `"priority"` is translated into `speed: "fast"` plus the fast-mode beta on
///   supported Opus models. Other tiers are ignored.
///
/// Per-family scoping is expressed by [`ServiceTierByFamily`], not by scoped sentinel values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceTier {
    Auto,
    Default,
    Flex,
    Scale,
    Priority,
    Ultrafast,
}

/// Provider families that expose an independent service-tier knob.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceTierFamily {
    Openai,
    Anthropic,
    Google,
}

/// Per-family service-tier selection. A request consults only the entry for the family its model
/// belongs to (see [`resolve_model_service_tier`]).
pub type ServiceTierByFamily = HashMap<ServiceTierFamily, ServiceTier>;

/// What the identity-class checks need: `Pick<Model, "provider" | "api" | "identity">` plus the
/// optional `serviceTiers`.
#[derive(Debug, Clone, Default)]
pub struct ServiceTierModel {
    pub provider: Provider,
    pub api: Api,
    /// `model.identity.class`, e.g. `"openai"`, `"anthropic"`, `"gemini"`.
    pub identity_class: String,
    pub service_tiers: Option<Vec<ServiceTier>>,
}

/// `Provider | ServiceTierModel` as accepted by [`should_send_service_tier`].
#[derive(Debug, Clone)]
pub enum ServiceTierTarget<'a> {
    Provider(&'a str),
    Model(&'a ServiceTierModel),
}

fn is_openai_service_tier_api(api: &str) -> bool {
    api == "openai-completions" || api == "openai-responses" || api == "openai-codex-responses"
}

fn excludes_inferred_openai_service_tier(provider: &str) -> bool {
    // Fireworks has its own priority-only control. GitHub Copilot proxies OpenAI models but
    // rejects OpenAI's `service_tier` request field.
    provider == "fireworks" || provider == "github-copilot"
}

fn is_openai_service_tier_model(model: &ServiceTierModel) -> bool {
    !excludes_inferred_openai_service_tier(&model.provider)
        && is_openai_service_tier_api(&model.api)
        && model.identity_class == "openai"
}

/// Classify a model into the service-tier family whose knob governs it, or `None` when the model
/// exposes no serving-priority control.
///
/// OpenRouter models are classified by id namespace (`anthropic/`, `google/`, `openai/`); Claude on
/// Bedrock/Vertex (api `anthropic-messages`) is the anthropic family even though its provider is
/// `amazon-bedrock`/`google-vertex`. Custom OpenAI-compatible relays that serve OpenAI model ids
/// are OpenAI family too unless the provider owns a separate tier control (Fireworks) or rejects
/// OpenAI's service-tier field (GitHub Copilot).
pub fn service_tier_family(model: &ServiceTierModel) -> Option<ServiceTierFamily> {
    let provider = model.provider.as_str();
    if provider == "openrouter" {
        return match model.identity_class.as_str() {
            "anthropic" => Some(ServiceTierFamily::Anthropic),
            "gemini" => Some(ServiceTierFamily::Google),
            "openai" => Some(ServiceTierFamily::Openai),
            _ => None,
        };
    }
    if provider == "openai" || provider == "openai-codex" {
        return Some(ServiceTierFamily::Openai);
    }
    if model.api == "anthropic-messages" {
        return Some(ServiceTierFamily::Anthropic);
    }
    if provider == "google" || provider == "google-vertex" {
        return Some(ServiceTierFamily::Google);
    }
    if is_openai_service_tier_model(model) {
        return Some(ServiceTierFamily::Openai);
    }
    None
}

/// Reduce a per-family tier map to the single wire tier for `model`: the entry for the model's
/// family, or `None` when the model has no family.
pub fn resolve_model_service_tier(
    tiers: Option<&ServiceTierByFamily>,
    model: &ServiceTierModel,
) -> Option<ServiceTier> {
    let tiers = tiers?;
    let family = service_tier_family(model)?;
    tiers.get(&family).copied()
}

/// True when the tier should be sent on the wire as the provider's service-tier request field.
/// `auto` is never forwarded: it is OpenAI's implicit default, so omitting `service_tier` is
/// identical to requesting `auto`, and the Codex (ChatGPT OAuth) endpoint rejects an explicit
/// `auto` outright. OpenAI / OpenAI-Codex accept every other [`ServiceTier`]; Google (Gemini API +
/// Vertex) and OpenRouter accept `flex`/`priority`; Fireworks Serverless realizes only its Priority
/// serving path. Anthropic is absent because it realizes `priority` via `speed: "fast"`.
///
/// Codex-backend models (`openai-codex-responses`): `ultrafast` is sent only when the model's
/// discovered `service_tiers` lists it. `priority`/`scale` are dropped only when that list is
/// non-empty and omits them (codex-rs `service_tier_for_request`); an empty or missing list counts
/// as "not reported" (accounts whose `/models` lists no tiers keep `/fast`), so the provider-level
/// answer stands. `flex` and `default` are never gated. First-party OpenAI takes `ultrafast`
/// as-is. An OpenAI-family relay on the Responses/Completions APIs gets `ultrafast` only when its
/// discovered `service_tiers` lists it. A bare provider string cannot carry the list, so it answers
/// for the provider alone.
pub fn should_send_service_tier(
    service_tier: Option<ServiceTier>,
    target: Option<ServiceTierTarget<'_>>,
) -> bool {
    let service_tier = match service_tier {
        None | Some(ServiceTier::Auto) => return false,
        Some(t) => t,
    };
    let provider: Option<&str> = match &target {
        Some(ServiceTierTarget::Provider(p)) => Some(p),
        Some(ServiceTierTarget::Model(m)) => Some(m.provider.as_str()),
        None => None,
    };
    let model: Option<&ServiceTierModel> = match &target {
        Some(ServiceTierTarget::Model(m)) => Some(m),
        _ => None,
    };
    if let Some(m) = model {
        if m.api == "openai-codex-responses"
            && service_tier != ServiceTier::Flex
            && service_tier != ServiceTier::Default
        {
            let advertised = m.service_tiers.as_deref();
            if service_tier == ServiceTier::Ultrafast {
                return advertised.map_or(false, |a| a.contains(&service_tier));
            }
            if let Some(a) = advertised {
                if !a.is_empty() {
                    return a.contains(&service_tier);
                }
            }
        }
    }
    if service_tier == ServiceTier::Ultrafast {
        if matches!(target, Some(ServiceTierTarget::Provider(_))) {
            return provider == Some("openai") || provider == Some("openai-codex");
        }
        if provider == Some("openai") {
            return true;
        }
        return model.map_or(false, |m| {
            is_openai_service_tier_model(m)
                && m.service_tiers.as_deref().map_or(false, |a| a.contains(&service_tier))
        });
    }
    if provider == Some("openai") || provider == Some("openai-codex") {
        return true;
    }
    if provider == Some("openrouter") {
        return matches!(
            service_tier,
            ServiceTier::Flex | ServiceTier::Scale | ServiceTier::Priority
        );
    }
    if let Some(m) = model {
        if is_openai_service_tier_model(m) {
            return true;
        }
    }
    if provider == Some("google") {
        return matches!(service_tier, ServiceTier::Flex | ServiceTier::Priority);
    }
    // Vertex realizes only priority (via header); flex has no documented control.
    if provider == Some("google-vertex") || provider == Some("fireworks") {
        return service_tier == ServiceTier::Priority;
    }
    false
}

/// True when `priority` will actually be realized on the wire for `model`. Direct Anthropic
/// realizes fast mode; OpenAI/Google/Fireworks emit the service-tier field; OpenRouter realizes it
/// only for its OpenAI- and Google-family upstreams. Bedrock/Vertex Claude and OpenRouter
/// Anthropic models do not realize priority and return `false`.
pub fn realizes_priority_service_tier(
    service_tier: Option<ServiceTier>,
    model: &ServiceTierModel,
) -> bool {
    if service_tier != Some(ServiceTier::Priority) {
        return false;
    }
    if model.provider == "anthropic" {
        return true;
    }
    if model.provider == "openrouter" {
        let family = service_tier_family(model);
        return family == Some(ServiceTierFamily::Openai) || family == Some(ServiceTierFamily::Google);
    }
    if model.api == "anthropic-messages" {
        return false;
    }
    should_send_service_tier(service_tier, Some(ServiceTierTarget::Model(model)))
}

/// Premium-request weight contributed by a request a provider bills above standard. Priority (Fast
/// mode) counts 1 on every provider that realizes it; `ultrafast` counts 1 on the OpenAI family,
/// where it is a premium serving tier (its cost premium is recorded separately in `usage.cost`).
/// Mirrors GitHub Copilot's `premiumRequests` accounting so the "premium requests" stat aggregates
/// premium traffic across the OpenAI family, direct Anthropic fast mode, and Google priority.
///
/// Returns 1 only when the tier is actually realized on the wire for `model` and the provider
/// bills it as a premium request. OpenRouter is excluded (it bills per its own pricing), as are
/// Bedrock/Vertex Claude, where priority is silently dropped.
///
/// Pass `served: true` when the tier is the one the provider reported serving rather than a
/// requested setting: realization is then already proven, so the wire gate is skipped and a model
/// without discovery metadata (a stats-backfill row) still counts.
pub fn get_premium_service_tier_requests(
    service_tier: Option<ServiceTier>,
    model: &ServiceTierModel,
    served: bool,
) -> u32 {
    let provider = model.provider.as_str();
    if service_tier == Some(ServiceTier::Ultrafast) {
        if provider != "openai" && provider != "openai-codex" {
            return 0;
        }
        let sent = should_send_service_tier(
            Some(ServiceTier::Ultrafast),
            Some(ServiceTierTarget::Model(model)),
        );
        return u32::from(served || sent);
    }
    if service_tier != Some(ServiceTier::Priority) {
        return 0;
    }
    // A served tier is proof it reached the wire, so the realization gate only applies to
    // requested-tier inference.
    if !served && !realizes_priority_service_tier(service_tier, model) {
        return 0;
    }
    u32::from(matches!(
        provider,
        "openai" | "openai-codex" | "anthropic" | "google" | "google-vertex"
    ))
}

/// Parse a provider-reported `service_tier` echo into a known tier, or `None` for anything else.
pub fn parse_service_tier(value: &Value) -> Option<ServiceTier> {
    match value.as_str()? {
        "auto" => Some(ServiceTier::Auto),
        "default" => Some(ServiceTier::Default),
        "flex" => Some(ServiceTier::Flex),
        "scale" => Some(ServiceTier::Scale),
        "priority" => Some(ServiceTier::Priority),
        "ultrafast" => Some(ServiceTier::Ultrafast),
        _ => None,
    }
}

fn tier_from_str(s: &str) -> Option<ServiceTier> {
    parse_service_tier(&Value::String(s.to_owned()))
}

/// Coerce a persisted service-tier value to a [`ServiceTierByFamily`]. Newer sessions store the
/// family map directly; legacy sessions stored a single scalar: `"priority"` applied everywhere,
/// `"openai-only"`/`"claude-only"` scoped to one family, and the remaining values were
/// OpenAI-only semantics.
pub fn coerce_service_tier_by_family(value: &Value) -> Option<ServiceTierByFamily> {
    use ServiceTier as T;
    use ServiceTierFamily as F;
    match value {
        Value::Null => None,
        Value::Object(src) => {
            let mut out = ServiceTierByFamily::new();
            for (key, family) in [
                ("openai", F::Openai),
                ("anthropic", F::Anthropic),
                ("google", F::Google),
            ] {
                if let Some(tier) = src.get(key).and_then(Value::as_str).and_then(tier_from_str) {
                    out.insert(family, tier);
                }
            }
            if out.is_empty() { None } else { Some(out) }
        }
        Value::String(s) => {
            let mk = |pairs: &[(F, T)]| Some(pairs.iter().copied().collect::<ServiceTierByFamily>());
            match s.as_str() {
                "priority" => mk(&[(F::Openai, T::Priority), (F::Anthropic, T::Priority), (F::Google, T::Priority)]),
                "openai-only" => mk(&[(F::Openai, T::Priority)]),
                "claude-only" => mk(&[(F::Anthropic, T::Priority)]),
                "auto" => mk(&[(F::Openai, T::Auto)]),
                "default" => mk(&[(F::Openai, T::Default)]),
                "flex" => mk(&[(F::Openai, T::Flex)]),
                "scale" => mk(&[(F::Openai, T::Scale)]),
                _ => None,
            }
        }
        _ => None,
    }
}

pub trait ProviderSessionState: Send + Sync {
    fn close(&self);
    /// Release one routing session after its final request, preserving shared provider fallbacks.
    fn release_session(&self, _session_id: &str) {}
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderResponseMetadata {
    pub status: u16,
    pub headers: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, Value>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RawSseEvent {
    pub event: Option<String>,
    pub data: String,
    pub raw: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodexCompactionTrigger {
    Manual,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexCompactionReason {
    UserRequested,
    ContextLimit,
    ModelDownshift,
    CompHashChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexCompactionPhase {
    StandaloneTurn,
    PreTurn,
    MidTurn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexCompactionStrategy {
    Memento,
    PrefixCompaction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexCompactionImplementation {
    Responses,
    ResponsesCompactionV2,
    ResponsesCompact,
}

/// Lifecycle fields shared by every Codex compaction implementation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexCompactionContext {
    /// Stable only for one logical compaction, including parallel summary calls.
    pub operation_id: String,
    pub trigger: CodexCompactionTrigger,
    pub reason: CodexCompactionReason,
    pub phase: CodexCompactionPhase,
    pub strategy: CodexCompactionStrategy,
}

/// Canonical nested metadata serialized into the Codex turn envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodexCompactionMetadata {
    pub trigger: CodexCompactionTrigger,
    pub reason: CodexCompactionReason,
    pub implementation: CodexCompactionImplementation,
    pub phase: CodexCompactionPhase,
    pub strategy: CodexCompactionStrategy,
}

/// Dispatch context combining canonical metadata with its local operation identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexCompactionRequestContext {
    #[serde(flatten)]
    pub metadata: CodexCompactionMetadata,
    pub operation_id: String,
}

/// On-demand compaction request (`compact-2026-09-04` beta).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnthropicCompactionRequest {
    /// Custom summarization prompt; replaces the API default entirely when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// Replayed summaries' file metadata due before this time (ms) ends the request; later
    /// metadata replays with the retained tail, which the new summary carries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_due_before: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenAIPromptCacheMode {
    Implicit,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpenAIPromptCacheTtl {
    #[serde(rename = "30m")]
    Minutes30,
    #[serde(rename = "12h")]
    Hours12,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpenAIPromptCacheBreakpoint {
    #[serde(rename = "latest-stable-message")]
    LatestStableMessage,
    #[serde(rename = "none")]
    None,
}

/// OpenAI's GPT-5.6+ explicit prompt-cache controls.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenAIPromptCacheOptions {
    /// `explicit` disables OpenAI's automatic latest-message breakpoint.
    pub mode: OpenAIPromptCacheMode,
    /// Minimum cache lifetime. `30m` is OpenAI's default; `12h` (Responses API only) bills writes
    /// at 2.5x input instead of 1.25x. Defaults to `12h` when `cacheRetention` resolves to `long`,
    /// otherwise `30m`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<OpenAIPromptCacheTtl>,
    /// By default, mark one existing block from stable history; `none` suppresses that marker.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub breakpoint: Option<OpenAIPromptCacheBreakpoint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpenAIResponseInclude {
    #[serde(rename = "file_search_call.results")]
    FileSearchCallResults,
    #[serde(rename = "web_search_call.results")]
    WebSearchCallResults,
    #[serde(rename = "web_search_call.action.sources")]
    WebSearchCallActionSources,
    #[serde(rename = "message.input_image.image_url")]
    MessageInputImageImageUrl,
    #[serde(rename = "computer_call_output.output.image_url")]
    ComputerCallOutputOutputImageUrl,
    #[serde(rename = "code_interpreter_call.outputs")]
    CodeInterpreterCallOutputs,
    #[serde(rename = "reasoning.encrypted_content")]
    ReasoningEncryptedContent,
    #[serde(rename = "message.output_text.logprobs")]
    MessageOutputTextLogprobs,
}

// ---------------------------------------------------------------------------
// Content blocks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextPhase {
    Commentary,
    FinalAnswer,
}

/// `TextSignatureV1`: legacy id strings or this JSON object, in `TextContent.text_signature`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextSignatureV1 {
    pub v: u8,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<TextPhase>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextContent {
    pub text: String,
    /// e.g., for OpenAI responses, message metadata (legacy id string or `TextSignatureV1` JSON)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_signature: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingContent {
    pub thinking: String,
    /// e.g., for OpenAI responses, the reasoning item ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_signature: Option<String>,
    /// item.id from output_item.added, used to match output_item.done
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    /// `true`: `thinking` is a provider-written summary of the model's reasoning (OpenAI Responses
    /// `summary_text`, Gemini thought summaries). `false`: the wire delivered it as the reasoning
    /// itself, not marked as a summary (chat-completions reasoning fields, Responses
    /// `reasoning_text`, Devin `thinking`); whether that is the full trace is vouched for per
    /// family by the catalog's `portable-reasoning`. Unset when provenance is unknown, including
    /// turns recorded before parsers set it. Only `false` blocks replay natively on another host.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<bool>,
    /// `true`: the block is narration, prose the model addressed to the user between tool calls,
    /// which the provider returned as a signed thinking block (Anthropic connector-text
    /// summarization). It stays a thinking block for replay; user-facing surfaces present it as
    /// answer text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub narration: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RedactedThinkingContent {
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FallbackModel {
    pub model: String,
}

/// Anthropic server-side-fallback boundary marker persisted on assistant turns whose response
/// switched models mid-stream, whether the request opted into `AnthropicOptions.fallbacks` or a
/// gateway opted in on its behalf. Consumers other than the Anthropic provider MUST ignore it:
/// `transformMessages` strips the block on any non-Anthropic hop, and `convertAnthropicMessages`
/// only puts it on the wire for an opted-in request to the official endpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnthropicFallbackContent {
    pub from: FallbackModel,
    pub to: FallbackModel,
}

/// Verbatim Anthropic web-search or tool-search call/result retained for same-provider history
/// replay. Other providers discard it in `transformMessages`. `block` is the wire block verbatim
/// (`server_tool_use`, `web_search_tool_result`, or `tool_search_tool_result`, plus extra keys).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnthropicServerToolContent {
    pub block: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderFileProvider {
    Openai,
    Anthropic,
    Google,
}

/// Provider-native uploaded file reference for image reuse without retransmitting bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderFileReference {
    pub provider: ProviderFileProvider,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageDetail {
    Auto,
    Low,
    High,
    Original,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageContent {
    /// base64 encoded image data
    pub data: String,
    /// e.g., "image/jpeg", "image/png"
    pub mime_type: String,
    /// OpenAI-only resolution hint. `"original"` preserves native resolution (required for
    /// snapcompact frames, whose glyphs do not survive the default `auto` downscale). Providers
    /// without a detail knob ignore it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<ImageDetail>,
    /// Provider-native file reference preferred only by its matching provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_file: Option<ProviderFileReference>,
    /// Optional https mirror of `data`, served by a caller-run blob server. Providers whose API
    /// fetches remote images send this URL instead of the base64 payload; every other provider
    /// ignores it. `data` remains the source of truth: the URL must serve exactly those bytes, and
    /// callers keep it stable across turns (prefix caches hash the URL string, and Anthropic
    /// silently forgets images when a resent turn differs byte-wise).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClickButton {
    Left,
    Right,
    Wheel,
    Back,
    Forward,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ComputerAction {
    Click {
        button: ClickButton,
        x: f64,
        y: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        keys: Option<Vec<String>>,
    },
    DoubleClick {
        x: f64,
        y: f64,
        keys: Option<Vec<String>>,
    },
    Drag {
        path: Vec<Point>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        keys: Option<Vec<String>>,
    },
    Keypress {
        keys: Vec<String>,
    },
    Move {
        x: f64,
        y: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        keys: Option<Vec<String>>,
    },
    Screenshot,
    Scroll {
        x: f64,
        y: f64,
        scroll_x: f64,
        scroll_y: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        keys: Option<Vec<String>>,
    },
    Type {
        text: String,
    },
    Wait,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComputerSafetyCheck {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// `ToolCallProviderMetadata` (only the `computer` variant exists).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerToolCallMetadata {
    pub provider_item_id: String,
    pub actions: Vec<ComputerAction>,
    pub pending_safety_checks: Vec<ComputerSafetyCheck>,
}

pub type ToolCallProviderMetadata = ComputerToolCallMetadata;

/// `{ type: "computer_screenshot"; image_url } | { type: "computer_screenshot"; file_id }`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComputerScreenshotRef {
    #[serde(rename = "type")]
    pub kind: ComputerScreenshotType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputerScreenshotType {
    ComputerScreenshot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerToolResultMetadata {
    pub screenshot: ComputerScreenshotRef,
    pub acknowledged_safety_checks: Vec<ComputerSafetyCheck>,
}

pub type ToolResultProviderMetadata = ComputerToolResultMetadata;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Map<String, Value>,
    /// The TS `[kStreamingPartialJson]` symbol-keyed scratch field: partial JSON while args stream.
    #[serde(skip)]
    pub streaming_partial_json: Option<String>,
    /// Google-specific: opaque signature for reusing thought context
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
    /// Harness-level intent metadata extracted from traced tool arguments
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    /// Verbatim in-band syntax block that produced this synthetic `ptc_*` call. Present only for
    /// owned prompt/tool-call formats; provider-native calls omit it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_block: Option<String>,
    /// Original wire-level name when the tool was invoked via OpenAI's custom-tool mechanism
    /// (e.g., `apply_patch`). Set by `openai-responses` on receive so the history-replay path can
    /// re-emit the call as `custom_tool_call` with its paired tool-result as
    /// `custom_tool_call_output`. Absent for regular JSON function tools.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_wire_name: Option<String>,
    /// Responses API namespace for namespaced function and custom tools.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Provider-native metadata required to execute and faithfully replay this call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ToolCallProviderMetadata>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
    #[serde(rename = "stop")]
    Stop,
    #[serde(rename = "length")]
    Length,
    #[serde(rename = "toolUse")]
    ToolUse,
    #[serde(rename = "error")]
    Error,
    #[serde(rename = "aborted")]
    Aborted,
}

/// One block of an assistant message's `content` array.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AssistantContent {
    #[serde(rename = "text")]
    Text(TextContent),
    #[serde(rename = "thinking")]
    Thinking(ThinkingContent),
    #[serde(rename = "redactedThinking")]
    RedactedThinking(RedactedThinkingContent),
    #[serde(rename = "fallback")]
    Fallback(AnthropicFallbackContent),
    #[serde(rename = "anthropicServerTool")]
    AnthropicServerTool(AnthropicServerToolContent),
    #[serde(rename = "image")]
    Image(ImageContent),
    #[serde(rename = "toolCall")]
    ToolCall(ToolCall),
}

/// `TextContent | ImageContent`, as carried by user/developer/tool-result messages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum UserContentPart {
    #[serde(rename = "text")]
    Text(TextContent),
    #[serde(rename = "image")]
    Image(ImageContent),
}

/// `string | (TextContent | ImageContent)[]`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<UserContentPart>),
}

/// Anthropic `output_config.effort` level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnthropicOutputEffort {
    Low,
    Medium,
    High,
    #[serde(rename = "xhigh")]
    XHigh,
    Max,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnthropicToolChangeKind {
    ToolAddition,
    ToolRemoval,
}

/// One `tool_addition`/`tool_removal` block of an Anthropic mid-conversation system message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnthropicToolChange {
    #[serde(rename = "type")]
    pub kind: AnthropicToolChangeKind,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnthropicClearAt {
    Never,
    NextUserMessage,
}

/// The OpenAI Responses history payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenAIResponsesHistoryPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dt: Option<bool>,
    pub items: Vec<serde_json::Map<String, Value>>,
}

/// Anthropic-only controls attached to a mid-conversation system message.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnthropicMessagePayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clear_at: Option<AnthropicClearAt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<AnthropicOutputEffort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_changes: Option<Vec<AnthropicToolChange>>,
}

/// Controls an Anthropic request declared, recorded on its response so later requests over the
/// same transcript replay a byte-identical prefix. Written by the Anthropic provider; read by it
/// and by the Agent's inactive-tool lookup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnthropicRequestControls {
    /// `context.messages.length` of the request that produced this response, i.e. the response's
    /// own index. A record found at another index belongs to a history that was rewritten before
    /// it (compaction, dropped messages) and is not replayed as controls.
    pub message_index: u64,
    /// Present when the request kept a stable tool declaration (`supportsMidConversationToolChanges`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<AnthropicRequestToolControls>,
    /// Present when the request kept a stable effort (`supportsPerMessageEffort`); `null` = API default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<AnthropicRequestEffortControls>,
}

/// Source tool names, not wire names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnthropicRequestToolControls {
    /// Top-level `tools` in wire order.
    pub declared: Vec<String>,
    /// Subset of `declared` sent with `defer_loading: true`.
    pub deferred: Vec<String>,
    /// Tools active at the end of the request, in `context.tools` order.
    pub active: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnthropicRequestEffortControls {
    pub top_level: Option<AnthropicOutputEffort>,
    pub tail: Option<AnthropicOutputEffort>,
}

/// File metadata replayed at a fixed point of a natively compacted conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnthropicCompactionFiles {
    pub text: String,
    /// Replays before the first message created after this time (ms).
    pub after: u64,
}

/// Anthropic on-demand compaction summary (`compact-2026-09-04` beta).
///
/// Produced by the Anthropic provider on the assistant message of a request that streamed a
/// `compaction` content block, and attached to the user-role compaction summary message that
/// replaces the compacted history so the provider can replay the block verbatim: the API drops
/// every block that precedes it. `content` is the plain-text summary, so every other provider
/// reads the message text and ignores the payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnthropicCompactionPayload {
    /// Provider that produced the summary; only that provider replays it natively.
    pub provider: String,
    pub content: String,
    /// Signature of an on-demand block; replayed verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// Net tool changes across the summarized range; covered by the signature, replayed verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_changes: Option<Vec<serde_json::Map<String, Value>>>,
    /// Legacy threshold block state (`compact-2026-01-12`); replay-only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_content: Option<String>,
    /// Harness-appended file metadata (`<files>` section) kept out of the byte-identical block.
    /// The converter replaces the summary message with the block and skips its text, so it replays
    /// this as a user message after the block's retained tail: before the first message created
    /// after the summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_text: Option<String>,
    /// File metadata of earlier summaries whose replay position lies inside this summary's
    /// retained tail. Retained messages must reach the API unchanged, so each keeps replaying
    /// where it did: before the first message created after `after` (the earlier summary's commit
    /// time).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retained_files: Option<Vec<AnthropicCompactionFiles>>,
    /// Set on summaries whose retained tail replays unchanged: file metadata after the tail,
    /// earlier metadata at `retainedFiles`. Summaries persisted without it keep their original
    /// layout (metadata after the first retained turn), since later thinking was signed against
    /// those bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exact_tail: Option<bool>,
}

/// Records Cursor's server wrote into the client's conversation store for one assistant turn: the
/// `rootPromptMessagesJson` entries after the user's message, reasoning signatures and redacted
/// reasoning included. Written and read by the Cursor provider, which sends them back unchanged to
/// the wire model that produced them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorHistoryPayload {
    /// Wire route the records were written under, as sent (requested model, details id, max mode, parameters).
    pub wire_route: String,
    /// Digest of the turn's opening user message, content and paired tool results when recorded;
    /// a rewritten turn rebuilds instead.
    pub digest: String,
    /// Record bytes (UTF-8 JSON) in conversation order; each blob id is the SHA-256 of its bytes.
    pub records: Vec<String>,
}

/// The `candidates[0].content` a Gemini model streamed for one turn, every part (thought text,
/// signatures, call ids) merged in arrival order. Written and read by the Factory Droid Gemini
/// transport, which replays it unchanged to the same model while the turn's content still matches
/// `digest`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeminiNativeContentPayload {
    /// Digest of the turn's text, thinking and tool calls when recorded; a rewritten turn rebuilds instead.
    pub digest: String,
    /// `{ role?, parts: Record<string, unknown>[], [key: string]: unknown }`
    pub content: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProviderPayload {
    #[serde(rename = "openaiResponsesHistory")]
    OpenAIResponsesHistory(OpenAIResponsesHistoryPayload),
    #[serde(rename = "anthropicMessage")]
    AnthropicMessage(AnthropicMessagePayload),
    #[serde(rename = "anthropicCompaction")]
    AnthropicCompaction(AnthropicCompactionPayload),
    #[serde(rename = "cursorHistory")]
    CursorHistory(CursorHistoryPayload),
    #[serde(rename = "geminiNativeContent")]
    GeminiNativeContent(GeminiNativeContentPayload),
}

/// Provider-reported rewrite applied to request content before inference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderInputTransformation {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// `[key: string]: unknown`
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParentIrc {
    pub from: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserMessage {
    pub content: MessageContent,
    /// Original sender and body of a parent-agent IRC steer, for transcript presentation only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_irc: Option<ParentIrc>,
    /// True if the message was injected by the system (e.g., auto-continue).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synthetic: Option<bool>,
    /// True when injected mid-turn as a steer; consumed by the agent's pre-LLM transform to wrap
    /// it for emphasis. Never rendered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steering: Option<bool>,
    /// True when the provider delivered this steer into the response it was streaming
    /// (`response.steer`). Display-only; never sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live_steered: Option<bool>,
    /// Timestamp of a client-side history rewrite represented by this message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_rewrite_at: Option<u64>,
    /// Who initiated this message for billing/attribution semantics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attribution: Option<MessageAttribution>,
    /// Provider-specific opaque payload used to reconstruct transport-native history.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_payload: Option<ProviderPayload>,
    /// Unix timestamp in milliseconds
    pub timestamp: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeveloperMessage {
    pub content: MessageContent,
    /// Who initiated this message for billing/attribution semantics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attribution: Option<MessageAttribution>,
    /// True if the message was injected by the system (e.g., auto-continue) and initiates a fresh
    /// run rather than continuing the current one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synthetic: Option<bool>,
    /// True when the synthetic prompt was a deliberate operator action (`.`, `c` continue
    /// shortcut) rather than an automatic continuation: its timestamp is the turn's prompt time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_initiated: Option<bool>,
    /// Provider-specific opaque payload used to reconstruct transport-native history.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_payload: Option<ProviderPayload>,
    /// Unix timestamp in milliseconds
    pub timestamp: u64,
}

/// How an automatic retry recovered or ultimately settled a failed attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssistantRetryRecoveryKind {
    Credential,
    Model,
    Wait,
    Plain,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrySupersededBy {
    pub timestamp: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    pub provider: String,
    pub model: String,
}

/// Persisted presentation state for an assistant error superseded by an automatic retry saga.
/// Both variants carry `kind: "auto-retry"`; `status` selects `recovered` or `superseded`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum AssistantRetryRecovery {
    #[serde(rename_all = "camelCase")]
    Recovered {
        kind: String,
        attempt: u32,
        recovered_at: String,
        recovery: AssistantRetryRecoveryKind,
        note: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        superseded_by: Option<RetrySupersededBy>,
    },
    #[serde(rename_all = "camelCase")]
    Superseded {
        kind: String,
        attempt: u32,
        recovery: AssistantRetryRecoveryKind,
        note: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSnapshot {
    /// authoritative provider prompt/input tokens
    pub prompt_tokens: u64,
    /// estimated non-message total at send time
    pub non_message_tokens: u64,
    /// Estimated tool-schema tokens included in `nonMessageTokens` (0 when the model defers
    /// schemas server-side). Lets a later model whose schema policy differs rebase the provider
    /// anchor instead of inheriting its schemas.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_schema_tokens: Option<u64>,
    /// Estimated prompt tokens removed by local history rewrites after this provider snapshot was recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_rewrite_tokens_removed: Option<u64>,
    /// Compaction epoch current when this snapshot's provider request was recorded. A later
    /// compaction bumps the session epoch, so an anchor whose epoch is older than the current
    /// in-flight snapshot describes pre-compaction history and must not override the rebased estimate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compaction_epoch: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message_timestamp: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessage {
    pub content: Vec<AssistantContent>,
    pub api: Api,
    pub provider: Provider,
    pub model: String,
    /// Stored credential row that produced this turn; absent for external or unknown keys.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_snapshot: Option<ContextSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_recovery: Option<AssistantRetryRecovery>,
    /// Provider-specific response/message identifier when the upstream API exposes one
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    /// Name of the upstream provider an aggregator routed this request to, as reported in the
    /// response (e.g. OpenRouter's top-level `provider` field: `"OpenAI"`, `"Anthropic"`,
    /// `"Together"`). Distinct from `provider`, which is the configured gateway we called
    /// (`"openrouter"`). `None` for direct providers that expose no such field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_provider: Option<String>,
    /// Concrete model that produced this turn when it is knowable independently of the requested
    /// id: reported by a router that selected one, or recovered from a signed thinking block
    /// (Anthropic signatures name the serving model). Compared against `model` to notice a
    /// gateway serving something other than what was requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_model: Option<String>,
    /// Service tier the provider reported serving this turn, when the API echoes one
    /// (`response.service_tier`), falling back to the tier the request carried when the response
    /// omits the echo. Absent when the provider reports no tier or the echo cannot be trusted
    /// (proxies). This is the tier the turn actually ran on, which is what cost, premium-request,
    /// and speed accounting key on: the session's live setting may already have changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<ServiceTier>,
    pub usage: Usage,
    /// Visible output tokens a history rewrite (shake) has since removed from `content`.
    /// `usage.output` still counts them, so a replay estimate subtracts them to keep the turn's
    /// reasoning without recharging the removed text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elided_output_tokens: Option<u64>,
    pub stop_reason: StopReason,
    /// `StopDetails | null` from `providers/anthropic-wire`; kept as raw JSON until that module is ported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_details: Option<Value>, // TODO(port-phase): providers::anthropic_wire::StopDetails
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    /// Stable recovery-classification text when errorMessage includes display-only diagnostics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_classification_message: Option<String>,
    /// True only when an exact request-body-read timeout failed on a full Responses replay, not a
    /// previous-response delta.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_body_read_timeout_full_replay: Option<bool>,
    /// Per-tool abort messages used when an aborted assistant turn needs different placeholder
    /// results per tool call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_abort_messages: Option<HashMap<String, String>>,
    /// HTTP status surfaced by the provider when the request failed. Populated by every
    /// provider's catch block alongside `errorMessage` so consumers (auth retry, telemetry, UI)
    /// can branch without regex-scraping the message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_status: Option<u16>,
    /// Structured machine-readable error classifier; see `utils/error-id.ts` for bit layout and helpers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_id: Option<u64>,
    /// Stable identifiers for request features the provider silently dropped during this turn
    /// (e.g. `"priority"`). Set when a server-side rejection triggered an in-provider fallback
    /// retry that succeeded without the feature. Callers can use this to sync user-facing toggles
    /// back to the server's actual state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_features: Option<Vec<String>>,
    /// Provider-reported input rewrites such as dropped bound-thinking blocks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_transformations: Option<Vec<ProviderInputTransformation>>,
    /// Controls an Anthropic request declared, recorded on its response so later requests over
    /// the same transcript replay a byte-identical prefix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_controls: Option<AnthropicRequestControls>,
    /// Provider-specific opaque payload used to reconstruct transport-native history.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_payload: Option<ProviderPayload>,
    /// In-memory fallback credit handle attached when a refusal response carries a fallback credit token.
    #[serde(skip)]
    pub fallback_credit_handle: Option<AnthropicFallbackCreditHandle>,
    /// Unix timestamp in milliseconds
    pub timestamp: u64,
    /// Request duration in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<u64>,
    /// Time to first token in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttft: Option<u64>,
    /// Local wall-clock time the response finished streaming (ms since epoch); stamped by the
    /// session at message_end so prompt-to-yield timing never depends on provider-reported duration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultMessage {
    pub tool_call_id: String,
    pub tool_name: String,
    /// Supports text and images
    pub content: Vec<UserContentPart>,
    /// `details?: TDetails` (`unknown` by default).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    pub is_error: bool,
    /// Who initiated this message for billing/attribution semantics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attribution: Option<MessageAttribution>,
    /// Timestamp when output was pruned (ms since epoch). `None` if unpruned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pruned_at: Option<u64>,
    /// Provider-native metadata required to faithfully replay this result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ToolResultProviderMetadata>,
    /// Tool-declared: this result carried no information worth retaining once consumed (zero
    /// matches, elapsed wait). Compaction passes may elide it. Never set together with `is_error`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub useless: Option<bool>,
    /// Unix timestamp in milliseconds
    pub timestamp: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role")]
pub enum Message {
    #[serde(rename = "user")]
    User(UserMessage),
    #[serde(rename = "developer")]
    Developer(DeveloperMessage),
    #[serde(rename = "assistant")]
    Assistant(AssistantMessage),
    #[serde(rename = "toolResult")]
    ToolResult(ToolResultMessage),
}

// ---------------------------------------------------------------------------
// Tools and context
// ---------------------------------------------------------------------------

/// Plain JSON Schema document used by extension-authored tools (legacy TypeBox emits this shape).
/// `TSchema = Type | TJsonSchema`; arktype `Type` values are carried as their JSON Schema too.
pub type TJsonSchema = serde_json::Map<String, Value>;
pub type TSchema = Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallExample {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    pub call: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCompareExample {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    pub bad: Value,
    pub good: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolNoteExample {
    pub caption: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolExample {
    Call(ToolCallExample),
    Compare(ToolCompareExample),
    Note(ToolNoteExample),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CustomFormatSyntax {
    Lark,
    Regex,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomFormat {
    pub syntax: CustomFormatSyntax,
    pub definition: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: TSchema,
    /// If true, tool is strictly typed and validated against the parameters schema before execution
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    /// Withhold this Anthropic tool until a `tool_addition` message references it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub defer_loading: Option<bool>,
    /// Optional grammar constraint for OpenAI custom-tool emission. When set, providers that
    /// support grammar-constrained tools (currently only `openai-responses` against models with
    /// the right capability flag) may emit this tool as `{type: "custom", format: {type:
    /// "grammar", ...}}` instead of a JSON function tool. Other providers ignore the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_format: Option<CustomFormat>,
    /// Optional wire-level name used when this tool is emitted as a custom tool (e.g. OpenAI's
    /// `{type: "custom"}` shape). Models trained on specific tool names, like GPT-5 on
    /// `apply_patch`, need to see that exact name on the wire, but it may differ from the
    /// harness-internal `name`. The agent-loop dispatcher matches both `name` and
    /// `customWireName` so returned tool calls route correctly. Absent for regular JSON function tools.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_wire_name: Option<String>,
    /// Selects a provider-native hosted tool instead of a JSON-schema function tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native: Option<NativeToolMarker>,
    /// Illustrative calls/notes; the AI layer renders them into an `<examples>` block in the
    /// model's native tool-call syntax and appends to the wire description. Author
    /// `call`/`bad`/`good` as plain argument objects WITHOUT `i`: when intent tracing injects `i`
    /// into the schema, the renderer adds a placeholder `i` automatically.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<ToolExample>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<Vec<String>>,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
    /// Definitions of tools the transcript's latest Anthropic request declared but that are no
    /// longer in `tools`; only the Anthropic provider reads it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inactive_tools: Option<Vec<Tool>>,
}

// ---------------------------------------------------------------------------
// Stream events
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DoneReason {
    #[serde(rename = "stop")]
    Stop,
    #[serde(rename = "length")]
    Length,
    #[serde(rename = "toolUse")]
    ToolUse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorReason {
    #[serde(rename = "aborted")]
    Aborted,
    #[serde(rename = "error")]
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantMessageEvent {
    #[serde(rename_all = "camelCase")]
    Start { partial: AssistantMessage },
    #[serde(rename_all = "camelCase")]
    TextStart { content_index: usize, partial: AssistantMessage },
    #[serde(rename_all = "camelCase")]
    TextDelta { content_index: usize, delta: String, partial: AssistantMessage },
    #[serde(rename_all = "camelCase")]
    TextEnd { content_index: usize, content: String, partial: AssistantMessage },
    #[serde(rename_all = "camelCase")]
    ThinkingStart { content_index: usize, partial: AssistantMessage },
    #[serde(rename_all = "camelCase")]
    ThinkingDelta { content_index: usize, delta: String, partial: AssistantMessage },
    #[serde(rename_all = "camelCase")]
    ThinkingEnd { content_index: usize, content: String, partial: AssistantMessage },
    #[serde(rename_all = "camelCase")]
    ImageEnd { content_index: usize, content: ImageContent, partial: AssistantMessage },
    #[serde(rename = "toolcall_start", rename_all = "camelCase")]
    ToolCallStart { content_index: usize, partial: AssistantMessage },
    #[serde(rename = "toolcall_delta", rename_all = "camelCase")]
    ToolCallDelta { content_index: usize, delta: String, partial: AssistantMessage },
    #[serde(rename = "toolcall_end", rename_all = "camelCase")]
    ToolCallEnd { content_index: usize, tool_call: ToolCall, partial: AssistantMessage },
    Done { reason: DoneReason, message: AssistantMessage },
    Error { reason: ErrorReason, error: AssistantMessage },
}

/// Anthropic fallback credit redemption handle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnthropicFallbackCreditHandle {
    pub token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefill_claim: Option<bool>,
    /// The refused request's body (`MessageCreateParamsStreaming`), replayed with the credit token
    /// on the fallback model.
    pub params: Value, // TODO(port-phase): providers::anthropic_wire::MessageCreateParamsStreaming
    #[serde(skip_serializing_if = "Option::is_none")]
    pub betas: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beta_header: Option<String>,
    pub expires_at: u64,
    /// The refused response's content, in `AssistantMessage` block form.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused_content: Option<Vec<AssistantContent>>,
}

// ---------------------------------------------------------------------------
// StreamOptions / SimpleStreamOptions (data fields; callbacks are boxed closures)
// ---------------------------------------------------------------------------

pub type OnPayload = Arc<dyn Fn(Value) -> Option<Value> + Send + Sync>;
pub type OnResponse = Arc<dyn Fn(&ProviderResponseMetadata) + Send + Sync>;
pub type OnSseEvent = Arc<dyn Fn(&RawSseEvent) + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnthropicPrefixMismatchBehavior {
    DropBlock,
    Error,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopGuardOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check_assistant_content: Option<bool>,
}

/// Base options all providers share. Callback and hook fields are `#[serde(skip)]`.
#[derive(Clone, Default)]
pub struct StreamOptions {
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub top_k: Option<f64>,
    pub min_p: Option<f64>,
    pub presence_penalty: Option<f64>,
    pub repetition_penalty: Option<f64>,
    /// Stop sequences. Anthropic encodes as `stop_sequences` (array, max 4); OpenAI
    /// chat-completions encodes as `stop` (string or array of up to 4); OpenAI Responses API has
    /// no `stop` field today (silently dropped by the provider when present).
    pub stop_sequences: Option<Vec<String>>,
    /// Frequency penalty (OpenAI). Penalizes new tokens based on existing frequency in the text
    /// so far. Range -2.0 to 2.0. Parallel to `presence_penalty`.
    pub frequency_penalty: Option<f64>,
    pub max_tokens: Option<u64>,
    /// `signal?: AbortSignal`
    pub signal: Option<tokio_util::sync::CancellationToken>,
    /// OpenAI chat-completions HTTP attempt budget. When set, transient provider errors are not
    /// replayed above the transport, so retries share this limit.
    pub max_http_attempts: Option<u32>,
    pub api_key: Option<String>,
    /// Stored credential row serving this request, when known. (@internal)
    pub credential_id: Option<i64>,
    /// `OAuthRequestIdentity`: non-secret identity of the bearer serving this attempt; never
    /// persisted in history. (@internal) TODO(port-phase): auth::types::OAuthRequestIdentity
    pub oauth_identity: Option<Value>,
    pub cache_retention: Option<CacheRetention>,
    /// Anthropic preserved-thinking behavior when a signed block no longer matches its
    /// conversation prefix. Binding-capable models default to `"drop_block"`.
    pub anthropic_prefix_mismatch_behavior: Option<AnthropicPrefixMismatchBehavior>,
    /// Anthropic on-demand compaction (`compact-2026-09-04` beta). Sends a top-level
    /// `compaction: { type: "summarize", instructions? }` request; the signed summary arrives as
    /// an [`AnthropicCompactionPayload`]. Ignored by providers and endpoints without on-demand
    /// compaction support.
    pub anthropic_compaction: Option<AnthropicCompactionRequest>,
    /// Attribute Anthropic Messages requests to this user profile (`anthropic-user-profile-id`).
    pub user_profile_id: Option<String>,
    /// Additional headers to include in provider requests. These are merged on top of model-defined headers.
    pub headers: Option<HashMap<String, String>>,
    /// Optional explicit request attribution override for providers that support it.
    pub initiator_override: Option<MessageAttribution>,
    /// Maximum delay in milliseconds to wait for a retry when the server requests a long wait. If
    /// the server's requested delay exceeds this value, the request fails immediately with an
    /// error containing the requested delay, allowing higher-level retry logic to handle it with
    /// user visibility. Default: 60000 (60 seconds). Set to 0 to disable the cap.
    pub max_retry_delay_ms: Option<u64>,
    /// Optional metadata to include in API requests. Providers extract the fields they understand
    /// and ignore the rest. For example, Anthropic uses `user_id` for abuse tracking and rate limiting.
    pub metadata: Option<HashMap<String, Value>>,
    /// Provider-owned request configuration. Provider hooks interpret this bag; generic API
    /// transports do not forward its fields onto the wire.
    pub provider_options: Option<HashMap<String, Value>>,
    /// OpenAI Responses/Codex response fields to include verbatim.
    pub include: Option<Vec<OpenAIResponseInclude>>,
    /// Config options for the thinking/response loop guard.
    pub loop_guard: Option<LoopGuardOptions>,
    /// Advisory token budget for a full agentic loop. Anthropic encodes this as
    /// `output_config.task_budget` with the `task-budgets-2026-03-13` beta header.
    pub task_budget: Option<TokenTaskBudget>,
    /// Optional session identifier for providers that support session-based routing, request
    /// affinity, or transport reuse. Providers may also use this as the prompt-cache key when
    /// `prompt_cache_key` is not set.
    pub session_id: Option<String>,
    /// Optional prompt-cache identity. OpenAI-family providers use this for `prompt_cache_key`
    /// payloads and cache-affinity headers such as `x-grok-conv-id`; when omitted, they fall back
    /// to `session_id`.
    pub prompt_cache_key: Option<String>,
    /// OpenAI GPT-5.6+ prompt-cache policy. Ignored by providers that do not support explicit
    /// OpenAI cache breakpoints; explicit mode fails locally on incompatible OpenAI-compatible endpoints.
    pub prompt_cache: Option<OpenAIPromptCacheOptions>,
    /// Server-side turn chaining: OpenAI Responses `previous_response_id` and Anthropic message
    /// threads. `false` sends every request in full (diagnostic callers comparing independent
    /// requests); `true` chains even where the provider would not by default. Unset follows the
    /// provider default.
    pub stateful_responses: Option<bool>,
    /// Store this request's result server-side on hosts that support it (`compat.storeResponses`,
    /// e.g. Muse Code), so a stream that drops mid-turn resumes from `GET /responses/{id}` instead
    /// of re-running the turn. Privacy: stored runs retain prompts and outputs on the provider.
    /// Unset falls back to `PI_MUSE_STORE_RESPONSES`, then the host's
    /// `configureProviderStoreResponses` default, else off. Ignored on hosts without the capability.
    pub store_responses: Option<bool>,
    /// Request the Daybreak Blue cyber access program on Codex when the account is eligible (see
    /// `openai-codex/access-programs.ts`). `false` omits `access_programs`. Unset falls back to
    /// the host's `configureCodexAccessPrograms` default, else on. Ignored elsewhere.
    pub codex_access_programs: Option<bool>,
    /// Disable native reasoning when the caller supplies an external scratchpad. OpenAI Responses
    /// emits `reasoning: { effort: "none" }`; Anthropic and Google transports use their native
    /// thinking-off controls.
    pub force_reasoning_off: Option<bool>,
    /// Provider-scoped mutable state store for this agent session. Providers can use this to
    /// persist transport/session state between turns.
    pub provider_session_state: Option<HashMap<String, Arc<dyn ProviderSessionState>>>,
    /// Session a side request branches from, so it can read the parent's cached prefix. Providers
    /// that keep per-conversation request controls (OpenAI `configuration_update` effort
    /// baselines) plan the request against a copy of that session's controls, never changing the
    /// parent's state; the Codex backend also routes it with the parent's cache affinity (session headers).
    pub parent_session_id: Option<String>,
    /// Run at the model's lowest effort where the transport carries the change as a per-message
    /// control that keeps the cached prefix (Anthropic per-message effort, OpenAI
    /// `configuration_update`). Elsewhere the requested effort applies unchanged: changing
    /// request-level reasoning forfeits the cache.
    pub minimize_effort: Option<bool>,
    /// Canonical Codex compaction classification; ignored by other providers.
    pub codex_compaction: Option<CodexCompactionRequestContext>,
    /// Optional per-provider concurrent request cap for LLM stream calls. Keys are provider ids
    /// (`model.provider`); positive numeric values cap in-flight requests across local OMP
    /// processes that share the same config root. Omitted providers are unlimited. Non-chat
    /// provider APIs that bypass stream helpers are not covered.
    pub max_in_flight_requests: Option<HashMap<String, u32>>,
    /// Called after the provider slot is acquired, before each model dispatch; returning an error
    /// refuses that attempt.
    pub before_dispatch: Option<Arc<dyn Fn() -> Result<(), String> + Send + Sync>>,
    /// Keep the in-flight permit until a provider's bounded terminal drain finishes. (@internal)
    pub wait_for_terminal_drain: Option<bool>,
    /// Optional callback for inspecting or replacing provider payloads before sending. Return
    /// `None` to keep the payload unchanged. A pre-send standard Responses WS fallback rebuilds
    /// the HTTP candidate and invokes this hook again. WS connection-local delta shaping follows it.
    pub on_payload: Option<OnPayload>,
    /// Optional callback for provider response metadata after headers are received. Standard
    /// Responses WS reports each physical handshake once with metadata.transport="websocket" and
    /// metadata.phase="handshake", not each generation. Its headers are not per-generation
    /// cache/request metadata.
    pub on_response: Option<OnResponse>,
    /// Optional callback for raw Server-Sent Events as they arrive from HTTP streaming providers,
    /// plus synthesized SSE-shaped frames for the Codex WebSocket transport (one synthetic frame
    /// per JSON request/response message). WebSocket frames are tagged with a leading
    /// `: ws -> <type>` (outbound) or `: ws <- <type>` (inbound) comment line in `RawSseEvent.raw`.
    ///
    /// Diagnostic only: provider implementations must ignore callback failures and must not let
    /// observers alter stream contents.
    pub on_sse_event: Option<OnSseEvent>,
    /// Optional override for the first-event watchdog in milliseconds. Built-in providers apply
    /// this budget twice when they can: once to the underlying SDK/request while waiting for the
    /// HTTP stream object to exist, then again in the iterator while waiting for the first
    /// semantic stream event. Set to `0` to disable both layers for this request. After the first
    /// semantic event arrives, `stream_idle_timeout_ms` governs inter-event stalls. Falls back to
    /// `PI_STREAM_FIRST_EVENT_TIMEOUT_MS` and then to a 100s default. OpenAI-family transports
    /// additionally honor `PI_OPENAI_STREAM_FIRST_EVENT_TIMEOUT_MS` as the most-specific override
    /// and floor the first-event budget at the resolved idle (per-call `stream_idle_timeout_ms` or
    /// `PI_OPENAI_STREAM_IDLE_TIMEOUT_MS`) so slow local OpenAI-compatible servers are not
    /// undercut during prompt processing.
    ///
    /// Iterator-level honored by: every built-in provider (via the lazy-stream forwarder in
    /// `register-builtins`). SDK-request honored by: `openai-completions`, `openai-responses`,
    /// `azure-openai-responses`, `anthropic-messages`.
    pub stream_first_event_timeout_ms: Option<u64>,
    /// Optional override for the maximum idle gap between streamed events in milliseconds. Once
    /// the first event arrives, this guards against silent mid-stream stalls (broker dies,
    /// half-open socket, model produces no real progress for too long). Set to `0` to disable.
    /// Falls back to `PI_STREAM_IDLE_TIMEOUT_MS` (alias: `PI_OPENAI_STREAM_IDLE_TIMEOUT_MS`) and
    /// then to a 120s default.
    pub stream_idle_timeout_ms: Option<u64>,
    /// Optional cap on Codex SSE pre-response attempts, including the initial request. WebSocket
    /// retries and outer agent retries have separate budgets. Finite values below `1` and
    /// non-finite values are clamped to one request; omission preserves the provider default.
    pub codex_sse_max_attempts: Option<f64>,
    /// Accept a normal provider stop with no visible text or tool call as a successful
    /// completion. Passive callers and zero-output cache refreshes use this because silence is
    /// their expected result; interactive agent turns retain empty-response retries by default.
    pub accept_empty_response: Option<bool>,
    /// Optional `fetch` implementation override. Providers route every HTTP request (direct
    /// calls, SDK clients, and retry helpers) through this implementation when set. Providers
    /// that do not use HTTP fetch (Bedrock's AWS SDK transport, Cursor's HTTP/2 channel)
    /// silently ignore the override. Standard Responses defaults to HTTP/SSE when set unless a
    /// request or model explicitly selects WebSockets. WebSocket connections do not use this hook.
    pub fetch: Option<FetchImpl>,
    /// Current session working directory for providers that need workspace-scoped discovery.
    pub cwd: Option<String>,
    /// Anthropic fallback credit redemption handle from a prior classifier refusal. When present,
    /// the Anthropic provider replays the frozen request body and betas with the new model and
    /// `fallback_credit_token` to redeem prompt cache credit.
    pub fallback_credit_redemption: Option<AnthropicFallbackCreditHandle>,
    // TODO(port-phase): `liveSteering` (LiveSteering trait), `execHandlers` (CursorExecHandlers),
    // `anthropicSlowMode` (AnthropicSlowModeHooks), and `providerRetryWait` once their modules exist.
}

impl std::fmt::Debug for StreamOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamOptions")
            .field("temperature", &self.temperature)
            .field("max_tokens", &self.max_tokens)
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextVerbosity {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiFormat {
    Openai,
    Anthropic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardrailTrace {
    Enabled,
    Disabled,
    EnabledFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AntigravityEndpointMode {
    Auto,
    Production,
    Sandbox,
}

/// Unified options with reasoning passed to `stream_simple()` and `complete_simple()`.
/// `extends Omit<StreamOptions, "apiKey">`: it carries every `StreamOptions` field (its
/// `api_key` is the resolved string) plus the fields below; the TS `apiKey: ApiKey` resolver
/// lands with `auth-retry`.
#[derive(Clone, Default)]
pub struct SimpleStreamOptions {
    pub base: StreamOptions,
    pub reasoning: Option<Effort>,
    /// Force-disable reasoning for the request even when the model supports it. Takes precedence
    /// over `reasoning`. Useful for fast utility calls (e.g. title generation) where the model
    /// would otherwise burn the entire output budget on internal thinking. Provider support is
    /// format-specific: some transports can disable reasoning directly, while generic
    /// effort-based OpenAI-compatible endpoints use the lowest supported effort.
    pub disable_reasoning: Option<bool>,
    /// If true, request that the provider omit thinking/reasoning summaries from the response
    /// (e.g. Anthropic `thinking.display = "omitted"`, OpenAI Responses `reasoning.summary` left
    /// unset). The model still reasons internally; only the human-readable summary stream is
    /// dropped. Useful when the UI hides thinking blocks anyway and the summary is wasted bandwidth.
    pub hide_thinking_summary: Option<bool>,
    /// OpenAI Responses/Codex `text.verbosity` response detail level.
    pub text_verbosity: Option<TextVerbosity>,
    /// Custom token budgets for thinking levels (token-based providers only)
    pub thinking_budgets: Option<ThinkingBudgets>,
    /// The caller's client runs every tool: Cursor offers them all as MCP tools and hands each call off to it.
    pub cursor_external_tool_executor: Option<bool>,
    /// Amazon Bedrock Guardrail settings forwarded through transports that do not dispatch
    /// directly to the Bedrock provider. Model-level values take precedence when both are present.
    pub guardrail_identifier: Option<String>,
    pub guardrail_version: Option<String>,
    pub guardrail_trace: Option<GuardrailTrace>,
    /// Bedrock invocation-log tags forwarded through transports that do not dispatch directly to
    /// the Bedrock provider. Unlike the guardrail fields above, these MERGE per key with the
    /// model's own `requestMetadata` (these win) rather than replacing it wholesale: they are
    /// independent attribution tags, not one value.
    pub request_metadata: Option<HashMap<String, String>>,
    /// Optional tool choice override for compatible providers
    pub tool_choice: Option<ToolChoice>,
    /// OpenAI service tier for processing priority/cost control. Ignored by non-OpenAI providers.
    pub service_tier: Option<ServiceTier>,
    /// Explicit Kimi Code API format override; omitted uses live per-model protocol metadata.
    pub kimi_api_format: Option<ApiFormat>,
    /// API format for Synthetic provider: "openai" or "anthropic" (default: "openai")
    pub synthetic_api_format: Option<ApiFormat>,
    /// Standard Responses is opt-in and catalog-gated; either model/request false vetoes true.
    /// Codex keeps its independent policy.
    pub prefer_websockets: Option<bool>,
    /// OpenRouter routing-variant suffix automatically appended to model IDs when the request
    /// targets OpenRouter (`model.provider === "openrouter"`). Common values: `"nitro"`
    /// (throughput), `"floor"` (cheapest), `"online"` (web search plugin), `"exacto"`
    /// (cherry-picked high-quality providers, only defined for some models). Ignored when the
    /// resolved model id already contains a `:<variant>` suffix (e.g. the user typed `:nitro`
    /// explicitly or the catalog entry already names the variant).
    pub openrouter_variant: Option<String>,
    /// Caller-owned Google context-cache resource name. Forwarded only to the direct Gemini
    /// GenerateContent and Vertex GenerateContent APIs; all other providers ignore it. Callers
    /// own the cache lifecycle and compatibility.
    pub cached_content: Option<String>,
    /// Antigravity endpoint routing mode: "auto" (default with failover), "production", "sandbox".
    pub antigravity_endpoint_mode: Option<AntigravityEndpointMode>,
    /// Anthropic `server-side-fallback-2026-06-01` fallback chain (top-level `fallbacks` request
    /// field). Opt-in ONLY: leaving this unset is the default and preserves the pre-fallback
    /// behavior on every provider. Non-Anthropic providers ignore the field.
    pub fallbacks: Option<Vec<Value>>, // TODO(port-phase): providers::anthropic_wire::FallbackParam
    /// Factory Droid only: omit refusal fallbacks and reject responses served by another model.
    pub disable_server_side_fallback: Option<bool>,
}

impl std::fmt::Debug for SimpleStreamOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimpleStreamOptions")
            .field("base", &self.base)
            .field("reasoning", &self.reasoning)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(provider: &str, api: &str, class: &str) -> ServiceTierModel {
        ServiceTierModel {
            provider: provider.into(),
            api: api.into(),
            identity_class: class.into(),
            service_tiers: None,
        }
    }

    #[test]
    fn auto_is_never_sent() {
        assert!(!should_send_service_tier(Some(ServiceTier::Auto), Some(ServiceTierTarget::Provider("openai"))));
    }

    #[test]
    fn family_by_provider_and_api() {
        assert_eq!(service_tier_family(&model("openai", "openai-responses", "openai")), Some(ServiceTierFamily::Openai));
        assert_eq!(service_tier_family(&model("amazon-bedrock", "anthropic-messages", "anthropic")), Some(ServiceTierFamily::Anthropic));
        assert_eq!(service_tier_family(&model("openrouter", "openai-completions", "gemini")), Some(ServiceTierFamily::Google));
        assert_eq!(service_tier_family(&model("github-copilot", "openai-completions", "openai")), None);
    }

    #[test]
    fn codex_ultrafast_needs_advertised_tier() {
        let mut m = model("openai-codex", "openai-codex-responses", "openai");
        assert!(!should_send_service_tier(Some(ServiceTier::Ultrafast), Some(ServiceTierTarget::Model(&m))));
        m.service_tiers = Some(vec![ServiceTier::Ultrafast]);
        assert!(should_send_service_tier(Some(ServiceTier::Ultrafast), Some(ServiceTierTarget::Model(&m))));
    }

    #[test]
    fn legacy_scalar_tier_coerces() {
        let got = coerce_service_tier_by_family(&Value::String("claude-only".into())).unwrap();
        assert_eq!(got.get(&ServiceTierFamily::Anthropic), Some(&ServiceTier::Priority));
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn assistant_message_round_trips_in_ts_shape() {
        let json = serde_json::json!({
            "role": "assistant",
            "content": [{"type": "text", "text": "hi"}, {"type": "toolCall", "id": "1", "name": "x", "arguments": {}}],
            "api": "openai-completions", "provider": "openai", "model": "m",
            "usage": {"input": 1, "output": 2, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 3,
                      "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}},
            "stopReason": "toolUse", "timestamp": 5
        });
        let msg: Message = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&msg).unwrap(), json);
    }
}
