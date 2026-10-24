//! 朗读（翻译面板上的小喇叭）：系统的 AVSpeechSynthesizer，用系统里装好的语音，不联网。
//! 合成器只在主线程上建、用，放在主线程的 thread_local 里一直留着（说话期间被释放就没声了）。
//! 语速、音调、每种语言用哪个声音按设置（翻译 → 朗读）。

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2_avf_audio::{
    AVSpeechBoundary, AVSpeechSynthesisVoice, AVSpeechSynthesisVoiceQuality, AVSpeechSynthesizer,
    AVSpeechUtterance,
};
use objc2_foundation::NSString;

use super::util::on_main;
use crate::error::AppResult;
use crate::platform::types::{SpeakOptions, Voice};

thread_local! {
    static SYNTH: RefCell<Option<Retained<AVSpeechSynthesizer>>> = const { RefCell::new(None) };
}

/// 我们的语言代码 → 系统语音的语言标签；`auto` / 不认识的按文字猜
fn voice_lang(lang: &str, text: &str) -> &'static str {
    match lang {
        "zh" | "zh-CN" => "zh-CN",
        "zh-TW" => "zh-TW",
        "en" => "en-US",
        "ja" => "ja-JP",
        "ko" => "ko-KR",
        "fr" => "fr-FR",
        "de" => "de-DE",
        "es" => "es-ES",
        "ru" => "ru-RU",
        "pt" => "pt-BR",
        "it" => "it-IT",
        "ar" => "ar-SA",
        "th" => "th-TH",
        "vi" => "vi-VN",
        _ => guess(text),
    }
}

fn guess(text: &str) -> &'static str {
    let has = |lo: u32, hi: u32| text.chars().any(|c| (lo..=hi).contains(&(c as u32)));
    if has(0x3040, 0x30FF) {
        "ja-JP"
    } else if has(0xAC00, 0xD7AF) {
        "ko-KR"
    } else if has(0x4E00, 0x9FFF) {
        "zh-CN"
    } else if has(0x0400, 0x04FF) {
        "ru-RU"
    } else {
        "en-US"
    }
}

/// 语速倍数（1 = 正常）→ 系统的语速（0 ~ 1，0.5 是正常；两头不对称：1.0 已经快得听不清）
fn av_rate(rate: f64) -> f32 {
    let r = rate.clamp(0.5, 2.0);
    let av = if r >= 1.0 {
        0.5 + (r - 1.0) * 0.22
    } else {
        0.5 - (1.0 - r) * 0.36
    };
    av as f32
}

/// 朗读一段文字（正在读的先停掉）
pub fn speak(text: &str, lang: &str, options: &SpeakOptions) -> AppResult<()> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Ok(());
    }
    let tag = voice_lang(lang, &text);
    let voice_id = options.voice_for(tag).map(str::to_string);
    let rate = av_rate(options.rate);
    let pitch = options.pitch.clamp(0.5, 2.0) as f32;
    on_main(move |_| {
        SYNTH.with(|cell| {
            let mut slot = cell.borrow_mut();
            // SAFETY: 合成器、语音、句子都在主线程上建和用
            unsafe {
                let synth = slot.get_or_insert_with(|| AVSpeechSynthesizer::new());
                if synth.isSpeaking() {
                    synth.stopSpeakingAtBoundary(AVSpeechBoundary::Immediate);
                }
                let utterance =
                    AVSpeechUtterance::speechUtteranceWithString(&NSString::from_str(&text));
                // 设置里给这种语言选了声音就用它（选的声音被删了的话退回系统默认）
                let voice = voice_id
                    .as_deref()
                    .and_then(|id| {
                        AVSpeechSynthesisVoice::voiceWithIdentifier(&NSString::from_str(id))
                    })
                    .or_else(|| {
                        AVSpeechSynthesisVoice::voiceWithLanguage(Some(&NSString::from_str(tag)))
                    });
                utterance.setVoice(voice.as_deref());
                utterance.setRate(rate);
                utterance.setPitchMultiplier(pitch);
                synth.speakUtterance(&utterance);
            }
        });
    });
    Ok(())
}

/// 是不是正在读
pub fn is_speaking() -> bool {
    on_main(|_| {
        SYNTH.with(|cell| {
            // SAFETY: 主线程上读状态
            cell.borrow()
                .as_ref()
                .is_some_and(|s| unsafe { s.isSpeaking() })
        })
    })
}

/// 系统里装好的声音（设置里选）。同一种语言里增强 / 高品质的排前面
pub fn voices() -> Vec<Voice> {
    // SAFETY: 只读系统的声音列表
    let mut list: Vec<Voice> = unsafe {
        AVSpeechSynthesisVoice::speechVoices()
            .iter()
            .map(|v| Voice {
                id: v.identifier().to_string(),
                name: v.name().to_string(),
                lang: v.language().to_string(),
                quality: match v.quality() {
                    AVSpeechSynthesisVoiceQuality::Premium => 3,
                    AVSpeechSynthesisVoiceQuality::Enhanced => 2,
                    _ => 1,
                },
            })
            .collect()
    };
    list.sort_by(|a, b| {
        a.lang
            .cmp(&b.lang)
            .then(b.quality.cmp(&a.quality))
            .then(a.name.cmp(&b.name))
    });
    list
}

/// 停下正在读的
pub fn stop_speaking() {
    on_main(|_| {
        SYNTH.with(|cell| {
            if let Some(synth) = cell.borrow().as_ref() {
                // SAFETY: 主线程上停
                unsafe {
                    synth.stopSpeakingAtBoundary(AVSpeechBoundary::Immediate);
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn rate_maps_into_the_system_range() {
        assert!((super::av_rate(1.0) - 0.5).abs() < 1e-6);
        assert!(super::av_rate(2.0) <= 0.75 && super::av_rate(2.0) > 0.7);
        assert!(super::av_rate(0.5) >= 0.3 && super::av_rate(0.5) < 0.35);
    }

    #[test]
    fn guesses_language_from_text() {
        assert_eq!(super::voice_lang("auto", "你好"), "zh-CN");
        assert_eq!(super::voice_lang("auto", "こんにちは"), "ja-JP");
        assert_eq!(super::voice_lang("auto", "안녕하세요"), "ko-KR");
        assert_eq!(super::voice_lang("auto", "hello"), "en-US");
        assert_eq!(super::voice_lang("ja", "漢字"), "ja-JP");
    }
}
