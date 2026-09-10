//! The one request this component knows how to make.
//!
//! Both routes are constants, the body is Codex's own body with the caller's prompt substituted,
//! and the header set is three fixed lines. Nothing about the destination, the method, or the
//! credential is reachable from input: the guest cannot be talked into a different endpoint, and
//! the two headers that carry authority are ones it is forbidden to set.

use dekopon_provider_http::{Header, Request, method};
use dekopon_provider_sdk::ProviderError;

use crate::error;
use crate::input::{ImageRequest, Operation};

/// The generation route.
pub(crate) const GENERATE_URL: &str = "https://chatgpt.com/backend-api/codex/images/generations";
/// The edit route.
pub(crate) const EDIT_URL: &str = "https://chatgpt.com/backend-api/codex/images/edits";
/// The model identifier Codex sends, and the only one this component sends.
pub(crate) const MODEL: &str = "gpt-image-2";
/// The client identity this component declares.
pub(crate) const ORIGINATOR: &str = "dekopon";

/// Everything after the prompt, byte for byte as Codex's client serializes it.
///
/// These four values are accepted and then ignored by the route — the 2026-09-10 spike sent
/// `quality: high|xhigh|low`, `size: 1536x1024|1024x1536`, `background: transparent`, and
/// `model: gpt-image-2.5-flare`, and every call came back with the service's own choice. They are
/// still sent, and sent exactly as the verified client sends them, because "what Codex sends" is
/// the only shape this route is known to accept. Nothing about them is selectable, so nothing about
/// them is an input field.
const FIXED_TAIL: &str =
    r#","model":"gpt-image-2","background":"auto","quality":"auto","size":"auto"}"#;

