//! The second validation layer: strict native decoding plus semantic bounds.
//!
//! The JSON Schema in the manifest is what a model is shown; this is what is enforced. Both are
//! closed — `additionalProperties: false` there, `deny_unknown_fields` here — so a field the route
//! ignores cannot be smuggled past the schema by a model that read OpenAI's platform documentation
//! instead of this provider's.

use dekopon_provider_sdk::ProviderError;
use serde::Deserialize;
use serde_json::Value;

use crate::{b64, error};

/// The two capabilities, and the only thing that differs between their request shapes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Operation {
    /// `gpt-image.generate`: a prompt alone.
    Generate,
    /// `gpt-image.edit`: a prompt plus one to three images.
    Edit,
}

/// Smallest prompt, in bytes, after trimming.
const MIN_PROMPT_BYTES: usize = 1;
/// Largest prompt, in bytes: the shell's inbound text bound.
const MAX_PROMPT_BYTES: usize = 16 * 1024;
/// Fewest images an edit takes.
const MIN_IMAGES: usize = 1;
/// Most images an edit takes. The route documents five; the frame budget allows three.
const MAX_IMAGES: usize = 3;
/// Largest decoded size of one input image: the gateway's per-asset bound.
const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
/// Smallest decoded size of one input image. Below this it cannot carry a file header.
const MIN_IMAGE_BYTES: usize = 16;
/// The data URL forms an edit accepts, in schema order.
const IMAGE_PREFIXES: [&str; 3] = [
    "data:image/png;base64,",
    "data:image/jpeg;base64,",
    "data:image/webp;base64,",
];
/// The gateway's reserved input marker. Reaching the guest means the route did not expand it.
const CHAT_ASSET_MARKER: &str = "chat-asset:";
/// Longest untrusted detail repeated back inside an error message.
const MAX_ECHOED_DETAIL: usize = 200;

/// A validated request, owning its strings so nothing is copied again on the way to the wire.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ImageRequest {
    /// The trimmed prompt.
    pub(crate) prompt: String,
    /// Validated `data:` URLs; empty for `generate`.
    pub(crate) images: Vec<String>,
}

/// `gpt-image.generate` input.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawGenerate {
    prompt: String,
}

/// `gpt-image.edit` input.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawEdit {
    prompt: String,
    images: Vec<String>,
}

/// Decodes and validates one capability's input.
///
/// `input` is consumed rather than borrowed: `serde_json::from_value` moves each string out of the
/// tree instead of copying it, which for an edit is the difference between holding one copy of
/// eight megabytes of base64 and holding two.
pub(crate) fn parse(operation: Operation, input: Value) -> Result<ImageRequest, ProviderError> {
    match operation {
        Operation::Generate => {
            let raw: RawGenerate = serde_json::from_value(input).map_err(|error| {
                error::invalid_input(format!(
                    "gpt-image.generate takes exactly one field, `prompt`: {}",
                    bounded(&error.to_string())
                ))
            })?;
            Ok(ImageRequest {
                prompt: prompt(raw.prompt)?,
                images: Vec::new(),
            })
        }
        Operation::Edit => {
            let raw: RawEdit = serde_json::from_value(input).map_err(|error| {
                error::invalid_input(format!(
                    "gpt-image.edit takes exactly two fields, `prompt` and `images`: {}",
                    bounded(&error.to_string())
                ))
            })?;
            Ok(ImageRequest {
                prompt: prompt(raw.prompt)?,
                images: images(raw.images)?,
            })
        }
    }
}

/// Trims the prompt and bounds it.
fn prompt(prompt: String) -> Result<String, ProviderError> {
    let trimmed = prompt.trim();
    if trimmed.len() < MIN_PROMPT_BYTES {
        return Err(error::invalid_input("prompt must not be blank"));
    }
    if trimmed.len() > MAX_PROMPT_BYTES {
        return Err(error::invalid_input(format!(
            "prompt is {} bytes; the limit is {MAX_PROMPT_BYTES}",
            trimmed.len()
        )));
    }
    // Trimming is the only rewrite: `--prompt -` arrives with the pipe's trailing newline, and the
    // route is handed what the caller meant rather than what the shell appended.
    if trimmed.len() == prompt.len() {
        Ok(prompt)
    } else {
        Ok(trimmed.to_owned())
    }
}

