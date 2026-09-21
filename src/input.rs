//! Strict native validation of metadata-only inputs. No host imports run here.

use dekopon_provider_sdk::ProviderError;
use serde::Deserialize;
use serde_json::Value;

use crate::error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Operation {
    Generate,
    Edit,
}

const MAX_PROMPT_BYTES: usize = 16 * 1024;
pub(crate) const MAX_IMAGES: usize = 5;
const MAX_ECHOED_DETAIL: usize = 200;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ImageRequest {
    pub(crate) prompt: String,
    /// References only, never image bytes; empty for generate.
    pub(crate) images: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawGenerate {
    prompt: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawEdit {
    prompt: String,
    images: Vec<String>,
}

pub(crate) fn parse(operation: Operation, input: Value) -> Result<ImageRequest, ProviderError> {
    let (prompt, images) = match operation {
        Operation::Generate => {
            let raw: RawGenerate = serde_json::from_value(input).map_err(|error| {
                error::invalid_input(format!(
                    "gpt-image.generate takes exactly one field, `prompt`: {}",
                    bounded(&error.to_string())
                ))
            })?;
            (raw.prompt, Vec::new())
        }
        Operation::Edit => {
            let raw: RawEdit = serde_json::from_value(input).map_err(|error| {
                error::invalid_input(format!(
                    "gpt-image.edit takes exactly two fields, `prompt` and `images`: {}",
                    bounded(&error.to_string())
                ))
            })?;
            validate_images(&raw.images)?;
            (raw.prompt, raw.images)
        }
    };
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        return Err(error::invalid_input("prompt must not be blank"));
    }
    if trimmed.len() > MAX_PROMPT_BYTES {
        return Err(error::invalid_input(format!(
            "prompt is {} bytes; the limit is {MAX_PROMPT_BYTES}",
            trimmed.len()
        )));
    }
    Ok(ImageRequest {
        prompt: trimmed.to_owned(),
        images,
    })
}

/// Also called by the pure command facade so data URLs never enter a proposal.
pub(crate) fn validate_images(images: &[String]) -> Result<(), ProviderError> {
    if images.is_empty() || images.len() > MAX_IMAGES {
        return Err(error::invalid_input(format!(
            "gpt-image.edit takes 1 to {MAX_IMAGES} images; {} were passed",
            images.len()
        )));
    }
    for (index, image) in images.iter().enumerate() {
        let valid = image.strip_prefix("chat-asset:").is_some_and(|number| {
            !number.is_empty()
                && number.len() <= 20
                && number.bytes().all(|byte| byte.is_ascii_digit())
                && number.parse::<u64>().is_ok()
        });
        if !valid {
            return Err(error::invalid_input(format!(
                "images[{index}] must be a chat-asset:<N> reference; data URLs are not accepted"
            )));
        }
    }
    Ok(())
}

fn bounded(detail: &str) -> String {
    if detail.len() <= MAX_ECHOED_DETAIL {
        return detail.to_owned();
    }
    let mut end = MAX_ECHOED_DETAIL;
    while !detail.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &detail[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strict_prompt_and_closed_fields() {
        assert_eq!(
            parse(Operation::Generate, json!({"prompt":"  tangerine\n"}))
                .unwrap()
                .prompt,
            "tangerine"
        );
        for prompt in [
            String::new(),
            " \n\t".into(),
            "a".repeat(MAX_PROMPT_BYTES + 1),
        ] {
            assert!(parse(Operation::Generate, json!({"prompt":prompt})).is_err());
        }
        assert!(
            parse(
                Operation::Generate,
                json!({"prompt":"a".repeat(MAX_PROMPT_BYTES)})
            )
            .is_ok()
        );
        for field in [
            "quality",
            "size",
            "background",
            "model",
            "n",
            "outputFormat",
            "images",
        ] {
            let error =
                parse(Operation::Generate, json!({"prompt":"draw",field:"high"})).unwrap_err();
            assert_eq!(error.code(), error::INVALID_INPUT);
            assert!(error.message().contains(field));
        }
        assert!(parse(Operation::Edit, json!({"prompt":"draw"})).is_err());
        assert!(
            parse(
                Operation::Edit,
                json!({"prompt":"draw","images":["chat-asset:1"],"model":"x"})
            )
            .is_err()
        );
    }

    #[test]
    fn one_through_five_numeric_references_only() {
        for count in 0..=6 {
            let request = parse(
                Operation::Edit,
                json!({"prompt":"remix","images":vec!["chat-asset:1";count]}),
            );
            assert_eq!(request.is_ok(), (1..=5).contains(&count));
        }
        for image in [
            "chat-asset:0",
            "chat-asset:31",
            "chat-asset:18446744073709551615",
        ] {
            assert!(validate_images(&[image.into()]).is_ok());
        }
        for image in [
            "chat-asset:",
            "chat-asset:two",
            "chat-asset:-1",
            "chat-asset:18446744073709551616",
            "chat-asset:1\n",
            "https://example.test/a.png",
            "data:image/png;base64,iVBORw0KGgo=",
        ] {
            let error = validate_images(&[image.into()]).unwrap_err();
            assert_eq!(error.code(), error::INVALID_INPUT);
            assert!(error.message().contains("chat-asset:<N>"));
        }
    }

    #[test]
    fn echoed_detail_is_bounded() {
        let error = parse(
            Operation::Generate,
            json!({"prompt":"draw","q".repeat(4096):1}),
        )
        .unwrap_err();
        assert!(error.message().len() < 400);
        assert!(error.message().ends_with('…'));
    }
}
