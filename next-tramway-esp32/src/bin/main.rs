#![no_std]
#![no_main]

use core::str::FromStr;
use heapless::{String, Vec};
use next_tramway_esp32::{display::{TramDisplay, TramNextPassage, UiCommand, UiState, apply_ui_command}, lcd::{Lcd, LcdRenderer}};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, mutex::Mutex};
use esp_hal::{Blocking, clock::CpuClock, gpio::{self, Input}, i2c::master::I2c, peripherals::TIMG0, time::Rate, timer::timg::{MwdtStage, MwdtStageAction, TimerGroup, Wdt}};
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer, Ticker};
use esp_radio::{
    Controller,
    wifi::{
        ClientConfig,
        ModeConfig,
        ScanConfig,
        WifiController,
        WifiDevice,
        WifiEvent,
        WifiStaState,
    },
};
use esp_alloc::HeapStats;
use embassy_net::{Runner, Stack, StackResources, tcp::TcpSocket};
use defmt::{Debug2Format};
use rust_mqtt::{
    buffer::AllocBuffer, client::{
        Client, event::Event, options::{
            ConnectOptions,
            SubscriptionOptions
        }
    }, config::{
        KeepAlive,
        SessionExpiryInterval
    }, types::{
        MqttBinary, MqttString, TopicName
    }
};
use static_cell::StaticCell;
use embassy_futures::select::{select, Either};

// When you are okay with using a nightly compiler it's better to use https://docs.rs/static_cell/2.1.0/static_cell/macro.make_static.html
macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write($val);
        x
    }};
}

fn str_to_msg(s: &str) -> heapless::String<80> {
    let mut msg = heapless::String::new();
    let _ = msg.push_str(s);
    msg
}


const SSID: &str = env!("SSID");
const PASSWORD: &str = env!("PASSWORD");

const KEEP_ALIVE_SECS: u16 = 12;
const SOCKET_TIMEOUT_SECS: u64 = 30;

const MQTT_HOST: &str = env!("MQTT_HOST");
const MQTT_PORT: &str = env!("MQTT_PORT");
const MQTT_USERNAME: &str = env!("MQTT_USERNAME");
const MQTT_PASSWORD: &str = env!("MQTT_PASSWORD");

const MQTT_CLIENT_ID: &str = env!("MQTT_CLIENT_ID"); 

#[cfg(feature = "debug")]
const DEBUG: bool = true;

#[cfg(not(feature = "debug"))]
const DEBUG: bool = false;

static RX_BUF: StaticCell<[u8; 4096]> = StaticCell::new();
static TX_BUF: StaticCell<[u8; 4096]> = StaticCell::new();


esp_bootloader_esp_idf::esp_app_desc!();

static I2C_BUS: Mutex<CriticalSectionRawMutex, Option<I2c<'static, Blocking>>> =
    Mutex::new(None);

static UI_CH: Channel<CriticalSectionRawMutex,  UiCommand,8> = Channel::new();

    

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    esp_println::println!("\n\n=== PANIC ===");

    if let Some(location) = info.location() {
        esp_println::println!(
            "At {}:{}:{}",
            location.file(),
            location.line(),
            location.column()
        );
    }
    esp_println::println!("Message: {}", info.message());

    loop {}
}

