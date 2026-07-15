//! mz-escaper: 원하는 말투로 바꿔 답하는 챗봇.
//!
//! 이름의 뜻: 알아듣기 힘든 요즘 말투(MZ)에서 벗어나, 내가 원하는 말투로 말을 옮겨 준다.
//!
//! 구조는 아주 단순하다.
//!   브라우저 → (이 서버) → LLM 게이트웨이
//! 이 서버가 존재하는 이유는 단 하나, API 키를 브라우저에서 떼어 놓기 위해서다.
//! 브라우저는 /api/chat 만 알고, 게이트웨이 주소도 모델명도 키도 알지 못한다.

// 릴리스 빌드에서는 콘솔창을 띄우지 않는다. 유저는 exe 를 더블클릭할 뿐이고, 네이티브
// 창(desktop.rs)이 UI 를 맡는다. 개발(debug) 빌드는 콘솔을 남겨 둬야 로그로 디버깅한다.
// 이 속성은 Windows 에만 의미가 있고 다른 OS 에서는 무시된다 — 백엔드(Linux)는 영향 없다.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod chat;
mod config;
mod ratelimit;
mod tone;
mod update;

// 네이티브 창은 Windows 에서만, 그리고 릴레이 모드일 때만 뜬다. Docker 백엔드(Linux)는
// 이 모듈을 컴파일조차 하지 않는다.
#[cfg(windows)]
mod desktop;

// 시작 시 자동 실행 토글도 Windows 전용(레지스트리 Run 키)이다.
#[cfg(windows)]
mod autostart;

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Json, Router,
};
use serde_json::json;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use config::Config;
use ratelimit::RateLimiter;
use update::{Decision, UpdateInfo};

/// 모든 핸들러가 공유하는 상태. Arc로 감싸 클론 비용을 포인터 복사로 만든다.
#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    /// reqwest 클라이언트는 내부에 커넥션 풀을 들고 있다. 요청마다 새로 만들면
    /// 매번 TCP+TLS 핸드셰이크를 다시 하게 되므로 반드시 하나를 재사용한다.
    pub http: reqwest::Client,
    pub limiter: Arc<RateLimiter>,
    /// 시작할 때 한 번 판정한 업데이트 상태. 매니페스트가 없거나 조회에 실패하면 None.
    /// 실행 중에 다시 확인하지는 않는다 — 유저가 exe를 껐다 켜는 주기가 곧 확인 주기다.
    pub update: Option<Arc<UpdateInfo>>,
}

/// UI는 빌드 시점에 바이너리 안으로 들어간다. 덕분에 배포물이 파일 하나로 끝나고,
/// 요청 때 디스크를 읽지 않는다.
const INDEX_HTML: &str = include_str!("../static/index.html");

fn main() {
    // 네이티브 창의 이벤트 루프는 메인 스레드를 차지해야 한다(Windows 요구사항). 그래서
    // tokio 를 #[tokio::main] 으로 메인에 걸지 않고, 런타임을 손수 만들어 서버는 그 위에서
    // 돌리고 메인 스레드는 창에 내준다. 헤드리스(백엔드)일 때는 창 없이 서버가 곧 프로세스다.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio 런타임 생성 실패");

    // 설정 로드 → 업데이트 확인(강제면 여기서 교체·재시작) → 리스너 바인딩까지.
    let prepared = runtime.block_on(prepare());

    if prepared.windowed {
        // 서버는 백그라운드에서 돌리고, 메인 스레드는 창 이벤트 루프에 넘긴다.
        runtime.spawn(serve(prepared.listener, prepared.app));

        #[cfg(windows)]
        desktop::run(prepared.local_addr); // 창이 닫히면 프로세스 종료 → 돌아오지 않는다

        // windowed 는 Windows 에서만 true 라(should_open_window) 여기 닿지 않는다.
        #[cfg(not(windows))]
        unreachable!("windowed 모드는 Windows 에서만 활성화된다");
    } else {
        // 헤드리스: 서버가 곧 프로세스다. 종료 시그널을 받을 때까지 여기서 블록된다.
        runtime.block_on(serve(prepared.listener, prepared.app));
    }
}

