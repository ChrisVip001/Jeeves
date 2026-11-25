//! One place for the two JSON-RPC error shapes every handler builds.
use jeeves_protocol::{ErrorObject, codes};

pub(crate) fn invalid(message: impl ToString) -> ErrorObject {
    ErrorObject {
        code: codes::INVALID_PARAMS,
        message: message.to_string(),
        data: None,
    }
}

pub(crate) fn internal(error: impl std::fmt::Display) -> ErrorObject {
    ErrorObject {
        code: codes::INTERNAL_ERROR,
        message: error.to_string(),
        data: None,
    }
}
