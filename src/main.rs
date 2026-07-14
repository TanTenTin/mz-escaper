//! mz-escaper: 원하는 말투로 바꿔 답하는 챗봇.
//!
//! 이름의 뜻: 알아듣기 힘든 요즘 말투(MZ)에서 벗어나, 내가 원하는 말투로 말을 옮겨 준다.
//!
//! 구조는 아주 단순하다.
//!   브라우저 → (이 서버) → LLM 게이트웨이
//! 이 서버가 존재하는 이유는 단 하나, API 키를 브라우저에서 떼어 놓기 위해서다.
//! 브라우저는 /api/chat 만 알고, 게이트웨이 주소도 모델명도 키도 알지 못한다.

mod chat;
mod config;
mod ratelimit;
mod tone;

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::json;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use config::Config;
use ratelimit::RateLimiter;

/// 모든 핸들러가 공유하는 상태. Arc로 감싸 클론 비용을 포인터 복사로 만든다.
#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    /// reqwest 클라이언트는 내부에 커넥션 풀을 들고 있다. 요청마다 새로 만들면
    /// 매번 TCP+TLS 핸드셰이크를 다시 하게 되므로 반드시 하나를 재사용한다.
    pub http: reqwest::Client,
    pub limiter: Arc<RateLimiter>,
}

/// UI는 빌드 시점에 바이너리 안으로 들어간다. 덕분에 배포물이 파일 하나로 끝나고,
/// 요청 때 디스크를 읽지 않는다.
const INDEX_HTML: &str = include_str!("../static/index.html");

#[tokio::main]
async fn main() {
    let cfg = match Config::from_env() {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("[설정 오류] {msg}");
            std::process::exit(1);
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

    let state = AppState {
        limiter: Arc::new(RateLimiter::new(cfg.rate_max_requests, cfg.rate_window)),
        http,
        cfg: Arc::new(cfg),
    };

    let app = Router::new()
        .route("/", get(index))
        .route("/api/presets", get(presets))
        .route("/api/chat", post(chat::handle_chat))
        .route("/healthz", get(|| async { "ok" }))
        // 클라이언트 IP를 판별해 익스텐션에 넣는 미들웨어. 레이트 리밋이 이 값을 쓴다.
        .layer(middleware::from_fn_with_state(state.clone(), resolve_client_ip))
        .with_state(state.clone());

    let addr: SocketAddr = state
        .cfg
        .bind_addr
        .parse()
        .unwrap_or_else(|_| panic!("BIND_ADDR 형식이 잘못되었습니다: {}", state.cfg.bind_addr));

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("{addr} 바인드 실패: {e}"));

    println!("mz-escaper 실행 중 → http://{addr}");
    println!("  모델: {}", state.cfg.model);
    println!(
        "  레이트 리밋: {}초당 {}회 / IP",
        state.cfg.rate_window.as_secs(),
        state.cfg.rate_max_requests
    );

    // into_make_service_with_connect_info: 핸들러가 소켓의 peer 주소를 볼 수 있게 한다.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .expect("서버 실행 실패");
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
