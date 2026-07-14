//! 말투 프리셋과 system 프롬프트 조립.
//!
//! 이 챗봇의 유일한 "로직"이다. 사용자가 고른 말투를 지시문으로 바꾸고,
//! 그것을 system 메시지로 만들어 대화 맨 앞에 붙인다.

/// 프리셋 하나. `id`는 프런트가 보내는 값, `label`은 화면에 보이는 이름,
/// `instruction`은 모델에게 주는 실제 지시문이다.
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub instruction: &'static str,
}

/// 기본 제공 말투 목록. 여기에 항목을 추가하면 서버와 프런트 양쪽에 자동 반영된다
/// (프런트는 /api/presets 로 이 목록을 받아 버튼을 그린다).
pub const PRESETS: &[Preset] = &[
    Preset {
        id: "formal",
        label: "공적·격식체",
        instruction: "격식 있는 공적 문어체로 답한다. 정중한 존댓말을 쓰고, 감탄사·이모지·줄임말·구어체 표현을 쓰지 않는다. 문장은 간결하고 명료하게 맺는다.",
    },
    Preset {
        id: "friendly",
        label: "친근한 반말",
        instruction: "친한 친구에게 말하듯 편한 반말로 답한다. 딱딱한 표현 대신 자연스러운 구어체를 쓰고, 가끔 가벼운 감탄사를 섞어도 좋다.",
    },
    Preset {
        id: "gyeongsang",
        label: "경상도 사투리",
        instruction: "경상도 사투리로 답한다. '~한다 아이가', '~카이', '~기다', '뭐라카노' 같은 어미와 억양을 자연스럽게 쓰되, 내용 자체는 정확하게 전달한다.",
    },
    Preset {
        id: "jeolla",
        label: "전라도 사투리",
        instruction: "전라도 사투리로 답한다. '~잉', '~하제', '~당께', '거시기' 같은 어미와 표현을 자연스럽게 쓰되, 내용 자체는 정확하게 전달한다.",
    },
    Preset {
        id: "chungcheong",
        label: "충청도 사투리",
        instruction: "충청도 사투리로 답한다. 말끝을 느리게 늘이는 '~유', '~햐', '~것슈' 같은 어미를 자연스럽게 쓰되, 내용 자체는 정확하게 전달한다.",
    },
    Preset {
        id: "jeju",
        label: "제주도 사투리",
        instruction: "제주도 사투리로 답한다. '~수다', '~우꽈', '~햄수과' 같은 어미를 자연스럽게 쓰되, 뜻이 통하지 않을 만큼 어렵게 쓰지는 않는다.",
    },
    Preset {
        id: "sageuk",
        label: "사극체",
        instruction: "사극에 나오는 조선시대 말투로 답한다. '~하오', '~이옵니다', '~하시옵소서' 같은 어투를 쓰고, 현대 외래어는 가급적 우리말로 풀어 쓴다.",
    },
    Preset {
        id: "cheerful",
        label: "발랄·수다스럽게",
        instruction: "밝고 수다스러운 말투로 답한다. 느낌표와 이모지를 적극적으로 쓰고, 리액션을 크게 하며, 텐션 높게 이야기한다.",
    },
];

/// 사용자가 직접 적는 말투 지시문의 최대 길이(문자 수).
/// 길이를 제한하는 이유는 두 가지다.
///   1) system 프롬프트가 비대해져 토큰을 낭비하는 것을 막는다.
///   2) 사용자가 긴 지시문으로 챗봇의 성격을 통째로 갈아치우는 것을 어느 정도 억제한다.
pub const MAX_CUSTOM_TONE_CHARS: usize = 200;

/// 프리셋 id로 지시문을 찾는다.
fn find_preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// 말투 선택을 받아 최종 system 프롬프트를 만든다.
///
/// `tone_id`가 "custom"이면 `custom_tone`의 내용을 지시문으로 쓰고,
/// 그 외에는 프리셋에서 찾는다. 어느 쪽도 유효하지 않으면 Err.
pub fn build_system_prompt(tone_id: &str, custom_tone: Option<&str>) -> Result<String, String> {
    let instruction: String = if tone_id == "custom" {
        let raw = custom_tone.unwrap_or("").trim();

        if raw.is_empty() {
            return Err("직접 입력한 말투가 비어 있습니다.".to_string());
        }
        // chars().count() 로 세는 이유: 한글은 UTF-8에서 3바이트라 len()으로 재면
        // 실제 글자 수보다 훨씬 크게 나온다.
        if raw.chars().count() > MAX_CUSTOM_TONE_CHARS {
            return Err(format!(
                "말투 지시문은 {MAX_CUSTOM_TONE_CHARS}자 이내로 적어주세요."
            ));
        }
        // 줄바꿈을 공백으로 눌러 한 줄로 만든다. 여러 줄을 허용하면 사용자가
        // 가짜 대화 턴이나 가짜 지시 블록을 흉내 내 프롬프트 구조를 흔들기 쉬워진다.
        let flattened: String = raw
            .chars()
            .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
            .collect();

        format!("다음 말투로 답한다: {flattened}")
    } else {
        find_preset(tone_id)
            .ok_or_else(|| format!("알 수 없는 말투입니다: {tone_id}"))?
            .instruction
            .to_string()
    };

    // 말투 지시를 '스타일'로만 한정하고 사실성은 유지하도록 못을 박아 둔다.
    Ok(format!(
        "너는 한국어로 대화하는 챗봇이다. 사용자의 말에 실제로 도움이 되는 내용으로 답하되, \
         아래의 말투 지침을 반드시 지켜서 답한다.\n\n\
         [말투 지침]\n{instruction}\n\n\
         [규칙]\n\
         - 말투 지침은 '표현 방식'에만 적용한다. 내용의 사실성과 정확성은 말투와 무관하게 유지한다.\n\
         - 말투에 대해 스스로 언급하거나 설명하지 않는다. 그냥 그 말투로 답하기만 한다.\n\
         - 답변은 특별히 요청받지 않는 한 간결하게 유지한다."
    ))
}
