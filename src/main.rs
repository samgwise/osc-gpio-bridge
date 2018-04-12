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

// The GPIO module uses BCM pin numbering. BCM 18 equates to physical pin 12.
const GPIO_LED: u8 = 18;
const GPIO_IN: u8 = 17;

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

    // Setup socket
    let socket = UdpSocket::bind(host_address).expect( format!("Unable to provision socket: {}", host_address).as_str() );
    // println!("Listening on {}...", host_address);

    // Start talking to the GPIO
    let device_info = DeviceInfo::new().expect("Unable to obtain device info");
    println!( "Model: {} (SoC: {})", device_info.model(), device_info.soc() );

    let mut gpio = Gpio::new().expect("Error intitalising GPIO interface");
    gpio.set_mode(GPIO_LED, Mode::Output);
    gpio.set_mode(GPIO_IN, Mode::Input);

    let mut last_in_level = Level::Low;
    loop {
        gpio.write(GPIO_LED, Level::High);
        thread::sleep( Duration::from_millis(500) );
        gpio.write(GPIO_LED, Level::Low);
        last_in_level = match gpio.read(GPIO_IN) {
            Ok (level)  => level,
            Err (e)     => {
                println!("Failed to read from pin {}, reason: {:?}", GPIO_IN, e);
                last_in_level
            }
        };
        println!("Last recorded level on pin {}: {:?}", GPIO_IN, last_in_level);
        let msg_buf = encoder::encode(&OscPacket::Message(OscMessage {
              addr: format!("/gpio/{}", GPIO_IN).to_string(),
              args: Some( vec![OscType::Bool( level_to_bool(&last_in_level) )] ),
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

fn level_to_bool(level: &Level) -> bool {
    match level {
        &Level::Low  => false,
        &Level::High   => true,
    }
}
