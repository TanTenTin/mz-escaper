//! 환경변수 로딩.
//!
//! API 키를 읽는 지점은 이 파일 하나뿐이다. 여기서 읽은 값은 `Config` 안에 담겨
//! 서버 프로세스 메모리에만 머무르며, 어떤 핸들러도 이 값을 응답 바디에 싣지 않는다.
//!
//! 같은 바이너리가 두 역할을 겸한다. 구분 기준은 `LLM_API_KEY` 하나다.
//!
//!   백엔드 모드 (키 있음) — 내가 호스팅하는 서버. 게이트웨이를 직접 부른다.
//!   릴레이 모드 (키 없음) — 유저에게 배포된 exe. 키를 갖지 않고 내 백엔드로 넘긴다.
//!
//! 유저가 받는 exe 에는 `.env` 가 없으므로 자동으로 릴레이가 된다. 이 구조 덕분에
//! 배포된 바이너리를 아무리 뜯어봐도 키가 나오지 않는다.

use std::time::Duration;

/// 업데이트 매니페스트의 기본 주소. GitHub Releases 의 "latest" 는 릴리스를 새로 내도
/// 주소가 바뀌지 않으므로, 아무리 오래된 바이너리도 이 URL 하나로 최신을 찾아간다.
const DEFAULT_UPDATE_MANIFEST_URL: &str =
    "https://github.com/TanTenTin/mz-escaper/releases/latest/download/version.json";

/// 릴레이 모드일 때 요청을 넘길 백엔드 주소.
///
/// TODO: 아직 실제 도메인이 없다. 배포 전에 실제 주소로 바꿔야 한다. 이 값이 틀리면
/// 배포된 exe 는 채팅이 전부 실패한다(업데이트는 별개 주소라 계속 동작한다).
const DEFAULT_BACKEND_URL: &str = "https://mz.tan-kim.com";

#[derive(Clone)]
pub struct Config {
    /// LLM 게이트웨이 API 키. 업스트림 요청의 Authorization 헤더에만 쓰인다.
    /// None 이면 릴레이 모드다 — 이 프로세스는 키를 모른다.
    pub api_key: Option<String>,
    /// OpenAI 호환 베이스 URL (예: https://llm.tan-kim.com/v1). 백엔드 모드에서만 쓴다.
    pub base_url: String,
    /// 릴레이 모드에서 요청을 넘길 내 백엔드 주소. 뒤에 /api/chat 이 붙는다.
    pub backend_url: String,
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
    /// 환경변수에서 설정을 읽는다. 형식이 틀린 값이 있으면 이유를 담아 Err를 돌려준다.
    ///
    /// `LLM_API_KEY` 가 없는 것은 오류가 아니다 — 릴레이 모드라는 뜻이다.
    pub fn from_env() -> Result<Self, String> {
        // .env가 있으면 읽고, 없으면 조용히 넘어간다(운영에서는 진짜 환경변수를 쓴다).
        // 유저에게 배포된 exe 에는 .env 가 없고, 그래서 릴레이 모드로 뜬다.
        let _ = dotenvy::dotenv();

        // 키를 넣긴 했는데 빈 값이면, 백엔드로 띄우려다 실수한 상황이다. 조용히 릴레이로
        // 흘려보내면 원인을 찾기 어려우니 여기서 끊는다.
        let api_key = match std::env::var("LLM_API_KEY") {
            Ok(v) if v.trim().is_empty() => {
                return Err(
                    "LLM_API_KEY 가 비어 있습니다. 릴레이 모드로 띄우려면 아예 설정하지 마세요."
                        .to_string(),
                );
            }
            Ok(v) => Some(v.trim().to_string()),
            Err(_) => None,
        };

        Ok(Config {
            api_key,
            // trim_end_matches('/'): 뒤에 슬래시를 붙여 넣어도 URL이 //로 깨지지 않게 한다.
            base_url: env_or("LLM_BASE_URL", "https://llm.tan-kim.com/v1")
                .trim_end_matches('/')
                .to_string(),
            backend_url: env_or("BACKEND_URL", DEFAULT_BACKEND_URL)
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

    /// 키를 쥐고 게이트웨이를 직접 부르는 쪽인가. 아니면 내 백엔드로 넘기는 쪽인가.
    pub fn is_backend(&self) -> bool {
        self.api_key.is_some()
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
