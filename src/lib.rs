//! Two GPT Image capabilities for Dekopon: generate one image, or remix one to three.
//!
//! The component accepts no endpoint, no credential, and no model selection. It builds exactly one
//! POST to a hardcoded `chatgpt.com` route, with Codex's own request body and the caller's prompt
//! substituted, and projects the answer into a bounded result whose only large field is the PNG the
//! route returned, base64 as it arrived. HTTP authority stays broker-owned; the credential is
//! injected inside the broker's native engine, for destinations inside its binding, where no guest
//! can observe it. This guest never sets `authorization` or `chatgpt-account-id` — the host rejects
//! both from a guest by construction rather than overwriting them.
//!
//! There are no retries. One invocation is one POST: the route spends the account's image quota and
//! publishes nothing idempotent, so a repeat is a second image and a second charge, which is a
//! decision for whoever is asking, not for this component.
//!
//! The image is the interesting engineering constraint, not the HTTP. An 8 MiB PNG is ~10.7 MiB of
//! base64, the guest store is 64 MiB by default, and the result has to carry the blob back out.
//! `response` documents the borrow-measure-trim discipline that fits both in one allocation, and
//! `peak_allocation` below measures it rather than asserting it.
//!
//! Unlike dekopon's own crates this guest cannot `#![forbid(unsafe_code)]`: the generated component
//! bindings contain `unsafe` by construction. No hand-written code in the shipped component is
//! unsafe; the one hand-written `unsafe` block in this crate is the measuring allocator in
//! `probe`, which is `#[cfg(test)]` and is never compiled into the component.

use dekopon_provider_http::{HttpError, Request, Response};
use dekopon_provider_sdk::{CapabilityId, CommandRun, Provider, ProviderError, ProviderManifest};
use serde_json::Value;

mod b64;
mod commands;
mod error;
mod input;
mod manifest;
mod response;
mod wire;

#[cfg(test)]
mod probe;

/// Generates one new image from a prompt.
pub(crate) const GENERATE: &str = "gpt-image.generate";
/// Remixes one to three supplied images.
pub(crate) const EDIT: &str = "gpt-image.edit";
/// The command word this provider contributes to the sandboxed shell.
///
/// Separator-free on purpose: `dekopon-core::command_word_conflicts` refuses a word that parses as a
/// capability identifier, so `gpt-image` could never be the word even though it is the provider id.
pub(crate) const COMMAND_WORD: &str = "image";

mod bindings {
    wit_bindgen::generate!({
        path: "wit",
        world: "provider",
        generate_all,
        pub_export_macro: true,
    });
}

/// The provider.
struct GptImage;

impl Provider for GptImage {
    fn manifest() -> ProviderManifest {
        manifest::manifest()
    }

    fn invoke(capability: &CapabilityId, input: Value) -> Result<Value, ProviderError> {
        invoke_with(capability, input, dekopon_provider_http::send)
    }

    fn run_command(argv: &[String], stdin: Option<&str>) -> Result<CommandRun, ProviderError> {
        commands::run(argv, stdin)
    }
}

/// The one place a capability becomes a request, with the transport injected.
///
/// Taking `send` as a closure is what lets every test assert the exact bytes of the request and the
/// exact projection of a response with no network and no host: the seam is the same one the
/// component uses, so what the tests exercise is what ships.
fn invoke_with<F>(
    capability: &CapabilityId,
    input: Value,
    mut send: F,
) -> Result<Value, ProviderError>
where
    F: FnMut(Request) -> Result<Response, HttpError>,
{
    let operation = match capability.as_str() {
        GENERATE => input::Operation::Generate,
        EDIT => input::Operation::Edit,
        _ => return Err(error::unknown_capability()),
    };
    let request = wire::http_request(operation, input::parse(operation, input)?)?;
    let response = send(request).map_err(|error| error::transport(&error))?;
    response::project(response)
}

dekopon_provider_sdk::export_provider_with_cli!(GptImage, bindings);

#[cfg(test)]
pub(crate) fn capability(value: &str) -> CapabilityId {
    value.parse().expect("valid capability fixture")
}

#[cfg(test)]
mod tests {
    use dekopon_provider_http::{HttpError, HttpErrorCode, Request, Response};
    use dekopon_provider_sdk::{ComponentResponse, Provider};
    use serde_json::{Value, json};

    use super::{EDIT, GENERATE, GptImage, capability, invoke_with};
    use crate::b64::tests::png_payload;
    use crate::error::{INVALID_INPUT, UPSTREAM_FAILURE};
    use crate::probe;

    const GENERATE_RESPONSE: &str = include_str!("../tests/fixtures/generate-response.json");

