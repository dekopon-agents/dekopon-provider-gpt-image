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
    #[test]
    fn help_and_refusal_guidance_remain_local() {
        let CommandRunOutcome::Rendered { stdout, status, .. } =
            provider::command::<GptImage>(&words(&["--help"]), false)
        else {
            panic!("help")
        };
        assert_eq!(status, 0);
        assert!(stdout.contains("`refused` result") && stdout.contains("is final"));
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
    fn invalid_references_and_ignored_flags_propose_nothing() {
        for args in [
            words(&[
                "edit",
                "--image",
                "data:image/png;base64,AA",
                "--prompt",
                "x",
            ]),
            words(&["generate", "--prompt", "x", "--quality", "high"]),
        ] {
            assert!(!matches!(
                provider::command::<GptImage>(&args, false),
                CommandRunOutcome::Proposed { .. }
            ));
        }
    }
}
