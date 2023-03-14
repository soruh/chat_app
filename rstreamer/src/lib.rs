use std::{future::Future, sync::Arc, time::Duration};

use futures::{future::join_all, join};
use tokio::{
    sync::{Mutex, OwnedMutexGuard},
    task::JoinHandle,
};

type Buffer<T> = async_ringbuf::AsyncHeapRb<T>;
type Producer<T> = async_ringbuf::AsyncHeapProducer<T>;
type Consumer<T> = async_ringbuf::AsyncHeapConsumer<T>;

#[derive(Debug, Clone, Copy)]
pub struct NodeInfo {
    pub output_buffer_size: usize,
    pub input_buffer_size: usize,
    pub input_buffer_duration: Duration,
}

pub struct Node<In, Out> {
    inputs: Vec<Consumer<In>>,
    outputs: Vec<Producer<Out>>,
    info: NodeInfo,
}

pub struct NodeHandle<In, Out> {
    pub node: Arc<Mutex<Node<In, Out>>>,
    pub handle: JoinHandle<Result<(), ()>>,
}

impl<In: Send + Sync + 'static, Out: Send + Sync + 'static> NodeHandle<In, Out> {
    pub async fn connect<U>(&self, other: &NodeHandle<Out, U>) {
        let (mut this, mut other) = join!(self.node.lock(), other.node.lock());

        this.connect(&mut *other);
    }
}

impl<In: Send + Sync + 'static, Out: Send + Sync + 'static> Node<In, Out> {
    pub fn remove_closed_inputs(&mut self) {
        self.inputs.retain(|consumer| !consumer.is_closed())
    }

    pub fn remove_closed_outputs(&mut self) {
        self.outputs.retain(|consumer| !consumer.is_closed())
    }

    fn connect_with_buffer_size<U>(&mut self, other: &mut Node<Out, U>, buffer_size: usize) {
        let (producer, consumer) = Buffer::<Out>::new(buffer_size).split();
        self.outputs.push(producer);
        other.inputs.push(consumer);
    }

    pub fn connect<U>(&mut self, other: &mut Node<Out, U>) {
        self.connect_with_buffer_size(
            other,
            self.info
                .output_buffer_size
                .max(other.info.input_buffer_size),
        )
    }

    pub fn from_task<Task: Future<Output = Result<(), ()>> + Send + 'static>(
        name: &str,
        info: NodeInfo,
        task: impl FnOnce(Arc<Mutex<Node<In, Out>>>) -> Task,
    ) -> (NodeHandle<In, Out>, OwnedMutexGuard<Node<In, Out>>) {
        let node = Arc::new(Mutex::new(Node {
            inputs: Vec::new(),
            outputs: Vec::new(),
            info,
        }));

        let lock = Mutex::try_lock_owned(node.clone()).unwrap();

        (
            NodeHandle {
                handle: tokio::task::Builder::new()
                    .name(name)
                    .spawn(task(node.clone()))
                    .unwrap(),
                node,
            },
            lock,
        )
    }

    pub fn from_fn(
        name: &str,
        info: NodeInfo,
        mut f: impl FnMut(std::slice::IterMut<Consumer<In>>, std::slice::IterMut<Producer<Out>>)
            + Send
            + 'static,
    ) -> (NodeHandle<In, Out>, OwnedMutexGuard<Node<In, Out>>) {
        Self::from_task(name, info, |this| async move {
            loop {
                let this = &mut *this.lock().await;
                let input_ready = this.inputs.iter().map(|input| async {
                    input.wait(info.input_buffer_size).await;
                });
                // should this be less as we have already fallen behind if this has elapsed?
                let timeout = info.input_buffer_duration;
                let _ = tokio::time::timeout(timeout, join_all(input_ready)).await;

                let inputs = this.inputs.iter_mut();
                let outputs = this.outputs.iter_mut();

                f(inputs, outputs);
            }
        })
    }

    pub fn inputs_mut(&mut self) -> &mut [Consumer<In>] {
        &mut self.inputs
    }

    pub fn outputs_mut(&mut self) -> &mut [Producer<Out>] {
        &mut self.outputs
    }
}
