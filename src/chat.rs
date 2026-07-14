//! /api/chat 핸들러: 검증 → system 프롬프트 조립 → 게이트웨이 SSE 릴레이.
//!
//! 성능상 핵심은 마지막 단계다. 업스트림이 보내오는 SSE를 서버에서 파싱하거나
//! 모아 두지 않고, 바이트 청크가 도착하는 즉시 그대로 브라우저로 흘려보낸다.
//! 덕분에 응답 길이와 무관하게 서버의 메모리 사용량이 일정하고, 첫 글자가 화면에
//! 뜨기까지의 지연이 게이트웨이 응답 속도에 그대로 수렴한다.

use axum::{
    body::Body,
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;
use std::net::IpAddr;

use crate::tone;
use crate::AppState;

/// 히스토리로 받아들일 최대 메시지 수. 이보다 길면 가장 최근 것들만 남긴다.
/// 히스토리는 브라우저가 매번 통째로 보내오므로 상한이 없으면 토큰이 무한정 늘어난다.
const MAX_HISTORY_MESSAGES: usize = 12;
/// 메시지 하나의 최대 길이(문자 수).
const MAX_MESSAGE_CHARS: usize = 2_000;
/// 전체 히스토리를 합친 최대 길이(문자 수).
const MAX_TOTAL_CHARS: usize = 8_000;
/// 모델이 생성할 최대 토큰 수.
const MAX_OUTPUT_TOKENS: u32 = 1_024;

/// 브라우저가 보내오는 요청 바디.
#[derive(Deserialize)]
pub struct ChatRequest {
    /// 지금까지의 대화. 서버는 상태를 갖지 않으므로 매번 전부 받는다.
    messages: Vec<InMessage>,
    /// 선택한 말투의 id. "custom"이면 custom_tone을 쓴다.
    tone: String,
    /// 직접 입력한 말투 지시문(tone이 "custom"일 때만 유효).
    custom_tone: Option<String>,
}

#[derive(Deserialize)]
struct InMessage {
    role: String,
    content: String,
}

pub async fn handle_chat(
    State(state): State<AppState>,
    // 클라이언트 IP는 main.rs의 미들웨어가 판별해 익스텐션에 넣어 준다.
    axum::extract::Extension(client_ip): axum::extract::Extension<IpAddr>,
    Json(req): Json<ChatRequest>,
) -> Response {
    // 1) 레이트 리밋. 가장 먼저 검사해서, 초과 시 검증조차 하지 않고 잘라낸다.
    if !state.limiter.check(client_ip) {
        return error_response(
            StatusCode::TOO_MANY_REQUESTS,
            &format!(
                "요청이 너무 잦습니다. {}초 뒤에 다시 시도해주세요.",
                state.limiter.window_secs()
            ),
        );
    }

    // 2) 말투 → system 프롬프트.
    let system_prompt = match tone::build_system_prompt(&req.tone, req.custom_tone.as_deref()) {
        Ok(p) => p,
        Err(msg) => return error_response(StatusCode::BAD_REQUEST, &msg),
    };

    // 3) 히스토리 검증.
    let history = match sanitize_history(req.messages) {
        Ok(h) => h,
        Err(msg) => return error_response(StatusCode::BAD_REQUEST, &msg),
    };

    // 4) 업스트림에 보낼 메시지 배열: system 하나 + 검증된 히스토리.
    let mut messages = Vec::with_capacity(history.len() + 1);
    messages.push(json!({ "role": "system", "content": system_prompt }));
    for m in history {
        messages.push(json!({ "role": m.role, "content": m.content }));
    }

    let upstream_body = json!({
        "model": state.cfg.model,
        "messages": messages,
        "stream": true,
        // 말투를 살리려면 약간의 자유도가 필요하다. 너무 낮으면 문체가 밋밋해진다.
        "temperature": 0.8,
        "max_tokens": MAX_OUTPUT_TOKENS,
    });

    // 5) 게이트웨이 호출. API 키는 이 헤더에만 쓰이고, 응답 어디에도 실리지 않는다.
    let url = format!("{}/chat/completions", state.cfg.base_url);
    let upstream = state
        .http
        .post(&url)
        .bearer_auth(&state.cfg.api_key)
        .json(&upstream_body)
        .send()
        .await;

    let upstream = match upstream {
        Ok(r) => r,
        Err(e) => {
            // 에러 원문은 서버 로그에만 남긴다. URL이나 키 관련 정보가 사용자에게
            // 새어 나가지 않도록 클라이언트에는 일반적인 메시지만 준다.
            eprintln!("[upstream] 요청 실패: {e}");
            return error_response(StatusCode::BAD_GATEWAY, "AI 서버에 연결하지 못했습니다.");
        }
    };

    if !upstream.status().is_success() {
        let status = upstream.status();
        // 업스트림 에러 본문도 로그로만. 여기에 키 관련 힌트가 들어 있을 수 있다.
        let body = upstream.text().await.unwrap_or_default();
        eprintln!("[upstream] {status} 응답: {body}");
        return error_response(
            StatusCode::BAD_GATEWAY,
            "AI 서버가 요청을 거부했습니다. 잠시 후 다시 시도해주세요.",
        );
    }

    // 6) 릴레이. bytes_stream()은 도착한 청크를 그대로 넘겨주고,
    //    Body::from_stream()은 그것을 복사 없이 응답 바디로 감싼다.
    //    응답 전체를 메모리에 모으는 지점이 한 군데도 없다.
    let stream = upstream.bytes_stream();

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        // nginx 등 리버스 프록시가 SSE를 버퍼링해 스트리밍을 망치는 것을 막는다.
        .header("X-Accel-Buffering", "no")
        .body(Body::from_stream(stream))
        // 헤더가 전부 상수라 실패할 수 없지만, unwrap 대신 명시적으로 처리한다.
        .unwrap_or_else(|_| error_response(StatusCode::INTERNAL_SERVER_ERROR, "응답 생성 실패"))
}

/// 검증을 통과한 메시지.
struct CleanMessage {
    role: &'static str,
    content: String,
}

/// 히스토리를 검증하고 다듬는다.
///
/// - 역할은 user/assistant만 허용한다. 클라이언트가 "system"을 끼워 넣어
///   서버가 세운 말투 지침을 덮어쓰는 것을 막기 위해서다.
/// - 최근 MAX_HISTORY_MESSAGES개만 남긴다.
/// - 길이 상한을 넘으면 거부한다.
fn sanitize_history(messages: Vec<InMessage>) -> Result<Vec<CleanMessage>, String> {
    if messages.is_empty() {
        return Err("메시지가 비어 있습니다.".to_string());
    }

    // 최근 것부터 남긴다. 오래된 앞부분이 잘려 나간다.
    let start = messages.len().saturating_sub(MAX_HISTORY_MESSAGES);
    let recent = &messages[start..];

    let mut total_chars = 0usize;
    let mut clean = Vec::with_capacity(recent.len());

    for m in recent {
        let role = match m.role.as_str() {
            "user" => "user",
            "assistant" => "assistant",
            other => return Err(format!("허용되지 않는 역할입니다: {other}")),
        };

        let content = m.content.trim();
        if content.is_empty() {
            continue; // 빈 메시지는 그냥 버린다.
        }

        let len = content.chars().count();
        if len > MAX_MESSAGE_CHARS {
            return Err(format!("메시지가 너무 깁니다. {MAX_MESSAGE_CHARS}자 이내로 입력해주세요."));
        }

        total_chars += len;
        if total_chars > MAX_TOTAL_CHARS {
            return Err("대화가 너무 깁니다. 새 대화를 시작해주세요.".to_string());
        }

        clean.push(CleanMessage {
            role,
            content: content.to_string(),
        });
    }

    if clean.is_empty() {
        return Err("보낼 내용이 없습니다.".to_string());
    }

    Ok(clean)
}

/// 에러를 JSON으로 돌려준다. 프런트는 응답이 SSE가 아니면 이 형식으로 파싱한다.
fn error_response(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}
