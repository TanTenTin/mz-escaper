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
mod update;

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

    // 업데이트 확인은 서버를 띄우기 전에 한다. 강제 업데이트면 여기서 교체 후 재시작하며,
    // 아래 코드는 실행되지 않는다.
    let update_info = check_for_update(&http, &cfg).await;

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
        .route("/healthz", get(|| async { "ok" }))
        // 클라이언트 IP를 판별해 익스텐션에 넣는 미들웨어. 레이트 리밋이 이 값을 쓴다.
        .layer(middleware::from_fn_with_state(
            state.clone(),
            resolve_client_ip,
        ))
        .with_state(state.clone());

    let addr: SocketAddr = state
        .cfg
        .bind_addr
        .parse()
        .unwrap_or_else(|_| panic!("BIND_ADDR 형식이 잘못되었습니다: {}", state.cfg.bind_addr));

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("{addr} 바인드 실패: {e}"));

    println!(
        "mz-escaper v{} 실행 중 → http://{addr}",
        update::CURRENT_VERSION
    );
    println!("  모델: {}", state.cfg.model);
    if let Some(info) = &state.update {
        if info.decision == Decision::Optional {
            println!(
                "  새 버전 v{} 이 있습니다. 브라우저 화면에서 업데이트할 수 있습니다.",
                info.manifest.latest
            );
        }
    }
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
