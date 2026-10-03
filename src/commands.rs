//! Pure image command grammar. Piped text is represented only by a marker until invocation.
use clap::{Args, Parser, Subcommand};
use dekopon_provider_sdk::provider::{Proposal, Usage};

use crate::{Edit, Generate, GptImage, input};

#[derive(Parser)]
#[command(
    name = "image",
    version,
    about = "Generate and edit images with GPT Image",
    long_about = "Generate and edit images with GPT Image. A `refused` result means the safety system blocked the request; that decision is final, so do not retry it or rephrase the prompt and try again."
)]
pub struct Image {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Generate one new image from a prompt
    Generate(GenerateArgs),
    /// Remix one to five images with a prompt
    Edit(EditArgs),
}

#[derive(Args)]
struct GenerateArgs {
    /// What to draw; the service picks quality, size, and format. `-` reads the piped value
    #[arg(long, value_name = "TEXT", required = true)]
    prompt: String,
}

#[derive(Args)]
struct EditArgs {
    /// A reference image: chat-asset:<N>. Repeatable, up to 5; PNG, JPEG or WebP
    #[arg(long = "image", value_name = "REF", required = true)]
    images: Vec<String>,
    /// How to change the images; the service picks quality, size, and format. `-` reads the piped value
    #[arg(long, value_name = "TEXT", required = true)]
    prompt: String,
}

