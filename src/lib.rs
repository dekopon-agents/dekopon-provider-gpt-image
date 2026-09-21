//! GPT Image generation and editing through broker-owned asset handles.
//!
//! One invocation is one fixed POST, with no retry and no guest-visible credential.
//! Inputs are streamed by the host; the response is borrowed and attached out of band.

use dekopon_provider_http::{Header, Response, StreamedRequest};
use dekopon_provider_sdk::{
    CapabilityId, CommandRun, Provider, ProviderError, ProviderManifest, asset,
};
use serde_json::Value;

mod b64;
mod commands;
mod error;
mod input;
mod manifest;
#[cfg(test)]
mod probe;
mod response;
mod wire;

pub(crate) const GENERATE: &str = "gpt-image.generate";
pub(crate) const EDIT: &str = "gpt-image.edit";
pub(crate) const COMMAND_WORD: &str = "image";

mod bindings {
    wit_bindgen::generate!({
        path: "wit",
        world: "provider",
        generate_all,
        pub_export_macro: true,
    });
}

struct GptImage;
impl Provider for GptImage {
    fn manifest() -> ProviderManifest {
        manifest::manifest()
    }
    fn invoke(capability: &CapabilityId, input: Value) -> Result<Value, ProviderError> {
        invoke_with(capability, input, &mut Host)
    }
    fn run_command(argv: &[String], stdin: Option<&str>) -> Result<CommandRun, ProviderError> {
        commands::run(argv, stdin)
    }
}

/// Native test seam for precisely the asset and HTTP operations used by production.
trait Transport {
    type Handle;
    type Writer;
    fn open(&mut self, reference: &str) -> Result<Self::Handle, ProviderError>;
    fn content_type(&self, handle: &Self::Handle) -> String;
    fn stream(
        &mut self,
        request: wire::Request<'_, Self::Handle>,
    ) -> Result<(u16, Vec<Header>, Self::Handle), ProviderError>;
    fn read_all(&mut self, handle: &Self::Handle) -> Result<Vec<u8>, ProviderError>;
    fn allocate(
        &mut self,
        content_type: &str,
        encoding: asset::Encoding,
    ) -> Result<Self::Writer, ProviderError>;
    fn write_all(&mut self, writer: &Self::Writer, bytes: &[u8]) -> Result<(), ProviderError>;
    fn attach(&mut self, writer: Self::Writer) -> Result<(), ProviderError>;
}

struct Host;
fn asset_error(error: asset::AssetError) -> ProviderError {
    // Only the stable code is echoed, not a host path or an upstream payload. A failure after
    // POST must never suggest silently repeating a paid generation.
    ProviderError::new(
        "asset-failure",
        format!(
            "image asset operation failed ({}); no retry was attempted",
            error.code
        ),
    )
}
impl Transport for Host {
    type Handle = asset::Handle;
    type Writer = asset::Writer;
    fn open(&mut self, reference: &str) -> Result<Self::Handle, ProviderError> {
        asset::open(reference).map_err(asset_error)
    }
    fn content_type(&self, handle: &Self::Handle) -> String {
        handle.info().content_type
    }
    fn stream(
        &mut self,
        request: wire::Request<'_, Self::Handle>,
    ) -> Result<(u16, Vec<Header>, Self::Handle), ProviderError> {
        let response = dekopon_provider_http::stream(StreamedRequest {
            method: request.method,
            uri: request.uri,
            headers: request.headers,
            body: request
                .body
                .into_iter()
                .map(|part| match part {
                    wire::Part::Literal(bytes) => dekopon_provider_http::Part::literal(bytes),
                    wire::Part::Asset { handle, encoding } => {
                        dekopon_provider_http::Part::asset(handle, encoding)
                    }
                })
                .collect(),
        })
        .map_err(|failure| error::transport(&failure))?;
        Ok((response.status, response.headers, response.body))
    }
    fn read_all(&mut self, handle: &Self::Handle) -> Result<Vec<u8>, ProviderError> {
        handle.read_all().map_err(asset_error)
    }
    fn allocate(
        &mut self,
        content_type: &str,
        encoding: asset::Encoding,
    ) -> Result<Self::Writer, ProviderError> {
        asset::allocate(content_type, encoding).map_err(asset_error)
    }
    fn write_all(&mut self, writer: &Self::Writer, bytes: &[u8]) -> Result<(), ProviderError> {
        writer.write_all(bytes).map_err(asset_error)
    }
    fn attach(&mut self, writer: Self::Writer) -> Result<(), ProviderError> {
        asset::attach(writer).map(|_| ()).map_err(asset_error)
    }
}