    /// A response that needs no network: one call, the fixture back.
    fn once(body: &str) -> impl FnMut(Request) -> Result<Response, HttpError> + use<'_> {
        let mut calls = 0_usize;
        move |_request| {
            calls += 1;
            assert_eq!(calls, 1, "one invocation is one POST");
            Ok(Response {
                status: 200,
                headers: Vec::new(),
                body: body.as_bytes().to_vec(),
            })
        }
    }

    /// The WIT in this repository is a mirror. Nothing shared keeps it honest out of tree, so the
    /// crates' own copies are the reference — in CI against the pinned source, and here against the
    /// compiled constants.
    #[test]
    fn mirrored_wit_exactly_matches_the_pinned_crates() {
        assert_eq!(
            include_str!("../wit/deps/provider.wit"),
            dekopon_provider_sdk::PROVIDER_WIT
        );
        assert_eq!(
            include_str!("../wit/deps/http.wit"),
            dekopon_provider_http::HTTP_WIT
        );
    }

    /// The manifest, pinned byte for byte. Effect and risk have to match the broker's constraint
    /// sets and the gateway's catalog exactly, and a snapshot is how a change to either of them
    /// becomes a diff a reviewer sees.
    #[test]
    fn manifest_snapshot() {
        let actual = format!(
            "{}\n",
            serde_json::to_string_pretty(&GptImage::manifest()).expect("manifest serializes")
        );
        let expected = include_str!("../tests/fixtures/manifest.json");
        assert_eq!(actual, expected);

        let decoded: Value = serde_json::from_str(expected).expect("the snapshot is JSON");
        assert_eq!(decoded["capabilities"].as_array().expect("array").len(), 2);
        assert_eq!(decoded["commandWords"], json!(["image"]));
    }

    #[test]
    fn one_invocation_is_one_post_and_one_attachment() {
        let output = invoke_with(
            &capability(GENERATE),
            json!({"prompt": "a tangerine"}),
            once(GENERATE_RESPONSE),
        )
        .expect("a usable response");
        assert_eq!(output["attachments"][0]["mediaType"], "image/png");
        assert_eq!(output["image"]["bytes"], 69);
        assert_eq!(output["model"], "gpt-image-2");
    }

    #[test]
    fn an_unknown_capability_and_a_transport_failure_are_classified() {
        let error = invoke_with(
            &capability("gpt-image.upscale"),
            json!({"prompt": "a tangerine"}),
            once(GENERATE_RESPONSE),
        )
        .expect_err("not implemented");
        assert_eq!(error.code(), INVALID_INPUT);

        let error = invoke_with(
            &capability(GENERATE),
            json!({"prompt": "a tangerine"}),
            |_request| {
                Err(HttpError {
                    code: HttpErrorCode::Connect,
                    message: "connect 10.1.2.3:443 refused".to_owned(),
                })
            },
        )
        .expect_err("the route was unreachable");
        assert_eq!(error.code(), UPSTREAM_FAILURE);
        assert!(!error.message().contains("10.1.2.3"));
    }

    /// Invalid input never reaches the network: the closure asserts it was not called.
    #[test]
    fn invalid_input_costs_no_request() {
        let error = invoke_with(&capability(EDIT), json!({"prompt": "remix"}), |_request| {
            panic!("no request may be sent for invalid input")
        })
        .expect_err("edit needs images");
        assert_eq!(error.code(), INVALID_INPUT);
    }

    /// The base64 an 8 MiB PNG arrives as, which is the deployment's worst case in both
    /// directions: `8,388,606` decoded bytes, a whole number of base64 groups.
    const FULL_SIZE: usize = 11_184_808;

    /// A full-size upstream response carrying `payload`, in one exactly-sized allocation — which is
    /// what the host's write into guest memory is.
    fn full_size_response(payload: &str, quality: &str) -> Vec<u8> {
        let mut body = String::with_capacity(payload.len() + 512);
        body.push_str(r#"{"created":1788998400,"data":[{"b64_json":""#);
        body.push_str(payload);
        body.push_str(r#"","generation_id":"img_01JQ8Z7K3M4N5P6Q7R8S9T0V1W"}],"background":"opaque","quality":""#);
        body.push_str(quality);
        body.push_str(r#"","size":"1370x1148","output_format":"png","usage":{"input_tokens":26,"output_tokens":772,"total_tokens":798}}"#);
        body.into_bytes()
    }

    /// What this component itself costs: one copy of the image, and nothing else of that size.
    ///
    /// This is the claim `response`'s discipline makes — the body arrives in one allocation, is
    /// parsed by borrowing from it, is trimmed in place, and is handed back as that same
    /// allocation — and it is the number that is actually under this crate's control.
    #[test]
    fn the_component_itself_holds_exactly_one_copy_of_the_image() {
        let payload = png_payload(FULL_SIZE);
        let (output, measured) = probe::measure(|| {
            let mut response = Some(Response {
                status: 200,
                headers: Vec::new(),
                body: full_size_response(&payload, "medium"),
            });
            invoke_with(
                &capability(GENERATE),
                json!({"prompt": "a tangerine on a desk"}),
                |_request| Ok(response.take().expect("exactly one call")),
            )
            .expect("a usable response")
        });

        assert_eq!(
            output["attachments"][0]["base64"]
                .as_str()
                .expect("base64")
                .len(),
            FULL_SIZE
        );
        let budget = FULL_SIZE + FULL_SIZE / 4;
        assert!(
            measured.copying < budget,
            "the component held more than one image: {}",
            measured.describe()
        );
        println!("component only: {}", measured.describe());
    }

    /// The generate path end to end, the way the SDK runs it: the host's body write, this
    /// component's projection, and then `serde_json` serializing the `Value` that
    /// `Provider::invoke` has to return.
    ///
    /// That last step is the second copy of the image and it is not optional at this SDK revision:
    /// `invoke` returns a `serde_json::Value`, the SDK serializes it, and its output buffer doubles
    /// geometrically — so the envelope is built in a buffer of its own and, when the allocator
    /// cannot extend that buffer in place, in two generations of one. The 64 MiB store has room for
    /// the worst of those; `docs` in `response` and the README record the arithmetic.
    #[test]
    fn peak_allocation_on_a_full_size_generate_stays_under_forty_megabytes() {
        let payload = png_payload(FULL_SIZE);
        let (envelope, measured) = probe::measure(|| {
            let mut response = Some(Response {
                status: 200,
                headers: Vec::new(),
                body: full_size_response(&payload, "medium"),
            });
            let output = invoke_with(
                &capability(GENERATE),
                json!({"prompt": "a tangerine on a desk"}),
                |_request| Ok(response.take().expect("exactly one call")),
            )
            .expect("a usable response");
            serde_json::to_string(&ComponentResponse::Succeeded { output })
                .expect("the envelope serializes")
                .len()
        });

        assert!(envelope > FULL_SIZE, "the envelope carries the image");
        assert!(
            measured.in_place < 40 * 1024 * 1024,
            "peak was {}, over the 40 MiB budget",
            measured.describe()
        );
        assert!(
            measured.copying < 48 * 1024 * 1024,
            "peak was {}, over the 48 MiB bound",
            measured.describe()
        );
        println!("full generate: {}", measured.describe());
    }

    /// The edit path, with the input counted too, which is the true worst case for the store.
    ///
    /// `Provider::invoke` takes a `serde_json::Value`, so the SDK has already parsed the input JSON
    /// into an owned tree before this component sees it, and it holds the original `input_json`
    /// string until the invocation returns. An edit at the deployment's ceiling therefore starts
    /// with two copies of ~11 MiB that no guest discipline can avoid. What this component controls is
    /// that it adds only one more — the request body — and frees each data URL as it writes it.
    #[test]
    fn peak_allocation_on_a_full_size_edit_stays_inside_the_store() {
        let input_payload = png_payload(FULL_SIZE);
        let response_payload = png_payload(FULL_SIZE);

        let (envelope, measured) = probe::measure(|| {
            let mut input_json = String::with_capacity(FULL_SIZE + 512);
            input_json.push_str(
                r#"{"prompt":"repaint as a loose watercolour sketch; keep the composition","images":["data:image/png;base64,"#,
            );
            input_json.push_str(&input_payload);
            input_json.push_str(r#""]}"#);
            // What the SDK does, and why the input counts twice: parse into an owned tree while the
            // original string stays alive for the rest of the invocation.
            let input: Value = serde_json::from_str(&input_json).expect("valid input JSON");

            let output = invoke_with(&capability(EDIT), input, |request| {
                // The request body is still resident while the response arrives, exactly as it is
                // when the host import is what writes the answer in.
                assert!(request.body.len() > FULL_SIZE, "the image reached the wire");
                Ok(Response {
                    status: 200,
                    headers: Vec::new(),
                    body: full_size_response(&response_payload, "low"),
                })
            })
            .expect("a usable response");

            let envelope = serde_json::to_string(&ComponentResponse::Succeeded { output })
                .expect("the envelope serializes")
                .len();
            // `input_json` is deliberately still alive here: so is the SDK's.
            assert!(!input_json.is_empty());
            envelope
        });

        assert!(envelope > FULL_SIZE);
        assert!(
            measured.in_place < 48 * 1024 * 1024,
            "peak was {}, over the 48 MiB budget",
            measured.describe()
        );
        assert!(
            measured.copying < 56 * 1024 * 1024,
            "peak was {}, over the 56 MiB bound",
            measured.describe()
        );
        println!("full edit: {}", measured.describe());
    }
}
