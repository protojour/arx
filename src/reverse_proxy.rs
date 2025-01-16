use std::fmt::Debug;

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use http::{header, HeaderValue, StatusCode};
use http_body::Body;
use http_body_util::BodyExt;
use hyper_util::rt::TokioIo;
use reqwest_websocket::RequestBuilderExt;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::tungstenite::protocol::{self, WebSocketConfig};
use tracing::{debug, error, info};

use crate::hyper::{empty_body, HttpError, HyperResponse};

/// Reverse-proxy a request.
/// The URI is already rewritten to point to the backend server.
pub async fn reverse_proxy<B>(
    mut req: http::Request<B>,
    client: &reqwest::Client,
) -> Result<HyperResponse, HttpError>
where
    B: Body<Data = bytes::Bytes> + Send + Sync + 'static,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    match req.headers().get(header::UPGRADE).map(|h| h.as_bytes()) {
        None => {}
        Some(b"websocket") => return proxy_websocket(req, client).await,
        Some(_) => return Err(HttpError::bad_request("unrecognized `Upgrade` header")),
    }

    let method = req.method().clone();
    let uri = req.uri().clone();
    let headers = std::mem::take(req.headers_mut());
    let req_body = http_body_util::BodyDataStream::new(req.into_body());

    let response_result = client
        .request(method, uri.to_string())
        .headers(headers)
        .body(reqwest::Body::wrap_stream(req_body))
        .send()
        .await;

    reqwest_to_hyper_response(response_result)
}

/// Reverse-proxy a request, where the request body is !Sync.
/// The URI is already rewritten to point to the backend server.
#[expect(unused)]
pub async fn reverse_proxy_unsync<B>(
    mut req: http::Request<B>,
    client: &reqwest::Client,
) -> Result<HyperResponse, HttpError>
where
    B: Body<Data = bytes::Bytes> + Send + Unpin + 'static,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>> + Send + Debug,
{
    match req.headers().get(header::UPGRADE).map(|h| h.as_bytes()) {
        None => {}
        Some(b"websocket") => return proxy_websocket(req, client).await,
        Some(_) => return Err(HttpError::bad_request("unrecognized `Upgrade` header")),
    }

    let method = req.method().clone();
    let uri = req.uri().clone();
    let headers = std::mem::take(req.headers_mut());
    let mut req_body = req.into_body();

    enum ForwardBodyError<B: Body> {
        Input(B::Error),
        TrailingHeaders,
    }

    // Because the request body is !Sync, it must be proxied through a channel first
    // FIXME(backpressure): should not start streaming the body before the proxy request(below) has been sent.
    // instead it should start polling as soon as reqwest starts polling _its_ body
    let (request_body_future, req_body_rx) = {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, B::Error>>(1);
        (
            tokio::spawn(async move {
                while let Some(frame_result) = req_body.frame().await {
                    match frame_result {
                        Ok(frame) => match frame.into_data() {
                            Ok(data) => {
                                let _ = tx.send(Ok(data)).await;
                            }
                            Err(_err) => return Err(ForwardBodyError::<B>::TrailingHeaders),
                        },
                        Err(err) => return Err(ForwardBodyError::Input(err)),
                    };
                }

                Ok(())
            }),
            rx,
        )
    };

    let req_body = tokio_stream::wrappers::ReceiverStream::new(req_body_rx);

    let response_future = client
        .request(method, uri.to_string())
        .headers(headers)
        .body(reqwest::Body::wrap_stream(req_body))
        .send();

    let (request_body_join_result, response_result) =
        tokio::join!(request_body_future, response_future);

    match request_body_join_result {
        Ok(Ok(())) => reqwest_to_hyper_response(response_result),
        Ok(Err(ForwardBodyError::Input(error))) => {
            info!("input body error: {error:?}");
            Err(HttpError::bad_request(""))
        }
        Ok(Err(ForwardBodyError::TrailingHeaders)) => Err(HttpError::Static(
            StatusCode::NOT_ACCEPTABLE,
            "trailing headers not supported",
        )),
        Err(_join_error) => Err(HttpError::Static(
            StatusCode::INTERNAL_SERVER_ERROR,
            "headers not sent",
        )),
    }
}

