# mz-escaper

원하는 말투로 바꿔 답하는 챗봇.

이름의 뜻은 **알아듣기 힘든 요즘 말투(MZ)로부터의 탈출**이다. 무슨 소린지 모르겠는 말을
던져 넣으면 내가 고른 말투로 다시 말해 주고, 아무 말이나 걸어도 그 말투로 답이 돌아온다.
공적인 문어체, 지역 사투리, 사극체, 혹은 직접 적어 넣은 아무 말투나 된다.

Rust(axum) 단일 바이너리 하나로 돌아가고, **API 키는 서버 밖으로 나가지 않는다.**

## 왜 서버가 필요한가

브라우저에서 LLM 게이트웨이를 직접 부르면 API 키가 반드시 클라이언트 코드에 들어가고,
그 순간 개발자 도구에서 그대로 보인다. 그래서 키를 쥔 얇은 서버를 하나 세우고, 브라우저는
그 서버의 `/api/chat` 만 알게 한다. 게이트웨이 주소도, 모델명도, 키도 브라우저는 모른다.

```
브라우저 ──POST /api/chat──▶ mz-escaper ──POST /v1/chat/completions──▶ llm.tan-kim.com
        ◀─── SSE 릴레이 ────            ◀──────── SSE 스트림 ─────────
                                  │
                           API 키는 여기에만 존재
```

## 실행

### 1. 사전 준비

- Rust 툴체인 (`rustup`). 버전은 `rust-toolchain.toml` 로 이 레포에 고정되어 있다.
- Windows라면 **MSVC 링커**가 필요하다. 없으면 빌드가 `linker link.exe not found` 로 실패한다.

  ```powershell
  winget install --id Microsoft.VisualStudio.2022.BuildTools --override `
    "--quiet --wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
  ```

### 2. 설정

```powershell
Copy-Item .env.example .env
notepad .env   # LLM_API_KEY 를 채운다
```

| 변수 | 기본값 | 설명 |
|---|---|---|
| `LLM_API_KEY` | (필수) | 게이트웨이 API 키. 없으면 서버가 즉시 종료한다 |
| `LLM_BASE_URL` | `https://llm.tan-kim.com/v1` | OpenAI 호환 베이스 URL |
| `LLM_MODEL` | `gemini-2.0-flash` | 사용할 모델 |
| `BIND_ADDR` | `127.0.0.1:8080` | 바인드 주소. 공개 서비스면 `0.0.0.0:8080` |
| `TRUST_PROXY` | `false` | 리버스 프록시 뒤에 있을 때만 `true` |
| `RATE_MAX_REQUESTS` | `20` | IP당 허용 요청 수 |
| `RATE_WINDOW_SECS` | `60` | 레이트 리밋 윈도우(초) |

### 3. 빌드 & 실행

```powershell
cargo run --release
# → http://127.0.0.1:8080
```

배포는 `target/release/mz-escaper.exe` **파일 하나만** 옮기면 된다 (약 2.8MB).
UI(HTML/CSS/JS)는 `include_str!` 로 바이너리에 내장되어 있어 정적 파일이 따로 필요 없다.

## 구조

| 파일 | 역할 |
|---|---|
| `src/main.rs` | 부트스트랩, 라우팅, 클라이언트 IP 판별 미들웨어 |
| `src/config.rs` | 환경변수 로딩. **API 키를 읽는 유일한 지점** |
| `src/tone.rs` | 말투 프리셋 → system 프롬프트 조립 |
| `src/ratelimit.rs` | IP별 고정 윈도우 레이트 리밋 |
| `src/chat.rs` | 입력 검증 + 게이트웨이 SSE 릴레이 |
| `static/index.html` | UI 전체 (빌드 시 바이너리에 내장) |

의존성은 6개뿐이다: `axum`, `tokio`, `reqwest`, `serde`, `serde_json`, `dotenvy`.

### 엔드포인트

| 메서드 | 경로 | 설명 |
|---|---|---|
| GET | `/` | UI |
| GET | `/api/presets` | 말투 목록. 프런트가 이걸로 버튼을 그린다 |
| POST | `/api/chat` | 채팅. SSE 스트림으로 응답 |
| GET | `/healthz` | 헬스체크 |

## 설계상의 선택

**스트리밍은 파싱 없이 릴레이한다.** 게이트웨이가 주는 SSE 바이트를 서버가 해석하거나
버퍼에 모으지 않고 그대로 브라우저로 넘긴다(`Body::from_stream`). 응답이 아무리 길어도
서버 메모리 사용량이 일정하고, 프록시를 한 단 거치면서 생기는 지연이 사실상 없다.

**서버는 무상태다.** 대화 히스토리는 브라우저 메모리에만 있고 매 요청에 통째로 실려 온다.
서버에 세션 저장소가 없으므로 인스턴스를 늘리는 데 아무 제약이 없다.

**말투 목록의 출처는 `tone.rs` 하나다.** 프런트는 `/api/presets` 로 목록을 받아 버튼을
그리므로, 말투를 추가하려면 `PRESETS` 배열에 항목 하나만 넣으면 된다. HTML은 손대지 않는다.

## 공개 서비스로 띄울 때

이 서버로 들어오는 모든 요청은 결국 **내 게이트웨이 토큰을 쓴다.** 그래서 기본으로 들어 있는 것:

- IP당 레이트 리밋 (기본 60초당 20회)
- 메시지 하나당 2,000자 / 대화 전체 8,000자 / 히스토리 최근 12개 상한
- 클라이언트가 `system` 역할 메시지를 끼워 넣어 말투 지침을 덮어쓰는 것을 차단
- 직접 입력 말투는 200자 제한 + 줄바꿈 제거

추가로 챙길 것:

- **`TRUST_PROXY` 는 진짜 프록시 뒤에 있을 때만 `true`로 둔다.** 프록시가 없는데 켜 두면
  아무나 `X-Forwarded-For` 를 위조해 매 요청마다 다른 IP인 척하며 레이트 리밋을 우회한다.
- HTTPS는 앞단(nginx / Cloudflare)에서 종단한다. 이 서버는 평문 HTTP만 말한다.
- 레이트 리밋은 프로세스 메모리에 있다. 인스턴스를 여러 개 띄우면 리밋이 인스턴스별로
  따로 세어지므로, 그때는 앞단 프록시에서 리밋을 거는 편이 낫다.

## 개발

```powershell
cargo clippy     # 린트
cargo fmt        # 포맷
```

이 디렉터리에서 `cargo` 를 실행하면 rustup이 `rust-toolchain.toml` 에 적힌 툴체인으로
자동 전환한다. `rust-analyzer`(LSP), `clippy`, `rustfmt` 가 그 툴체인에 함께 설치된다.
