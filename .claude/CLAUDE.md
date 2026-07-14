# mz-escaper

원하는 말투로 바꿔 답하는 챗봇. Rust(axum) 서버 하나가 LLM 게이트웨이 앞에 서서
**API 키를 브라우저로부터 격리**하는 것이 이 프로젝트의 존재 이유다.

## 절대 깨뜨리면 안 되는 것

1. **API 키는 서버 밖으로 나가지 않는다.**
   - 키를 읽는 지점은 `src/config.rs` 하나뿐이다. 다른 파일에서 `LLM_API_KEY` 를 읽지 않는다.
   - 키·업스트림 URL·모델명을 응답 바디나 헤더에 싣지 않는다. 업스트림 에러 본문에는
     키 관련 힌트가 들어 있을 수 있으므로 **로그로만** 내보내고, 클라이언트에는 일반적인
     메시지만 준다 (`src/chat.rs` 의 `error_response`).
   - 프런트가 게이트웨이를 직접 호출하게 만드는 변경은 이 프로젝트의 목적을 무효화한다.

2. **공개 서비스라는 전제를 유지한다.**
   - 이 서버로 들어오는 모든 요청은 내 게이트웨이 토큰을 소모한다. 레이트 리밋과 길이
     상한을 제거하거나 느슨하게 만들지 않는다.
   - 클라이언트가 보낸 `system` 역할 메시지는 거부한다 (`sanitize_history`). 허용하면
     서버가 세운 말투 지침을 클라이언트가 덮어쓸 수 있다.
   - `TRUST_PROXY` 는 실제 리버스 프록시 뒤에 있을 때만 `true`. 아니면 `X-Forwarded-For`
     위조로 레이트 리밋이 무력화된다.

3. **스트리밍은 파싱하지 않고 릴레이한다.**
   - `src/chat.rs` 는 업스트림 SSE를 `Body::from_stream` 으로 그대로 흘려보낸다.
     응답 전체를 문자열로 모으는 코드를 넣지 않는다. 응답 길이와 무관하게 서버 메모리가
     일정한 것이 이 설계의 핵심이다.

## 구조

```
브라우저 ──POST /api/chat──▶ mz-escaper ──POST /v1/chat/completions──▶ llm.tan-kim.com
        ◀─── SSE 릴레이 ────            ◀──────── SSE 스트림 ─────────
```

| 파일 | 역할 |
|---|---|
| `src/main.rs` | 부트스트랩, 라우팅, 클라이언트 IP 판별 미들웨어 |
| `src/config.rs` | 환경변수 로딩. **키를 읽는 유일한 지점** |
| `src/tone.rs` | 말투 프리셋 → system 프롬프트 조립 |
| `src/ratelimit.rs` | IP별 고정 윈도우 레이트 리밋 (표준 라이브러리만 사용) |
| `src/chat.rs` | 입력 검증 + 게이트웨이 SSE 릴레이 |
| `static/index.html` | UI 전체. `include_str!` 로 바이너리에 내장된다 |

서버는 **무상태**다. 대화 히스토리는 브라우저에만 있고 매 요청에 통째로 실려 온다.
서버에 세션 저장소를 추가하지 않는다 — 인스턴스를 늘릴 때 제약이 생긴다.

## 자주 하는 작업

**말투 추가**: `src/tone.rs` 의 `PRESETS` 배열에 항목 하나를 넣는다. 프런트는
`/api/presets` 로 목록을 받아 버튼을 그리므로 HTML은 건드릴 필요가 없다.
말투 목록의 유일한 출처는 `tone.rs` 다.

**UI 수정**: `static/index.html` 하나만 고친다. 빌드 도구도 프레임워크도 없다.
바이너리에 내장되므로 수정 후 반드시 다시 빌드해야 반영된다.

## 개발 환경

- Windows / PowerShell 기준. Bash 문법(`&&` 등)을 쓰지 않는다.
- 툴체인은 `rust-toolchain.toml` 로 이 레포에 고정되어 있다 (`rust-analyzer`, `clippy`,
  `rustfmt` 포함). 이 디렉터리에서 `cargo` 를 실행하면 rustup이 자동으로 전환한다.
- MSVC 링커가 필요하다 (VS Build Tools의 C++ 워크로드).

```powershell
cargo run --release     # 실행
cargo clippy            # 린트
cargo fmt               # 포맷
```

## 주의

- `.env` 는 절대 커밋하지 않는다. `.gitignore` 에 있다. 새 설정 항목을 추가하면
  `.env.example` 에도 반영한다.
- 주석은 충분히 단다. 특히 "왜 이렇게 했는지"를 남긴다.
- 에러 처리는 필요한 곳에만. 방어 코드를 남발하지 않는다.
- 커밋 메시지는 Conventional Commits, 본문은 한국어.
