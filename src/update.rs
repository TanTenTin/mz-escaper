//! 자체 업데이트.
//!
//! 유저에게 exe 하나를 배포하는 구조라, 새 버전을 알리고 갈아끼우는 일을 바이너리가
//! 스스로 해야 한다. 원격 매니페스트(version.json)에 적힌 두 개의 버전이 정책을 결정한다.
//!
//!   minimum : 이 버전보다 낮으면 실행을 허용하지 않는다. 시작하면서 자동으로 교체한다.
//!             (프로토콜이 바뀌었거나 보안 문제가 있는 구버전을 살려두지 않기 위한 장치)
//!   latest  : 최신 버전. minimum 이상이면 서버는 정상 기동하고, 유저가 UI에서 원할 때
//!             교체한다.
//!
//! 신뢰 경계에 대해:
//!   - 매니페스트와 에셋은 HTTPS로만 받는다. 평문 HTTP면 중간자가 바이너리를 바꿔치기할
//!     수 있고, 그 바이너리는 유저 PC에서 그대로 실행된다.
//!   - 내려받은 파일은 매니페스트의 sha256과 대조한다. 릴리스 스토리지가 오염됐거나
//!     전송이 깨진 경우를 잡는다. (매니페스트 자체의 서명 검증은 아직 없다 — 매니페스트를
//!     호스팅하는 서버가 신뢰 기반이다.)
//!   - 업데이트를 발동시키는 HTTP 엔드포인트는 루프백에서 온 요청만 받는다(main.rs).
//!     유저가 실수로 0.0.0.0 에 바인드해도 외부인이 바이너리 교체를 트리거하지 못한다.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::Duration;

/// 빌드된 바이너리의 버전. Cargo.toml 의 version 이 그대로 박힌다.
/// 릴리스할 때 Cargo.toml 만 올리면 되고, 코드에 버전을 중복해서 적지 않는다.
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 원격 매니페스트. 아래 형태의 JSON을 기대한다.
///
/// ```json
/// {
///   "latest": "0.2.0",
///   "minimum": "0.2.0",
///   "notes": "말투 프리셋 3종 추가",
///   "assets": {
///     "windows-x86_64": {
///       "url": "https://example.com/mz-escaper-0.2.0-windows-x86_64.exe",
///       "sha256": "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
///     }
///   }
/// }
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub latest: String,
    pub minimum: String,
    #[serde(default)]
    pub notes: String,
    /// 플랫폼 키 → 에셋. 키는 `platform_key()` 가 만드는 문자열과 같아야 한다.
    #[serde(default)]
    pub assets: HashMap<String, Asset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    pub url: String,
    pub sha256: String,
}

/// 매니페스트에서 내 플랫폼의 에셋을 찾을 때 쓰는 키. 예: `windows-x86_64`, `linux-x86_64`.
pub fn platform_key() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// 현재 버전과 매니페스트를 견줘 나온 결론.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// 최신이다. 할 일 없음.
    UpToDate,
    /// 새 버전이 있지만 실행은 가능하다. 유저가 결정한다.
    Optional,
    /// minimum 미만이다. 교체하지 않으면 실행하지 않는다.
    Mandatory,
}

/// 시작 시 판정 결과. 서버가 사는 동안 그대로 들고 있다가 /api/update 로 노출한다.
#[derive(Clone)]
pub struct UpdateInfo {
    pub manifest: Manifest,
    pub decision: Decision,
}

/// 매니페스트를 받아온다.
///
/// 실패(오프라인, 서버 다운, JSON 깨짐)는 에러로 돌려주되, 호출부는 이걸 치명적으로
/// 다루지 않는다. 업데이트 서버가 죽었다고 유저의 챗봇까지 못 쓰게 만들 이유는 없다.
pub async fn fetch_manifest(http: &reqwest::Client, url: &str) -> Result<Manifest, String> {
    if !url.starts_with("https://") {
        return Err(format!("UPDATE_MANIFEST_URL 은 https 여야 합니다: {url}"));
    }

    let resp = http
        .get(url)
        // 업데이트 확인 때문에 기동이 늘어지면 안 된다. 짧게 끊는다.
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("매니페스트 요청 실패: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("매니페스트 응답 코드 {}", resp.status()));
    }

    resp.json::<Manifest>()
        .await
        .map_err(|e| format!("매니페스트 파싱 실패: {e}"))
}

/// 현재 버전이 매니페스트 기준으로 어디에 있는지 판정한다.
pub fn decide(current: &str, m: &Manifest) -> Result<Decision, String> {
    // 세 값 모두 semver 여야 한다. 문자열 비교였다면 "0.10.0" < "0.9.0" 이 되어 최신 버전을
    // 구버전으로 오판한다.
    let cur = semver::Version::parse(current)
        .map_err(|e| format!("현재 버전 파싱 실패({current}): {e}"))?;
    let latest = semver::Version::parse(&m.latest)
        .map_err(|e| format!("latest 파싱 실패({}): {e}", m.latest))?;
    let minimum = semver::Version::parse(&m.minimum)
        .map_err(|e| format!("minimum 파싱 실패({}): {e}", m.minimum))?;

    if cur < minimum {
        Ok(Decision::Mandatory)
    } else if cur < latest {
        Ok(Decision::Optional)
    } else {
        Ok(Decision::UpToDate)
    }
}

