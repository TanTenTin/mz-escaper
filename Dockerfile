# 백엔드(키를 쥐는 쪽) 이미지.
#
# 유저에게 배포하는 exe 는 GitHub Releases 로 나가고, 이 이미지는 내 서버에서만 돈다.
# 같은 소스지만 .env 로 LLM_API_KEY 를 주므로 백엔드 모드로 뜬다.

# ── 빌드 ──
# rust-toolchain.toml 을 일부러 복사하지 않는다. 그 파일은 rust-analyzer/rust-src 까지
# 요구하는데, 컨테이너 빌드에는 필요 없는 수백 MB 다. 툴체인은 아래 이미지 태그로 고정한다.
FROM rust:1-slim-bookworm AS builder

WORKDIR /app

# 의존성만 먼저 빌드해 레이어에 캐시한다. 소스만 고칠 때 크레이트를 다시 컴파일하지 않는다.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && cargo build --release && rm -rf src

COPY src ./src
COPY static ./static
# 더미 main.rs 로 만든 바이너리가 최신으로 남아 있어, touch 로 재빌드를 강제한다.
RUN touch src/main.rs && cargo build --release

# ── 실행 ──
FROM debian:bookworm-slim

# ca-certificates: 게이트웨이(https)를 부르려면 루트 인증서가 필요하다.
# curl: 아래 compose 의 healthcheck 가 쓴다.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/mz-escaper /usr/local/bin/mz-escaper

# 루트로 돌릴 이유가 없다. 이 프로세스는 파일을 쓰지 않는다.
USER 1000:1000

# 컨테이너 안에서 127.0.0.1 에 바인드하면 밖에서 닿지 않는다.
ENV BIND_ADDR=0.0.0.0:8080
EXPOSE 8080

CMD ["mz-escaper"]