/// Validates every image reference: the marker first, then the data URL, then its base64 and size.
fn images(images: Vec<String>) -> Result<Vec<String>, ProviderError> {
    if images.len() < MIN_IMAGES || images.len() > MAX_IMAGES {
        return Err(error::invalid_input(format!(
            "gpt-image.edit takes {MIN_IMAGES} to {MAX_IMAGES} images; {} were passed",
            images.len()
        )));
    }
    for (index, image) in images.iter().enumerate() {
        check_image(index, image)?;
    }
    Ok(images)
}

/// One image reference, by position so a message can name which one failed.
fn check_image(index: usize, image: &str) -> Result<(), ProviderError> {
    if is_chat_asset_marker(image) {
        // The gateway expands `chat-asset:<N>` into a data URL before proposing, but only for the
        // capabilities a route lists in `chatAssetInputs`. An unexpanded marker therefore is not a
        // malformed input; it is an unconfigured route, and the message has to say so.
        return Err(error::invalid_input(
            "route does not allow chat asset inputs for gpt-image.edit",
        ));
    }
    let Some(payload) = IMAGE_PREFIXES
        .iter()
        .find_map(|prefix| image.strip_prefix(prefix))
    else {
        return Err(error::invalid_input(format!(
            "images[{index}] must be a data URL: one of {}",
            IMAGE_PREFIXES.join(" ")
        )));
    };
    if !b64::is_standard(payload) {
        return Err(error::invalid_input(format!(
            "images[{index}] is not standard base64"
        )));
    }
    let Some(bytes) = b64::decoded_len(payload) else {
        return Err(error::invalid_input(format!(
            "images[{index}] is not a whole number of base64 groups"
        )));
    };
    if bytes < MIN_IMAGE_BYTES {
        return Err(error::invalid_input(format!(
            "images[{index}] decodes to {bytes} bytes, which cannot be an image"
        )));
    }
    if bytes > MAX_IMAGE_BYTES {
        return Err(error::invalid_input(format!(
            "images[{index}] decodes to {bytes} bytes; the limit is {MAX_IMAGE_BYTES} per image"
        )));
    }
    Ok(())
}

/// Whether `value` is exactly the gateway's `chat-asset:<N>` marker.
fn is_chat_asset_marker(value: &str) -> bool {
    value.strip_prefix(CHAT_ASSET_MARKER).is_some_and(|number| {
        !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
    })
}

