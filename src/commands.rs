//! The `image` command word: a small command-line program, rendered by the guest.
//!
//! `image --help`, `image generate --help`, `image --version`, and every usage error are answered
//! here and authorize nothing — the SDK's clap layer renders them as text with an exit status, the
//! way the upstream tool's `main` would. A well-formed argv becomes a *proposal*, which then travels
//! the identical authorization path a direct `cap gpt-image.generate {…}` call takes: constraint-set
//! lookup, Cedar, then credential injection inside the broker's HTTP engine. Naming a capability the
//! caller was not granted is a denial, not an escalation.
//!
//! There are no `--quality`, `--size`, `--background`, or `--model` flags, because the route has no
//! such controls: it accepts those fields and ignores them. A model that types one gets clap's usage
//! error naming the flag, which is more useful than silently accepting a setting that does nothing.

use dekopon_provider_sdk::clap::{self, Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use dekopon_provider_sdk::{CommandInvocation, CommandRun, ProviderError, cli};
use serde_json::json;

use crate::{EDIT, GENERATE};

/// The value that means "read the text piped into the word".
const PIPED: &str = "-";

// The `image` tree, declared once and rendered by clap. Plain comments, not doc comments: clap
// renders a doc comment as the `about` line above `Usage:`.
#[derive(Parser)]
#[command(
    name = "image",
    version,
    about = "Generate and edit images with GPT Image"
)]
struct Image {
    #[command(subcommand)]
    action: Action,
}

// Each subcommand proposes exactly one capability, named by the `const` the manifest declares, so a
// renamed capability is a compile error rather than an exit code discovered mid-session.
#[derive(Subcommand)]
enum Action {
    /// Generate one new image from a prompt
    Generate(Generate),
    /// Remix one to three images with a prompt
    Edit(Edit),
}

#[derive(Args)]
struct Generate {
    /// What to draw; the service picks quality, size, and format. `-` reads the piped value
    #[arg(long, value_name = "TEXT", required = true)]
    prompt: String,
}

#[derive(Args)]
struct Edit {
    /// A reference image: chat-asset:<N>, or a data:image/...;base64,... URL. Repeatable, up to 3
    #[arg(long = "image", value_name = "REF", required = true)]
    images: Vec<String>,
    /// How to change the images; the service picks quality, size, and format. `-` reads the piped value
    #[arg(long, value_name = "TEXT", required = true)]
    prompt: String,
}

/// Runs one `image` argv.
pub(crate) fn run(argv: &[String], stdin: Option<&str>) -> Result<CommandRun, ProviderError> {
    cli::run_command(Image::command(), argv, stdin, dispatch)
}

/// Turns clap's matches into the proposal for the selected subcommand.
///
/// Runs only after clap accepted the argv, so what is left to decide is what clap cannot know:
/// whether anything was piped. Every semantic bound — the prompt length, the image count, the data
/// URL shape — is checked once, in `invoke`, against the input a direct call would send too.
fn dispatch(
    matches: clap::ArgMatches,
    stdin: Option<&str>,
) -> Result<CommandInvocation, ProviderError> {
    let image = Image::from_arg_matches(&matches)
        .map_err(|error| ProviderError::new("usage", error.to_string()))?;
    match image.action {
        Action::Generate(generate) => Ok(CommandInvocation {
            capability: GENERATE.parse().expect("static capability ID"),
            input: json!({"prompt": prompt(generate.prompt, stdin, "generate")?}),
        }),
        Action::Edit(edit) => Ok(CommandInvocation {
            capability: EDIT.parse().expect("static capability ID"),
            input: json!({
                "prompt": prompt(edit.prompt, stdin, "edit")?,
                "images": edit.images,
            }),
        }),
    }
}

/// The prompt, or the piped value when the caller wrote `-`.
fn prompt(prompt: String, stdin: Option<&str>, word: &str) -> Result<String, ProviderError> {
    if prompt != PIPED {
        return Ok(prompt);
    }
    stdin.map(str::to_owned).ok_or_else(|| {
        ProviderError::new(
            "usage",
            format!("image {word} --prompt -: nothing was piped in"),
        )
    })
}

