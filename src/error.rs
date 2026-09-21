//! The provider's closed error taxonomy.
//!
//! HTTP/response codes and one bounded quotation from upstream: a
//! refused call carries the upstream error envelope's own `code` (or `type`) and `message`, control
//! characters stripped and cut to 240 characters. A 400 whose reason is invisible is a call nobody
//! can fix, and the response body of an authorized endpoint is not a secret.
//!
//! Nothing else upstream ever crosses into a message: not a request header, not the injected
//! credential, not the request body, not a data URL, not a host's own transport text. What the
//! operator needs beyond the quotation is in the broker's audit log and the provider span, not in
//! the text a prompt can read back.

use dekopon_provider_http::{HttpError, HttpErrorCode};
use dekopon_provider_sdk::ProviderError;

/// The caller's metadata or asset reference is invalid.
pub(crate) const INVALID_INPUT: &str = "invalid-input";
/// The injected credential was refused upstream.
pub(crate) const UPSTREAM_UNAUTHORIZED: &str = "upstream-unauthorized";
/// The subscription's image quota is exhausted.
pub(crate) const UPSTREAM_QUOTA: &str = "upstream-quota";
/// Upstream refused the request for a reason that is not quota or credential.
pub(crate) const UPSTREAM_REJECTED: &str = "upstream-rejected";
/// Upstream failed, or never answered.
pub(crate) const UPSTREAM_FAILURE: &str = "upstream-failure";
/// Upstream answered with something that is not a usable image response.
pub(crate) const RESPONSE_INVALID: &str = "response-invalid";
/// A response exceeding the decoded asset size or HTTP response allowance.
pub(crate) const RESPONSE_TOO_LARGE: &str = "response-too-large";

/// Longest quotation of an upstream error envelope carried in a message, in characters.
///
/// The refusals actually observed on this route are 2,618–2,859 byte bodies whose `message` is one
/// or two sentences, so 240 characters quotes the reason in full and truncates only prose that was
/// never going to be read.
const MAX_UPSTREAM_DETAIL_CHARACTERS: usize = 240;

/// The input does not satisfy the capability contract. `detail` is written here, never echoed.
pub(crate) fn invalid_input(detail: impl Into<String>) -> ProviderError {
    ProviderError::new(INVALID_INPUT, detail)
}

/// 401 or 403: the broker-injected credential is no longer good for this route.
pub(crate) fn unauthorized() -> ProviderError {
    ProviderError::new(
        UPSTREAM_UNAUTHORIZED,
        "credential rejected; the operator must re-login the gpt-image credential",
    )
}

/// 429: the account's image allowance is spent. `detail` carries only validated tokens.
pub(crate) fn quota(detail: String) -> ProviderError {
    ProviderError::new(UPSTREAM_QUOTA, detail)
}

/// Any other 4xx. The status and `detail` are the only upstream values in the message.
pub(crate) fn rejected(status: u16, detail: Option<String>) -> ProviderError {
    ProviderError::new(
        UPSTREAM_REJECTED,
        match detail {
            Some(detail) => format!(
                "the image route refused the request with HTTP {status} ({detail}); revise the \
                 request"
            ),
            None => {
                format!(
                    "the image route refused the request with HTTP {status}; revise the request"
                )
            }
        },
    )
}

/// A 5xx, or any other status that is neither success nor a classified refusal.
pub(crate) fn failure_status(status: u16, detail: Option<String>) -> ProviderError {
    ProviderError::new(
        UPSTREAM_FAILURE,
        match detail {
            Some(detail) => {
                format!("the image route answered HTTP {status} ({detail}); this may be transient")
            }
            None => format!("the image route answered HTTP {status}; this may be transient"),
        },
    )
}

/// A 5xx, a transport failure, or a broker refusal. `detail` is one of this module's sentences.
pub(crate) fn failure(detail: &'static str) -> ProviderError {
    ProviderError::new(UPSTREAM_FAILURE, detail)
}

/// The response parsed but is not a usable single PNG image. `detail` names the failed check.
pub(crate) fn response_invalid(detail: &'static str) -> ProviderError {
    ProviderError::new(
        RESPONSE_INVALID,
        format!("the image route returned an unusable response: {detail}"),
    )
}

/// The decoded PNG would exceed the asset ceiling.
pub(crate) fn response_too_large(bytes: usize, ceiling: usize) -> ProviderError {
    ProviderError::new(
        RESPONSE_TOO_LARGE,
        format!(
            "the decoded image is {bytes} bytes and the asset limit is {ceiling}; ask for a smaller size"
        ),
    )
}

