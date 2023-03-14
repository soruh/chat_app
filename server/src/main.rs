use std::{collections::HashMap, sync::Arc, time::Duration};

use rstreamer::{Node, NodeHandle, NodeInfo};
use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, UdpSocket},
    sync::Mutex,
};

const PORT: u16 = 8594;

const MS40: usize = 48000 * 40 / 1000;

#[tokio::main]
async fn main() {
    console_subscriber::init();

    let tcp_listener = TcpListener::bind(("0.0.0.0", PORT)).await.unwrap();
    let upd_socket = Arc::new(UdpSocket::bind(("0.0.0.0", PORT)).await.unwrap());

    let mut last_id = 0u64;

    struct State {
        muxers: HashMap<u64, NodeHandle<i16, i16>>,
        sources: HashMap<u64, NodeHandle<(), i16>>,
    }

    let state = Arc::new(Mutex::new(State {
        muxers: HashMap::new(),
        sources: HashMap::new(),
    }));

    loop {
        let id = last_id;
        last_id = last_id.wrapping_add(1);

        let (mut tcp_socket, addr) = tcp_listener.accept().await.unwrap();

        let state = state.clone();
        let upd_socket = upd_socket.clone();
        tokio::task::Builder::new()
            .name(&format!("TCP connection to {addr:?}"))
            .spawn(async move {
                let (decoder, mut decoder_lock) = Node::<(), _>::from_task(
                    &format!("OPUS decoder for {addr:?}"),
                    NodeInfo {
                        output_buffer_size: MS40,
                        input_buffer_size: 0,
                        input_buffer_duration: Duration::from_millis(0),
                    },
                    {
                        let upd_socket = upd_socket.clone();
                        |node| async move {
                            let mut decoder = audiopus::coder::Decoder::new(
                                audiopus::SampleRate::Hz48000,
                                audiopus::Channels::Mono,
                            )
                            .unwrap();

                            let mut input_buffer = vec![0; MS40];
                            let mut output_buffer = vec![0; MS40];

                            loop {
                                let (_, remote_addr) =
                                    upd_socket.peek_from(&mut input_buffer).await.unwrap();

                                if remote_addr != addr {
                                    tokio::task::yield_now().await;
                                    continue;
                                }

                                let (n, addr) =
                                    upd_socket.recv_from(&mut input_buffer).await.unwrap();

                                let n = decoder
                                    .decode(Some(&input_buffer[..n]), &mut output_buffer, false)
                                    .unwrap();

                                println!("decoded {n} bytes for {addr:?}");

                                let node = &mut *node.lock().await;
                                for output in node.outputs_mut() {
                                    output.as_mut_base().push_slice(&output_buffer[..n]);
                                }
                            }
                        }
                    },
                );

                let (encoder, mut encoder_lock) = Node::<_, ()>::from_task(
                    &format!("OPUS encoder for {addr:?}"),
                    NodeInfo {
                        output_buffer_size: 0,
                        input_buffer_size: MS40,
                        input_buffer_duration: Duration::from_millis(20),
                    },
                    |node| async move {
                        let encoder = audiopus::coder::Encoder::new(
                            audiopus::SampleRate::Hz48000,
                            audiopus::Channels::Mono,
                            audiopus::Application::Audio,
                        )
                        .unwrap();

                        let mut input_buffer = vec![0; MS40]; // 20 ms
                        let mut output_buffer = vec![0; MS40];

                        loop {
                            node.lock().await.inputs_mut()[0]
                                .pop_slice(&mut input_buffer)
                                .await
                                .unwrap();

                            let n = encoder.encode(&input_buffer, &mut output_buffer).unwrap();

                            println!("encoded {n} bytes for {addr:?}");

                            upd_socket.send_to(&output_buffer[..n], addr).await.unwrap();
                        }
                    },
                );

                let mut muxer_scratch_buffer = vec![0i16; MS40];
                let mut muxer_output_buffer = vec![0i16; MS40];
                let (muxer, mut muxer_lock) = Node::from_fn(
                    &format!("multiplexer for {addr:?}"),
                    NodeInfo {
                        output_buffer_size: MS40,
                        input_buffer_size: MS40,
                        input_buffer_duration: Duration::from_millis(20),
                    },
                    move |inputs, outputs| {
                        muxer_output_buffer.fill(0);

                        let mut n_max = 0;
                        for input in inputs {
                            let n = input.as_mut_base().pop_slice(&mut muxer_scratch_buffer);

                            n_max = n_max.max(n);

                            for (src, dst) in muxer_scratch_buffer[..n]
                                .iter()
                                .zip(muxer_output_buffer.iter_mut())
                            {
                                *dst = dst.saturating_add(*src);
                            }
                        }

                        for output in outputs {
                            output
                                .as_mut_base()
                                .push_slice(&muxer_output_buffer[..n_max]);
                        }
                    },
                );

                {
                    let state = &mut *state.lock().await;

                    for (_id, muxer) in &mut state.muxers {
                        let node = &mut *muxer.node.lock().await;

                        decoder_lock.connect(node);
                    }

                    for (_id, client) in &mut state.sources {
                        let mut node = client.node.lock().await;

                        node.connect(&mut *muxer_lock);
                    }

                    muxer_lock.connect(&mut *encoder_lock);

                    state.muxers.insert(id, muxer);
                    state.sources.insert(id, decoder);
                }

                drop(muxer_lock);
                drop(decoder_lock);
                drop(encoder_lock);

                println!("{addr:?} is now connected");

                let mut buf = [0; 128];
                loop {
                    if let Err(_err) = tcp_socket.read_exact(&mut buf).await {
                        let state = &mut *state.lock().await;
                        state.sources.remove(&id).unwrap().handle.abort();
                        state.muxers.remove(&id).unwrap().handle.abort();
                        encoder.handle.abort();

                        for (_id, muxer) in &mut state.muxers {
                            muxer.node.lock().await.remove_closed_inputs();
                        }

                        println!("{addr:?} disconnected");

                        break;
                    }
                }
            })
            .unwrap();
    }
}
