//! Fixed Codex POST bodies, with image bytes supplied only by host-streamed asset parts.

use dekopon_provider_http::{Header, method};
use dekopon_provider_sdk::{ProviderError, asset::Encoding};

use crate::{error, input::ImageRequest, input::Operation};

pub(crate) const GENERATE_URL: &str = "https://chatgpt.com/backend-api/codex/images/generations";
pub(crate) const EDIT_URL: &str = "https://chatgpt.com/backend-api/codex/images/edits";
pub(crate) const MODEL: &str = "gpt-image-2";
pub(crate) const ORIGINATOR: &str = "dekopon";
const FIXED_TAIL: &str =
    r#","model":"gpt-image-2","background":"auto","quality":"auto","size":"auto"}"#;

/// Private generic seam: production maps these directly to the SDK's streamed parts;
/// tests compose them with fake assets without requiring a component asset host.
pub(crate) enum Part<'a, H> {
    Literal(Vec<u8>),
    Asset { handle: &'a H, encoding: Encoding },
}

pub(crate) struct Request<'a, H> {
    pub(crate) method: String,
    pub(crate) uri: String,
    pub(crate) headers: Vec<Header>,
    pub(crate) body: Vec<Part<'a, H>>,
}

pub(crate) fn http_request<'a, H>(
    operation: Operation,
    request: &ImageRequest,
    images: &'a [(H, String)],
) -> Result<Request<'a, H>, ProviderError> {
    let mut body = Vec::new();
    let mut literal = Vec::from(&b"{"[..]);
    if !images.is_empty() {
        literal.extend_from_slice(br#""images":["#);
        for (index, (handle, content_type)) in images.iter().enumerate() {
            // Validate before interpolating metadata into a JSON string, even for direct tests.
            if !matches!(
                content_type.as_str(),
                "image/png" | "image/jpeg" | "image/webp"
            ) {
                return Err(error::invalid_input(
                    "reference images must be image/png, image/jpeg or image/webp",
                ));
            }
            if index > 0 {
                literal.push(b',');
            }
            literal.extend_from_slice(br#"{"image_url":"data:"#);
            literal.extend_from_slice(content_type.as_bytes());
            literal.extend_from_slice(b";base64,");
            body.push(Part::Literal(std::mem::take(&mut literal)));
            body.push(Part::Asset {
                handle,
                encoding: Encoding::Base64,
            });
            literal.extend_from_slice(br#""}"#);
        }
        literal.extend_from_slice(b"],");
    }
    literal.extend_from_slice(br#""prompt":"#);
    serde_json::to_writer(&mut literal, &request.prompt)
        .map_err(|_| error::invalid_input("prompt could not be encoded as JSON"))?;
    literal.extend_from_slice(FIXED_TAIL.as_bytes());
    body.push(Part::Literal(literal));
    let headers = [
        ("content-type", "application/json"),
        ("accept", "application/json"),
        ("originator", ORIGINATOR),
    ]
    .into_iter()
    .map(|(name, value)| {
        Header::text(name, value)
            .map_err(|_| error::failure("the fixed image request could not be constructed"))
    })
    .collect::<Result<_, _>>()?;
    Ok(Request {
        method: method::POST.into(),
        uri: match operation {
            Operation::Generate => GENERATE_URL,
            Operation::Edit => EDIT_URL,
        }
        .into(),
        headers,
        body,
    })
}