#[cfg(test)]
mod tests {
    use dekopon_provider_sdk::{CommandInvocation, CommandRun, Provider};
    use serde_json::json;

    use super::run;
    use crate::{EDIT, GENERATE, GptImage};

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    fn rendered(words: &[&str], stdin: Option<&str>) -> (String, String, u8) {
        let run = run(&argv(words), stdin).expect("clap answers are rendered, not declined");
        let CommandRun::Rendered {
            stdout,
            stderr,
            status,
        } = run
        else {
            panic!("expected rendered text for {words:?}, got {run:?}");
        };
        (stdout, stderr, status)
    }

    fn proposal(words: &[&str], stdin: Option<&str>) -> CommandInvocation {
        match run(&argv(words), stdin).expect("a well-formed argv proposes") {
            CommandRun::Proposal(invocation) => invocation,
            other => panic!("expected a proposal for {words:?}, got {other:?}"),
        }
    }

    #[test]
    fn help_and_version_render_on_stdout_at_status_zero() {
        for words in [&["--help"][..], &["-h"][..]] {
            let (stdout, stderr, status) = rendered(words, None);
            assert_eq!(status, 0, "{words:?}");
            assert!(
                stdout.starts_with("Generate and edit images with GPT Image"),
                "{stdout}"
            );
            assert!(stdout.contains("Usage: image <COMMAND>"), "{stdout}");
            assert!(stdout.contains("generate"), "{stdout}");
            assert!(stdout.contains("edit"), "{stdout}");
            assert!(stderr.is_empty(), "{stderr}");
        }

        let (stdout, _, status) = rendered(&["generate", "--help"], None);
        assert_eq!(status, 0);
        assert!(
            stdout.contains("Usage: image generate --prompt <TEXT>"),
            "{stdout}"
        );
        // The help page is where a model learns that asking for a size is pointless.
        assert!(
            stdout.contains("the service picks quality, size, and format"),
            "{stdout}"
        );

        let (stdout, _, status) = rendered(&["edit", "--help"], None);
        assert_eq!(status, 0);
        assert!(stdout.contains("--image <REF>"), "{stdout}");
        assert!(stdout.contains("chat-asset:<N>"), "{stdout}");

        let (stdout, _, status) = rendered(&["--version"], None);
        assert_eq!(status, 0);
        assert_eq!(stdout, format!("image {}\n", env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn usage_errors_render_on_stderr_at_status_two() {
        for words in [
            // A bare `image` is a usage error whose text is the help page, as the upstream tool's
            // would be: nothing was asked, so nothing is proposed.
            &[][..],
            &["bogus"][..],
            &["generate"][..],
            &["generate", "--prompt"][..],
            &["generate", "a tangerine"][..],
            &["edit", "--prompt", "remix"][..],
            &["edit", "--image", "chat-asset:1"][..],
        ] {
            let (stdout, stderr, status) = rendered(words, None);
            assert_eq!(status, 2, "{words:?}");
            assert!(stdout.is_empty(), "{words:?}: {stdout}");
            assert!(!stderr.is_empty(), "{words:?}");
        }

        // Where clap has a usage line to print, the line starts with the word the model typed
        // rather than with the subcommand alone.
        for (words, usage) in [
            (&[][..], "Usage: image <COMMAND>"),
            (&["bogus"][..], "Usage: image <COMMAND>"),
            (&["generate"][..], "Usage: image generate --prompt <TEXT>"),
            (
                &["edit", "--prompt", "remix"][..],
                "Usage: image edit --image <REF>",
            ),
        ] {
            let (_, stderr, _) = rendered(words, None);
            assert!(stderr.contains(usage), "{words:?}: {stderr}");
        }

        for words in [
            &["bogus"][..],
            &["generate"][..],
            &["generate", "--prompt"][..],
        ] {
            let (_, stderr, _) = rendered(words, None);
            assert!(stderr.starts_with("error: "), "{words:?}: {stderr}");
        }
    }

    /// The flags the route ignores do not exist, and clap says so by name rather than accepting a
    /// setting that would do nothing.
    #[test]
    fn the_flags_the_route_ignores_are_refused_by_name() {
        for flag in [
            "--quality",
            "--size",
            "--background",
            "--model",
            "--output-format",
        ] {
            let (_, stderr, status) =
                rendered(&["generate", "--prompt", "a tangerine", flag, "high"], None);
            assert_eq!(status, 2, "{flag}");
            assert!(
                stderr.contains(flag) && stderr.contains("unexpected argument"),
                "{flag}: {stderr}"
            );
        }
    }

    #[test]
    fn a_well_formed_argv_proposes_a_camel_case_input() {
        let invocation = proposal(&["generate", "--prompt", "a tangerine on a desk"], None);
        assert_eq!(invocation.capability.as_str(), GENERATE);
        assert_eq!(invocation.input, json!({"prompt": "a tangerine on a desk"}));

        let invocation = proposal(
            &[
                "edit",
                "--image",
                "chat-asset:2",
                "--image",
                "data:image/png;base64,iVBORw0KGgo=",
                "--prompt",
                "repaint as a watercolour",
            ],
            None,
        );
        assert_eq!(invocation.capability.as_str(), EDIT);
        assert_eq!(
            invocation.input,
            json!({
                "prompt": "repaint as a watercolour",
                "images": ["chat-asset:2", "data:image/png;base64,iVBORw0KGgo="]
            })
        );
    }

    #[test]
    fn a_dash_prompt_reads_the_piped_value_and_declines_when_nothing_was_piped() {
        let piped = "a tangerine, rendered as a woodcut\n";
        let invocation = proposal(&["generate", "--prompt", "-"], Some(piped));
        assert_eq!(invocation.input, json!({"prompt": piped}));

        let invocation = proposal(
            &["edit", "--image", "chat-asset:1", "--prompt", "-"],
            Some(piped),
        );
        assert_eq!(invocation.input["prompt"], piped);

        let error = run(&argv(&["generate", "--prompt", "-"]), None)
            .expect_err("a decline, reported to the model as a usage error");
        assert_eq!(error.code(), "usage");
        assert_eq!(
            error.message(),
            "image generate --prompt -: nothing was piped in"
        );
    }

    /// The rendered text is plain: the SDK's clap is built without `color`, so no escape byte can
    /// reach a model's transcript.
    #[test]
    fn no_rendered_text_contains_an_escape_byte() {
        for words in [
            &["--help"][..],
            &["--version"][..],
            &["generate", "--help"][..],
            &["edit", "--help"][..],
            &["bogus"][..],
            &["generate"][..],
        ] {
            let (stdout, stderr, _) = rendered(words, None);
            assert!(!stdout.contains('\u{1b}'), "{words:?}: {stdout:?}");
            assert!(!stderr.contains('\u{1b}'), "{words:?}: {stderr:?}");
        }
    }

    /// Every capability the facade can propose is one the manifest declares. Without this, a
    /// renamed capability would be discovered by a model at runtime as an authorization denial.
    #[test]
    fn every_dispatch_target_is_declared_in_the_manifest() {
        let declared: Vec<String> = GptImage::manifest()
            .capabilities
            .iter()
            .map(|capability| capability.id.to_string())
            .collect();
        for words in [
            &["generate", "--prompt", "a tangerine"][..],
            &["edit", "--image", "chat-asset:1", "--prompt", "remix"][..],
        ] {
            let invocation = proposal(words, None);
            assert!(
                declared.contains(&invocation.capability.to_string()),
                "{words:?} proposes {} which the manifest does not declare",
                invocation.capability
            );
        }
    }
}
