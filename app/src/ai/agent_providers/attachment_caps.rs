//! In BYOP mode, infers which multimodal attachment types a model supports by `api_type` × `model_id`.
//!
//! genai 0.6's `ContentPart::Binary` is fully auto-adapted at the wire-protocol layer (see
//! the comment table in `chat_stream.rs`):
//! - OpenAI: image→`image_url{data:URL}`, pdf/file→`type:"file" file_data:data:URL`, audio→`input_audio`
//! - Anthropic: image→`image base64`, others→`document base64` (only PDF actually works)
//! - Gemini: everything goes through `inline_data`
//!
//! But **wire-protocol support** ≠ **model support**. Here we only put the "what the model can actually consume" determination,
//! to avoid sending images to text-only models like GPT-3.5 or Claude Sonnet 1.0 and causing upstream errors.
//!
//! The determination uses model_id substring matching, aligned with the `prompt_renderer::resolve_template` style.
//! The substring rule is deliberately loose (a substring match counts), with the goal of "covering future minor upgrades within the same family"
//! rather than "precise version enumeration"; the tradeoff between misfire probability and maintenance cost leans toward the latter.

use super::models_dev;
use crate::settings::{AgentProviderApiType, AgentProviderModel};

/// A table of a model's support capability for attachment types.
#[derive(Debug, Clone, Copy, Default)]
pub struct AttachmentCaps {
    /// Whether images are supported (image/* MIME).
    pub images: bool,
    /// Whether PDF is supported (application/pdf MIME).
    pub pdf: bool,
    /// Whether audio is supported (audio/* MIME).
    pub audio: bool,
}

impl AttachmentCaps {
    /// No multimodal capability at all → upstream must fall back to the text-only path.
    pub fn is_text_only(&self) -> bool {
        !self.images && !self.pdf && !self.audio
    }

    /// Given a mime, asks whether the model can take this binary attachment.
    pub fn supports_mime(&self, mime: &str) -> bool {
        let lower = mime.trim().to_ascii_lowercase();
        if lower.starts_with("image/") {
            return self.images;
        }
        if lower == "application/pdf" {
            return self.pdf;
        }
        if lower.starts_with("audio/") {
            return self.audio;
        }
        false
    }
}

/// Looks up the models.dev catalog first; on a catalog miss, falls back by (api_type, model_id substring).
///
/// The catalog is the authoritative source of real model capabilities (fetched when the user clicks "Sync from models.dev"
/// in settings, or via the 24h auto-refresh); the fallback rule ensures mainstream models still work while offline / before the fetch.
pub fn caps_for(api_type: AgentProviderApiType, model_id: &str) -> AttachmentCaps {
    if let Some(c) = models_dev::lookup_caps("", model_id) {
        return AttachmentCaps {
            images: c.vision,
            pdf: c.pdf,
            audio: c.audio,
        };
    }
    caps_for_by_substring(api_type, model_id)
}

/// Resolves a single model's final capability, **with user three-state override**. Three-tier priority:
/// 1. user explicitly set `Some(_)` in settings → use directly, bypassing inference
/// 2. `None` → models.dev catalog inference
/// 3. catalog miss → substring fallback
///
/// `provider_id` is used for the catalog's exact provider match (to handle the special path of aggregator
/// providers like OpenRouter); on a catalog miss, the fallback path doesn't need provider_id.
pub fn resolve_for_model(
    provider_id: &str,
    api_type: AgentProviderApiType,
    model: &AgentProviderModel,
) -> AttachmentCaps {
    let inferred = if let Some(c) = models_dev::lookup_caps(provider_id, &model.id) {
        AttachmentCaps {
            images: c.vision,
            pdf: c.pdf,
            audio: c.audio,
        }
    } else {
        caps_for_by_substring(api_type, &model.id)
    };
    AttachmentCaps {
        images: model.image.unwrap_or(inferred.images),
        pdf: model.pdf.unwrap_or(inferred.pdf),
        audio: model.audio.unwrap_or(inferred.audio),
    }
}

/// An "inference result" snapshot for the UI (ignores user overrides, looks only at catalog/fallback).
/// Used to display the "Auto: catalog says supported" semantics in the chip tooltip.
pub fn inferred_for_model(
    provider_id: &str,
    api_type: AgentProviderApiType,
    model_id: &str,
) -> AttachmentCaps {
    if let Some(c) = models_dev::lookup_caps(provider_id, model_id) {
        AttachmentCaps {
            images: c.vision,
            pdf: c.pdf,
            audio: c.audio,
        }
    } else {
        caps_for_by_substring(api_type, model_id)
    }
}

