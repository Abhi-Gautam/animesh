//! Local-socket transport between the CLI client and the daemon server.
//!
//! Cross-platform via `interprocess` (unix domain socket on macOS/Linux, named
//! pipe on Windows) — all of that is contained in this module. Messages are
//! length-delimited frames of JSON-encoded [`Request`]/[`Reply`].

use anyhow::{Context, Result};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use interprocess::local_socket::traits::tokio::{Listener as _, Stream as _};
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, Name, ToNsName};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

use crate::api::{Reply, Request};

/// Concrete transport types, re-exported so callers don't touch `interprocess`.
pub(crate) type Listener = interprocess::local_socket::tokio::Listener;
pub(crate) type Stream = interprocess::local_socket::tokio::Stream;

/// The agreed socket identifier. Namespaced so it works on all platforms.
fn socket_name() -> Result<Name<'static>> {
    "animesh.sock"
        .to_ns_name::<GenericNamespaced>()
        .context("build local socket name")
}

/// Daemon side: bind the socket and start listening.
pub(crate) fn bind() -> Result<Listener> {
    let name = socket_name()?;
    ListenerOptions::new()
        .name(name)
        .create_tokio()
        .context("bind animesh daemon socket (another daemon already running?)")
}

/// Daemon side: wait for the next client connection.
pub(crate) async fn accept(listener: &Listener) -> Result<Stream> {
    listener.accept().await.context("accept client connection")
}

/// Daemon side: read one [`Request`] off `stream`, run `handler`, send its [`Reply`].
pub(crate) async fn serve_once<F, Fut>(stream: Stream, handler: F) -> Result<()>
where
    F: FnOnce(Request) -> Fut,
    Fut: std::future::Future<Output = Reply>,
{
    let mut framed = Framed::new(stream, LengthDelimitedCodec::new());

    let Some(frame) = framed.next().await else {
        return Ok(()); // client hung up before sending
    };
    let req: Request =
        serde_json::from_slice(&frame.context("read request frame")?).context("decode request")?;

    let reply = handler(req).await;

    let bytes = Bytes::from(serde_json::to_vec(&reply).context("encode reply")?);
    framed.send(bytes).await.context("send reply")?;
    Ok(())
}

/// Client side: connect to the daemon, send `req`, return its [`Reply`].
pub(crate) async fn request(req: &Request) -> Result<Reply> {
    let name = socket_name()?;
    let stream = Stream::connect(name)
        .await
        .context("connect to animesh daemon — is it running? start it with `animesh daemon`")?;

    let mut framed = Framed::new(stream, LengthDelimitedCodec::new());

    let bytes = Bytes::from(serde_json::to_vec(req).context("encode request")?);
    framed.send(bytes).await.context("send request")?;

    let frame = framed
        .next()
        .await
        .context("daemon closed the connection without a reply")?
        .context("read reply frame")?;
    serde_json::from_slice(&frame).context("decode reply")
}
