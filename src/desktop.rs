//! Windows 네이티브 창. `cfg(windows)` 에서만 컴파일된다.
//!
//! 유저에게 배포되는 exe 를 더블클릭하면 콘솔창이 아니라 이 창이 뜬다. 창 안에는
//! WebView2(엣지 엔진)로 기존 채팅 UI 를 그린다 — 즉 UI 는 그대로 두고 담는 그릇만
//! 브라우저에서 네이티브 창으로 바꾼 것이다.
//!
//! 서버(axum)는 백그라운드 스레드에서 돌고, 이 창의 이벤트 루프가 메인 스레드를 차지한다.
//! 창을 닫으면 프로세스가 통째로 끝나므로 서버도 함께 내려간다.

use std::net::SocketAddr;

use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};
use wry::WebViewBuilder;

/// 창을 열고 이벤트 루프를 돈다. 창이 닫히면 프로세스를 끝내므로 돌아오지 않는다.
///
/// `addr` 는 백그라운드에서 이미 떠 있는 로컬 서버의 주소다. WebView 는 이 주소를 연다.
pub fn run(addr: SocketAddr) -> ! {
    let event_loop = EventLoop::new();

    let window = match WindowBuilder::new()
        .with_title("MZ 탈출기")
        // 채팅 UI 라 세로로 긴 창이 어울린다. 유저가 크기를 바꿀 수 있게 기본은 resizable.
        .with_inner_size(LogicalSize::new(480.0, 760.0))
        .with_min_inner_size(LogicalSize::new(360.0, 480.0))
        .build(&event_loop)
    {
        Ok(w) => w,
        Err(e) => fatal(&format!("창을 만들지 못했습니다.\n\n{e}")),
    };

    // 서버가 이미 떠 있는 로컬 주소를 연다.
    let url = format!("http://{addr}");
    let webview = match WebViewBuilder::new(&window).with_url(url).build() {
        Ok(v) => v,
        // 거의 유일하게 현실적인 실패는 WebView2 런타임이 없는 경우다(구형 Windows).
        // 콘솔이 숨겨진 릴리스 빌드에서는 로그가 보이지 않으므로 대화상자로 안내한다.
        Err(e) => fatal(&format!(
            "WebView2 런타임을 불러오지 못했습니다.\n\
             Windows 11 에는 기본 내장되어 있으나, 구형 Windows 라면 Microsoft 의 \
             'Evergreen WebView2 런타임' 을 설치해야 합니다.\n\n{e}"
        )),
    };

    event_loop.run(move |event, _, control_flow| {
        // 이벤트가 없으면 잠들어 있는다. 폴링하지 않으므로 유휴 시 CPU 를 쓰지 않는다.
        *control_flow = ControlFlow::Wait;

        // webview 를 클로저 안에 살려 둔다. 여기서 drop 되면 창 내용이 사라진다.
        let _ = &webview;

        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            // Exit 는 tao 가 프로세스를 종료시킨다. 백그라운드 서버 스레드도 함께 끝난다.
            *control_flow = ControlFlow::Exit;
        }
    });
}

/// 네이티브 오류 대화상자를 띄운다. 릴리스 빌드는 콘솔이 없어 eprintln 이 아무 데도 안
/// 보이므로, 유저에게 이유를 알리는 유일한 수단이다. main.rs 의 fatal() 도 이걸 쓴다.
pub fn show_error(message: &str) {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("mz-escaper")
        .set_description(message)
        .show();
}

/// 창 생성 단계의 치명적 오류. 대화상자로 알리고 종료한다.
fn fatal(message: &str) -> ! {
    show_error(message);
    std::process::exit(1);
}