/// Maps a broker HTTP failure onto the taxonomy without leaking the host's message.
///
/// `request-too-large` is the caller's fault — it is the size of the images they passed — so it is
/// the one transport class that comes back as `invalid-input`. Everything else is upstream: a
/// denial is an operator configuration fact, not something a model can fix by retrying, and the
/// component retries nothing either way.
pub(crate) fn transport(error: &HttpError) -> ProviderError {
    match error.code {
        HttpErrorCode::RequestTooLarge => invalid_input(
            "the assembled request exceeded the authorized request size; pass fewer or smaller \
             images",
        ),
        HttpErrorCode::ResponseTooLarge => ProviderError::new(
            RESPONSE_TOO_LARGE,
            "the image response exceeded the authorized response size; ask for a smaller size",
        ),
        HttpErrorCode::Timeout => failure(
            "the image request timed out before the route answered; generation can take minutes, \
             so retry once deliberately",
        ),
        HttpErrorCode::Denied | HttpErrorCode::HostCallLimit => failure(
            "the broker denied the image request; the capability is not authorized to call it",
        ),
        HttpErrorCode::Dns
        | HttpErrorCode::Connect
        | HttpErrorCode::Tls
        | HttpErrorCode::Protocol
        | HttpErrorCode::InvalidMethod
        | HttpErrorCode::InvalidUri
        | HttpErrorCode::InvalidHeader
        | HttpErrorCode::Internal => failure("the image route could not be reached"),
    }
}

/// A capability this component does not implement reached `invoke`.
pub(crate) fn unknown_capability() -> ProviderError {
    invalid_input(
        "unknown capability; this provider implements gpt-image.generate and gpt-image.edit",
    )
}

/// The upstream error envelope's own `code` (or `type`) and `message`, bounded for a message.
///
/// This is the one place upstream text is allowed through, so the bound is here rather than at the
/// call site: every control character becomes a space, runs of whitespace collapse, and the result
/// is cut to [`MAX_UPSTREAM_DETAIL_CHARACTERS`] with an ellipsis. A model reads this string, so it
/// is quoted rather than interpreted — the taxonomy code above it is what anything should branch
/// on.
pub(crate) fn upstream_detail(code: Option<&str>, message: Option<&str>) -> Option<String> {
    let code = code.map(sanitize).filter(|value| !value.is_empty());
    let message = message.map(sanitize).filter(|value| !value.is_empty());
    let detail = match (code, message) {
        (Some(code), Some(message)) => format!("{code}: {message}"),
        (Some(only), None) | (None, Some(only)) => only,
        (None, None) => return None,
    };
    Some(bounded(detail))
}

/// Upstream text with every control character turned into a space and whitespace runs collapsed.
fn sanitize(value: &str) -> String {
    let mut sanitized = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_control() || character.is_whitespace() {
            if !sanitized.ends_with(' ') {
                sanitized.push(' ');
            }
        } else {
            sanitized.push(character);
        }
    }
    sanitized.trim().to_owned()
}

/// `detail` cut to [`MAX_UPSTREAM_DETAIL_CHARACTERS`] characters, ellipsis included in the count.
fn bounded(mut detail: String) -> String {
    let mut characters = detail.char_indices();
    let Some((cut, _)) = characters.nth(MAX_UPSTREAM_DETAIL_CHARACTERS - 1) else {
        return detail;
    };
    if characters.next().is_none() {
        return detail;
    }
    detail.truncate(cut);
    detail.push('\u{2026}');
    detail
}

#[cfg(test)]
mod tests {
    use dekopon_provider_http::{HttpError, HttpErrorCode};

    use super::{
        INVALID_INPUT, MAX_UPSTREAM_DETAIL_CHARACTERS, RESPONSE_TOO_LARGE, UPSTREAM_FAILURE,
        failure_status, rejected, response_invalid, response_too_large, transport, unauthorized,
        upstream_detail,
    };

    #[test]
    fn transport_failures_are_classified_without_echoing_host_detail() {
        let secret = "authorization=Bearer sk-live-abcdef internal-route=10.0.0.1";
        for (code, expected) in [
            (HttpErrorCode::RequestTooLarge, INVALID_INPUT),
            (HttpErrorCode::ResponseTooLarge, RESPONSE_TOO_LARGE),
            (HttpErrorCode::Timeout, UPSTREAM_FAILURE),
            (HttpErrorCode::Denied, UPSTREAM_FAILURE),
            (HttpErrorCode::HostCallLimit, UPSTREAM_FAILURE),
            (HttpErrorCode::Tls, UPSTREAM_FAILURE),
            (HttpErrorCode::Dns, UPSTREAM_FAILURE),
            (HttpErrorCode::Internal, UPSTREAM_FAILURE),
        ] {
            let error = transport(&HttpError {
                code,
                message: secret.to_owned(),
            });
            assert_eq!(error.code(), expected, "{code:?}");
            assert!(!error.message().contains("Bearer"), "{code:?}");
            assert!(!error.message().contains("10.0.0.1"), "{code:?}");
        }
    }

