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

유저에게는 `mz-escaper.exe` 를 나눠 주는데, **이 exe 에도 키를 넣지 않는다.** 유저 손에
있는 바이너리는 뜯어볼 수 있기 때문이다. exe 는 UI를 띄우고 요청을 내 백엔드로 넘기기만
한다. 같은 바이너리가 두 역할을 겸하고, 구분 기준은 `LLM_API_KEY` 하나다.

```
유저 PC                                        내 서버
┌──────────┐   ┌──────────────────┐        ┌──────────────────┐      ┌───────────────┐
│ 브라우저 │──▶│ mz-escaper.exe   │───────▶│ mz-escaper       │─────▶│ llm.tan-kim   │
│          │◀──│ (릴레이 모드)    │◀───────│ (백엔드 모드)    │◀─────│ .com          │
└──────────┘   └──────────────────┘  SSE   └──────────────────┘ SSE  └───────────────┘
                  키 없음                     API 키는 여기에만 존재
                  LLM_API_KEY 미설정          LLM_API_KEY 설정됨
```

| 모드 | 조건 | 하는 일 |
|---|---|---|
| 백엔드 | `LLM_API_KEY` 있음 | system 프롬프트를 조립하고 게이트웨이를 직접 호출 |
| 릴레이 | `LLM_API_KEY` 없음 | 검증만 하고 `BACKEND_URL` 의 `/api/chat` 으로 전달 |

배포된 exe 에는 `.env` 가 없으므로 자동으로 릴레이가 된다. 플래그를 따로 줄 필요가 없다.

**백엔드는 사실상 공개 엔드포인트다.** exe 에 비밀이 없으니 exe 를 흉내 내 백엔드를 직접
때리는 것도 막을 수 없다. 그래서 백엔드의 IP 레이트 리밋과 길이 상한이 유일한 방어선이다
(아래 "공개 서비스로 띄울 때" 참고).

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
| `LLM_API_KEY` | (없음) | 게이트웨이 API 키. **있으면 백엔드 모드, 없으면 릴레이 모드** |
| `BACKEND_URL` | `https://mz.tan-kim.com` | 릴레이 모드에서 요청을 넘길 백엔드 |
| `LLM_BASE_URL` | `https://llm.tan-kim.com/v1` | OpenAI 호환 베이스 URL (백엔드 모드) |
| `LLM_MODEL` | `gemini-2.0-flash` | 사용할 모델 (백엔드 모드) |
| `BIND_ADDR` | `127.0.0.1:8080` | 바인드 주소. 공개 서비스면 `0.0.0.0:8080` |
| `TRUST_PROXY` | `false` | 리버스 프록시 뒤에 있을 때만 `true` |
| `RATE_MAX_REQUESTS` | `20` | IP당 허용 요청 수 |
| `RATE_WINDOW_SECS` | `60` | 레이트 리밋 윈도우(초) |
| `UPDATE_MANIFEST_URL` | GitHub Releases 의 `latest` | 업데이트 매니페스트 주소. 빈 값으로 두면 업데이트 확인을 끈다 |

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

## 백엔드 배포 (내 서버)

Docker 로 띄운다. TLS 와 `mz.tan-kim.com` 도메인은 같은 호스트의 nginx 가 맡고, 컨테이너는
평문 HTTP 로 루프백에만 열린다 — nginx 를 건너뛴 직접 접근을 막기 위해서다.

```
DNS: mz.tan-kim.com
        │
        ▼
   nginx (TLS 종료)  ──proxy_pass──▶  127.0.0.1:8080
                                          │
                                   docker compose
                                     mz-escaper (백엔드 모드)
                                          │
                                          ▼
                                   llm.tan-kim.com
```

### 최초 1회 — 서버 준비

```bash
git clone https://github.com/TanTenTin/mz-escaper.git ~/mz-escaper
cd ~/mz-escaper

# .env 작성 (git 제외 대상. CI 의 git reset --hard 가 건드리지 않는다)
cat > .env <<'EOF'
LLM_API_KEY=<게이트웨이 키>
EOF

docker compose up -d --build
curl -fsS http://127.0.0.1:8080/healthz   # → ok
```

nginx 는 `deploy/nginx.conf.example` 을 참고해 사이트를 하나 추가하고, 인증서는
`sudo certbot --nginx -d mz.tan-kim.com` 로 발급한다.

### CI/CD (GitHub Actions)