fn reqwest_to_hyper_response(
    response_result: Result<reqwest::Response, reqwest::Error>,
) -> Result<HyperResponse, HttpError> {
    let response: http::Response<_> = response_result
        .map_err(|err| {
            if let Some(status) = err.status() {
                HttpError::Dynamic(status, err.to_string())
            } else {
                HttpError::Dynamic(StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
            }
        })?
        .into();

    let (parts, body) = response.into_parts();
    Ok(http::Response::from_parts(
        parts,
        body.map_err(|err| err.into()).boxed_unsync(),
    ))
}

async fn proxy_websocket<B>(
    mut req: http::Request<B>,
    client: &reqwest::Client,
) -> Result<HyperResponse, HttpError>
where
    B: Body<Data = bytes::Bytes> + Send + 'static,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    req.headers()
        .get(header::SEC_WEBSOCKET_VERSION)
        .filter(|header| header.as_bytes() == b"13")
        .ok_or(HttpError::bad_request("invalid websocket version"))?;

    let sec_websocket_key = req
        .headers()
        .get(header::SEC_WEBSOCKET_KEY)
        .cloned()
        .ok_or(HttpError::bad_request("`Sec-Websocket-Key` header missing"))?;

    let sec_websocket_protocol = req.headers().get(header::SEC_WEBSOCKET_PROTOCOL).cloned();
    let headers = std::mem::take(req.headers_mut());

    // establish proxy connection
    let upgrade_response = client
        .get(req.uri().to_string())
        .headers(headers)
        .upgrade()
        .send()
        .await
        .map_err(|err| {
            debug!(?err, "failed to send ws proxy request");
            HttpError::bad_gateway("bad gateway")
        })?;

    let back_socket = upgrade_response
        .into_websocket()
        .await
        .map_err(|err| match err {
            reqwest_websocket::Error::Handshake(
                reqwest_websocket::HandshakeError::UnexpectedStatusCode(status),
            ) => {
                // This happens when upstream responds with e.g. FORBIDDEN,
                // this status code is reflected directly.
                // Unfortunately the body/message is private within reqwest_websocket, probably a little misdesigned
                HttpError::Static(status, "upstream refused upgrade")
            }
            reqwest_websocket::Error::Handshake(_) => HttpError::bad_gateway("handshake failed"),
            reqwest_websocket::Error::Reqwest(err) => {
                error!(?err, "unexpected upgrade reqwest error");
                HttpError::bad_gateway("unspecified request error")
            }
            reqwest_websocket::Error::Tungstenite(err) => {
                info!(?err, "tungstenite error");
                HttpError::bad_gateway("protocol error")
            }
            _ => {
                info!(?err, "unknown ws error");
                HttpError::bad_gateway("protocol error")
            }
        })?;

    // post-upgrade:
    tokio::task::spawn(async move {
        let upgraded = match hyper::upgrade::on(&mut req).await {
            Ok(upgraded) => upgraded,
            Err(err) => {
                info!(?err, "upgrade error");
                return;
            }
        };

        let front_socket = tokio_tungstenite::WebSocketStream::from_raw_socket(
            TokioIo::new(upgraded),
            protocol::Role::Server,
            Some(WebSocketConfig::default()),
        )
        .await;

        ws_tunnel(front_socket, back_socket).await;
    });

    // pre-upgrade:
    let mut response_builder = http::Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header(header::CONNECTION, HeaderValue::from_static("upgrade"))
        .header(header::UPGRADE, HeaderValue::from_static("websocket"))
        .header(
            header::SEC_WEBSOCKET_ACCEPT,
            tungstenite::handshake::derive_accept_key(sec_websocket_key.as_bytes()),
        )
        .header(header::SEC_WEBSOCKET_KEY, sec_websocket_key);

    if let Some(sec_websocket_protocol) = sec_websocket_protocol {
        response_builder =
            response_builder.header(header::SEC_WEBSOCKET_PROTOCOL, sec_websocket_protocol);
    }

    Ok(response_builder.body(empty_body()).unwrap())
}

async fn ws_tunnel<S>(
    mut front_socket: tokio_tungstenite::WebSocketStream<S>,
    mut back_socket: reqwest_websocket::WebSocket,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (back_close_code, back_close_message): (reqwest_websocket::CloseCode, Option<String>) = loop {
        tokio::select! {
            msg = front_socket.next() => {
                // from client, to back server
                match msg {
                    None => {
                        // client hung up
                        break (reqwest_websocket::CloseCode::Normal, None);
                    }
                    Some(Ok(tungstenite::protocol::Message::Text(text))) => {
                        let _ = back_socket.send(reqwest_websocket::Message::Text(text)).await;
                    }
                    Some(Ok(tungstenite::protocol::Message::Binary(binary))) => {
                        let _ = back_socket.send(reqwest_websocket::Message::Binary(binary)).await;
                    }
                    Some(Ok(tungstenite::protocol::Message::Close(Some(close_frame)))) => {
                        break (close_frame.code.into(), Some(close_frame.reason.to_string()));
                    }
                    Some(Ok(tungstenite::protocol::Message::Close(None))) => {
                        break (reqwest_websocket::CloseCode::Normal, None);
                    }
                    Some(Ok(_)) => {}
                    Some(Err(err)) => {
                        debug!(?err, "error receiving from front websocket");
                    }
                }
            }
            msg = back_socket.next() => {
                // from back server, to client
                match msg {
                    None => {
                        break (reqwest_websocket::CloseCode::Normal, None);
                    }
                    Some(Ok(reqwest_websocket::Message::Text(text))) => {
                        let _ = front_socket.send(tungstenite::protocol::Message::Text(text)).await;
                    }
                    Some(Ok(reqwest_websocket::Message::Binary(binary))) => {
                        let _ = front_socket.send(tungstenite::protocol::Message::Binary(binary)).await;
                    }
                    Some(Ok(reqwest_websocket::Message::Ping(_))) => {}
                    Some(Ok(reqwest_websocket::Message::Pong(_))) => {}
                    Some(Ok(reqwest_websocket::Message::Close { .. })) => {
                        break (reqwest_websocket::CloseCode::Normal, None);
                    }
                    Some(Err(err)) => {
                        debug!(?err, "error receiving from back websocket");
                    }
                }
            }
        }
    };

    let _ = front_socket.close(None).await;
    let _ = back_socket
        .close(back_close_code, back_close_message.as_deref())
        .await;
}
