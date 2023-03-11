mod protocol;

use std::net::Ipv4Addr;

use rstreamer::{Node, NodeInfo};
use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, UdpSocket},
};

const PORT: u16 = 8594;

#[tokio::main]
async fn main() {
    console_subscriber::init();

    let tcp_listener = TcpListener::bind(("0.0.0.0", PORT)).await.unwrap();

    loop {
        let (mut tcp_socket, addr) = tcp_listener.accept().await.unwrap();

        tokio::task::Builder::new()
            .name(&format!("TCP connection to {addr:?}"))
            .spawn(async move {
                let mut header = [0; 4 + 2];

                tcp_socket.read_exact(&mut header).await.unwrap();

                let addr: [u8; 4] = header[..4].try_into().unwrap();
                let port: [u8; 2] = header[4..].try_into().unwrap();

                let addr = (Ipv4Addr::from(addr), u16::from_le_bytes(port));

                let upd_socket = UdpSocket::bind(("0.0.0.0", PORT)).await.unwrap();
                upd_socket.connect(addr).await.unwrap();

                let (producer, consumer) = async_ringbuf::AsyncHeapRb::<u16>::new(4096).split();
            })
            .unwrap();
    }
}