/// Truncates untrusted detail at a character boundary so an error message stays bounded.
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
    use serde_json::json;

    use super::{
        IMAGE_PREFIXES, MAX_IMAGE_BYTES, MAX_PROMPT_BYTES, Operation, is_chat_asset_marker, parse,
    };
    use crate::b64::tests::encode;
    use crate::error::INVALID_INPUT;

    /// A `data:image/png;base64,` URL whose payload decodes to `bytes` bytes.
    fn data_url(bytes: usize) -> String {
        format!("{}{}", IMAGE_PREFIXES[0], encode(&vec![0x89_u8; bytes]))
    }

    #[test]
    fn generate_takes_a_prompt_and_nothing_else() {
        let request = parse(Operation::Generate, json!({"prompt": "  a tangerine  "}))
            .expect("a prompt is enough");
        assert_eq!(request.prompt, "a tangerine");
        assert!(request.images.is_empty());
    }

    /// Every request field the route ignores is refused rather than accepted as a no-op. A model
    /// that learned the platform API's parameters gets told so instead of believing it chose a size.
    #[test]
    fn ignored_platform_parameters_are_refused_by_name() {
        for field in [
            "quality",
            "size",
            "background",
            "model",
            "n",
            "outputFormat",
        ] {
            let input = json!({"prompt": "a tangerine", field: "high"});
            let error = parse(Operation::Generate, input).expect_err("closed input");
            assert_eq!(error.code(), INVALID_INPUT, "{field}");
            assert!(
                error.message().contains(field),
                "{field}: {}",
                error.message()
            );
        }
    }

    #[test]
    fn a_blank_or_oversized_prompt_is_refused() {
        for blank in ["", "   ", "\n\t "] {
            let error = parse(Operation::Generate, json!({"prompt": blank}))
                .expect_err("a blank prompt is no prompt");
            assert_eq!(error.code(), INVALID_INPUT);
            assert!(error.message().contains("blank"));
        }
        let long = "a".repeat(MAX_PROMPT_BYTES + 1);
        let error = parse(Operation::Generate, json!({"prompt": long}))
            .expect_err("the prompt bound is enforced");
        assert!(
            error
                .message()
                .contains(&(MAX_PROMPT_BYTES + 1).to_string())
        );

        let exact = "a".repeat(MAX_PROMPT_BYTES);
        assert!(parse(Operation::Generate, json!({"prompt": exact})).is_ok());
    }

    #[test]
    fn edit_requires_between_one_and_three_images() {
        let url = data_url(64);
        for count in [1_usize, 2, 3] {
            let images = vec![url.clone(); count];
            let request = parse(
                Operation::Edit,
                json!({"prompt": "remix", "images": images}),
            )
            .expect("one to three images");
            assert_eq!(request.images.len(), count);
        }
        for count in [0_usize, 4] {
            let images = vec![url.clone(); count];
            let error = parse(
                Operation::Edit,
                json!({"prompt": "remix", "images": images}),
            )
            .expect_err("outside the bound");
            assert_eq!(error.code(), INVALID_INPUT);
            assert!(error.message().contains(&count.to_string()));
        }
    }

    #[test]
    fn an_unexpanded_chat_asset_marker_names_the_route() {
        let input = json!({"prompt": "remix", "images": ["chat-asset:2"]});
        let error = parse(Operation::Edit, input).expect_err("the gateway did not expand it");
        assert_eq!(error.code(), INVALID_INPUT);
        assert_eq!(
            error.message(),
            "route does not allow chat asset inputs for gpt-image.edit"
        );

        assert!(is_chat_asset_marker("chat-asset:0"));
        assert!(is_chat_asset_marker("chat-asset:31"));
        assert!(!is_chat_asset_marker("chat-asset:"));
        assert!(!is_chat_asset_marker("chat-asset:two"));
        assert!(!is_chat_asset_marker("data:image/png;base64,iVBORw0K"));
    }

    #[test]
    fn image_references_must_be_data_urls_with_valid_base64_under_the_size_bound() {
        let cases = [
            ("https://example.test/cat.png", "data URL"),
            ("data:image/gif;base64,R0lGODlh", "data URL"),
            ("data:image/png;base64,not base64!", "standard base64"),
            ("data:image/png;base64,aGVsbG8=", "cannot be an image"),
        ];
        for (image, expected) in cases {
            let input = json!({"prompt": "remix", "images": [image]});
            let error = parse(Operation::Edit, input).expect_err("rejected");
            assert_eq!(error.code(), INVALID_INPUT, "{image}");
            assert!(
                error.message().contains(expected),
                "{image}: {}",
                error.message()
            );
        }

        for prefix in IMAGE_PREFIXES {
            let image = format!("{prefix}{}", encode(&[0x89_u8; 64]));
            let input = json!({"prompt": "remix", "images": [image]});
            assert!(parse(Operation::Edit, input).is_ok(), "{prefix}");
        }
    }

    /// The size bound is checked from the base64 length, so the oversize case never allocates the
    /// decoded image — which is the whole point of checking it that way.
    #[test]
    fn an_image_over_eight_megabytes_is_refused_without_decoding_it() {
        let oversize = data_url(MAX_IMAGE_BYTES + 1);
        let input = json!({"prompt": "remix", "images": [oversize]});
        let error = parse(Operation::Edit, input).expect_err("over the bound");
        assert_eq!(error.code(), INVALID_INPUT);
        assert!(error.message().contains(&(MAX_IMAGE_BYTES + 1).to_string()));

        let exact = data_url(MAX_IMAGE_BYTES);
        let input = json!({"prompt": "remix", "images": [exact]});
        assert!(parse(Operation::Edit, input).is_ok());
    }

    #[test]
    fn generate_refuses_images_and_edit_refuses_their_absence() {
        let error = parse(
            Operation::Generate,
            json!({"prompt": "a tangerine", "images": [data_url(64)]}),
        )
        .expect_err("generate takes no images");
        assert_eq!(error.code(), INVALID_INPUT);
        assert!(error.message().contains("images"));

        let error =
            parse(Operation::Edit, json!({"prompt": "remix"})).expect_err("edit needs images");
        assert_eq!(error.code(), INVALID_INPUT);
        assert!(error.message().contains("images"));
    }

    /// Untrusted detail inside a message is truncated; a model cannot inflate a failure into a
    /// megabyte of transcript by naming a field badly.
    #[test]
    fn echoed_detail_is_bounded() {
        let field = "q".repeat(4096);
        let input = json!({"prompt": "a tangerine", field: 1});
        let error = parse(Operation::Generate, input).expect_err("unknown field");
        assert!(error.message().len() < 400, "{}", error.message().len());
        assert!(error.message().ends_with('…'));
    }
}
