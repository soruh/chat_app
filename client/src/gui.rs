use std::cell::Cell;
use std::fmt::Display;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{Device, HostId};
use iced::futures::stream::BoxStream;
use iced::widget::{button, column, pick_list, row, text};
use iced::{Alignment, Application, Command, Element, Subscription};

use tokio::sync::mpsc::UnboundedReceiver;

pub struct Gui {
    value: i32,
    receiver: Cell<Option<UnboundedReceiver<Message>>>,
    audio: AudioConfig,
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
}

impl Application for Gui {
    type Message = Message;
    type Executor = iced::executor::Default;
    type Theme = iced::theme::Theme;
    type Flags = ();

    fn new(_flags: ()) -> (Self, Command<Message>) {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();

        tokio::task::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                sender.send(Message::DecrementPressed).unwrap();
            }
        });

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
        }

        Command::none()
    }

    fn view(&self) -> Element<Message> {
        let audio = &self.audio;
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
