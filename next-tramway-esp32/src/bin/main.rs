#![no_std]
#![no_main]

use core::str::FromStr;
use next_tramway_esp32::lcd::Lcd;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use esp_hal::{ram, Blocking, clock::CpuClock, i2c::master::I2c, time::Rate, timer::timg::TimerGroup};
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
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
use embassy_net::{Runner, StackResources, tcp::TcpSocket};
use defmt::{Debug2Format};
use rust_mqtt::{
    client::{
        Client,
        options::{
            ConnectOptions
        }
    },
    config::{
        KeepAlive,
        SessionExpiryInterval
    },
    types::{
        MqttString,
        MqttBinary
    }
};
use static_cell::StaticCell;

// When you are okay with using a nightly compiler it's better to use https://docs.rs/static_cell/2.1.0/static_cell/macro.make_static.html
macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
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

static RX_BUF: StaticCell<[u8; 4096]> = StaticCell::new();
static TX_BUF: StaticCell<[u8; 4096]> = StaticCell::new();


esp_bootloader_esp_idf::esp_app_desc!();

static I2C_BUS: Mutex<CriticalSectionRawMutex, Option<I2c<'static, Blocking>>> =
    Mutex::new(None);

    

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

async fn scan_i2c_bus() {
    esp_println::println!("Waiting for i2c mutex...");
    let mut guard = I2C_BUS.lock().await;
    let i2c = guard.as_mut().expect("I2C not initialized");
    esp_println::println!("Scanning I2C bus...");

    for addr in 0x08..=0x77 {
        let result = i2c.write(addr, &[]);

        if result.is_ok() {
            esp_println::println!("I2C device found at 0x{:02X}", addr);
        }
    }

    esp_println::println!("Scan done.");
}




#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    
    // esp_alloc::heap_allocator!(#[ram(reclaimed)] size: 96 * 1024);
    esp_alloc::heap_allocator!(size: 64 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    

    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    esp_println::logger::init_logger_from_env();
    esp_println::println!("Embassy init !");

    let i2c_bus = esp_hal::i2c::master::I2c::new(
        peripherals.I2C0,
        esp_hal::i2c::master::Config::default().with_frequency(Rate::from_khz(100)),
    )
        .unwrap()
        .with_scl(peripherals.GPIO21)
        .with_sda(peripherals.GPIO22);

    I2C_BUS.lock().await.replace(i2c_bus);
    esp_println::println!("I2C Bus init !");


    let esp_radio_ctrl = &*mk_static!(Controller<'static>, esp_radio::init().unwrap());
    esp_println::println!("radio controlller init !");

    let (controller, interfaces) =
        esp_radio::wifi::new(&esp_radio_ctrl, peripherals.WIFI, Default::default()).unwrap();
    esp_println::println!("Wifi controlller init !");



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

    Timer::after(Duration::from_secs(1)).await;

    spawner.spawn(connection(controller)).ok();
    spawner.spawn(net_task(runner)).ok();

    let stats: HeapStats = esp_alloc::HEAP.stats();
    esp_println::println!("{}", stats);

    loop {
        if stack.is_link_up() {
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }

    esp_println::println!("Waiting to get IP address...");
    loop {
        if let Some(config) = stack.config_v4() {
            esp_println::println!("Got IP: {}", config.address);
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }


    let rx = RX_BUF.init([0; 4096]);
    let tx = TX_BUF.init([0; 4096]);
    
    esp_println::println!("Connecting to socket...");
    let mut socket = TcpSocket::new(stack, rx, tx);
    socket.set_timeout(Some(embassy_time::Duration::from_secs(SOCKET_TIMEOUT_SECS)));
    loop {
        let port: u16 = MQTT_PORT.parse().expect("Couldn't parse MQTT_PORT as u16");
        let address = embassy_net::IpAddress::from_str(MQTT_HOST).expect("Invalid IPv4 address");
        let remote_endpoint = (address, port);

        if let Err(e) = socket.connect(remote_endpoint).await {
            esp_println::println!("Connection error : {:?}", Debug2Format(&e));
            continue;
        }
        esp_println::println!("connected");
        break;
    } 

    esp_println::println!("Connecting to MQTT server...");

    let mut mqtt_buffer = rust_mqtt::buffer::AllocBuffer;

    let mut mqtt_client = rust_mqtt::client::Client::<'_, _, _, 1, 1, 1>::new(&mut mqtt_buffer);
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
            esp_println::println!("{:?}", mqtt_client.client_config());
            esp_println::println!("{:?}", mqtt_client.server_config());
            esp_println::println!("{:?}", mqtt_client.shared_config());
            esp_println::println!("{:?}", mqtt_client.session());
        },
        Err(e) => {
            esp_println::println!("Failed to connect to server {:?}", e)
        },
    }
    
    Timer::after(Duration::from_millis(50)).await;
    scan_i2c_bus().await;

    let mut lcd = Lcd::new(&I2C_BUS);
    lcd.init().await;
    lcd.print("Estrogen\nUwu").await;

    loop {
        Timer::after(Duration::from_secs(1)).await;
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

            esp_println::println!("Scan");
            let scan_config = ScanConfig::default().with_max(1).with_ssid(SSID);
            let result = controller
                .scan_with_config_async(scan_config)
                .await
                .unwrap();
            for ap in result {
                esp_println::println!("{:?}", ap);
            }
        }
        esp_println::println!("About to connect...");
    let stats: HeapStats = esp_alloc::HEAP.stats();
    esp_println::println!("{}", stats);

        match controller.connect_async().await {
            Ok(_) => esp_println::println!("Wifi connected!"),
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

