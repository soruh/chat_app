use std::cell::Cell;
use std::fmt::Display;
use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{Device, HostId};
use iced::futures::stream::BoxStream;
use iced::futures::StreamExt;
use iced::widget::{button, column, pick_list, row, text, text_input};
use iced::{Alignment, Application, Command, Element, Subscription};

use protocol::greeter_client::GreeterClient;
use protocol::{GetHelloUpdatesRequest, HelloReply, HelloRequest};
use tokio::sync::{mpsc::UnboundedReceiver, Mutex};
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};
use tonic::Request;

pub struct Gui {
    value: i32,
    receiver: Cell<Option<UnboundedReceiver<Message>>>,
    audio: AudioConfig,
    name: String,
    your_greeting: String,
    latest_greeting: String,
    client: Arc<Mutex<GreeterClient<Channel>>>,
}

struct AudioConfig {
    host: cpal::Host,
    hosts: Vec<Host>,
    host_id: Host,
    input_devices: Vec<String>,
    output_devices: Vec<String>,
    input_device: Option<Device>,
    output_device: Option<Device>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Host(pub HostId);

impl Display for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    IncrementPressed,
    DecrementPressed,
    SelectHost(Host),
    SelectInput(String),
    SelectOutput(String),
    SayHello,
    HelloResponse(HelloReply),
    SetName(String),
    UpdatesLatestGreeting(String),
}

impl Application for Gui {
    type Message = Message;
    type Executor = iced::executor::Default;
    type Theme = iced::theme::Theme;
    type Flags = ();

    fn new(_flags: ()) -> (Self, Command<Message>) {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();

        let identity = Identity::from_pem(
            include_bytes!("../../certs/client_0.crt"),
            include_bytes!("../../certs/client_0.key"),
        );

        let ca = Certificate::from_pem(include_bytes!("../../certs/ca.crt"));

        let client = Arc::new(Mutex::new(GreeterClient::new(
            Channel::from_static("https://h.glsys.de:8594")
                .tls_config(ClientTlsConfig::new().identity(identity).ca_certificate(ca))
                .unwrap()
                .connect_lazy(),
        )));

        {
            let sender = sender.clone();
            tokio::task::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    sender.send(Message::DecrementPressed).unwrap();
                }
            });
        }
        {
            let client = client.clone();
            tokio::task::spawn(async move {
                let mut updates = client
                    .lock()
                    .await
                    .get_hello_updates(GetHelloUpdatesRequest {})
                    .await
                    .unwrap()
                    .into_inner();

                while let Some(update) = updates.next().await {
                    sender
                        .send(Message::UpdatesLatestGreeting(update.unwrap().message))
                        .unwrap();
                }
            });
        }

        let hosts: Vec<Host> = cpal::available_hosts().into_iter().map(Host).collect();

        let host = cpal::default_host();

        let input_devices: Vec<String> = host
            .input_devices()
            .unwrap()
            .map(|device| device.name().unwrap())
            .collect();

        let output_devices: Vec<String> = host
            .output_devices()
            .unwrap()
            .map(|device| device.name().unwrap())
            .collect();

        let input_device = host.default_input_device();
        let output_device = host.default_output_device();

        (
            Self {
                value: 0,
                receiver: Cell::new(Some(receiver)),
                audio: AudioConfig {
                    host_id: Host(host.id()),
                    hosts,
                    input_devices,
                    output_devices,
                    input_device,
                    output_device,
                    host,
                },
                your_greeting: String::new(),
                name: String::new(),
                latest_greeting: String::new(),
                client,
            },
            Command::none(),
        )
    }

    fn subscription(&self) -> iced::Subscription<Self::Message> {
        channel_subscription(self.receiver.replace(None))
    }

    fn title(&self) -> String {
        format!("Counter: {}", self.value)
    }

    fn update(&mut self, message: Message) -> Command<Message> {
        let audio = &mut self.audio;

        match message {
            Message::IncrementPressed => self.value += 1,
            Message::DecrementPressed => self.value -= 1,
            Message::SelectHost(host_id) => {
                audio.host_id = host_id;
                audio.host = cpal::host_from_id(host_id.0).unwrap();

                audio.input_devices = audio
                    .host
                    .input_devices()
                    .unwrap()
                    .map(|device| device.name().unwrap())
                    .collect();

                audio.output_devices = audio
                    .host
                    .output_devices()
                    .unwrap()
                    .map(|device| device.name().unwrap())
                    .collect();

                audio.input_device = audio.host.default_input_device();
                audio.output_device = audio.host.default_output_device();
            }
            Message::SelectInput(name) => {
                audio.input_device = audio
                    .host
                    .input_devices()
                    .unwrap()
                    .find(|device| device.name().unwrap() == name)
            }
            Message::SelectOutput(name) => {
                audio.output_device = audio
                    .host
                    .input_devices()
                    .unwrap()
                    .find(|device| device.name().unwrap() == name)
            }
            Message::SayHello => {
                let name = self.name.clone();
                let client = self.client.clone();
                let value = self.value;
                return Command::perform(
                    async move {
                        client
                            .lock()
                            .await
                            .say_hello(HelloRequest { name, value })
                            .await
                            .unwrap()
                    },
                    |response| Message::HelloResponse(response.into_inner()),
                );
            }
            Message::HelloResponse(reply) => self.your_greeting = reply.message,
            Message::SetName(name) => self.name = name,
            Message::UpdatesLatestGreeting(greeting) => self.latest_greeting = greeting,
        }

        Command::none()
    }

    fn view(&self) -> Element<Message> {
        let audio = &self.audio;
        column![
            row![
                text_input("name", &self.name, Message::SetName),
                button("say hello").on_press(Message::SayHello),
            ],
            row![
                column![
                    button("Increment").on_press(Message::IncrementPressed),
                    text(self.value).size(50),
                    button("Decrement").on_press(Message::DecrementPressed)
                ]
                .padding(20)
                .align_items(Alignment::Center),
                column![
                    pick_list(&audio.hosts, Some(audio.host_id), Message::SelectHost),
                    pick_list(
                        &audio.input_devices,
                        audio
                            .input_device
                            .as_ref()
                            .map(|device| device.name().unwrap()),
                        Message::SelectInput
                    ),
                    pick_list(
                        &audio.output_devices,
                        audio
                            .output_device
                            .as_ref()
                            .map(|device| device.name().unwrap()),
                        Message::SelectOutput
                    ),
                ]
                .padding(20),
                column![
                    text(format!("your greeting: {}", self.your_greeting)),
                    text(format!("latest greeting: {}", self.latest_greeting))
                ]
            ]
        ]
        .into()
    }

    fn theme(&self) -> iced::Theme {
        iced::Theme::Dark
    }
}