    #[test]
    fn sanitized_messages_name_the_check_and_the_numbers() {
        assert_eq!(
            unauthorized().message(),
            "credential rejected; the operator must re-login the gpt-image credential"
        );
        assert!(
            response_invalid("data[0].b64_json is missing")
                .message()
                .ends_with("data[0].b64_json is missing")
        );
        let too_large = response_too_large(13_000_000, 12_582_912);
        assert!(too_large.message().contains("13000000"));
        assert!(too_large.message().contains("12582912"));
    }

    /// The sentence a refused call now reads as, both with upstream's reason and without one.
    #[test]
    fn a_refusal_quotes_the_upstream_code_and_message() {
        let detail = upstream_detail(
            Some("moderation_blocked"),
            Some("Your request was rejected as a result of our safety system."),
        );
        assert_eq!(
            rejected(400, detail).message(),
            "the image route refused the request with HTTP 400 (moderation_blocked: Your request \
             was rejected as a result of our safety system.); revise the request"
        );
        assert_eq!(
            failure_status(503, upstream_detail(None, Some("upstream is unavailable"))).message(),
            "the image route answered HTTP 503 (upstream is unavailable); this may be transient"
        );

        // No envelope, no quotation: the sentence is the one this provider has always written.
        assert_eq!(
            rejected(404, None).message(),
            "the image route refused the request with HTTP 404; revise the request"
        );
        assert_eq!(
            failure_status(500, None).message(),
            "the image route answered HTTP 500; this may be transient"
        );
    }

    /// `type` is the fallback for a code, and an envelope that says nothing quotes nothing.
    #[test]
    fn a_detail_needs_one_of_the_two_fields() {
        assert_eq!(
            upstream_detail(Some("invalid_request_error"), None).as_deref(),
            Some("invalid_request_error")
        );
        assert_eq!(
            upstream_detail(None, Some("bad image")).as_deref(),
            Some("bad image")
        );
        assert_eq!(upstream_detail(None, None), None);
        assert_eq!(upstream_detail(Some("  "), Some("\n\t")), None);
    }

    /// Upstream prose is quoted, never obeyed: control characters become spaces so nothing can
    /// forge transcript structure, and the quotation is cut to a fixed number of characters.
    #[test]
    fn a_detail_is_stripped_of_control_characters_and_cut_to_the_bound() {
        assert_eq!(
            upstream_detail(
                Some("bad\u{0}code"),
                Some("first line\r\nsecond\tline\u{7}  spaced  "),
            )
            .as_deref(),
            Some("bad code: first line second line spaced")
        );

        let long = upstream_detail(Some("moderation_blocked"), Some(&"reason ".repeat(200)))
            .expect("a detail");
        assert_eq!(long.chars().count(), MAX_UPSTREAM_DETAIL_CHARACTERS);
        assert!(long.ends_with('\u{2026}'), "{long}");
        assert!(
            long.starts_with("moderation_blocked: reason reason"),
            "{long}"
        );

        // Exactly at the bound is not truncated; one character past it is.
        let at_bound = upstream_detail(None, Some(&"x".repeat(MAX_UPSTREAM_DETAIL_CHARACTERS)))
            .expect("a detail");
        assert_eq!(at_bound.chars().count(), MAX_UPSTREAM_DETAIL_CHARACTERS);
        assert!(!at_bound.ends_with('\u{2026}'));
        let past_bound =
            upstream_detail(None, Some(&"x".repeat(MAX_UPSTREAM_DETAIL_CHARACTERS + 1)))
                .expect("a detail");
        assert_eq!(past_bound.chars().count(), MAX_UPSTREAM_DETAIL_CHARACTERS);
        assert!(past_bound.ends_with('\u{2026}'));

        // A multi-byte character at the cut is not split: the bound counts characters.
        let wide = upstream_detail(
            None,
            Some(&"\u{1f34a}".repeat(MAX_UPSTREAM_DETAIL_CHARACTERS + 8)),
        )
        .expect("a detail");
        assert_eq!(wide.chars().count(), MAX_UPSTREAM_DETAIL_CHARACTERS);
    }
}
