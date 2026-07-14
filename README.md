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
| `UPDATE_MANIFEST_URL` | (없음) | 업데이트 매니페스트 주소. 비우면 업데이트 기능이 꺼진다 |

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
| `src/update.rs` | 버전 판정, 바이너리 다운로드·검증·자체 교체 |
| `static/index.html` | UI 전체 (빌드 시 바이너리에 내장) |

의존성은 9개다: `axum`, `tokio`, `reqwest`, `serde`, `serde_json`, `dotenvy`,
그리고 자체 업데이트용 `semver`, `sha2`, `self-replace`.

### 엔드포인트

| 메서드 | 경로 | 설명 |
|---|---|---|
| GET | `/` | UI |
| GET | `/api/presets` | 말투 목록. 프런트가 이걸로 버튼을 그린다 |
| POST | `/api/chat` | 채팅. SSE 스트림으로 응답 |
| GET | `/healthz` | 헬스체크 |
| GET | `/api/update` | 새 버전 여부. **루프백 전용** |
| POST | `/api/update/apply` | 새 버전으로 교체 후 재시작. **루프백 전용** |

## 배포와 업데이트

유저에게 `mz-escaper.exe` 를 나눠 주는 방식이라, 새 버전을 알리고 갈아끼우는 일을
바이너리가 스스로 한다. `UPDATE_MANIFEST_URL` 에 아래 형태의 JSON 주소를 넣어 두면 켜진다.

```json
{
  "latest":  "0.2.0",
  "minimum": "0.2.0",
  "notes":   "말투 프리셋 3종 추가",
  "assets": {
    "windows-x86_64": {
      "url":    "https://example.com/mz-escaper-0.2.0-windows-x86_64.exe",
      "sha256": "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
    }
  }
}
```

`assets` 의 키는 `{OS}-{ARCH}` 다 (`windows-x86_64`, `linux-x86_64`, `macos-aarch64` …).
`sha256` 은 PowerShell 기준 `Get-FileHash <exe> -Algorithm SHA256` 로 뽑는다.

### 두 개의 버전이 정책을 만든다

| 조건 | 동작 |
|---|---|
| 현재 < `minimum` | **강제.** 시작하면서 자동으로 내려받아 교체하고 재시작한다. 교체에 실패하면 실행하지 않는다 |
| `minimum` ≤ 현재 < `latest` | **선택.** 서버는 정상 기동하고, UI 상단에 배너가 뜬다. 유저가 "지금 업데이트" 를 누를 때만 교체한다 |
| 현재 ≥ `latest` | 아무 일도 없다 |

버전 비교는 semver다. 문자열로 비교하면 `0.10.0 < 0.9.0` 이 되어 최신 버전이 구버전으로
오판된다. 현재 버전은 `Cargo.toml` 의 `version` 이 그대로 박히므로, 릴리스할 때 올릴 곳은
`Cargo.toml` 한 군데뿐이다.

### 확인 시점

매니페스트는 **프로세스가 켜질 때 한 번만** 조회한다. 유저가 exe를 껐다 켜는 주기가 곧
업데이트 확인 주기이고, 서버가 도는 동안 주기적으로 폴링하지 않는다.

조회에 실패하면(오프라인, 매니페스트 서버 다운) 경고만 찍고 그대로 실행한다. 업데이트
서버가 잠깐 죽었다고 유저의 챗봇이 안 켜지는 편이 더 나쁘기 때문이다. 대신 **최소버전
강제는 "온라인일 때 반드시 적용된다" 수준의 보장**이라는 뜻이기도 하다. 오프라인 유저에게
구버전 사용을 원천 차단하지는 못한다.

### 신뢰 경계

- 매니페스트와 에셋은 **https 만** 받는다. 평문이면 중간자가 바꿔치기한 바이너리가
  유저 PC에서 그대로 실행된다.
- 내려받은 파일은 매니페스트의 `sha256` 과 대조한 **뒤에** 교체한다. 검증에 실패하면
  기존 바이너리가 그대로 남아 유저는 쓰던 버전을 계속 쓸 수 있다.
- 매니페스트 자체의 서명 검증은 없다. **매니페스트를 호스팅하는 서버가 신뢰 기반**이다.
  그 서버가 털리면 임의의 바이너리를 배포할 수 있다.
- 업데이트 엔드포인트는 **루프백에서 온 요청만** 받는다. 유저가 `BIND_ADDR` 을 `0.0.0.0`
  으로 열어도 외부인이 바이너리 교체를 트리거하지 못한다.

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
