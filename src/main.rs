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

#[derive(Debug, PartialEq, Clone, Copy)]
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
    let host_address_in = match SocketAddrV4::from_str( format!("{}:{}", config.host, config.port_in).as_str() ) {
        Ok(addr)    => addr,
        Err(_)      => {
            println!("Unable to use host and port config fields ('{}:{}') as an address!", config.host, config.port_in);
            ::std::process::exit(1)
        }
    };
    // try bulding sender address
    let host_address_out = match SocketAddrV4::from_str( format!("{}:{}", config.host, config.port_out).as_str() ) {
        Ok(addr)    => addr,
        Err(_)      => {
            println!("Unable to use host and port config fields ('{}:{}') as an address!", config.host, config.port_out);
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

    let mut gpio_in = Gpio::new().expect("Error intitalising GPIO interface");

    // Setup our GPIO and pin states
    let mut pins_readable   :Vec<PinState> = Vec::new();
    let mut pins_writeable  :Vec<PinState> = Vec::new();
    for pin in &config.pins {
        let state = PinState { state: bool_to_level(&pin.state), pin: pin.pin };
        match pin.io {
            config::PinIO::Writeable => {
                // gpio.set_mode(pin.pin, Mode::Output); //set this later in the writer thread
                pins_writeable.push(state)
            },
            config::PinIO::Readable  => {
                gpio_in.set_mode(pin.pin, Mode::Input);
                pins_readable.push(state)
            },
        }
    }

    // Setup sockets
    let socket_in  = UdpSocket::bind(host_address_in).expect( format!("Unable to provision socket: {}", host_address_in).as_str() );
    let socket_out = UdpSocket::bind(host_address_out).expect( format!("Unable to provision socket: {}", host_address_out).as_str() );
    println!("Listening on {}...", host_address_in);

    // setup listener thread
    let _listener = thread::spawn(move || {
        let mut gpio_out = Gpio::new().expect("Error intitalising GPIO interface");
        for ouput in &pins_writeable {
            gpio_out.set_mode(ouput.pin, Mode::Output);
        }

        let mut packet_buffer = [0u8; rosc::decoder::MTU];
        loop {
            match socket_in.recv_from(&mut packet_buffer) {
                Ok((size, _address)) => {
                    match rosc::decoder::decode(&packet_buffer[..size]) {
                        Ok (p)  => {
                            match packet_to_message(p).and_then(gpio_message) {
                                Ok(new_state) => {
                                    for out in &pins_writeable {
                                        if out.pin == new_state.pin {
                                            // set gpio state
                                            gpio_out.write(out.pin, new_state.state)
                                        }
                                    }
                                },
                                Err(e) => {
                                    println!("Unexpected message: {:?}", e);
                                }
                            }
                        },
                        Err (e) => {
                            println!("Error unpacking OSC packet: {:?}", e);
                        }
                    }
                },
                Err(e) => {
                    println!("Error recieving OSC message: {}", e);
                }
            }
        }
    });

    // Check for and publish changes to listeners
    loop {
        thread::sleep( Duration::from_millis(config.poll_ms) );

        // for pin in &pins_writeable {
        //     gpio.write(pin.pin, pin.state)
        // }

        for pin in &mut pins_readable {
            let current_level = match gpio_in.read(pin.pin) {
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
                      addr: format!("/gpio/pin/{}", pin.pin).to_string(),
                      args: Some( vec![OscType::Bool( level_to_bool(&current_level) )] ),
                  })).unwrap();

                for addr in &client_addrs {
                    match socket_out.send_to(&msg_buf, addr) {
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

// OSC handling

fn packet_to_message(packet: OscPacket) -> Result<OscMessage, &'static str> {
    match packet {
        OscPacket::Message(msg) => Ok(msg),
        OscPacket::Bundle(_)    => Err("Expected OSC message but ecieved OSC bundle!"),
    }
}

fn gpio_message(message: OscMessage) -> Result<PinState, &'static str> {
    assert_gpio_message_path(&message)
        .and_then(gpio_message_to_state)
}

fn assert_gpio_message_path(message: &OscMessage) -> Result<&OscMessage, &'static str> {
    if message.addr == "/gpio/write" {
        Ok(message)
    }
    else {
        Err("Expected message with path: /gpio/write")
    }
}

fn gpio_message_to_state(message: &OscMessage) -> Result<PinState, &'static str> {
    match message.args {
        Some(ref params) => {
            let mut params = params.iter();
            let maybe_pin   = params.next().and_then(u8_from_osc);
            let maybe_level = params.next().and_then(bool_from_osc);

            if maybe_pin.is_some() && maybe_level.is_some() {
                Ok(PinState { pin: maybe_pin.unwrap(), state: bool_to_level(&maybe_level.unwrap()) })
            }
            else {
                Err("Expected args <u8, bool> for /gpio/write.")
            }
        },
        None => Err("No args in message, expected <u8, bool> for /gpio/write")
    }
}

fn u8_from_osc(osc_arg: &OscType) -> Option<u8> {
    match osc_arg {
        &OscType::Int(int32)    => Some(int32 as u8),
        &OscType::Char(int8)    => Some(int8 as u8),
        _                       => None
    }
}

fn bool_from_osc(osc_arg: &OscType) -> Option<bool> {
    match osc_arg {
        &OscType::Bool(boolean) => Some(boolean),
        _                       => None
    }
}
