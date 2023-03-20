#![feature(type_alias_impl_trait)]

use std::net::ToSocketAddrs;
use tokio_stream::{Stream, StreamExt};

use time::format_description::OwnedFormatItem;

use tokio_stream::wrappers::ReceiverStream;
use tonic::{transport::Server, Request, Response, Status};

use protocol::greeter_server::{Greeter, GreeterServer};
use protocol::{GetHelloUpdatesRequest, HelloReply, HelloRequest};
use tracing::info;

const PORT: u16 = 8594;

static TIME_ZONE_OFFSET: once_cell::sync::OnceCell<time::UtcOffset> =
    once_cell::sync::OnceCell::new();

static TIME_FORMAT: once_cell::sync::OnceCell<OwnedFormatItem> = once_cell::sync::OnceCell::new();

fn setup_tracing() {
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::{filter, fmt};

    let registry = tracing_subscriber::registry();

    let registry = registry.with(console_subscriber::spawn());

    registry
        .with(
            fmt::layer()
                .with_timer(fmt::time::OffsetTime::new(
                    *TIME_ZONE_OFFSET.get().unwrap(),
                    TIME_FORMAT.get().unwrap(),
                ))
                .with_filter(filter::LevelFilter::DEBUG)
                .with_filter(tracing_subscriber::filter::filter_fn(|meta| {
                    meta.target().starts_with(env!("CARGO_CRATE_NAME"))
                })),
        )
        .init();
}

fn main() -> anyhow::Result<()> {
    TIME_FORMAT
        .set(time::format_description::parse_owned::<2>(
            "[hour]:[minute]:[second] [day].[month].[year]",
        )?)
        .unwrap();

    // we need to get this while still single threaded
    // as getting the time zone offset in a multithreaded programm
    // is UB in some environments
    TIME_ZONE_OFFSET
        .set(time::UtcOffset::current_local_offset()?)
        .unwrap();

    setup_tracing();

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(tokio_main())
}

#[derive(Debug)]
pub struct MyGreeter {
    sender: tokio::sync::broadcast::Sender<HelloReply>,
    receiver: tokio::sync::broadcast::Receiver<HelloReply>,
}

impl MyGreeter {
    pub fn new() -> Self {
        let (sender, receiver) = tokio::sync::broadcast::channel(16);

        Self { sender, receiver }
    }
}

#[tonic::async_trait]
impl Greeter for MyGreeter {
    async fn say_hello(
        &self,
        request: Request<HelloRequest>,
    ) -> Result<Response<HelloReply>, Status> {
        let request = request.into_inner();

        info!(?request, "Got a request");
        let reply = HelloReply {
            message: format!("Hello {}! Your number is {}.", request.name, request.value),
        };

        let _ = self.sender.send(reply.clone());

        Ok(Response::new(reply))
    }

    type GetHelloUpdatesStream = impl Stream<Item = Result<HelloReply, Status>>;
    async fn get_hello_updates(
        &self,
        _request: Request<GetHelloUpdatesRequest>,
    ) -> Result<Response<Self::GetHelloUpdatesStream>, Status> {
        Ok(Response::new(
            tokio_stream::wrappers::BroadcastStream::new(self.receiver.resubscribe())
                .map(|elem| elem.map_err(|err| Status::internal(err.to_string()))),
        ))
    }
}

async fn tokio_main() -> anyhow::Result<()> {
    let addr = ("0.0.0.0", PORT).to_socket_addrs()?.next().unwrap();
    let greeter = MyGreeter::new();

    Server::builder()
        .add_service(GreeterServer::new(greeter))
        .serve(addr)
        .await?;

    Ok(())
}
