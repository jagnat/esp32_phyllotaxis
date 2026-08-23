use core::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use embassy_executor::Spawner;
use embassy_net::{Runner, Stack, StackResources};
use embassy_time::{Duration, Timer};
use esp_radio::wifi::{
    sta::StationConfig, Config as WifiConfig, Interface, WifiController,
};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use picoserve::routing::{delete, get, parse_path_segment, post};
use picoserve::{AppBuilder, AppRouter};
use static_cell::StaticCell;

use defmt::{error, info, unwrap};

pub static SKETCH_SELECT: AtomicI32 = AtomicI32::new(-1);

pub static WIFI_STARTED: AtomicBool = AtomicBool::new(false);
/// Set once DHCP completes; radio bring-up before this point can wedge other
/// peripherals' DMA interrupts, so latency-fragile drivers should wait for it.
pub static NET_UP: AtomicBool = AtomicBool::new(false);

const SSID: &str = match option_env!("PHYLLO_SSID") {
    Some(s) => s,
    None => "",
};
const PASSWORD: &str = match option_env!("PHYLLO_PASS") {
    Some(s) => s,
    None => "",
};

const HTTP_TASKS: usize = 2;

// One socket each for DHCP, DNS, and the DDP UDP listener, plus the HTTP pool.
static RESOURCES: StaticCell<StackResources<{ HTTP_TASKS + 3 }>> = StaticCell::new();
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
            .route(
                "/stream",
                get(|ws: picoserve::response::ws::WebSocketUpgrade| async move {
                    ws.on_upgrade(crate::stream::WsStream)
                }),
            )
            .route("/api/sketches", get(sketches_list))
            .route(
                (
                    "/api/sketches",
                    parse_path_segment::<usize>(),
                    parse_path_segment::<String>(),
                ),
                post(sketches_upload),
            )
            .route(
                ("/api/sketches", parse_path_segment::<usize>()),
                delete(sketches_delete),
            )
            .route(
                ("/api/run", parse_path_segment::<usize>()),
                post(sketches_run),
            )
    }
}

async fn sketches_list() -> String {
    use core::fmt::Write;
    match crate::gallery::with(|g| g.list()).await {
        Some(slots) => {
            let mut out = String::new();
            for (i, slot) in slots.iter().enumerate() {
                if let Some(s) = slot {
                    let _ = writeln!(out, "{} {} {}", i, s.name(), s.len);
                }
            }
            if out.is_empty() {
                out.push_str("(no sketches)\n");
            }
            out
        }
        None => String::from("no sketches partition\n"),
    }
}

async fn sketches_upload((slot, name): (usize, String), body: Vec<u8>) -> String {
    if slot >= crate::gallery::SLOTS {
        return format!("bad slot (0..{})\n", crate::gallery::SLOTS - 1);
    }
    if body.len() > crate::gallery::MAX_SKETCH {
        return format!(
            "too big: {} > {} bytes\n",
            body.len(),
            crate::gallery::MAX_SKETCH
        );
    }
    if let Err(e) = crate::wasm::validate(&body) {
        return format!("invalid module: {:?}\n", e);
    }
    match crate::gallery::with(|g| g.write(slot, &name, &body)).await {
        Some(Ok(())) => format!("ok: slot {} <- {} ({} bytes)\n", slot, name, body.len()),
        Some(Err(e)) => format!("flash write failed: {:?}\n", e),
        None => String::from("no sketches partition\n"),
    }
}

async fn sketches_delete(slot: usize) -> String {
    match crate::gallery::with(|g| g.erase(slot)).await {
        Some(Ok(())) => String::from("ok\n"),
        Some(Err(e)) => format!("delete failed: {:?}\n", e),
        None => String::from("no sketches partition\n"),
    }
}

async fn sketches_run(slot: usize) -> String {
    match crate::gallery::with(|g| g.read(slot)).await {
        Some(Some(bytes)) => {
            crate::wasm::submit_module(bytes);
            SKETCH_SELECT.store(
                crate::sketches::SketchBank::WASM_SLOT as i32,
                Ordering::Relaxed,
            );
            String::from("ok\n")
        }
        Some(None) => String::from("empty slot\n"),
        None => String::from("no sketches partition\n"),
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

    WIFI_STARTED.store(true, Ordering::Relaxed);

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
    spawner.spawn(unwrap!(crate::stream::ddp_task(stack)));
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
    NET_UP.store(true, Ordering::Relaxed);
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