/// prepare() 의 결과. 서버를 띄울 재료와, 창을 열지 여부·주소를 함께 담는다.
struct Prepared {
    listener: tokio::net::TcpListener,
    app: Router,
    /// 실제로 바인딩된 주소. 창(WebView)이 이 주소를 연다. 동적 포트(:0)라도 확정값이다.
    local_addr: SocketAddr,
    /// 네이티브 창을 열지 여부. Windows + 릴레이 모드 + MZ_HEADLESS 미설정일 때만 true.
    windowed: bool,
}

/// 서버를 띄우기 직전까지의 준비. 리스너를 실제로 바인딩해 확정된 주소까지 확보한다.
///
/// 업데이트 확인이 여기 포함된다. 강제 업데이트(현재 버전 < minimum)면 이 안에서 교체 후
/// 재시작하므로 이 함수는 돌아오지 않는다.
async fn prepare() -> Prepared {
    let cfg = match Config::from_env() {
        Ok(c) => c,
        Err(msg) => {
            fatal(&format!("설정 오류: {msg}"));
        }
    };

    let http = reqwest::Client::builder()
        // 게이트웨이가 응답 헤더조차 주지 않고 매달려 있는 상황을 끊는다.
        // 스트림 전체가 아니라 '연결~첫 응답' 구간에만 걸리는 타임아웃이라
        // 답변이 길어도 도중에 끊기지 않는다.
        .connect_timeout(Duration::from_secs(10))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
        .expect("HTTP 클라이언트 생성 실패");

    // 업데이트 확인은 서버를 띄우기 전에 한다. 강제 업데이트면 여기서 교체 후 재시작한다.
    let update_info = check_for_update(&http, &cfg).await;

    let windowed = should_open_window(&cfg);

    // 창을 여는 데스크톱 모드에서 BIND_ADDR 을 따로 주지 않았으면, 고정 포트 대신 빈 포트를
    // 자동 할당받는다(:0). 8080 이 이미 점유돼 있어도 충돌 없이 뜨고, 유저는 포트를 알 필요가
    // 없다 — 창이 확정된 주소를 알아서 연다.
    let bind_str = if windowed && std::env::var("BIND_ADDR").is_err() {
        "127.0.0.1:0".to_string()
    } else {
        cfg.bind_addr.clone()
    };

    let addr: SocketAddr = bind_str
        .parse()
        .unwrap_or_else(|_| fatal(&format!("BIND_ADDR 형식이 잘못되었습니다: {bind_str}")));

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| fatal(&format!("{addr} 바인드 실패: {e}")));

    // :0 으로 바인딩했으면 실제 포트는 지금 확정된다. 창은 이 값을 열어야 한다.
    let local_addr = listener
        .local_addr()
        .expect("바인딩 주소를 읽지 못했습니다");

    let state = AppState {
        limiter: Arc::new(RateLimiter::new(cfg.rate_max_requests, cfg.rate_window)),
        http,
        cfg: Arc::new(cfg),
        update: update_info.map(Arc::new),
    };

    let app = Router::new()
        .route("/", get(index))
        .route("/api/presets", get(presets))
        .route("/api/chat", post(chat::handle_chat))
        .route("/api/update", get(update_status))
        .route("/api/update/apply", post(update_apply))
        .route("/api/autostart", get(autostart_status).post(autostart_set))
        .route("/healthz", get(|| async { "ok" }))
        // 클라이언트 IP를 판별해 익스텐션에 넣는 미들웨어. 레이트 리밋이 이 값을 쓴다.
        .layer(middleware::from_fn_with_state(
            state.clone(),
            resolve_client_ip,
        ))
        .with_state(state.clone());

    // 로그는 debug 빌드에서만 콘솔에 보인다(릴리스는 windows_subsystem=windows 로 콘솔 없음).
    println!(
        "mz-escaper v{} 실행 중 → http://{local_addr}",
        update::CURRENT_VERSION
    );
    if state.cfg.is_backend() {
        println!("  모드: 백엔드 (게이트웨이 직접 호출)");
        println!("  모델: {}", state.cfg.model);
    } else {
        println!("  모드: 릴레이 (API 키 없음)");
        println!("  백엔드: {}", state.cfg.backend_url);
    }
    if let Some(info) = &state.update {
        if info.decision == Decision::Optional {
            println!(
                "  새 버전 v{} 이 있습니다. 화면에서 업데이트할 수 있습니다.",
                info.manifest.latest
            );
        }
    }
    println!(
        "  레이트 리밋: {}초당 {}회 / IP",
        state.cfg.rate_window.as_secs(),
        state.cfg.rate_max_requests
    );

    Prepared {
        listener,
        app,
        local_addr,
        windowed,
    }
}

