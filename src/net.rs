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

use picoserve::extract::{FromRequest, FromRequestParts};
use picoserve::io::{Read, Write};
use picoserve::request::{RequestBody, RequestParts};
use picoserve::response::{Content, StatusCode};
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
const API_TOKEN: &str = match option_env!("PHYLLO_API_TOKEN") {
    Some(s) => s,
    None => "",
};

const HTTP_TASKS: usize = 2;

// One socket each for DHCP, DNS, and the DDP UDP listener, plus the HTTP pool.
static RESOURCES: StaticCell<StackResources<{ HTTP_TASKS + 3 }>> = StaticCell::new();
static APP: StaticCell<AppRouter<App>> = StaticCell::new();
static SERVER_CONFIG: StaticCell<picoserve::Config> = StaticCell::new();

struct App;

struct Json(String);

impl Content for Json {
    fn content_type(&self) -> &'static str {
        "application/json"
    }

    fn content_length(&self) -> usize {
        self.0.len()
    }

    async fn write_content<W: Write>(self, mut writer: W) -> Result<(), W::Error> {
        writer.write_all(self.0.as_bytes()).await
    }
}

type ApiResponse = (StatusCode, Json);

fn response(status: StatusCode, body: String) -> ApiResponse {
    (status, Json(body))
}

fn error_response(status: StatusCode, code: &str) -> ApiResponse {
    response(status, format!("{{\"ok\":false,\"error\":\"{}\"}}\n", code))
}

fn write_api_enabled() -> bool {
    API_TOKEN.len() >= 16
}

struct ApiAuth;

impl<'r, State> FromRequestParts<'r, State> for ApiAuth {
    type Rejection = ApiResponse;

    async fn from_request_parts(
        _state: &'r State,
        request_parts: &RequestParts<'r>,
    ) -> Result<Self, Self::Rejection> {
        if !write_api_enabled() {
            return Err(error_response(StatusCode::SERVICE_UNAVAILABLE, "write_api_disabled"));
        }
        let authorization = request_parts
            .headers()
            .get("authorization")
            .and_then(|value| value.as_str().ok())
            .unwrap_or("");
        if crate::api_security::bearer_matches(API_TOKEN, authorization) {
            Ok(Self)
        } else {
            Err(error_response(StatusCode::UNAUTHORIZED, "unauthorized"))
        }
    }
}

struct MenuColor([u8; 3]);

impl<'r, State> FromRequestParts<'r, State> for MenuColor {
    type Rejection = ApiResponse;

    async fn from_request_parts(
        _state: &'r State,
        request_parts: &RequestParts<'r>,
    ) -> Result<Self, Self::Rejection> {
        let Some(value) = request_parts.headers().get("x-phyllo-color") else {
            return Ok(Self(crate::api_security::DEFAULT_MENU_COLOR));
        };
        let value = value
            .as_str()
            .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid_color"))?;
        crate::api_security::parse_hex_color(value)
            .map(Self)
            .ok_or_else(|| error_response(StatusCode::UNPROCESSABLE_ENTITY, "invalid_color"))
    }
}

struct SketchBody(Vec<u8>);

impl<'r, State> FromRequest<'r, State> for SketchBody {
    type Rejection = ApiResponse;

    async fn from_request<R: Read>(
        _state: &'r State,
        request_parts: RequestParts<'r>,
        request_body: RequestBody<'r, R>,
    ) -> Result<Self, Self::Rejection> {
        let content_type = request_parts
            .headers()
            .get("content-type")
            .and_then(|value| value.as_str().ok())
            .unwrap_or("");
        if !matches!(content_type, "application/wasm" | "application/octet-stream") {
            return Err(error_response(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "expected_wasm_content_type",
            ));
        }
        let len = request_body.content_length();
        if len > crate::gallery::MAX_SKETCH {
            return Err(error_response(StatusCode::PAYLOAD_TOO_LARGE, "module_too_large"));
        }
        let mut body = Vec::new();
        body.try_reserve_exact(len)
            .map_err(|_| error_response(StatusCode::INSUFFICIENT_STORAGE, "out_of_memory"))?;
        body.resize(len, 0);
        request_body
            .reader()
            .read_exact(&mut body)
            .await
            .map_err(|_| error_response(StatusCode::BAD_REQUEST, "incomplete_body"))?;
        Ok(Self(body))
    }
}

impl AppBuilder for App {
    type PathRouter = impl picoserve::routing::PathRouter;

