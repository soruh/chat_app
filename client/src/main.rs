use std::{
    io::Write,
    net::{TcpStream, UdpSocket},
    sync::{atomic::AtomicUsize, Arc},
    time::Duration,
};

use anyhow::Context;
use clap::arg;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use iced::{Application, Settings};
use ringbuf::HeapRb;

use crate::gui::Gui;

mod gui;

const PORT: u16 = 8594;

#[derive(Debug)]
struct Opt {
    gain: i32,
    jack: bool,
    fixed: bool,
    latency: f32,
    input_device: String,
    output_device: String,
}

impl Opt {
    fn from_args() -> anyhow::Result<Self> {
        let mut app = clap::Command::new("test app")
            .arg(arg!(-l --latency [DELAY_MS] "Specify the delay between input and output [default: 150]"))
            .arg(arg!(-g --gain [GAIN] "Specify the gain in Q8 dB [default: 3000]"))
            .arg(arg!(-f --fixed "Use a fixed buffer size"))
            .arg(arg!([IN] "The input audio device to use"))
            .arg(arg!([OUT] "The output audio device to use"));

        #[cfg(target_os = "linux")]
        {
            app = app.arg(arg!(-j --jack "Use Jack [default: false]"));
        }

        let matches = app.get_matches();
        let latency: f32 = matches
            .value_of("latency")
            .unwrap_or("150")
            .parse()
            .context("parsing latency option")?;
        let input_device = matches.value_of("IN").unwrap_or("default").to_string();
        let output_device = matches.value_of("OUT").unwrap_or("default").to_string();
        let gain = matches
            .value_of("gain")
            .unwrap_or("3000")
            .to_string()
            .parse()
            .unwrap();

        let jack = matches.contains_id("jack");

        let fixed = matches.contains_id("fixed");

        Ok(Opt {
            latency,
            jack,
            fixed,
            gain,
            input_device,
            output_device,
        })
    }
}

fn main() {
    Gui::run(Settings::default()).unwrap();
}

