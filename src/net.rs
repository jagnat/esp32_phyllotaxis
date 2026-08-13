use core::sync::atomic::{AtomicI32, Ordering};

use embassy_executor::Spawner;
use embassy_net::{Runner, Stack, StackResources};
use embassy_time::{Duration, Timer};
use esp_radio::wifi::{
    sta::StationConfig, Config as WifiConfig, Interface, WifiController,
};
use picoserve::routing::{get, parse_path_segment, post};
use picoserve::{AppBuilder, AppRouter};
use static_cell::StaticCell;

use defmt::{error, info, unwrap};

pub static SKETCH_SELECT: AtomicI32 = AtomicI32::new(-1);

const SSID: &str = match option_env!("PHYLLO_SSID") {
    Some(s) => s,
    None => "",
};
const PASSWORD: &str = match option_env!("PHYLLO_PASS") {
    Some(s) => s,
    None => "",
};

const HTTP_TASKS: usize = 2;

static RESOURCES: StaticCell<StackResources<{ HTTP_TASKS + 2 }>> = StaticCell::new();
static APP: StaticCell<AppRouter<App>> = StaticCell::new();
static SERVER_CONFIG: StaticCell<picoserve::Config> = StaticCell::new();

struct App;

impl AppBuilder for App {
    type PathRouter = impl picoserve::routing::PathRouter;

    fn build_app(self) -> picoserve::Router<Self::PathRouter> {
        picoserve::Router::new()
            .route(
                "/",
                get(|| async { "phyllotaxis: alive\n" }),
            )
            .route(
                ("/api/select", parse_path_segment::<u32>()),
                post(|idx: u32| async move {
                    SKETCH_SELECT.store(idx as i32, Ordering::Relaxed);
                    "ok\n"
                }),
            )
    }
}

pub fn start(
    spawner: Spawner,
    wifi: esp_hal::peripherals::WIFI<'static>,
    seed: u64,
) {
    if SSID.is_empty() {
        info!("no PHYLLO_SSID set at build time; wifi disabled");
        return;
    }

    let (controller, interfaces) = match esp_radio::wifi::new(wifi, Default::default()) {
        Ok(x) => x,
        Err(e) => {
            error!("wifi init failed: {}", e);
            return;
        }
    };

    let (stack, runner) = embassy_net::new(
        interfaces.station,
        embassy_net::Config::dhcpv4(Default::default()),
        RESOURCES.init(StackResources::new()),
        seed,
    );

    spawner.spawn(unwrap!(connection(controller)));
    spawner.spawn(unwrap!(net_runner(runner)));

    let app = APP.init(App.build_app());
    let config = SERVER_CONFIG.init(picoserve::Config::new(picoserve::Timeouts::const_default()));

    for id in 0..HTTP_TASKS {
        spawner.spawn(unwrap!(http(id, stack, app, config)));
    }

    spawner.spawn(unwrap!(announce(stack)));
}

#[embassy_executor::task]
async fn connection(mut controller: WifiController<'static>) {
    let config = WifiConfig::Station(
        StationConfig::default()
            .with_ssid(SSID)
            .with_password(PASSWORD.into()),
    );
    if let Err(e) = controller.set_config(&config) {
        error!("wifi config failed: {}", e);
        return;
    }
    loop {
        match controller.connect_async().await {
            Ok(_) => {
                info!("wifi connected to {}", SSID);
                let _ = controller.wait_for_disconnect_async().await;
                info!("wifi disconnected");
            }
            Err(e) => {
                error!("wifi connect failed: {}", e);
                Timer::after(Duration::from_secs(5)).await;
            }
        }
    }
}

#[embassy_executor::task]
async fn net_runner(mut runner: Runner<'static, Interface<'static>>) {
    runner.run().await
}

#[embassy_executor::task]
async fn announce(stack: Stack<'static>) {
    stack.wait_config_up().await;
    if let Some(cfg) = stack.config_v4() {
        info!("ip: {}", cfg.address);
    }
}

#[embassy_executor::task(pool_size = 2)]
async fn http(
    id: usize,
    stack: Stack<'static>,
    app: &'static AppRouter<App>,
    config: &'static picoserve::Config,
) {
    let mut tcp_rx = [0u8; 1024];
    let mut tcp_tx = [0u8; 1024];
    let mut http_buf = [0u8; 2048];
    loop {
        picoserve::Server::new(app, config, &mut http_buf)
            .listen_and_serve(id, stack, 80, &mut tcp_rx, &mut tcp_tx)
            .await;
    }
}