/// Assembles the single POST for one validated request.
///
/// `request` is consumed and each image string is dropped as soon as it has been written, so the
/// body buffer and the input images are never both fully resident.
pub(crate) fn http_request(
    operation: Operation,
    request: ImageRequest,
) -> Result<Request, ProviderError> {
    let uri = match operation {
        Operation::Generate => GENERATE_URL,
        Operation::Edit => EDIT_URL,
    };
    let mut body = Vec::with_capacity(capacity(&request));
    body.push(b'{');
    if !request.images.is_empty() {
        body.extend_from_slice(br#""images":["#);
        // `into_iter` moves each URL out and drops it at the end of its own iteration: at most one
        // image is duplicated between the input vector and the body at any moment.
        for (index, image) in request.images.into_iter().enumerate() {
            if index > 0 {
                body.push(b',');
            }
            body.extend_from_slice(br#"{"image_url":""#);
            // Validated against the base64 alphabet behind a literal ASCII prefix, so the value
            // needs no JSON escaping and can be written as itself.
            body.extend_from_slice(image.as_bytes());
            body.extend_from_slice(br#""}"#);
        }
        body.extend_from_slice(b"],");
    }
    body.extend_from_slice(br#""prompt":"#);
    serde_json::to_writer(&mut body, &request.prompt)
        .map_err(|_| error::invalid_input("prompt could not be encoded as JSON"))?;
    body.extend_from_slice(FIXED_TAIL.as_bytes());

    // `Request::new` and `Header::text` validate the method token, a nonempty URI, and header
    // syntax. All six values are constants here, so a failure is a bug in this module rather than
    // anything a caller can cause — it still fails closed instead of panicking.
    let request = Request::new(method::POST, uri)
        .map_err(|_| error::failure("the fixed image request could not be constructed"))?
        .with_header(
            Header::text("content-type", "application/json")
                .map_err(|_| error::failure("the fixed image request could not be constructed"))?,
        )
        .with_header(
            Header::text("accept", "application/json")
                .map_err(|_| error::failure("the fixed image request could not be constructed"))?,
        )
        .with_header(
            Header::text("originator", ORIGINATOR)
                .map_err(|_| error::failure("the fixed image request could not be constructed"))?,
        )
        .with_body(body);
    Ok(request)
}

/// Enough capacity for the whole body in one allocation: exact for the images, generous for the
/// prompt, whose escaping can expand it.
fn capacity(request: &ImageRequest) -> usize {
    let images: usize = request
        .images
        .iter()
        .map(|image| image.len() + br#"{"image_url":""#.len() + 3)
        .sum();
    images + request.prompt.len() * 2 + FIXED_TAIL.len() + 64
}

#[cfg(test)]
mod tests {
    use super::{EDIT_URL, GENERATE_URL, MODEL, ORIGINATOR, http_request};
    use crate::input::{ImageRequest, Operation};

    fn request(prompt: &str, images: &[&str]) -> ImageRequest {
        ImageRequest {
            prompt: prompt.to_owned(),
            images: images.iter().map(|image| (*image).to_owned()).collect(),
        }
    }

    fn body(operation: Operation, request: ImageRequest) -> String {
        let http = http_request(operation, request).expect("the fixed request is constructible");
        String::from_utf8(http.body).expect("the body is UTF-8 JSON")
    }

    /// The generate body, byte for byte. Codex's own body with this prompt substituted.
    #[test]
    fn the_generate_body_is_codexs_fixed_body() {
        assert_eq!(
            body(Operation::Generate, request("a tangerine", &[])),
            r#"{"prompt":"a tangerine","model":"gpt-image-2","background":"auto","quality":"auto","size":"auto"}"#
        );
    }

    /// The edit body, byte for byte: images first, then the identical fixed tail.
    #[test]
    fn the_edit_body_carries_images_then_codexs_fixed_body() {
        assert_eq!(
            body(
                Operation::Edit,
                request(
                    "repaint as a watercolour",
                    &[
                        "data:image/png;base64,iVBORw0KGgo=",
                        "data:image/webp;base64,UklGRg=="
                    ],
                ),
            ),
            concat!(
                r#"{"images":[{"image_url":"data:image/png;base64,iVBORw0KGgo="},"#,
                r#"{"image_url":"data:image/webp;base64,UklGRg=="}],"#,
                r#""prompt":"repaint as a watercolour","model":"gpt-image-2","#,
                r#""background":"auto","quality":"auto","size":"auto"}"#
            )
        );
    }

    #[test]
    fn the_prompt_is_json_escaped_and_nothing_else_is() {
        let body = body(
            Operation::Generate,
            request("a \"tangerine\"\n\tand a \\ backslash", &[]),
        );
        assert!(
            body.starts_with(r#"{"prompt":"a \"tangerine\"\n\tand a \\ backslash","#),
            "{body}"
        );
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(parsed["prompt"], "a \"tangerine\"\n\tand a \\ backslash");
        assert_eq!(parsed["model"], MODEL);
    }

    /// The two headers that carry authority are the broker's to add. A guest that sets either is
    /// rejected by the host rather than overwritten, so this asserts their absence directly.
    #[test]
    fn the_guest_sets_three_headers_and_no_credential() {
        for (operation, expected_uri) in [
            (Operation::Generate, GENERATE_URL),
            (Operation::Edit, EDIT_URL),
        ] {
            let images = ["data:image/png;base64,iVBORw0KGgo="];
            let input = match operation {
                Operation::Generate => request("a tangerine", &[]),
                Operation::Edit => request("remix", &images),
            };
            let http = http_request(operation, input).expect("constructible");
            assert_eq!(http.method, "POST");
            assert_eq!(http.uri, expected_uri);
            let names: Vec<&str> = http.headers.iter().map(|h| h.name.as_str()).collect();
            assert_eq!(names, ["content-type", "accept", "originator"]);
            for header in &http.headers {
                assert!(
                    !header.name.eq_ignore_ascii_case("authorization")
                        && !header.name.eq_ignore_ascii_case("chatgpt-account-id"),
                    "{}",
                    header.name
                );
            }
            let originator = http
                .headers
                .iter()
                .find(|header| header.name == "originator")
                .expect("the originator header");
            assert_eq!(originator.value, ORIGINATOR.as_bytes());
        }
    }

    /// The body is assembled in one allocation; a reallocation mid-assembly would mean two copies
    /// of an eight-megabyte image existing at once, which the 64 MiB store cannot spare.
    #[test]
    fn the_body_buffer_is_allocated_once() {
        let image = format!("data:image/png;base64,{}", "A".repeat(4096));
        let http = http_request(
            Operation::Edit,
            request(&"p".repeat(1024), &[image.as_str(), image.as_str()]),
        )
        .expect("constructible");
        assert!(
            http.body.capacity() >= http.body.len(),
            "{} < {}",
            http.body.capacity(),
            http.body.len()
        );
        assert_eq!(
            http.body.capacity(),
            super::capacity(&request(
                &"p".repeat(1024),
                &[image.as_str(), image.as_str()]
            )),
            "the buffer grew past its reservation"
        );
    }
}
