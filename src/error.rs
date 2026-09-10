//! The provider's closed error taxonomy.
//!
//! Seven codes, and no upstream body or host detail inside any message. What reaches a model is a
//! stable code and a sentence written here; what the operator needs beyond that is in the broker's
//! audit log and the provider span, not in the text a prompt can read back.

use dekopon_provider_http::{HttpError, HttpErrorCode};
use dekopon_provider_sdk::ProviderError;

/// The caller's input, or an expansion the route did not perform, is wrong.
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
/// A usable response that does not fit the authorized result size.
pub(crate) const RESPONSE_TOO_LARGE: &str = "response-too-large";

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

/// Any other 4xx. The status is the only upstream value in the message.
pub(crate) fn rejected(status: u16) -> ProviderError {
    ProviderError::new(
        UPSTREAM_REJECTED,
        format!("the image route refused the request with HTTP {status}; revise the request"),
    )
}

/// A 5xx, or any other status that is neither success nor a classified refusal.
pub(crate) fn failure_status(status: u16) -> ProviderError {
    ProviderError::new(
        UPSTREAM_FAILURE,
        format!("the image route answered HTTP {status}; this may be transient"),
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

/// The success envelope would exceed the ceiling this component fails closed at.
pub(crate) fn response_too_large(envelope: usize, ceiling: usize) -> ProviderError {
    ProviderError::new(
        RESPONSE_TOO_LARGE,
        format!(
            "the image is too large to return: the result would be {envelope} bytes and the limit \
             is {ceiling}; ask for a smaller size"
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

#[cfg(test)]
mod tests {
    use dekopon_provider_http::{HttpError, HttpErrorCode};

    use super::{
        INVALID_INPUT, RESPONSE_TOO_LARGE, UPSTREAM_FAILURE, response_invalid, response_too_large,
        transport, unauthorized,
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
}