/// 새 바이너리를 받아 현재 실행 파일을 교체한다. 교체까지만 하고 재시작은 하지 않는다.
///
/// 순서가 중요하다: **검증을 끝낸 뒤에** 교체한다. 받는 도중에 실패하거나 해시가 어긋나면
/// 기존 바이너리는 그대로 남아 있어, 유저는 최소한 쓰던 버전을 계속 쓸 수 있다.
pub async fn apply(http: &reqwest::Client, m: &Manifest) -> Result<(), String> {
    let key = platform_key();
    let asset = m
        .assets
        .get(&key)
        .ok_or_else(|| format!("이 플랫폼({key})용 에셋이 매니페스트에 없습니다."))?;

    if !asset.url.starts_with("https://") {
        return Err(format!("에셋 URL 은 https 여야 합니다: {}", asset.url));
    }

    let resp = http
        .get(&asset.url)
        // 바이너리는 수 MB~수십 MB다. 매니페스트보다 넉넉히 준다.
        .timeout(Duration::from_secs(180))
        .send()
        .await
        .map_err(|e| format!("다운로드 실패: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("다운로드 응답 코드 {}", resp.status()));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("다운로드 본문 읽기 실패: {e}"))?;

    // 해시 대조. 여기서 걸리면 교체 없이 중단한다.
    let actual = hex_sha256(&bytes);
    let expected = asset.sha256.trim().to_ascii_lowercase();
    if actual != expected {
        return Err(format!(
            "해시가 일치하지 않습니다. 기대 {expected}, 실제 {actual}"
        ));
    }

    // 임시 파일에 먼저 쓴다. self_replace 는 '완성된 파일'을 제자리로 옮기는 역할만 한다.
    let tmp = std::env::temp_dir().join(format!("mz-escaper-{}.new", m.latest));
    std::fs::write(&tmp, &bytes).map_err(|e| format!("임시 파일 쓰기 실패: {e}"))?;

    // 유닉스에서는 실행 비트가 없으면 교체해봐야 실행이 안 된다.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("실행 권한 설정 실패: {e}"))?;
    }

    // 파일 I/O라 블로킹이다. 런타임 워커를 잡아두지 않도록 별도 스레드에서 돌린다.
    let tmp_for_task = tmp.clone();
    let result = tokio::task::spawn_blocking(move || self_replace::self_replace(&tmp_for_task))
        .await
        .map_err(|e| format!("교체 작업 실행 실패: {e}"))?;

    // 성공이든 실패든 임시 파일은 남길 이유가 없다.
    let _ = std::fs::remove_file(&tmp);

    result.map_err(|e| format!("바이너리 교체 실패: {e}"))
}

/// 교체된 새 바이너리를 같은 인자로 다시 띄우고, 이 프로세스는 종료한다.
///
/// 돌아오지 않는 함수다. 호출 전에 HTTP 응답을 이미 내보냈어야 한다.
pub fn restart() -> ! {
    let exe = std::env::current_exe().expect("현재 실행 파일 경로를 알 수 없습니다.");
    let args: Vec<String> = std::env::args().skip(1).collect();

    match std::process::Command::new(&exe).args(&args).spawn() {
        Ok(_) => {
            println!("새 버전으로 재시작합니다.");
            std::process::exit(0);
        }
        Err(e) => {
            // 교체는 끝났는데 재실행이 안 된 상황. 유저가 직접 다시 켜면 새 버전이 뜬다.
            eprintln!("[업데이트] 재시작 실패: {e}. 프로그램을 직접 다시 실행해주세요.");
            std::process::exit(1);
        }
    }
}

/// sha256 을 소문자 16진 문자열로.
fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(latest: &str, minimum: &str) -> Manifest {
        Manifest {
            latest: latest.to_string(),
            minimum: minimum.to_string(),
            notes: String::new(),
            assets: HashMap::new(),
        }
    }

    #[test]
    fn 최신이면_할_일이_없다() {
        let m = manifest("1.2.0", "1.0.0");
        assert_eq!(decide("1.2.0", &m).unwrap(), Decision::UpToDate);
    }

    #[test]
    fn 최소버전_이상이면_선택_업데이트다() {
        let m = manifest("1.2.0", "1.0.0");
        assert_eq!(decide("1.1.0", &m).unwrap(), Decision::Optional);
    }

    #[test]
    fn 최소버전_미만이면_강제_업데이트다() {
        let m = manifest("1.2.0", "1.1.0");
        assert_eq!(decide("1.0.9", &m).unwrap(), Decision::Mandatory);
    }

    /// 문자열 비교로 구현했다면 "0.10.0" < "0.9.0" 이라 최신인데도 강제 업데이트가 걸린다.
    #[test]
    fn 두자리_마이너버전을_숫자로_비교한다() {
        let m = manifest("0.10.0", "0.10.0");
        assert_eq!(decide("0.10.0", &m).unwrap(), Decision::UpToDate);
        assert_eq!(decide("0.9.0", &m).unwrap(), Decision::Mandatory);
    }

    #[test]
    fn 빈_바이트열의_sha256() {
        assert_eq!(
            hex_sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
