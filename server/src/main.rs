#![feature(type_alias_impl_trait)]

use std::collections::HashMap;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio_stream::{Stream, StreamExt};

use time::format_description::OwnedFormatItem;

use tonic::service::interceptor;
use tonic::transport::{Certificate, Identity, ServerTlsConfig};
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

impl Default for MyGreeter {
    fn default() -> Self {
        let (sender, receiver) = tokio::sync::broadcast::channel(16);

        Self { sender, receiver }
    }
}

// from https://github.com/dndx/phantun/blob/35f7b35ff5352c90dca527e3a80bae58deca91d6/phantun/src/bin/server.rs#L14
fn new_udp_reuseport(addr: SocketAddr) -> UdpSocket {
    let udp_sock = socket2::Socket::new(
        if addr.is_ipv4() {
            socket2::Domain::IPV4
        } else {
            socket2::Domain::IPV6
        },
        socket2::Type::DGRAM,
        None,
    )
    .unwrap();
    udp_sock.set_reuse_port(true).unwrap();
    // from tokio-rs/mio/blob/master/src/sys/unix/net.rs
    udp_sock.set_cloexec(true).unwrap();
    udp_sock.set_nonblocking(true).unwrap();
    udp_sock.bind(&socket2::SockAddr::from(addr)).unwrap();
    std::net::UdpSocket::from(udp_sock).try_into().unwrap()
}

#[tonic::async_trait]
impl Greeter for MyGreeter {
    async fn say_hello(
        &self,
        request: Request<HelloRequest>,
    ) -> Result<Response<HelloReply>, Status> {
        let sid = request.metadata().get("sid");
        let request = request.get_ref();

        info!(?request, ?sid, "Got a request");

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
        dbg!(&_request.extensions().get::<User>());

        Ok(Response::new(
            tokio_stream::wrappers::BroadcastStream::new(self.receiver.resubscribe())
                .map(|elem| elem.map_err(|err| Status::internal(err.to_string()))),
        ))
    }

    async fn start_udp_sidechannel_setup(
        &self,
        request: tonic::Request<protocol::UdpSetupRequest>,
    ) -> Result<tonic::Response<protocol::UdpSetupResponse>, tonic::Status> {
        todo!()
    }

    async fn finish_udp_sidechannel_setup(
        &self,
        request: tonic::Request<protocol::UdpSetupFinishRequest>,
    ) -> Result<tonic::Response<protocol::UdpSetupFinishRespone>, tonic::Status> {
        todo!()
    }
}

#[derive(Debug)]
struct User {
    uid: u64,
    name: String,
}

struct Users {
    names: HashMap<u64, String>,
}

fn auth_interceptor(users: &Users, mut request: Request<()>) -> Result<Request<()>, Status> {
    let certs = request.peer_certs();

    dbg!(certs.iter().flat_map(|x| x.iter()).count());

    use x509_parser::prelude::*;

    let uid = certs
        .iter()
        .flat_map(|x| x.iter())
        .flat_map(|cert| X509Certificate::from_der(cert.get_ref()).ok())
        .find_map(|(_, cert)| {
            cert.subject()
                .iter_common_name()
                .flat_map(|name| name.as_str().ok())
                .flat_map(|name| name.strip_prefix("uid="))
                .find_map(|name| u64::from_str_radix(name, 16).ok())
        });

    if let Some(uid) = uid {
        if let Some(name) = users.names.get(&uid).cloned() {
            request.extensions_mut().insert(User { uid, name });

            return Ok(request);
        }
    }

    Err(Status::unauthenticated("invalid credentials"))
}

async fn tokio_main() -> anyhow::Result<()> {
    let addr = ("0.0.0.0", PORT).to_socket_addrs()?.next().unwrap();
    let greeter = MyGreeter::default();

    let users = Arc::new(Users {
        names: [(0xbfe984442ebcc61b, "soruh".to_owned())]
            .into_iter()
            .collect(),
    });

    let identity = Identity::from_pem(
        include_bytes!("../../certs/localhost.crt"),
        include_bytes!("../../certs/localhost.key"),
    );

    let client_ca = Certificate::from_pem(include_bytes!("../../certs/ca.crt"));

    Server::builder()
        .tls_config(
            ServerTlsConfig::new()
                .identity(identity)
                .client_ca_root(client_ca),
        )
        .unwrap()
        .layer(interceptor(move |req| auth_interceptor(&users, req)))
        .add_service(GreeterServer::new(greeter))
        .serve(addr)
        .await?;

    Ok(())
}
