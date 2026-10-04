use dekopon_gpt_image_provider::GptImage;
use dekopon_provider_sdk_testkit::Harness;
use serde_json::json;
use std::{path::PathBuf, process::Command};

#[test]
fn real_component_conforms_and_has_only_authorized_imports() {
    let component = std::env::var_os("DEKOPON_PROVIDER_COMPONENT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("gpt-image-provider.wasm"));
    assert!(
        component.is_file(),
        "build the real component before running conformance: {}",
        component.display()
    );
    dekopon_provider_sdk_testkit::conformance::<GptImage>(&component).unwrap();
    let wit = Command::new("wasm-tools")
        .args(["component", "wit"])
        .arg(&component)
        .output()
        .expect("wasm-tools is installed");
    assert!(
        wit.status.success(),
        "{}",
        String::from_utf8_lossy(&wit.stderr)
    );
    let json = String::from_utf8(wit.stdout).unwrap();
    for import in [
        "dekopon:stdio/streams@0.1.0",
        "dekopon:http/client@1.1.0",
        "dekopon:asset/asset@0.1.0",
    ] {
        assert!(json.contains(import), "missing {import}");
    }
    assert!(
        !json.contains("dekopon:http/client@1.2.0"),
        "obsolete HTTP import"
    );
    assert!(!json.contains("wasi:"), "unexpected ambient WASI import");
}

#[test]
fn real_component_rejects_bad_piped_prompts_before_any_paid_effect() {
    let component = std::env::var_os("DEKOPON_PROVIDER_COMPONENT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("gpt-image-provider.wasm"));
    for piped in [vec![], vec![0xff], vec![b'a'; 16 * 1024 + 1]] {
        let outcome = Harness::<GptImage>::get(&component)
            .stdin(piped)
            .call(
                "gpt-image.generate",
                json!({"prompt":"-", "pipedPrompt":true}),
            )
            .unwrap();
        assert_ne!(outcome.status, 0);
        assert!(outcome.stdout.is_empty());
        assert!(
            outcome.http_calls.is_empty(),
            "invalid input must never spend quota"
        );
    }
    assert_eq!(Harness::<GptImage>::compiled_identities(), 1);
}