pub(crate) fn propose(args: Image, stdin_piped: bool) -> Result<Proposal<GptImage>, Usage> {
    match args.action {
        Action::Generate(args) => {
            let piped_prompt = args.prompt == "-";
            if piped_prompt && !stdin_piped {
                return Err(Usage::new(
                    "image generate --prompt -: nothing was piped in",
                ));
            }
            if !piped_prompt {
                input::validate_prompt(&args.prompt).map_err(|e| Usage::new(e.to_string()))?;
            }
            Ok(Proposal::to::<Generate>(input::GenerateInput {
                prompt: args.prompt,
                piped_prompt,
            }))
        }
        Action::Edit(args) => {
            input::validate_images(&args.images).map_err(|e| Usage::new(e.to_string()))?;
            let piped_prompt = args.prompt == "-";
            if piped_prompt && !stdin_piped {
                return Err(Usage::new("image edit --prompt -: nothing was piped in"));
            }
            if !piped_prompt {
                input::validate_prompt(&args.prompt).map_err(|e| Usage::new(e.to_string()))?;
            }
            Ok(Proposal::to::<Edit>(input::EditInput {
                prompt: args.prompt,
                images: args.images,
                piped_prompt,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dekopon_provider_sdk::{CommandRunOutcome, provider};
    fn words(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| (*s).into()).collect()
    }
    fn rendered(args: &[&str]) -> (String, String, u8) {
        match provider::command::<GptImage>(&words(args), false) {
            CommandRunOutcome::Rendered {
                stdout,
                stderr,
                status,
            } => (stdout, stderr, status),
            _ => panic!("expected rendered result for {args:?}"),
        }
    }

    #[test]
    fn help_and_version_render_on_stdout_at_zero() {
        for args in [&["--help"][..], &["-h"][..]] {
            let (stdout, stderr, status) = rendered(args);
            assert_eq!(status, 0, "{args:?}");
            assert!(stdout.starts_with("Generate and edit images with GPT Image"));
            assert!(stdout.contains("Usage: image <COMMAND>"));
            assert!(stdout.contains("generate") && stdout.contains("edit"));
            assert!(stderr.is_empty());
        }
        let (stdout, stderr, status) = rendered(&["generate", "--help"]);
        assert_eq!(status, 0);
        assert!(stdout.contains("Usage: image generate --prompt <TEXT>"));
        assert!(stdout.contains("the service picks quality, size, and format"));
        assert!(stderr.is_empty());
        let (stdout, stderr, status) = rendered(&["edit", "--help"]);
        assert_eq!(status, 0);
        assert!(stdout.contains("--image <REF>") && stdout.contains("chat-asset:<N>"));
        assert!(stderr.is_empty());
        let (stdout, stderr, status) = rendered(&["--version"]);
        assert_eq!(status, 0);
        assert_eq!(stdout, format!("image {}\n", env!("CARGO_PKG_VERSION")));
        assert!(stderr.is_empty());
    }

    #[test]
    fn refusal_guidance_is_only_in_long_help() {
        let (stdout, _, status) = rendered(&["--help"]);
        assert_eq!(status, 0);
        assert!(stdout.contains("`refused` result") && stdout.contains("is final"));
        let (short, _, status) = rendered(&["-h"]);
        assert_eq!(status, 0);
        assert!(!short.contains("refused"));
    }

    #[test]
    fn usage_renders_on_stderr_with_exit_two() {
        for args in [
            &[][..],
            &["bogus"][..],
            &["generate"][..],
            &["generate", "--prompt"][..],
            &["generate", "a tangerine"][..],
            &["edit", "--prompt", "remix"][..],
            &["edit", "--image", "chat-asset:1"][..],
        ] {
            let (stdout, stderr, status) = rendered(args);
            assert_eq!(status, 2, "{args:?}");
            assert!(stdout.is_empty(), "{args:?}: {stdout}");
            assert!(!stderr.is_empty(), "{args:?}");
        }
        for args in [
            &["bogus"][..],
            &["generate"][..],
            &["generate", "--prompt"][..],
        ] {
            assert!(rendered(args).1.starts_with("error: "), "{args:?}");
        }
        for (args, expected) in [
            (&[][..], "Usage: image <COMMAND>"),
            (&["bogus"][..], "Usage: image <COMMAND>"),
            (&["generate"][..], "Usage: image generate --prompt <TEXT>"),
            (
                &["edit", "--prompt", "remix"][..],
                "Usage: image edit --image <REF>",
            ),
        ] {
            assert!(rendered(args).1.contains(expected), "{args:?}");
        }
    }
    #[test]
    fn proposal_keeps_pipe_marker_not_contents() {
        let argv = words(&["generate", "--prompt", "-"]);
        let CommandRunOutcome::Proposed {
            capability, input, ..
        } = provider::command::<GptImage>(&argv, true)
        else {
            panic!("proposal")
        };
        assert_eq!(capability.as_str(), "gpt-image.generate");
        assert_eq!(input, serde_json::json!({"prompt":"-", "pipedPrompt":true}));
        assert!(matches!(
            provider::command::<GptImage>(&argv, false),
            CommandRunOutcome::Failed { .. }
        ));
        let CommandRunOutcome::Proposed {
            capability, input, ..
        } = provider::command::<GptImage>(
            &words(&["edit", "--image", "chat-asset:2", "--prompt", "-"]),
            true,
        )
        else {
            panic!("edit")
        };
        assert_eq!(capability.as_str(), "gpt-image.edit");
        assert_eq!(input["images"], serde_json::json!(["chat-asset:2"]));
    }
    #[test]
    fn invalid_references_never_enter_proposals() {
        let args = words(&[
            "edit",
            "--image",
            "data:image/png;base64,AA",
            "--prompt",
            "x",
        ]);
        assert!(matches!(
            provider::command::<GptImage>(&args, false),
            CommandRunOutcome::Failed { .. }
        ));
    }

    #[test]
    fn every_ignored_flag_is_refused_by_name() {
        for flag in [
            "--quality",
            "--size",
            "--background",
            "--model",
            "--output-format",
        ] {
            let (stdout, stderr, status) = rendered(&["generate", "--prompt", "x", flag, "high"]);
            assert_eq!(status, 2, "{flag}");
            assert!(stdout.is_empty());
            assert!(
                stderr.contains(flag) && stderr.contains("unexpected argument"),
                "{flag}: {stderr}"
            );
        }
    }

    #[test]
    fn rendered_text_has_no_escape_bytes() {
        for args in [
            &["--help"][..],
            &["-h"][..],
            &["--version"][..],
            &["generate", "--help"][..],
            &["edit", "--help"][..],
            &["bogus"][..],
            &["generate"][..],
        ] {
            let (stdout, stderr, _) = rendered(args);
            assert!(
                !stdout.contains('\u{1b}') && !stderr.contains('\u{1b}'),
                "{args:?}"
            );
        }
    }
}