`main` 에 `src/**` · `static/**` · `Cargo.*` · `Dockerfile` · `docker-compose.yml` 변경을 push 하면
`.github/workflows/deploy.yml` 이 **fmt · clippy · test 를 먼저 돌리고(테스트 게이트), 통과 시에만**
SSH 로 서버에 붙어 `git reset --hard origin/main` → `docker compose up -d --build` 한다.
검사가 깨지면 운영에 나가지 않는다. PR 에서는 검사만 하고 배포하지 않는다.

필요한 GitHub Secrets (`llm-server` 레포와 같은 값):

| Secret | 값 |
|--------|----|
| `ORACLE_HOST` | 서버 공인 IP |
| `ORACLE_USER` | SSH 유저 |
| `ORACLE_SSH_KEY` | SSH 개인키 전체 내용 |

### nginx 에서 틀리기 쉬운 세 줄

- **`proxy_buffering off`** — 없으면 nginx 가 SSE 를 통째로 모았다가 내보내서 스트리밍이
  무너진다. 답변이 다 끝난 뒤에야 화면에 뜬다.
- **`proxy_read_timeout 300s`** — 기본값 60초를 넘기는 긴 답변에서 스트림이 끊긴다.
- **`proxy_set_header X-Forwarded-For $remote_addr;`** — 흔히 쓰는
  `$proxy_add_x_forwarded_for` 를 쓰면 안 된다. 그건 클라이언트가 보낸 헤더 뒤에 실제 IP 를
  덧붙이는데, 서버는 맨 앞 항목을 신뢰한다(`resolve_client_ip`). 유저가 헤더를 위조해
  레이트 리밋을 우회할 수 있다.

### 컨테이너에서는 자체 업데이트를 끈다

compose 가 `UPDATE_MANIFEST_URL: ""` 로 덮어쓴다. 컨테이너 안에서 바이너리를 갈아끼워도
재시작하면 이미지의 것으로 되돌아가므로 의미가 없다. 백엔드 버전은 push 하면 CI 가 올린다.
유저 exe 의 자체 업데이트와는 무관하다.

## 배포와 업데이트 (유저 exe)

유저에게 `mz-escaper.exe` 를 나눠 주는 방식이라, 새 버전을 알리고 갈아끼우는 일을
바이너리가 스스로 한다. 배포는 **GitHub Releases** 로 하고, 릴리스에 함께 올라가는
`version.json` 이 업데이트 정책을 실어 나른다.

### 새 버전 내는 법

```powershell
# 1. 버전을 올린다. (강제 업데이트를 걸 거면 MINIMUM_VERSION 도 함께 올린다)
#    Cargo.toml 의 version = "0.2.0"

# 2. 태그를 민다. 나머지는 GitHub Actions 가 한다.
git tag -a v0.2.0 -m "말투 프리셋 3종 추가"
git push origin v0.2.0
```

`.github/workflows/release.yml` 가 태그를 받아 빌드하고, sha256 을 계산하고,
`version.json` 을 만들어 exe와 함께 릴리스에 올린다. **해시를 손으로 적는 곳은 없다** —
손으로 적으면 언젠가 반드시 틀리고, 틀리는 순간 모든 유저의 업데이트가 실패한다.

태그 메시지의 첫 줄은 유저의 업데이트 배너에 그대로 표시된다.

워크플로는 두 가지를 먼저 검증하고 실패시킨다:

- **태그 ≠ `Cargo.toml` 의 version** → 중단. 이게 어긋나면 업데이트가 무한 루프에 빠진다.
  (매니페스트의 `latest` 보다 바이너리에 박힌 버전이 낮으면, 업데이트를 끝낸 새 바이너리가
  여전히 자신을 구버전으로 판단한다.)
- **`MINIMUM_VERSION` > 릴리스 버전** → 중단. 존재하지 않는 버전으로 전원이 강제 업데이트된다.

### version.json

```json
{
  "latest":  "0.2.0",
  "minimum": "0.1.0",
  "notes":   "말투 프리셋 3종 추가",
  "assets": {
    "windows-x86_64": {
      "url":    "https://github.com/TanTenTin/mz-escaper/releases/download/v0.2.0/mz-escaper-0.2.0-windows-x86_64.exe",
      "sha256": "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
    }
  }
}
```

`assets` 의 키는 `{OS}-{ARCH}` 다 (`windows-x86_64`, `linux-x86_64`, `macos-aarch64` …).
지금 워크플로는 Windows 만 빌드한다.

바이너리가 조회하는 주소는 `…/releases/latest/download/version.json` 하나로 고정이다.
이 주소는 항상 최신 릴리스의 파일을 가리키므로, 아무리 오래된 바이너리도 최신을 찾아간다.
이 값은 `src/config.rs` 에 기본값으로 박혀 있다 — 배포된 exe 에는 `.env` 가 없기 때문이다.

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