fn invoke_with<T: Transport>(
    capability: &CapabilityId,
    input: Value,
    transport: &mut T,
) -> Result<Value, ProviderError> {
    let operation = match capability.as_str() {
        GENERATE => input::Operation::Generate,
        EDIT => input::Operation::Edit,
        _ => return Err(error::unknown_capability()),
    };
    let input = input::parse(operation, input)?;
    let mut images = Vec::with_capacity(input.images.len());
    for reference in &input.images {
        let handle = transport.open(reference)?;
        let content_type = transport.content_type(&handle);
        images.push((handle, content_type));
    }
    let request = wire::http_request(operation, &input, &images)?;
    let (status, headers, handle) = transport.stream(request)?;
    drop(images);
    let body = transport.read_all(&handle)?;
    response::project(
        Response {
            status,
            headers,
            body,
        },
        |payload| {
            let writer = transport.allocate("image/png", asset::Encoding::Base64)?;
            transport.write_all(&writer, payload)?;
            transport.attach(writer)
        },
    )
}

dekopon_provider_sdk::export_provider_with_cli!(GptImage, bindings);

#[cfg(test)]
fn capability(value: &str) -> CapabilityId {
    value.parse().expect("valid fixture")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::b64::tests::{encode, png_payload};
    use serde_json::json;

    const RESPONSE: &str = include_str!("../tests/fixtures/generate-response.json");
    struct Fake {
        events: Vec<&'static str>,
        opened: Vec<String>,
        body: Vec<u8>,
        content_length: usize,
        status: u16,
        response: Option<Vec<u8>>,
        mime: String,
        fail: &'static str,
        written: usize,
    }
    impl Default for Fake {
        fn default() -> Self {
            Self {
                events: vec![],
                opened: vec![],
                body: vec![],
                content_length: 0,
                status: 200,
                response: Some(RESPONSE.as_bytes().to_vec()),
                mime: "image/png".into(),
                fail: "",
                written: 0,
            }
        }
    }
    impl Fake {
        fn step(&mut self, event: &'static str) -> Result<(), ProviderError> {
            self.events.push(event);
            if self.fail == event {
                return Err(error::failure("injected failure"));
            }
            Ok(())
        }
    }
    impl Transport for Fake {
        type Handle = usize;
        type Writer = ();
        fn open(&mut self, reference: &str) -> Result<usize, ProviderError> {
            self.step("open")?;
            self.opened.push(reference.into());
            Ok(reference
                .strip_prefix("chat-asset:")
                .unwrap()
                .parse()
                .unwrap())
        }
        fn content_type(&self, _: &usize) -> String {
            self.mime.clone()
        }
        fn stream(
            &mut self,
            request: wire::Request<'_, usize>,
        ) -> Result<(u16, Vec<Header>, usize), ProviderError> {
            self.step("post")?;
            assert_eq!(self.events.iter().filter(|e| **e == "post").count(), 1);
            assert_eq!(request.method, "POST");
            assert_eq!(
                request.uri,
                if self.opened.is_empty() {
                    wire::GENERATE_URL
                } else {
                    wire::EDIT_URL
                }
            );
            assert_eq!(
                request.headers,
                vec![
                    Header::text("content-type", "application/json").unwrap(),
                    Header::text("accept", "application/json").unwrap(),
                    Header::text("originator", "dekopon").unwrap()
                ]
            );
            // Emulate the host's fixed-length composition, encoding only outside the guest.
            // Each fake asset is three distinct bytes; the response handle is deliberately separate.
            for part in request.body {
                let bytes = match part {
                    wire::Part::Literal(bytes) => bytes,
                    wire::Part::Asset { handle, encoding } => {
                        assert!(matches!(encoding, asset::Encoding::Base64));
                        encode(&[*handle as u8; 3]).into_bytes()
                    }
                };
                self.content_length += bytes.len();
                self.body.extend_from_slice(&bytes);
            }
            Ok((self.status, vec![], 999))
        }
        fn read_all(&mut self, handle: &usize) -> Result<Vec<u8>, ProviderError> {
            self.step("read-response")?;
            assert_eq!(
                *handle, 999,
                "input bytes must never be read into the guest"
            );
            Ok(self.response.take().unwrap())
        }
        fn allocate(&mut self, mime: &str, encoding: asset::Encoding) -> Result<(), ProviderError> {
            self.step("allocate")?;
            assert_eq!(mime, "image/png");
            assert!(matches!(encoding, asset::Encoding::Base64));
            Ok(())
        }
        fn write_all(&mut self, _: &(), bytes: &[u8]) -> Result<(), ProviderError> {
            self.step("write")?;
            assert!(crate::b64::starts_with_png_signature(
                str::from_utf8(bytes).unwrap()
            ));
            self.written = bytes.len();
            Ok(())
        }
        fn attach(&mut self, _: ()) -> Result<(), ProviderError> {
            self.step("attach")
        }
    }

    #[test]
    fn mirrors_and_manifest_snapshot() {
        assert_eq!(
            include_str!("../wit/deps/provider.wit"),
            dekopon_provider_sdk::PROVIDER_WIT
        );
        assert_eq!(
            include_str!("../wit/deps/http.wit"),
            dekopon_provider_http::HTTP_WIT
        );
        assert_eq!(
            include_str!("../wit/deps/asset.wit"),
            dekopon_provider_sdk::ASSET_WIT
        );
        assert_eq!(
            format!(
                "{}\n",
                serde_json::to_string_pretty(&GptImage::manifest()).unwrap()
            ),
            include_str!("../tests/fixtures/manifest.json")
        );
    }

    #[test]
    fn golden_one_and_five_inputs_have_exact_body_and_fixed_length() {
        for (count, golden, length) in [
            (
                1,
                include_str!("../tests/fixtures/edit-request-one.json"),
                158,
            ),
            (
                5,
                include_str!("../tests/fixtures/edit-request-five.json"),
                330,
            ),
        ] {
            let mut fake = Fake::default();
            let images: Vec<_> = (1..=count).map(|id| format!("chat-asset:{id}")).collect();
            let output = invoke_with(
                &capability(EDIT),
                json!({"prompt":"repaint \"orange\"\n","images":images}),
                &mut fake,
            )
            .unwrap();
            assert_eq!(fake.body, golden.trim_end().as_bytes());
            assert_eq!(fake.content_length, length);
            assert_eq!(fake.body.len(), length);
            assert_eq!(fake.opened, images);
            assert_eq!(
                &fake.events[count..],
                ["post", "read-response", "allocate", "write", "attach"]
            );
            assert_eq!(output["image"]["bytes"], 69);
            assert!(output.get("attachments").is_none());
        }
    }

    #[test]
    fn generate_keeps_its_fixed_body_and_attaches_once() {
        let mut fake = Fake::default();
        invoke_with(
            &capability(GENERATE),
            json!({"prompt":"a tangerine"}),
            &mut fake,
        )
        .unwrap();
        assert_eq!(
            str::from_utf8(&fake.body).unwrap(),
            r#"{"prompt":"a tangerine","model":"gpt-image-2","background":"auto","quality":"auto","size":"auto"}"#
        );
        assert_eq!(
            fake.events,
            ["post", "read-response", "allocate", "write", "attach"]
        );
    }

    #[test]
    fn invalid_inputs_and_unsupported_mime_cost_no_post() {
        for (id, input) in [
            ("gpt-image.upscale", json!({"prompt":"x"})),
            (EDIT, json!({"prompt":"x"})),
            (
                EDIT,
                json!({"prompt":"x","images":["data:image/png;base64,AAAA"]}),
            ),
        ] {
            let mut fake = Fake::default();
            assert!(invoke_with(&capability(id), input, &mut fake).is_err());
            assert!(fake.events.is_empty());
        }
        for mime in ["image/gif", "image/png\"},\"injected\":true", "text/plain"] {
            let mut fake = Fake {
                mime: mime.into(),
                ..Fake::default()
            };
            assert!(
                invoke_with(
                    &capability(EDIT),
                    json!({"prompt":"x","images":["chat-asset:1"]}),
                    &mut fake
                )
                .is_err()
            );
            assert_eq!(fake.events, ["open"]);
        }
    }

    #[test]
    fn every_host_failure_stops_without_retry_or_later_effects() {
        let sequence = [
            "open",
            "post",
            "read-response",
            "allocate",
            "write",
            "attach",
        ];
        for (index, fail) in sequence.iter().enumerate() {
            let mut fake = Fake {
                fail,
                ..Fake::default()
            };
            assert!(
                invoke_with(
                    &capability(EDIT),
                    json!({"prompt":"x","images":["chat-asset:1"]}),
                    &mut fake
                )
                .is_err()
            );
            assert_eq!(fake.events, sequence[..=index]);
        }
        for (status, body) in [(429, RESPONSE), (500, RESPONSE), (200, "not json")] {
            let mut fake = Fake {
                status,
                response: Some(body.as_bytes().to_vec()),
                ..Fake::default()
            };
            assert!(invoke_with(&capability(GENERATE), json!({"prompt":"x"}), &mut fake).is_err());
            assert_eq!(fake.events, ["post", "read-response"]);
        }
    }

    #[test]
    fn full_size_response_and_five_references_keep_json_small_and_one_image_allocation() {
        let payload = png_payload(11_184_808);
        let body = format!(r#"{{"data":[{{"b64_json":"{payload}"}}]}}"#);
        let (output, measured) = probe::measure(|| {
            let mut fake = Fake {
                response: Some(body.as_bytes().to_vec()),
                ..Fake::default()
            };
            let output = invoke_with(&capability(EDIT), json!({"prompt":"x","images":["chat-asset:1","chat-asset:2","chat-asset:3","chat-asset:4","chat-asset:5"]}), &mut fake).unwrap();
            assert_eq!(fake.written, payload.len());
            serde_json::to_string(&dekopon_provider_sdk::ComponentResponse::Succeeded { output })
                .unwrap()
        });
        assert!(output.len() < 1024);
        assert!(
            measured.copying < 12 * 1024 * 1024,
            "{}",
            measured.describe()
        );
        println!("asset edit: {}", measured.describe());
    }
}
