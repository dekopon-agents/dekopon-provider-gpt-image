//! The manifest: what a model is shown, and what the broker checks its constraint sets against.
//!
//! The input schemas are closed (`additionalProperties: false`) and carry exactly the two fields
//! the route actually honours. The 2026-09-10 spike established that `quality`, `size`,
//! `background`, `model`, and `output_format` are accepted and then ignored — the service picks
//! quality, size, and format itself — so they are not input fields here. A schema that offered them
//! would be a schema that lies to a model about what it controls, and the model would spend turns
//! asking for a size it cannot have.

use dekopon_provider_sdk::{
    EffectKind, ProviderApiVersion, ProviderCapability, ProviderManifest, RiskLevel,
};
use serde_json::{Value, json};

use crate::{COMMAND_WORD, EDIT, GENERATE};

/// Largest prompt the schema admits, matching the native bound.
const MAX_PROMPT_BYTES: usize = 16 * 1024;
/// Longest `data:` URL the schema admits: 8 MiB base64-encoded behind the longest prefix.
const MAX_IMAGE_URL_CHARACTERS: usize = 11_184_835;

/// How the prompt is the only steering wheel, said once and reused in both schemas.
const PROMPT_DESCRIPTION: &str = "What to draw, in words. The service chooses the quality, the \
     size, and the format itself — there is no quality, size, background, or model field — so ask \
     in the prompt for whatever you want steered: the subject, the style, the palette, the \
     composition, and the orientation (\"a tall portrait poster\", \"a wide landscape banner\", \
     \"square\"). One image per call.";

/// The component's manifest.
pub(crate) fn manifest() -> ProviderManifest {
    ProviderManifest {
        api_version: ProviderApiVersion::V1Alpha1,
        id: "gpt-image".parse().expect("static provider ID"),
        description:
            "Generates and edits images with OpenAI's GPT Image models, billed to a ChatGPT \
             subscription, and returns one PNG per invocation as a result attachment"
                .to_owned(),
        command_words: vec![COMMAND_WORD.to_owned()],
        capabilities: vec![
            ProviderCapability {
                id: GENERATE.parse().expect("static capability ID"),
                description:
                    "Generates one new image from a prompt and returns it as a PNG attachment"
                        .to_owned(),
                effect: EffectKind::ExternalWrite,
                risk: RiskLevel::Medium,
                input_schema: generate_schema(),
            },
            ProviderCapability {
                id: EDIT.parse().expect("static capability ID"),
                description:
                    "Remixes one to three supplied images into one new image from a prompt and \
                     returns it as a PNG attachment"
                        .to_owned(),
                effect: EffectKind::ExternalWrite,
                risk: RiskLevel::Medium,
                input_schema: edit_schema(),
            },
        ],
    }
}

/// `gpt-image.generate`: a prompt, and nothing else.
fn generate_schema() -> Value {
    json!({
        "type": "object",
        "required": ["prompt"],
        "additionalProperties": false,
        "properties": {
            "prompt": prompt_property()
        }
    })
}

/// `gpt-image.edit`: a prompt and one to three reference images.
fn edit_schema() -> Value {
    json!({
        "type": "object",
        "required": ["prompt", "images"],
        "additionalProperties": false,
        "properties": {
            "prompt": prompt_property(),
            "images": {
                "type": "array",
                "minItems": 1,
                "maxItems": 3,
                "description":
                    "The images to remix, as data URLs: data:image/png;base64,… (png, jpeg, or \
                     webp), at most 8 MiB decoded each. On a route that allows chat asset inputs \
                     for this capability, write chat-asset:<N> instead and the gateway substitutes \
                     the attachment's bytes before the call; on any other route that marker is \
                     refused.",
                "items": {
                    "type": "string",
                    "minLength": 24,
                    "maxLength": MAX_IMAGE_URL_CHARACTERS,
                    "pattern": "^(data:image/(png|jpeg|webp);base64,|chat-asset:)"
                }
            }
        }
    })
}

/// The prompt property, identical in both schemas.
fn prompt_property() -> Value {
    json!({
        "type": "string",
        "minLength": 1,
        "maxLength": MAX_PROMPT_BYTES,
        "description": PROMPT_DESCRIPTION
    })
}

#[cfg(test)]
mod tests {
    use dekopon_provider_sdk::{EffectKind, RiskLevel};

    use super::manifest;
    use crate::{COMMAND_WORD, EDIT, GENERATE};

    /// The command word cannot contain a separator: `dekopon-core::command_word_conflicts` refuses a
    /// word that parses as a capability identifier, which is why this is `image` and not
    /// `gpt-image`.
    #[test]
    fn the_command_word_is_separator_free() {
        assert_eq!(COMMAND_WORD, "image");
        assert!(!COMMAND_WORD.contains(['.', '-', '_']));
    }

    /// Both capabilities spend the account's quota and create content, so repeating one creates
    /// more. The broker refuses to start when a constraint set disagrees with either of these two.
    /// The third classification, `idempotency`, was removed end to end in dekopon 0.13.0 — the SDK
    /// reads and drops it for one release so pre-0.13.0 components keep loading, but a manifest
    /// built today must not emit it.
    #[test]
    fn both_capabilities_are_medium_risk_external_writes() {
        let manifest = manifest();
        assert_eq!(manifest.id.as_str(), "gpt-image");
        assert_eq!(manifest.command_words, vec!["image".to_owned()]);
        assert_eq!(manifest.capabilities.len(), 2);
        for capability in &manifest.capabilities {
            assert_eq!(capability.effect, EffectKind::ExternalWrite);
            assert_eq!(capability.risk, RiskLevel::Medium);
            assert!(
                capability.id.as_str().starts_with("gpt-image."),
                "{}",
                capability.id
            );
        }
        let ids: Vec<&str> = manifest
            .capabilities
            .iter()
            .map(|capability| capability.id.as_str())
            .collect();
        assert_eq!(ids, [GENERATE, EDIT]);
    }

    /// Closed schemas, and a schema that does not offer a field the route ignores.
    #[test]
    fn the_schemas_are_closed_and_offer_only_what_the_route_honours() {
        for capability in manifest().capabilities {
            let schema = &capability.input_schema;
            assert_eq!(schema["additionalProperties"], false, "{}", capability.id);
            let properties = schema["properties"]
                .as_object()
                .expect("object-shaped properties");
            for ignored in [
                "quality",
                "size",
                "background",
                "model",
                "n",
                "outputFormat",
            ] {
                assert!(
                    !properties.contains_key(ignored),
                    "{} offers {ignored}",
                    capability.id
                );
            }
            let expected: Vec<&str> = if capability.id.as_str() == GENERATE {
                vec!["prompt"]
            } else {
                vec!["images", "prompt"]
            };
            let mut actual: Vec<&str> = properties.keys().map(String::as_str).collect();
            actual.sort_unstable();
            assert_eq!(actual, expected, "{}", capability.id);
            assert!(
                schema["properties"]["prompt"]["description"]
                    .as_str()
                    .expect("a prompt description")
                    .contains("orientation"),
                "{}: the prompt is where orientation is asked for",
                capability.id
            );
        }
    }
}