/// axum 서버를 띄우고 종료 시그널까지 블록된다.
async fn serve(listener: tokio::net::TcpListener, app: Router) {
    // into_make_service_with_connect_info: 핸들러가 소켓의 peer 주소를 볼 수 있게 한다.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .expect("서버 실행 실패");
}

/// 네이티브 창을 열지 판단한다.
///
/// Windows + 릴레이 모드일 때만 연다. 백엔드는 헤드리스(Docker)로 돌아야 하므로 창을 열지
/// 않는다. `MZ_HEADLESS` 를 주면 Windows 릴레이라도 창 없이 띄운다 — 자동화 테스트용이다.
fn should_open_window(cfg: &Config) -> bool {
    #[cfg(windows)]
    {
        !cfg.is_backend() && std::env::var("MZ_HEADLESS").is_err()
    }
    #[cfg(not(windows))]
    {
        let _ = cfg;
        false
    }
}

/// 치명적 오류로 종료한다. 릴리스 빌드는 콘솔이 없어 eprintln 이 안 보이므로, Windows 에서는
/// 네이티브 대화상자로도 알린다.
fn fatal(message: &str) -> ! {
    eprintln!("[치명적 오류] {message}");
    #[cfg(windows)]
    desktop::show_error(message);
    std::process::exit(1);
}

/// 시작 시 업데이트 매니페스트를 확인한다.
///
/// 강제 업데이트(현재 버전 < minimum)면 여기서 교체하고 재시작하므로 돌아오지 않는다.
/// 그 외에는 판정 결과를 돌려주고 서버 기동을 계속한다.
///
/// 매니페스트 조회가 실패하면 경고만 남기고 그냥 실행한다. 업데이트 서버가 잠깐 죽었다고
/// 유저의 챗봇이 안 켜지는 편이 더 나쁘다. 다만 이 경우 강제 업데이트를 강제할 수단이
/// 없다는 뜻이기도 하다 — 최소버전 정책은 "온라인일 때 반드시 적용된다" 수준의 보장이다.
async fn check_for_update(http: &reqwest::Client, cfg: &Config) -> Option<UpdateInfo> {
    let url = cfg.update_manifest_url.as_ref()?;

    let manifest = match update::fetch_manifest(http, url).await {
        Ok(m) => m,
        Err(e) => {
            eprintln!("[업데이트] 확인을 건너뜁니다: {e}");
            return None;
        }
    };

    let decision = match update::decide(update::CURRENT_VERSION, &manifest) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("[업데이트] 버전 비교 실패: {e}");
            return None;
        }
    };

    if decision == Decision::Mandatory {
        println!(
            "[업데이트] 현재 v{} 는 더 이상 지원되지 않습니다. v{} 로 업데이트합니다...",
            update::CURRENT_VERSION,
            manifest.latest
        );

        if let Err(e) = update::apply(http, &manifest).await {
            // 교체에 실패하면 실행을 허용하지 않는다. minimum 의 존재 이유가 사라지기 때문이다.
            eprintln!("[업데이트] 실패: {e}");
            eprintln!("네트워크를 확인한 뒤 다시 실행하거나, 최신 버전을 직접 내려받아 주세요.");
            std::process::exit(1);
        }

        update::restart();
    }

    Some(UpdateInfo { manifest, decision })
}