    fn build_app(self) -> picoserve::Router<Self::PathRouter> {
        picoserve::Router::new()
            .route(
                "/",
                get(|| async {
                    response(
                        StatusCode::OK,
                        format!(
                            "{{\"name\":\"phyllotaxis\",\"write_enabled\":{}}}\n",
                            write_api_enabled()
                        ),
                    )
                }),
            )
            .route(
                "/api/health",
                get(|| async { response(StatusCode::OK, crate::health::json()) }),
            )
            .route(
                ("/api/select", parse_path_segment::<usize>()),
                post(select_sketch),
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

async fn select_sketch(slot: usize, _auth: ApiAuth) -> ApiResponse {
    if slot >= crate::sketches::SketchBank::COUNT {
        return error_response(StatusCode::NOT_FOUND, "unknown_native_slot");
    }
    SKETCH_SELECT.store(slot as i32, Ordering::Relaxed);
    response(StatusCode::ACCEPTED, format!("{{\"ok\":true,\"slot\":{}}}\n", slot))
}

async fn sketches_list() -> ApiResponse {
    use core::fmt::Write;
    match crate::gallery::with(|g| g.list()).await {
        Some(slots) => {
            let mut out = String::new();
            let _ = write!(
                out,
                "{{\"abi_version\":{},\"slot_count\":{},\"slot_size\":{},\"max_module_size\":{},\"write_enabled\":{},\"sketches\":[",
                crate::wasm::ABI_VERSION,
                crate::gallery::SLOTS,
                crate::gallery::SLOT_SIZE,
                crate::gallery::MAX_SKETCH,
                write_api_enabled(),
            );
            let mut first = true;
            for (i, slot) in slots.iter().enumerate() {
                if let Some(s) = slot {
                    if !first {
                        out.push(',');
                    }
                    first = false;
                    let _ = write!(
                        out,
                        "{{\"slot\":{},\"name\":\"{}\",\"bytes\":{},\"abi_version\":{},\"caps\":{},\"color\":\"#{:02x}{:02x}{:02x}\"}}",
                        i,
                        s.name(),
                        s.len,
                        s.abi_version,
                        s.caps,
                        s.color[0],
                        s.color[1],
                        s.color[2],
                    );
                }
            }
            out.push_str("]}\n");
            response(StatusCode::OK, out)
        }
        None => error_response(StatusCode::SERVICE_UNAVAILABLE, "gallery_unavailable"),
    }
}

async fn sketches_upload(
    (slot, name): (usize, String),
    _auth: ApiAuth,
    MenuColor(color): MenuColor,
    SketchBody(body): SketchBody,
) -> ApiResponse {
    if slot >= crate::gallery::SLOTS {
        return error_response(StatusCode::NOT_FOUND, "unknown_slot");
    }
    if !crate::api_security::valid_name(&name) {
        return error_response(StatusCode::UNPROCESSABLE_ENTITY, "invalid_name");
    }
    let info = match crate::wasm::validate(&body) {
        Ok(info) => info,
        Err(_) => return error_response(StatusCode::UNPROCESSABLE_ENTITY, "invalid_module"),
    };
    match crate::gallery::with(|g| g.write(slot, &name, &body, info.caps, color)).await {
        Some(Ok(())) => response(
            StatusCode::CREATED,
            format!(
                "{{\"ok\":true,\"slot\":{},\"name\":\"{}\",\"bytes\":{},\"abi_version\":{},\"caps\":{},\"color\":\"#{:02x}{:02x}{:02x}\"}}\n",
                slot,
                name,
                body.len(),
                info.abi_version,
                info.caps,
                color[0],
                color[1],
                color[2],
            ),
        ),
        Some(Err(_)) => error_response(StatusCode::INSUFFICIENT_STORAGE, "flash_write_failed"),
        None => error_response(StatusCode::SERVICE_UNAVAILABLE, "gallery_unavailable"),
    }
}

async fn sketches_delete(slot: usize, _auth: ApiAuth) -> ApiResponse {
    if slot >= crate::gallery::SLOTS {
        return error_response(StatusCode::NOT_FOUND, "unknown_slot");
    }
    match crate::gallery::with(|g| g.erase(slot)).await {
        Some(Ok(())) => response(StatusCode::OK, format!("{{\"ok\":true,\"slot\":{}}}\n", slot)),
        Some(Err(_)) => error_response(StatusCode::INSUFFICIENT_STORAGE, "flash_erase_failed"),
        None => error_response(StatusCode::SERVICE_UNAVAILABLE, "gallery_unavailable"),
    }
}

async fn sketches_run(slot: usize, _auth: ApiAuth) -> ApiResponse {
    if slot >= crate::gallery::SLOTS {
        return error_response(StatusCode::NOT_FOUND, "unknown_slot");
    }
    match crate::gallery::with(|g| g.read(slot)).await {
        Some(Some(bytes)) => {
            if crate::wasm::validate(&bytes).is_err() {
                return error_response(StatusCode::UNPROCESSABLE_ENTITY, "invalid_module");
            }
            crate::wasm::submit_module(bytes);
            SKETCH_SELECT.store(
                crate::sketches::SketchBank::WASM_SLOT as i32,
                Ordering::Relaxed,
            );
            response(StatusCode::ACCEPTED, format!("{{\"ok\":true,\"slot\":{}}}\n", slot))
        }
        Some(None) => error_response(StatusCode::NOT_FOUND, "empty_or_corrupt_slot"),
        None => error_response(StatusCode::SERVICE_UNAVAILABLE, "gallery_unavailable"),
    }
}

pub fn start(
    spawner: Spawner,
    wifi: esp_hal::peripherals::WIFI<'static>,
    seed: u64,
) {
    if write_api_enabled() {
        info!("api: authenticated mutations enabled");
    } else {
        info!("api: mutations disabled (set PHYLLO_API_TOKEN to 16+ bytes)");
    }
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