async fn scan_i2c_bus() {
    esp_println::println!("Scanning I2C bus...");

    for addr in 0x08..=0x77 {
        let mut guard = I2C_BUS.lock().await;
        let i2c = guard.as_mut().expect("I2C not initialized");
        esp_println::print!("0x{:02X}...", addr);
        if i2c.write(addr, &[]).is_ok() {
            esp_println::println!("I2C device found at 0x{:02X}", addr);
        }
    }

    esp_println::println!("Scan done.");
}

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 64 * 1024);
    // esp_alloc::heap_allocator!(size: 64 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    

    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);
    let mut wdt = timg0.wdt;
    wdt.set_timeout(MwdtStage::Stage0, esp_hal::time::Duration::from_secs(10));
    wdt.set_stage_action(MwdtStage::Stage0, MwdtStageAction::ResetSystem);
    wdt.enable();
    spawner.spawn(watchdog_task(wdt)).ok();

    esp_println::logger::init_logger_from_env();
    esp_println::println!("Embassy init !");
    let i2c_scl = peripherals.GPIO21;
    let i2c_sda = peripherals.GPIO22;

    let i2c_bus = esp_hal::i2c::master::I2c::new(
        peripherals.I2C0,
        esp_hal::i2c::master::Config::default().with_frequency(Rate::from_khz(400)),
    )
        .unwrap()
        .with_scl(i2c_scl)
        .with_sda(i2c_sda);

    I2C_BUS.lock().await.replace(i2c_bus);
    esp_println::println!("I2C Bus init !");
    UI_CH.send(UiCommand::UpdateMessage(str_to_msg("I2C Bus initialized"))).await;
    if DEBUG {
        scan_i2c_bus().await;
    }

    let esp_radio_ctrl = &*mk_static!(Controller<'static>, esp_radio::init().unwrap());
    esp_println::println!("radio controlller init !");
    UI_CH.send(UiCommand::UpdateMessage(str_to_msg("radio controlller init !"))).await;

    let (controller, interfaces) =
        esp_radio::wifi::new(esp_radio_ctrl, peripherals.WIFI, Default::default()).unwrap();
    esp_println::println!("Wifi controlller init !");
    UI_CH.send(UiCommand::UpdateMessage(str_to_msg("Wifi controlller init !"))).await;

    let wifi_interface = interfaces.sta;

    let config = embassy_net::Config::dhcpv4(Default::default());

    let rng = esp_hal::rng::Rng::new();
    let seed = (rng.random() as u64) << 32 | rng.random() as u64;

    // Init network stack
    let (stack, runner) = embassy_net::new(
        wifi_interface,
        config,
        mk_static!(StackResources<8>, StackResources::<8>::new()),
        seed,
    );

    spawner.spawn(connection(controller)).ok();
    spawner.spawn(net_task(runner)).ok();
    spawner.spawn(mqtt(stack)).ok();

    let lcd = Lcd::new(&I2C_BUS, next_tramway_esp32::lcd::LcdGeometry::L2004);
    lcd.init().await;
    spawner.spawn(renderer(LcdRenderer::new(lcd))).ok();

    let button = Input::new(peripherals.GPIO11, gpio::InputConfig::default()
    .with_pull(gpio::Pull::Up));
    spawner.spawn(button_task(button)).ok();

    let stats: HeapStats = esp_alloc::HEAP.stats();
    esp_println::println!("{}", stats);

    
    Timer::after(Duration::from_millis(50)).await;
}



#[embassy_executor::task]
async fn renderer(mut display: LcdRenderer<'static>) {

    let mut state = UiState {
        lines: heapless::Vec::new(),
        current_message: None,
        current_line: 0,
        current_direction_id: 0
    };
    esp_println::println!("Renderer ready !");
    loop {
        let cmd = UI_CH.receive().await;
        esp_println::println!("Applying ui_command");
        apply_ui_command(&mut state, cmd);
        display.render(&state).await;
    }
}

#[embassy_executor::task]
async fn connection(mut controller: WifiController<'static>) {
    esp_println::println!("start connection task");
    esp_println::println!("Device capabilities: {:?}", controller.capabilities());
    esp_println::println!("{SSID}");

    loop {
        if esp_radio::wifi::sta_state() == WifiStaState::Connected {
            // wait until we're no longer connected
            controller.wait_for_event(WifiEvent::StaDisconnected).await;
            esp_println::println!("Disconnected");
            Timer::after(Duration::from_millis(5000)).await
        }
        if !matches!(controller.is_started(), Ok(true)) {
            let client_config = ModeConfig::Client(
                ClientConfig::default()
                    .with_ssid(SSID.into())
                    .with_password(PASSWORD.into()),
            );
            controller.set_config(&client_config).unwrap();
            esp_println::println!("Starting wifi");
            controller.start_async().await.unwrap();
            esp_println::println!("Wifi started!");

            if DEBUG {
                esp_println::println!("Scan");
                UI_CH.send(UiCommand::UpdateMessage(str_to_msg("Scanning wifi..."))).await;
                let scan_config = ScanConfig::default().with_max(1).with_ssid(SSID);
                let result = controller
                    .scan_with_config_async(scan_config)
                    .await
                    .unwrap();
                for ap in result {
                    esp_println::println!("{:?}", ap);
                }
            }
        }
        esp_println::println!("About to connect...");
        UI_CH.send(UiCommand::UpdateMessage(str_to_msg("About to connect..."))).await;
    let stats: HeapStats = esp_alloc::HEAP.stats();
    esp_println::println!("{}", stats);

        match controller.connect_async().await {
            Ok(_) => { 
                esp_println::println!("Wifi connected!");
                UI_CH.send(UiCommand::UpdateMessage(str_to_msg("Wifi connected !"))).await;
            },
            Err(e) => {
                esp_println::println!("Failed to connect to wifi: {e:?}");
                Timer::after(Duration::from_millis(500)).await
            }
        }
    }
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, WifiDevice<'static>>) {
    runner.run().await
}

