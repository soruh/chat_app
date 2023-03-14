use std::time::Duration;

use rand::Rng;
use rstreamer::{Node, NodeInfo};

#[tokio::main]
async fn main() {
    console_subscriber::init();

    let mut producer_a_buffer = vec![0; 4096];
    let (producer_a, mut producer_a_lock) = Node::<(), u16>::from_task(
        "producer a",
        NodeInfo {
            output_buffer_size: producer_a_buffer.len(),
            input_buffer_size: 0,
            input_buffer_duration: Duration::from_secs(0),
        },
        move |node| async move {
            loop {
                {
                    let mut rng = rand::thread_rng();
                    producer_a_buffer.fill_with(|| rng.gen());
                }

                let this = &mut *node.lock().await;

                for output in this.outputs_mut() {
                    // waits for space in destination
                    output.push_slice(&producer_a_buffer).await.unwrap();

                    // println!("producer a wrote {} bytes", producer_a_buffer.len());
                }

                // tokio::time::sleep(Duration::from_millis(20)).await;
            }
        },
    );

    let mut producer_b_buffer = vec![0; 4096];
    let (producer_b, mut producer_b_lock) = Node::<(), u16>::from_task(
        "producer b",
        NodeInfo {
            output_buffer_size: producer_b_buffer.len(),
            input_buffer_size: 0,
            input_buffer_duration: Duration::from_secs(0),
        },
        move |node| async move {
            loop {
                {
                    let mut rng = rand::thread_rng();
                    producer_b_buffer.fill_with(|| rng.gen());
                }

                let this = &mut *node.lock().await;

                for output in this.outputs_mut() {
                    // does not wait for space in desitnation
                    let n = output.as_mut_base().push_slice(&producer_b_buffer);
                    // println!("producer b wrote {n} bytes");
                }

                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        },
    );

    let transformer_a_buffer_size = 4096;
    let mut transformer_a_buffer_out = vec![0u16; transformer_a_buffer_size];
    let mut transformer_a_buffer_scratch = vec![0u16; transformer_a_buffer_size];
    let (transformer_a, mut transformer_a_lock) = Node::<u16, u16>::from_fn(
        "transformer a",
        NodeInfo {
            input_buffer_size: transformer_a_buffer_size,
            output_buffer_size: transformer_a_buffer_size,
            input_buffer_duration: Duration::from_secs(1),
        },
        move |inputs, outputs| {
            transformer_a_buffer_out.fill(0);

            for input in inputs {
                let removed = input
                    .as_mut_base()
                    .pop_slice(&mut transformer_a_buffer_scratch);

                if removed < transformer_a_buffer_size {
                    eprintln!(
                        "input fell behind by {} samples",
                        transformer_a_buffer_size - removed
                    );
                }

                for (dst, src) in transformer_a_buffer_out
                    .iter_mut()
                    .zip(transformer_a_buffer_scratch.iter())
                {
                    *dst = dst.saturating_add(src / 10);
                }
            }

            for output in outputs {
                let free = transformer_a_buffer_size.min(output.free_len());

                output
                    .as_mut_base()
                    .push_slice(&transformer_a_buffer_out[..free]);
            }

            // println!("transformer a transformed {transformer_a_buffer_size} bytes");
        },
    );

    let mut sink_a_buffer = vec![0; 4096];
    let (sink_a, mut sink_a_lock) = Node::<u16, ()>::from_task(
        "sink a",
        NodeInfo {
            output_buffer_size: 0,
            input_buffer_size: sink_a_buffer.len(),
            input_buffer_duration: Duration::from_secs(1),
        },
        move |node| async move {
            loop {
                let this = &mut *node.lock().await;

                for input in this.inputs_mut() {
                    // does not wait for space in desitnation
                    input.pop_slice(&mut sink_a_buffer).await.unwrap();
                    // println!("sink a read {} bytes", sink_a_buffer.len());
                }
            }
        },
    );

    producer_a_lock.connect(&mut *transformer_a_lock);
    println!("connected producer_a to transformer_a");
    producer_b_lock.connect(&mut *transformer_a_lock);
    println!("connected producer_b to transformer_a");
    transformer_a_lock.connect(&mut *sink_a_lock);
    println!("connected transformer_a to sink_a");

    {
        drop(producer_a_lock);
        drop(producer_b_lock);
        drop(transformer_a_lock);
        drop(sink_a_lock);
    }

    tokio::task::Builder::new()
        .name("Sleep")
        .spawn(async {
            tokio::time::sleep(Duration::from_secs(30)).await;
        })
        .unwrap()
        .await
        .unwrap();
}
