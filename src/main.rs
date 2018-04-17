#[macro_use]
extern crate serde_derive;
extern crate serde_yaml;

extern crate rosc;
use rosc::{OscPacket, OscMessage, OscType};
use rosc::encoder;


extern crate rppal;
use rppal::gpio::{Gpio, Mode, Level};
use rppal::system::DeviceInfo;

use std::thread;
use std::time::Duration;
use std::env;
use std::net::{UdpSocket, SocketAddrV4};
use std::str::FromStr;

mod config;

pub struct PinState {
    state:  Level,
    pin:    u8,
}

fn main() {
    // Handle args
    let args: Vec<String> = env::args().collect();
    let usage = format!("Usage {} [osc-to-gpio-config.yml]", &args[0]);

    let config_file_name =
        if args.len() > 2 {
            // Too many arguments
            println!("{}", usage);
            ::std::process::exit(1)
        }
        else if args.len() == 2 {
            // custom configuration file
            format!("{}", args[1])
        }
        else {
            // Default value
            String::from("osc-to-gpio-config.yml")
        };


    // Read config
    let config = config::load_from_file(&config_file_name).expect("Unable to continue without valid configuration");
    println!("config: {:?}", config);

    // try bulding listening address
    let host_address = match SocketAddrV4::from_str( format!("{}:{}", config.host, config.port).as_str() ) {
        Ok(addr)    => addr,
        Err(_)      => {
            println!("Unable to use host and port config fields ('{}:{}') as an address!", config.host, config.port);
            ::std::process::exit(1)
        }
    };

    let mut client_addrs: Vec<SocketAddrV4> = Vec::with_capacity( config.clients.len() );
    for client in &config.clients {
        let address = match SocketAddrV4::from_str( format!("{}:{}", client.host, client.port).as_str() ) {
            Ok(addr)    => addr,
            Err(_)      => {
                println!("Unable to use host and port client fields ('{}:{}') as an address!", client.host, client.port);
                ::std::process::exit(1)
            }
        };
        client_addrs.push(address);
    }

    // Start talking to the GPIO
    let device_info = DeviceInfo::new().expect("Unable to obtain device info");
    println!( "Model: {} (SoC: {})", device_info.model(), device_info.soc() );

    let mut gpio = Gpio::new().expect("Error intitalising GPIO interface");

    // Setup our GPIO and pin states
    let mut pins_readable   :Vec<PinState> = Vec::new();
    let mut pins_writeable  :Vec<PinState> = Vec::new();
    for pin in &config.pins {
        let state = PinState { state: bool_to_level(&pin.state), pin: pin.pin };
        match pin.io {
            config::PinIO::Writeable => {
                gpio.set_mode(pin.pin, Mode::Output);
                pins_writeable.push(state)
            },
            config::PinIO::Readable  => {
                gpio.set_mode(pin.pin, Mode::Input);
                pins_readable.push(state)
            },
        }
    }

    // Setup socket
    let socket = UdpSocket::bind(host_address).expect( format!("Unable to provision socket: {}", host_address).as_str() );
    // println!("Listening on {}...", host_address);


    loop {
        thread::sleep( Duration::from_millis(config.poll_ms) );

        for pin in &pins_writeable {
            gpio.write(pin.pin, pin.state)
        }

        for pin in &mut pins_readable {
            let current_level = match gpio.read(pin.pin) {
                Ok (level)  => level,
                Err (e)     => {
                    println!("Failed to read from pin {}, reason: {:?}", pin.pin, e);
                    // Return last level instead to keep us rolling
                    pin.state
                }
            };

            if pin.state != current_level {
                println!("Change in level on pin {}: {:?} => {:?}", pin.pin, pin.state, current_level);
                pin.state = current_level;

                let msg_buf = encoder::encode(&OscPacket::Message(OscMessage {
                      addr: format!("/gpio/{}", pin.pin).to_string(),
                      args: Some( vec![OscType::Bool( level_to_bool(&current_level) )] ),
                  })).unwrap();

                for addr in &client_addrs {
                    match socket.send_to(&msg_buf, addr) {
                        Ok (_) => (),
                        Err (e) => {
                            println!("Error sending to client: {}, reason: {}", addr, e);
                            ()
                        }
                    }
                }
            }
        }
    }
}

fn level_to_bool(level: &Level) -> bool {
    match level {
        &Level::Low  => false,
        &Level::High => true,
    }
}

fn bool_to_level(level: &bool) -> Level {
    match level {
        &false => Level::Low,
        &true  => Level::High,
    }
}
