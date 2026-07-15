//! Windows 시작 시 자동 실행 토글. `cfg(windows)` 에서만 컴파일된다.
//!
//! 현재 로그인 사용자의 레지스트리 Run 키에 이 exe 의 경로를 등록/삭제한다.
//!   HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run
//! HKCU 를 쓰므로 관리자 권한이 필요 없고, 그 사용자에게만 적용된다. 유저 PC 에서
//! 도는 릴레이 모드에 딱 맞는다.
//!
//! 켜 두면 부팅 후 로그인할 때마다 이 창이 뜬다. 그게 성가시면 유저가 다시 끄면 된다.

use std::io;

use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
use winreg::RegKey;

/// Run 키 경로. 사용자 로그인 시 여기 등록된 값들이 실행된다.
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// 우리 항목의 값 이름. 이 이름으로 등록/조회/삭제한다.
const VALUE_NAME: &str = "mz-escaper";

/// 자동 실행이 켜져 있는가.
///
/// 단순히 값의 존재만 보지 않고 현재 exe 경로와 일치하는지까지 확인한다. exe 를 다른
/// 폴더로 옮겼다면 예전 등록은 우리 것이 아니라고 보고 false 를 준다 — 그래야 토글이
/// "지금 이 exe 가 자동 실행되는가"를 정확히 반영한다.
pub fn is_enabled() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(run) = hkcu.open_subkey(RUN_KEY) else {
        return false;
    };
    let Ok(registered) = run.get_value::<String, _>(VALUE_NAME) else {
        return false;
    };

    // 등록값은 공백 있는 경로 대비 따옴표로 감싸 저장한다. 벗겨서 비교한다.
    let registered = registered.trim().trim_matches('"');
    registered.eq_ignore_ascii_case(&exe.to_string_lossy())
}

/// 자동 실행을 켜거나 끈다.
///
/// 켜기: 현재 exe 경로를 Run 키에 쓴다(따옴표로 감싼다). 끄기: 값이 있으면 지우고,
/// 이미 없으면 성공으로 친다 — 결과 상태("꺼짐")가 같기 때문이다.
pub fn set_enabled(on: bool) -> io::Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);

    if on {
        let exe = std::env::current_exe()?;
        let (run, _) = hkcu.create_subkey(RUN_KEY)?;
        // 경로에 공백이 있어도 하나의 인자로 실행되도록 따옴표로 감싼다.
        let value = format!("\"{}\"", exe.to_string_lossy());
        run.set_value(VALUE_NAME, &value)?;
    } else {
        // 삭제만 할 것이므로 쓰기 권한으로 연다. 키 자체가 없거나 값이 없으면
        // 이미 "꺼짐" 상태이므로 오류로 보지 않는다.
        match hkcu.open_subkey_with_flags(RUN_KEY, KEY_SET_VALUE) {
            Ok(run) => match run.delete_value(VALUE_NAME) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }

    Ok(())
}
