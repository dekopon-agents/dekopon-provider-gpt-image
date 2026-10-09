use base64::Engine as _;
use dekopon_gpt_image_provider::GptImage;
use dekopon_provider_sdk::provider::{Header, Response};
use dekopon_provider_sdk_testkit::{AssetConstraints, Harness};
use serde_json::{Value, json};
use std::os::unix::fs::FileExt;

fn replay(name: &str, input: Value) {
    let fixture: Value = serde_json::from_str(match name {
        "generate" => include_str!("cassettes/gpt-image/generate.json"),
        "edit" => include_str!("cassettes/gpt-image/edit.json"),
        _ => unreachable!(),
    })
    .unwrap();
    assert_eq!(fixture["version"], 1);
    let response = &fixture["response"];
    let run = Harness::<GptImage>::get(
        std::env::var_os("DEKOPON_PROVIDER_COMPONENT")
            .expect("DEKOPON_PROVIDER_COMPONENT must name the freshly built component"),
    )
    .assets(AssetConstraints {
        attach: true,
        ..Default::default()
    })
    .http(dekopon_provider_sdk_testkit::HttpScript::new(
        "localhost",
        "POST",
        Response {
            status: response["status"].as_u64().unwrap().try_into().unwrap(),
            headers: vec![Header::text("content-type", "application/json").unwrap()],
            body: serde_json::to_vec(&response["body"]["json"]).unwrap(),
        },
    ));
    let run = if name == "edit" {
        run.asset(
            1,
            "image/png",
            include_bytes!("fixtures/synthetic-image.png").to_vec(),
        )
        .asset(
            2,
            "image/png",
            include_bytes!("fixtures/synthetic-mask.png").to_vec(),
        )
    } else {
        run
    };
    let origin = run.origin().unwrap().to_owned();
    let output = run
        .settings(json!({"baseUrl": format!("{origin}/proxy/codex/")}))
        .call(&format!("gpt-image.{name}"), input)
        .unwrap();
    assert_eq!(output.status, 0, "{}", output.stderr);
    assert_eq!(output.http_calls.len(), 1);
    let sent = output.http_request.as_ref().expect("captured request");
    let request = &fixture["request"];
    assert_eq!(sent.method, request["method"]);
    let query = request["query"]
        .as_str()
        .map_or_else(String::new, |query| format!("?{query}"));
    assert_eq!(
        sent.uri,
        format!(
            "{origin}/proxy/codex{}{query}",
            request["path"].as_str().unwrap()
        )
    );
    assert_eq!(
        sent.body,
        request["body"]["text"].as_str().unwrap().as_bytes()
    );
    for (name, value) in request["headers"].as_object().unwrap() {
        let header = sent
            .headers
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case(name))
            .unwrap();
        assert_eq!(header.value, value.as_str().unwrap().as_bytes());
    }
    assert!(
        !sent
            .headers
            .iter()
            .any(|header| header.name.eq_ignore_ascii_case("authorization"))
    );
    let length = sent
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("content-length"))
        .unwrap();
    assert_eq!(length.value, sent.body.len().to_string().as_bytes());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["image"]["generationId"], format!("synthetic-{name}"));
    assert_eq!(value["image"]["size"], "1x1");
    assert_eq!(
        value["image"]["bytes"],
        include_bytes!("fixtures/synthetic-output.png").len()
    );
    assert!(value.get("attachments").is_none());
    assert_eq!(output.assets.attached.len(), 1);
    let attached = &output.assets.attached[0];
    let file = output.assets.files[attached.descriptor as usize].file();
    assert_eq!(attached.content_type, "image/png");
    assert_eq!(
        serde_json::to_value(attached).unwrap()["encoding"],
        "base64"
    );
    let encoded = response["body"]["json"]["data"][0]["b64_json"]
        .as_str()
        .unwrap();
    assert_eq!(attached.bytes, encoded.len() as u64);
    assert_eq!(file.metadata().unwrap().len(), encoded.len() as u64);
    let mut bytes = vec![0; encoded.len()];
    file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(bytes, encoded.as_bytes());
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(&bytes)
            .unwrap(),
        include_bytes!("fixtures/synthetic-output.png")
    );
}

#[test]
fn authored_generation_cassette_replays_through_real_component() {
    replay("generate", json!({"prompt":"a synthetic orange square"}));
}

#[test]
fn authored_edit_cassette_streams_image_and_mask_references_and_reads_output() {
    replay(
        "edit",
        json!({"prompt":"a synthetic orange square","images":["chat-asset:1","chat-asset:2"]}),
    );
}
