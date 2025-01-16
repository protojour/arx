use bytes::Bytes;
use http::{Response, StatusCode};
use http_body_util::{combinators::UnsyncBoxBody, BodyExt, Empty, Full};

pub type DynHttpError = Box<(dyn std::error::Error + Send + Sync + 'static)>;
pub type HyperBody = UnsyncBoxBody<Bytes, DynHttpError>;
pub type HyperResponse = Response<HyperBody>;

#[derive(Debug)]
pub enum HttpError {
    Static(StatusCode, &'static str),
    Dynamic(StatusCode, String),
}

impl HttpError {
    pub const fn bad_request(msg: &'static str) -> Self {
        Self::Static(StatusCode::BAD_REQUEST, msg)
    }

    pub const fn bad_gateway(msg: &'static str) -> Self {
        Self::Static(StatusCode::BAD_GATEWAY, msg)
    }

    pub fn into_hyper_response(self) -> HyperResponse {
        match self {
            Self::Static(status, msg) => Response::builder()
                .status(status)
                .body(
                    Full::new(msg.into())
                        .map_err(|never| match never {})
                        .boxed_unsync(),
                )
                .unwrap(),
            Self::Dynamic(status, msg) => Response::builder()
                .status(status)
                .body(
                    Full::new(msg.into())
                        .map_err(|never| match never {})
                        .boxed_unsync(),
                )
                .unwrap(),
        }
    }
}

pub fn empty_body() -> HyperBody {
    Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed_unsync()
}
