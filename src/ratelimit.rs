//! IP별 레이트 리밋.
//!
//! 공개 서비스이므로 반드시 필요하다. 이 서버로 들어온 모든 요청은 결국 내 게이트웨이
//! 토큰을 소모하므로, 제한이 없으면 아무나 토큰을 무제한으로 태울 수 있다.
//!
//! 구현은 고정 윈도우(fixed window) 카운터다. 토큰 버킷보다 정밀하진 않지만
//! 항목당 (Instant, u32) 두 개만 들고 있으면 되고 잠금 시간이 아주 짧다.
//! 외부 크레이트를 끌어오지 않고 표준 라이브러리만으로 끝난다.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// IP 하나의 현재 윈도우 상태.
struct Window {
    /// 이 윈도우가 시작된 시각.
    started_at: Instant,
    /// 이 윈도우에서 지금까지 받은 요청 수.
    count: u32,
}

pub struct RateLimiter {
    /// 윈도우당 허용 요청 수.
    max_requests: u32,
    /// 윈도우 길이.
    window: Duration,
    /// IP → 윈도우 상태. 잠금 구간에서 하는 일이 산술 몇 줄뿐이라
    /// 비동기 Mutex가 아니라 표준 Mutex로 충분하다(대기가 사실상 없다).
    buckets: Mutex<HashMap<IpAddr, Window>>,
}

impl RateLimiter {
    pub fn new(max_requests: u32, window: Duration) -> Self {
        RateLimiter {
            max_requests,
            window,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// 이 IP의 요청을 허용할지 판단하고, 허용한다면 카운트를 1 올린다.
    /// 허용이면 true, 한도 초과면 false.
    pub fn check(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        // lock()이 실패하는 경우는 다른 스레드가 잠금을 쥔 채 패닉한 때뿐이다.
        // 그때는 안쪽 데이터가 오염됐을 수 있으니 그냥 꺼내 쓴다(카운터라 손해가 없다).
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());

        // 맵이 무한정 커지는 것을 막는다. 만료된 항목은 다시 조회될 이유가 없으므로
        // 일정 크기를 넘으면 한 번 훑어서 정리한다.
        if buckets.len() > 10_000 {
            buckets.retain(|_, w| now.duration_since(w.started_at) < self.window);
        }

        let entry = buckets.entry(ip).or_insert(Window {
            started_at: now,
            count: 0,
        });

        // 윈도우가 지났으면 새 윈도우를 연다.
        if now.duration_since(entry.started_at) >= self.window {
            entry.started_at = now;
            entry.count = 0;
        }

        if entry.count >= self.max_requests {
            return false;
        }

        entry.count += 1;
        true
    }

    /// 429 응답에 Retry-After 헤더로 실어 줄 초.
    pub fn window_secs(&self) -> u64 {
        self.window.as_secs()
    }
}