/// 업데이트 상태 조회. 프런트가 배너를 띄울지 판단하는 데 쓴다.
///
/// 루프백 전용이다. `local_only` 주석 참고.
async fn update_status(
    State(state): State<AppState>,
    Extension(ip): Extension<IpAddr>,
) -> Response {
    if let Some(denied) = reject_remote(ip) {
        return denied;
    }

    match &state.update {
        // Optional 일 때만 새 버전 정보를 준다. UpToDate 면 배너를 띄울 이유가 없다.
        Some(info) if info.decision == Decision::Optional => Json(json!({
            "current": update::CURRENT_VERSION,
            "latest": info.manifest.latest,
            "notes": info.manifest.notes,
            "updateAvailable": true,
        }))
        .into_response(),
        _ => Json(json!({
            "current": update::CURRENT_VERSION,
            "updateAvailable": false,
        }))
        .into_response(),
    }
}

/// 유저가 UI에서 "지금 업데이트"를 눌렀을 때. 다운로드·검증·교체까지 끝낸 뒤 응답하고,
/// 응답이 브라우저에 닿을 시간을 준 다음 재시작한다.
///
/// 다운로드가 수십 초 걸릴 수 있으므로 프런트는 이 요청을 기다리는 동안 진행 표시를 띄운다.
async fn update_apply(State(state): State<AppState>, Extension(ip): Extension<IpAddr>) -> Response {
    if let Some(denied) = reject_remote(ip) {
        return denied;
    }

    let Some(info) = state.update.clone() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "적용할 업데이트가 없습니다." })),
        )
            .into_response();
    };

    if info.decision != Decision::Optional {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "적용할 업데이트가 없습니다." })),
        )
            .into_response();
    }

    if let Err(e) = update::apply(&state.http, &info.manifest).await {
        eprintln!("[업데이트] 실패: {e}");
        // 실패 이유를 그대로 내보내지 않는다. URL 같은 배포 인프라 정보가 섞일 수 있다.
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "업데이트에 실패했습니다. 잠시 후 다시 시도해주세요." })),
        )
            .into_response();
    }

    // 교체는 끝났다. 응답을 흘려보낸 뒤 새 바이너리로 갈아탄다.
    tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        update::restart();
    });

    Json(json!({ "restarting": true, "version": info.manifest.latest })).into_response()
}

/// 자동 실행 토글 요청 본문.
#[derive(serde::Deserialize)]
struct AutostartReq {
    enabled: bool,
}

/// 시작 시 자동 실행 상태 조회.
///
/// 업데이트 엔드포인트와 같은 이유로 루프백 전용이다. 남이 원격에서 남의 PC 부팅 항목을
/// 들여다볼 이유가 없다. `supported` 는 이 플랫폼에서 기능을 쓸 수 있는지 — Windows 가
/// 아니면 프런트가 토글 자체를 숨긴다.
async fn autostart_status(Extension(ip): Extension<IpAddr>) -> Response {
    if let Some(denied) = reject_remote(ip) {
        return denied;
    }

    #[cfg(windows)]
    {
        Json(json!({ "supported": true, "enabled": autostart::is_enabled() })).into_response()
    }
    #[cfg(not(windows))]
    {
        Json(json!({ "supported": false, "enabled": false })).into_response()
    }
}

