//! /api/chat 핸들러: 검증 → 업스트림 호출 → SSE 릴레이.
//!
//! 업스트림이 어디인지는 모드에 따라 다르다 (config.rs 참고).
//!
//!   백엔드 모드 — 키를 쥐고 LLM 게이트웨이를 직접 부른다. system 프롬프트도 여기서 만든다.
//!   릴레이 모드 — 유저 PC의 exe. 키가 없으므로 검증만 하고 내 백엔드의 /api/chat 으로 넘긴다.
//!
//! 어느 쪽이든 마지막 단계는 같다. 업스트림이 보내오는 SSE를 파싱하거나 모아 두지 않고,
//! 바이트 청크가 도착하는 즉시 그대로 브라우저로 흘려보낸다. 덕분에 응답 길이와 무관하게
//! 메모리 사용량이 일정하고, 릴레이를 한 단 더 거쳐도 첫 글자까지의 지연이 거의 늘지 않는다.

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

    // 2) 말투 검증. 릴레이 모드에서도 여기서 한 번 거른다. 잘못된 말투 id를 백엔드까지
    //    보내 봐야 어차피 거부당하고, 왕복만 한 번 낭비된다.
    let system_prompt = match tone::build_system_prompt(&req.tone, req.custom_tone.as_deref()) {
        Ok(p) => p,
        Err(msg) => return error_response(StatusCode::BAD_REQUEST, &msg),
    };

    // 3) 히스토리 검증. 릴레이 모드에서도 길이 상한을 여기서 적용한다. 유저 PC의 exe가
    //    1차 관문이 되므로, 백엔드에 닿는 요청의 크기가 미리 걸러진다.
    //    (물론 백엔드도 같은 검증을 다시 한다 — exe는 유저 손에 있어 신뢰할 수 없다.)
    let history = match sanitize_history(req.messages) {
        Ok(h) => h,
        Err(msg) => return error_response(StatusCode::BAD_REQUEST, &msg),
    };

    // 4) 모드에 따라 업스트림이 갈린다.
    let upstream = if let Some(api_key) = &state.cfg.api_key {
        // ── 백엔드 모드 ──
        // 메시지 배열: system 하나 + 검증된 히스토리. system 프롬프트는 키를 쥔 이 서버만
        // 만든다. 클라이언트가 보낸 system 역할은 sanitize_history 가 이미 거부했다.
        let mut messages = Vec::with_capacity(history.len() + 1);
        messages.push(json!({ "role": "system", "content": system_prompt }));
        for m in history {
            messages.push(json!({ "role": m.role, "content": m.content }));
        }

        let body = json!({
            "model": state.cfg.model,
            "messages": messages,
            "stream": true,
            // 말투를 살리려면 약간의 자유도가 필요하다. 너무 낮으면 문체가 밋밋해진다.
            "temperature": 0.8,
            "max_tokens": MAX_OUTPUT_TOKENS,
        });

        // API 키는 이 헤더에만 쓰이고, 응답 어디에도 실리지 않는다.
        let url = format!("{}/chat/completions", state.cfg.base_url);
        state
            .http
            .post(&url)
            .bearer_auth(api_key)
            .json(&body)
            .send()
    } else {
        // ── 릴레이 모드 ──
        // 검증된 히스토리와 말투를 그대로 백엔드의 /api/chat 에 넘긴다. system 프롬프트를
        // 여기서 만들어 보내지 않는 이유: 백엔드는 클라이언트가 준 system 을 거부한다.
        // 말투 조립은 키를 쥔 쪽의 몫이다.
        let messages: Vec<_> = history
            .iter()
            .map(|m| json!({ "role": m.role, "content": m.content }))
            .collect();

        let body = json!({
            "messages": messages,
            "tone": req.tone,
            "custom_tone": req.custom_tone,
        });

        let url = format!("{}/api/chat", state.cfg.backend_url);
        state.http.post(&url).json(&body).send()
    };

    let upstream = match upstream.await {
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
        let body = upstream.text().await.unwrap_or_default();
        eprintln!("[upstream] {status} 응답: {body}");

        // 릴레이 모드의 업스트림은 내 백엔드다. 그쪽 에러 본문은 이미 사용자용으로 다듬어져
        // 있고 키 관련 정보가 없으므로, 상태와 메시지를 그대로 넘겨준다. 그래야 "요청이
        // 너무 잦습니다" 같은 안내가 유저에게 제대로 보인다.
        if !state.cfg.is_backend() {
            return relay_error(status, &body);
        }

        // 백엔드 모드의 업스트림은 게이트웨이다. 본문에 키 관련 힌트가 있을 수 있으므로
        // 로그로만 남기고 클라이언트에는 일반적인 메시지만 준다.
        return error_response(
            StatusCode::BAD_GATEWAY,
            "AI 서버가 요청을 거부했습니다. 잠시 후 다시 시도해주세요.",
        );
    }

    // 5) 릴레이. bytes_stream()은 도착한 청크를 그대로 넘겨주고,
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
            return Err(format!(
                "메시지가 너무 깁니다. {MAX_MESSAGE_CHARS}자 이내로 입력해주세요."
            ));
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

/// 릴레이 모드에서 백엔드가 준 에러를 그대로 유저에게 전달한다.
///
/// 백엔드의 에러 본문은 이 코드가 만든 것(`error_response`)이라 형식을 안다. 다만 백엔드가
/// 아닌 무언가(프록시, 로드밸런서)가 응답했을 수도 있으므로, `error` 필드가 없으면 일반
/// 메시지로 바꾼다. 유저에게 HTML 에러 페이지 원문을 보여줄 이유는 없다.
fn relay_error(status: StatusCode, body: &str) -> Response {
    let message = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error")?.as_str().map(str::to_string))
        .unwrap_or_else(|| "AI 서버가 요청을 거부했습니다. 잠시 후 다시 시도해주세요.".to_string());

    error_response(status, &message)
}