async fn wait_for_ip(stack: Stack<'_>) {
    esp_println::println!("Waiting to get IP address...");
    UI_CH.send(UiCommand::UpdateMessage(str_to_msg("Waiting to get IP address..."))).await;
    loop {
        if let Some(config) = stack.config_v4() {
            esp_println::println!("Got IP: {}", config.address);
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }
}

async fn wait_for_network(stack: Stack<'_>) {
    loop {
        if stack.is_link_up() {
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }
}

async fn mqtt_connect<'a>(stack: Stack<'a>, mqtt_buffer: &'a mut AllocBuffer, rx: &'a mut [u8; 4096], tx:&'a mut [u8; 4096]) -> Option<Client<'a, TcpSocket<'a>, AllocBuffer, 1, 1, 1>> {
    esp_println::println!("Connecting to socket...");
    let mut socket = TcpSocket::new(stack, rx, tx);
    socket.set_timeout(Some(embassy_time::Duration::from_secs(SOCKET_TIMEOUT_SECS)));
    loop {
        let port: u16 = MQTT_PORT.parse().expect("Couldn't parse MQTT_PORT as u16");
        let address = embassy_net::IpAddress::from_str(MQTT_HOST).expect("Invalid IPv4 address");
        let remote_endpoint = (address, port);

        if let Err(e) = socket.connect(remote_endpoint).await {
            esp_println::println!("Connection error : {:?}", Debug2Format(&e));
            // could use an exponential backoff here
            Timer::after(Duration::from_secs(2)).await;
            continue;
        }
        esp_println::println!("connected");
        break;
    } 

    esp_println::println!("Connecting to MQTT server...");


    let mut mqtt_client = rust_mqtt::client::Client::<'_, _, _, 1, 1, 1>::new(mqtt_buffer);
    let connect_options = ConnectOptions { 
        clean_start: true, 
        keep_alive: KeepAlive::Seconds(KEEP_ALIVE_SECS), 
        session_expiry_interval: SessionExpiryInterval::EndOnDisconnect, 
        user_name: Some(MqttString::try_from(MQTT_USERNAME).unwrap()), 
        password: Some(MqttBinary::try_from(MQTT_PASSWORD).unwrap()), 
        will: None 
    };
    match mqtt_client.connect(socket, &connect_options, Some(MqttString::try_from(MQTT_CLIENT_ID).unwrap())).await {
        Ok(c) => {
            esp_println::println!("Connected to server: {:?}", c);
            UI_CH.send(UiCommand::UpdateMessage(str_to_msg("Connected to MQTT server !"))).await;
            esp_println::println!("{:?}", mqtt_client.client_config());
            esp_println::println!("{:?}", mqtt_client.server_config());
            esp_println::println!("{:?}", mqtt_client.shared_config());
            esp_println::println!("{:?}", mqtt_client.session());
        },
        Err(e) => {
            esp_println::println!("Failed to connect to server {:?}", e);
            UI_CH.send(UiCommand::UpdateMessage(str_to_msg("Failed to connect to MQTT server !"))).await;
            // could use an exponential backoff here
            Timer::after(Duration::from_secs(2)).await;
        },
    }
    let sub_options = SubscriptionOptions {
        retain_handling: rust_mqtt::client::options::RetainHandling::SendIfNotSubscribedBefore, 
        retain_as_published: true, 
        no_local: true, 
        qos: rust_mqtt::types::QoS::ExactlyOnce 

    };
    let s = MqttString::from_slice("next-tramway/line/#").unwrap();
    let topic = unsafe {
        TopicName::new_unchecked(s)
    };
    match mqtt_client.subscribe(topic.into(), sub_options).await {
        Ok(_) => esp_println::println!("Successfully subscribed !"),
        Err(e) => {
            esp_println::println!("Failed to subscribe: {:?}", e);
            return None
        }
    };
    Some(mqtt_client)
}

#[embassy_executor::task]
async fn mqtt(stack: embassy_net::Stack<'static>) {
    let rx = RX_BUF.init([0; 4096]);
    let tx = TX_BUF.init([0; 4096]);
    
    loop {
        wait_for_network(stack).await;
        wait_for_ip(stack).await;
        let mut mqtt_buffer = rust_mqtt::buffer::AllocBuffer;
        let mut mqtt_client = match mqtt_connect(stack, &mut mqtt_buffer, rx, tx).await {
            Some(c) => c,
            None => {
                Timer::after(Duration::from_secs(2)).await;
                continue;
            }
        };


        let mut ticker = Ticker::every(Duration::from_secs(KEEP_ALIVE_SECS as u64 / 2));
        // loop MQTT
        loop {
            match select(mqtt_client.poll(), ticker.next()).await {
                Either::First(res) => {
                    match res {
                        Ok(event) => handle_mqtt_event(event).await,
                        Err(e) => {
                            esp_println::println!("MQTT error: {:?}", e);
                            break;
                        }
                    }
                },
                Either::Second(_) => {
                    if mqtt_client.ping().await.is_err() {
                        esp_println::println!("Ping failed");
                        break;
                    }
                }
            }
        }
        esp_println::println!("Connection to MQTT server lost...");
        // could use an exponential backoff here
        Timer::after(Duration::from_secs(2)).await;
    }
}

async fn handle_mqtt_event(event: Event<'_>) {
    let Event::Publish(p) = event else { return };
    if let Ok(text) = core::str::from_utf8(p.message.as_ref()) {
        if let Some(cmd) = parse_mqtt_event(&p.topic, text) {
            UI_CH.send(cmd).await;
        } else {
            esp_println::println!("Failed to parse MQTT event: {:?}", p);
        }
    }
}

fn parse_mqtt_event(topic: &MqttString, text: &str) -> Option<UiCommand> {
    let mut parts = topic.as_ref().rsplit('/');
    
    if let (Some(direction_id), Some(_)) = (parts.next(), parts.next()) {
        let mut next_passages: heapless::Vec<TramNextPassage, 3> = Vec::new();
        let mut text_split_iter = text.split('\n');
        let mut line: String<16> = heapless::String::new();
        let _ = line.push_str(text_split_iter.next()?);
        if let Some(update_at) = text_split_iter.next_back() {
            for passage in text_split_iter {
                let mut destination_buffer: String<32> = heapless::String::new();
                let mut passage_parts = passage.split("|");
                if let (Some(destination), Some(relative_arrival), Some(_)) = (passage_parts.next(), passage_parts.next(), passage_parts.next()) {
                    let _ = destination_buffer.push_str(destination);
                    let _ = next_passages.push(TramNextPassage {
                        destination: destination_buffer,
                        relative_arrival: match relative_arrival.parse() {
                            Ok(value) => value,
                            Err(_) => {
                                esp_println::println!("Failed to parse relative_arrival: {}", relative_arrival);
                                return None;
                            }
                        }
                    });
                }
            }
            let mut update_at_buffer:  String<10> = heapless::String::new();
            let _ = update_at_buffer.push_str(update_at);

            let direction_id = match direction_id.parse() {
                Ok(id) => id,
                Err(_) => {
                    esp_println::println!("Failed to parse direction_id: {}", direction_id);
                    return None;
                }
            };

            let cmd = UiCommand::UpdateDirection { 
                line, 
                direction_id, 
                next_passages, 
                update_at: update_at_buffer 
            };
            esp_println::println!("{:?}", cmd);
            return Some(cmd)
        }
        

    }
    None
}

#[embassy_executor::task]
async fn button_task(mut button: Input<'static>) {
    loop {
        button.wait_for_falling_edge().await;

        Timer::after(Duration::from_millis(50)).await;

        if button.is_low() {
            esp_println::println!("BOUTON");
            UI_CH.send(UiCommand::NextScreen).await;
        }

        button.wait_for_rising_edge().await;
    }
}

#[embassy_executor::task]
async fn watchdog_task(mut wdt: Wdt<TIMG0<'static>>) {
    let mut ticker = Ticker::every(Duration::from_secs(2));

    loop {
        ticker.next().await;
        wdt.feed();
    }
}