/// 시작 시 자동 실행 켜기/끄기. 루프백 전용이다.
async fn autostart_set(
    Extension(ip): Extension<IpAddr>,
    Json(req): Json<AutostartReq>,
) -> Response {
    if let Some(denied) = reject_remote(ip) {
        return denied;
    }

    #[cfg(windows)]
    {
        match autostart::set_enabled(req.enabled) {
            Ok(()) => Json(json!({ "enabled": req.enabled })).into_response(),
            Err(e) => {
                // 실패 원인(레지스트리 경로 등)은 로그로만. 클라이언트에는 일반 메시지.
                eprintln!("[자동실행] 설정 실패: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "자동 실행 설정에 실패했습니다." })),
                )
                    .into_response()
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = req;
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "이 플랫폼에서는 지원하지 않습니다." })),
        )
            .into_response()
    }
}

/// 업데이트 엔드포인트는 이 PC에서 온 요청만 받는다.
///
/// 이 바이너리는 유저 PC에서 도는 것을 전제로 하지만, 누군가 BIND_ADDR 을 0.0.0.0 으로
/// 두면 외부에서도 닿는다. 그때 남이 바이너리 교체를 트리거할 수 있어서는 안 된다.
/// 404로 답하는 이유: 403이면 "여기 뭔가 있다"는 사실을 알려주는 셈이다.
fn reject_remote(ip: IpAddr) -> Option<Response> {
    if ip.is_loopback() {
        None
    } else {
        Some((StatusCode::NOT_FOUND, "not found").into_response())
    }
}

/// 클라이언트 IP를 판별해 요청 익스텐션에 넣는다.
///
/// TRUST_PROXY=true 일 때만 X-Forwarded-For를 신뢰한다. 프록시가 없는데 이 헤더를
/// 믿으면, 아무나 헤더를 위조해 매 요청마다 다른 IP인 척하며 레이트 리밋을 무력화할 수 있다.
async fn resolve_client_ip(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    mut req: Request,
    next: Next,
) -> Response {
    let ip = if state.cfg.trust_proxy {
        req.headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            // XFF는 "client, proxy1, proxy2" 형태다. 맨 앞이 원 클라이언트.
            .and_then(|v| v.split(',').next())
            .and_then(|v| v.trim().parse::<IpAddr>().ok())
            .unwrap_or_else(|| peer.ip())
    } else {
        peer.ip()
    };

    req.extensions_mut().insert(ip);
    next.run(req).await
}

/// 내장된 UI를 돌려준다.
async fn index() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        INDEX_HTML,
    )
}

/// 말투 목록. 프런트는 이걸 받아 버튼을 그리므로, 프리셋 추가는 tone.rs만 고치면 된다.
async fn presets() -> impl IntoResponse {
    let list: Vec<_> = tone::PRESETS
        .iter()
        .map(|p| json!({ "id": p.id, "label": p.label }))
        .collect();

    Json(json!({
        "presets": list,
        "maxCustomToneChars": tone::MAX_CUSTOM_TONE_CHARS,
    }))
}

/// Ctrl+C를 받으면 진행 중인 응답을 끝낸 뒤 종료한다.
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    println!("\n종료합니다.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 루프백_요청은_업데이트_엔드포인트를_쓸_수_있다() {
        assert!(reject_remote("127.0.0.1".parse().unwrap()).is_none());
        assert!(reject_remote("::1".parse().unwrap()).is_none());
    }

    /// 유저가 BIND_ADDR 을 0.0.0.0 으로 열어둔 상태에서 외부인이 바이너리 교체를
    /// 트리거하는 것을 막는다. 이 검사가 사라지면 원격 코드 실행이 된다.
    #[test]
    fn 외부_요청은_업데이트_엔드포인트를_볼_수_없다() {
        let denied = reject_remote("203.0.113.7".parse().unwrap()).expect("차단되어야 한다");
        assert_eq!(denied.status(), StatusCode::NOT_FOUND);
    }
}