/// Fallback table lookup by (api_type, model_id substring).
///
/// By default it conservatively returns "all false" for all unknown models; the benefit is it won't mistakenly
/// stuff binary into an unsupported model and cause a 400; the cost is that new models need to be added manually after launch (acceptable, since every new model
/// also has other config like reasoning_effort / context_window to update).
fn caps_for_by_substring(api_type: AgentProviderApiType, model_id: &str) -> AttachmentCaps {
    let lower = model_id.to_ascii_lowercase();
    match api_type {
        AgentProviderApiType::OpenAi | AgentProviderApiType::OpenAiResp => {
            // GPT-4o / 4.1 / 5 series: image + pdf. The 3.5 series is text-only.
            if lower.contains("gpt-4o")
                || lower.contains("gpt-4.1")
                || lower.contains("gpt-5")
                || lower.contains("o1")
                || lower.contains("o3")
                || lower.contains("o4")
            {
                AttachmentCaps {
                    images: true,
                    pdf: true,
                    audio: false,
                }
            } else if lower.contains("gpt-4o-audio") || lower.contains("gpt-realtime") {
                AttachmentCaps {
                    images: true,
                    pdf: true,
                    audio: true,
                }
            } else {
                AttachmentCaps::default()
            }
        }
        AgentProviderApiType::Anthropic => {
            // The entire Claude 3 / 3.5 / 4 / 4.5 / 4.7 line supports vision + document (PDF).
            if lower.contains("claude-3")
                || lower.contains("claude-4")
                || lower.contains("claude-opus")
                || lower.contains("claude-sonnet")
                || lower.contains("claude-haiku")
            {
                AttachmentCaps {
                    images: true,
                    pdf: true,
                    audio: false,
                }
            } else {
                AttachmentCaps::default()
            }
        }
        AgentProviderApiType::Gemini => {
            // The entire Gemini 1.5+ / 2 / 2.5 line is multimodal; inline_data supports image/pdf/audio/video.
            if lower.contains("gemini-1.5")
                || lower.contains("gemini-2")
                || lower.contains("gemini-pro-vision")
            {
                AttachmentCaps {
                    images: true,
                    pdf: true,
                    audio: true,
                }
            } else {
                AttachmentCaps::default()
            }
        }
        AgentProviderApiType::Ollama => {
            // Most Ollama models are text-only. Vision models (LLaVA / bakllava / llama3.2-vision /
            // qwen2-vl / minicpm-v / moondream) enable image capability by model_id substring matching.
            // PDF/audio are basically unworkable under the Ollama protocol, so conservatively return false.
            let vision = lower.contains("llava")
                || lower.contains("bakllava")
                || lower.contains("vision")
                || lower.contains("-vl")
                || lower.contains("minicpm-v")
                || lower.contains("moondream");
            AttachmentCaps {
                images: vision,
                pdf: false,
                audio: false,
            }
        }
        AgentProviderApiType::DeepSeek => {
            // DeepSeek's existing public models (v3/r1/coder/chat) are currently all text-only.
            // Enable this when the deepseek-vl series launches in the future.
            if lower.contains("vl") {
                AttachmentCaps {
                    images: true,
                    pdf: false,
                    audio: false,
                }
            } else {
                AttachmentCaps::default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_4o_supports_image_and_pdf() {
        // Uses the fallback rule: in the test environment the models.dev catalog isn't loaded, so lookup_caps returns None.
        let caps = caps_for_by_substring(AgentProviderApiType::OpenAi, "gpt-4o-2024-08-06");
        assert!(caps.images);
        assert!(caps.pdf);
        assert!(!caps.audio);
    }

    #[test]
    fn openai_3_5_text_only() {
        let caps = caps_for_by_substring(AgentProviderApiType::OpenAi, "gpt-3.5-turbo");
        assert!(caps.is_text_only());
    }

    #[test]
    fn claude_sonnet_supports_image_and_pdf() {
        let caps = caps_for_by_substring(AgentProviderApiType::Anthropic, "claude-sonnet-4-5");
        assert!(caps.images);
        assert!(caps.pdf);
    }

    #[test]
    fn gemini_2_5_full_multimodal() {
        let caps = caps_for_by_substring(AgentProviderApiType::Gemini, "gemini-2.5-pro");
        assert!(caps.images);
        assert!(caps.pdf);
        assert!(caps.audio);
    }

    #[test]
    fn ollama_default_text_only() {
        let caps = caps_for_by_substring(AgentProviderApiType::Ollama, "qwen2.5:7b");
        assert!(caps.is_text_only());
    }

    #[test]
    fn ollama_vision_models_get_images() {
        let caps = caps_for_by_substring(AgentProviderApiType::Ollama, "llava:13b");
        assert!(caps.images);
        assert!(!caps.pdf);
    }

    #[test]
    fn deepseek_chat_text_only() {
        let caps = caps_for_by_substring(AgentProviderApiType::DeepSeek, "deepseek-chat");
        assert!(caps.is_text_only());
    }

    #[test]
    fn supports_mime_routing() {
        let full = AttachmentCaps {
            images: true,
            pdf: true,
            audio: true,
        };
        assert!(full.supports_mime("image/png"));
        assert!(full.supports_mime("application/pdf"));
        assert!(full.supports_mime("audio/mp3"));
        assert!(!full.supports_mime("application/zip"));
        assert!(!full.supports_mime("text/plain"));
    }
}