// TODO: this can't be the correct way to do this
fn channel_subscription(receiver: Option<UnboundedReceiver<Message>>) -> Subscription<Message> {
    const ID: &[u8] = "this way of checking equality is stupid".as_bytes();
    if let Some(receiver) = receiver {
        struct Recipe(UnboundedReceiver<Message>);
        impl<H: std::hash::Hasher, Event> iced::subscription::Recipe<H, Event> for Recipe {
            type Output = Message;
            fn hash(&self, state: &mut H) {
                state.write(ID);
            }
            fn stream(self: Box<Self>, _input: BoxStream<Event>) -> BoxStream<Self::Output> {
                struct Streamer(UnboundedReceiver<Message>);
                impl iced::futures::Stream for Streamer {
                    type Item = Message;
                    fn poll_next(
                        mut self: std::pin::Pin<&mut Self>,
                        cx: &mut std::task::Context<'_>,
                    ) -> std::task::Poll<Option<Self::Item>> {
                        self.0.poll_recv(cx)
                    }
                }
                Box::pin(Streamer(self.0))
            }
        }
        iced::Subscription::from_recipe(Recipe(receiver))
    } else {
        struct DummyRecipe;
        impl<H: std::hash::Hasher, Event> iced::subscription::Recipe<H, Event> for DummyRecipe {
            type Output = Message;
            fn hash(&self, state: &mut H) {
                state.write(ID);
            }
            fn stream(self: Box<Self>, _input: BoxStream<Event>) -> BoxStream<Self::Output> {
                struct Streamer;
                impl iced::futures::Stream for Streamer {
                    type Item = Message;
                    fn poll_next(
                        self: std::pin::Pin<&mut Self>,
                        _cx: &mut std::task::Context<'_>,
                    ) -> std::task::Poll<Option<Self::Item>> {
                        unreachable!("this stream should never be polled")
                    }
                }
                Box::pin(Streamer)
            }
        }
        iced::Subscription::from_recipe(DummyRecipe)
    }
}
