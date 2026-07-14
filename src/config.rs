//! 환경변수 로딩.
//!
//! API 키를 읽는 지점은 이 파일 하나뿐이다. 여기서 읽은 값은 `Config` 안에 담겨
//! 서버 프로세스 메모리에만 머무르며, 어떤 핸들러도 이 값을 응답 바디에 싣지 않는다.

use std::time::Duration;

/// 업데이트 매니페스트의 기본 주소. GitHub Releases 의 "latest" 는 릴리스를 새로 내도
/// 주소가 바뀌지 않으므로, 아무리 오래된 바이너리도 이 URL 하나로 최신을 찾아간다.
const DEFAULT_UPDATE_MANIFEST_URL: &str =
    "https://github.com/TanTenTin/mz-escaper/releases/latest/download/version.json";

#[derive(Clone)]
pub struct Config {
    /// LLM 게이트웨이 API 키. 업스트림 요청의 Authorization 헤더에만 쓰인다.
    pub api_key: String,
    /// OpenAI 호환 베이스 URL (예: https://llm.tan-kim.com/v1)
    pub base_url: String,
    /// 사용할 모델 이름.
    pub model: String,
    /// 서버가 바인드할 주소.
    pub bind_addr: String,
    /// 리버스 프록시 뒤에 있는지 여부. true일 때만 X-Forwarded-For를 신뢰한다.
    pub trust_proxy: bool,
    /// 레이트 리밋: 윈도우당 허용 요청 수.
    pub rate_max_requests: u32,
    /// 레이트 리밋: 윈도우 길이.
    pub rate_window: Duration,
    /// 업데이트 매니페스트(version.json) 주소. 비어 있으면 업데이트 기능 전체가 꺼진다.
    /// 유저에게 배포하는 빌드에서는 반드시 채워야 강제 업데이트가 동작한다.
    pub update_manifest_url: Option<String>,
}

impl Config {
    /// 환경변수에서 설정을 읽는다. 필수값이 없으면 이유를 담아 Err를 돌려준다.
    pub fn from_env() -> Result<Self, String> {
        // .env가 있으면 읽고, 없으면 조용히 넘어간다(운영에서는 진짜 환경변수를 쓴다).
        let _ = dotenvy::dotenv();

        let api_key = std::env::var("LLM_API_KEY").map_err(|_| {
            "LLM_API_KEY 가 설정되지 않았습니다. .env.example 을 .env 로 복사해 채우세요."
                .to_string()
        })?;

        if api_key.trim().is_empty() {
            return Err("LLM_API_KEY 가 비어 있습니다.".to_string());
        }

        Ok(Config {
            api_key,
            // trim_end_matches('/'): 뒤에 슬래시를 붙여 넣어도 URL이 //로 깨지지 않게 한다.
            base_url: env_or("LLM_BASE_URL", "https://llm.tan-kim.com/v1")
                .trim_end_matches('/')
                .to_string(),
            model: env_or("LLM_MODEL", "gemini-2.0-flash"),
            bind_addr: env_or("BIND_ADDR", "127.0.0.1:8080"),
            trust_proxy: env_or("TRUST_PROXY", "false").eq_ignore_ascii_case("true"),
            rate_max_requests: env_parse("RATE_MAX_REQUESTS", 20),
            rate_window: Duration::from_secs(env_parse("RATE_WINDOW_SECS", 60)),
            // 기본값을 코드에 박아 두는 이유: 유저에게는 exe 하나만 건네므로 .env 가 없다.
            // 환경변수에서만 읽으면 배포된 바이너리는 업데이트가 꺼진 채로 돌게 된다.
            // `UPDATE_MANIFEST_URL=` (빈 값)로 두면 개발 중에 확인을 끌 수 있다.
            //
            // /releases/latest/download/ 는 "최신 릴리스"를 따라가는 고정 주소다. 릴리스를
            // 새로 내도 이 URL 은 바뀌지 않으므로 구버전 바이너리도 늘 최신을 찾아간다.
            update_manifest_url: match std::env::var("UPDATE_MANIFEST_URL") {
                // 명시적으로 빈 값을 넣었으면 "끈다"는 뜻이다.
                Ok(v) if v.trim().is_empty() => None,
                Ok(v) => Some(v.trim().to_string()),
                Err(_) => Some(DEFAULT_UPDATE_MANIFEST_URL.to_string()),
            },
        })
    }
}

/// 환경변수를 읽되, 없거나 비어 있으면 기본값을 쓴다.
fn env_or(key: &str, default: &str) -> String {
    match std::env::var(key) {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => default.to_string(),
    }
}

/// 환경변수를 파싱하되, 없거나 형식이 틀리면 기본값을 쓴다.
fn env_parse<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}