fn _main() -> anyhow::Result<()> {
    let opt = Opt::from_args()?;

    #[cfg(target_os = "linux")]
    let host = if opt.jack {
        cpal::host_from_id(
            cpal::available_hosts()
                .into_iter()
                .find(|id| *id == cpal::HostId::Jack)
                .unwrap(),
        )
        .expect("jack host unavailable")
    } else {
        cpal::default_host()
    };

    #[cfg(not(target_os = "linux"))]
    let host = cpal::default_host();

    // Find devices.
    let input_device = if opt.input_device == "default" {
        host.default_input_device()
    } else {
        host.input_devices()?
            .find(|x| x.name().map(|y| y == opt.input_device).unwrap_or(false))
    }
    .expect("failed to find input device");

    let output_device = if opt.output_device == "default" {
        host.default_output_device()
    } else {
        host.output_devices()?
            .find(|x| x.name().map(|y| y == opt.output_device).unwrap_or(false))
    }
    .expect("failed to find output device");

    println!("Using input device: \"{}\"", input_device.name()?);
    println!("Using output device: \"{}\"", output_device.name()?);

    // We'll try and use the same configuration between streams to keep it simple.
    let mut config: cpal::StreamConfig = input_device.default_input_config()?.into();

    let sample_rate = config.sample_rate.0;

    let _supported_input = input_device
        .supported_input_configs()
        .unwrap()
        .collect::<Vec<_>>();

    let _supported_output = input_device
        .supported_input_configs()
        .unwrap()
        .collect::<Vec<_>>();

    // dbg!(_supported_input, _supported_output);

    // Create a delay in case the input and output devices aren't synced.
    let latency_frames = ((opt.latency / 1000.) * sample_rate as f32) as u32 * 2;
    // let latency_frames = latency_frames.max(sample_rate / 5);
    let latency_samples = latency_frames as usize * config.channels as usize;

    if opt.fixed {
        config.buffer_size = cpal::BufferSize::Fixed(latency_frames);
    } else {
        config.buffer_size = cpal::BufferSize::Default;
    }

    let package_buffer_size = sample_rate as usize * 40 / 1000;

    // The buffer to share samples
    let ring_a = HeapRb::<f32>::new(2 * package_buffer_size);
    let (mut producer_a, mut consumer_a) = ring_a.split();

    // producer_a.push_iter(&mut std::iter::repeat(0.).take(producer_a.capacity()));

    let ring_b = HeapRb::<f32>::new(2 * package_buffer_size);
    let (mut producer_b, mut consumer_b) = ring_b.split();

    // producer_b.push_iter(&mut std::iter::repeat(0.).take(producer_b.capacity()));

    let loss = 0.0;

    let tcp_socket = TcpStream::connect(("h.glsys.de", PORT)).unwrap();

    let socket = Arc::new(UdpSocket::bind((
        "0.0.0.0",
        tcp_socket.local_addr().unwrap().port(),
    ))?);
    socket.connect(("h.glsys.de", PORT)).unwrap();

    let s = socket.clone();
    std::thread::spawn(move || {
        let socket = s;

        let mut input = vec![0_f32; package_buffer_size];
        let mut output = vec![0_u8; package_buffer_size * std::mem::size_of::<i16>()];

        let mut encoder = audiopus::coder::Encoder::new(
            audiopus::SampleRate::Hz48000,
            audiopus::Channels::Mono,
            audiopus::Application::Audio,
        )
        .unwrap();

        // encoder.set_max_bandwidth(audiopus::Bandwidth::Superwideband).unwrap();
        // encoder.set_packet_loss_perc((loss * 100.) as u8).unwrap();
        // encoder.set_inband_fec(true).unwrap();
        // encoder.set_bitrate(audiopus::Bitrate::BitsPerSecond(16_000)).unwrap();

        let bitrate_cap = 100_000_000;

        loop {
            while consumer_a.len() < package_buffer_size {
                // println!("{} < {package_buffer_size}", consumer_a.len());
                std::thread::sleep(Duration::from_millis(1));
            }

            let n_input = consumer_a.pop_slice(&mut input);
            let input = &input[..n_input];

            let n_output = encoder.encode_float(input, &mut output).unwrap();
            let output = &output[..n_output];

            let delay = Duration::from_secs_f64(n_output as f64 * 8. / bitrate_cap as f64);

            println!("package size: {n_output} ; delay: {delay:?}");

            std::thread::sleep(delay);

            if let Err(err) = socket.send(output) {
                println!("failed to send datagram: {err}");
            }
        }
    });

    std::thread::spawn(move || {
        let mut input = vec![0_u8; package_buffer_size * std::mem::size_of::<f32>()];
        let mut output = vec![0_f32; package_buffer_size * 2];

        let mut decoder =
            audiopus::coder::Decoder::new(audiopus::SampleRate::Hz48000, audiopus::Channels::Mono)
                .unwrap();

        decoder.set_gain(opt.gain).unwrap();

        loop {
            let n_input = socket.recv(&mut input).unwrap();
            let input = &input[..n_input];

            let data = (rand::random::<f32>() >= loss).then_some(&*input);

            let n_output = decoder.decode_float(data, &mut output, false).unwrap();
            let output = &output[..n_output];

            producer_b.push_slice(output);
        }
    });

    let input_data_fn = move |data: &[f32], _: &cpal::InputCallbackInfo| {
        let mut output_fell_behind = false;
        for &sample in data {
            if producer_a.push(sample).is_err() {
                output_fell_behind = true;
            }
        }
        if output_fell_behind {
            eprintln!("output stream fell behind: try increasing latency");
        }
    };

    let buffer_fill = Arc::new(AtomicUsize::new(0));

    let status = buffer_fill.clone();
    let output_data_fn = move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
        let mut fell_behind_at = None;

        for (i, sample) in data.chunks_mut(2).enumerate() {
            if let Some(data) = consumer_b.pop() {
                sample[0] = data;
                sample[1] = data;
            } else {
                fell_behind_at = Some(i);
                break;
            }
        }

        if let Some(i) = fell_behind_at {
            let missing_samples = data.len() / 2 - i;
            eprintln!(
                "input stream fell behind by {} samples ({:?}): try increasing latency",
                missing_samples,
                Duration::from_micros(missing_samples as u64 * 1_000_000 / sample_rate as u64)
            );
        }

        status.store(consumer_b.len(), std::sync::atomic::Ordering::Relaxed);
    };

    std::thread::spawn(move || loop {
        return;

        print!(
            "\x1b[Kbuffer size: {}\r",
            buffer_fill.load(std::sync::atomic::Ordering::Relaxed)
        );
        let _ = std::io::stdout().flush();

        std::thread::sleep(Duration::from_millis(100));
    });

    // Build streams.
    println!(
        "Attempting to build streams with {} Samples, sample rate: {:?} and buffer size: {:?}.",
        std::any::type_name::<f32>(),
        config.sample_rate,
        config.buffer_size
    );

    config.channels = 1;
    let input_stream = input_device.build_input_stream(&config, input_data_fn, err_fn, None)?;

    config.channels = 2;
    let output_stream = output_device.build_output_stream(&config, output_data_fn, err_fn, None)?;
    println!("Successfully built streams.");

    // Play the streams.
    println!(
        "Starting the input and output streams with `{}` milliseconds of latency.",
        opt.latency
    );
    input_stream.play()?;
    output_stream.play()?;

    let duration = Duration::from_secs(60 * 60);

    // Run for 30 seconds before closing.
    println!("Playing for {duration:?}... ");
    std::thread::sleep(duration);
    drop(input_stream);
    drop(output_stream);
    println!("Done!");
    Ok(())
}

fn err_fn(err: cpal::StreamError) {
    eprintln!("an error occurred on stream: {}", err);
}
