//! Words for the stacked channel view, in English and Traditional Chinese.
//!
//! The rest of the window is written in English only and has no language setting yet, so the
//! window uses English. A test copy (see `startup::test_instance`) can be started with
//! `BACKTRACKING_TEST_LANGUAGE=zh-Hant` to check the other wording.

use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Language {
    English,
    TraditionalChinese,
}

impl Language {
    pub(crate) fn current() -> Self {
        static CURRENT: OnceLock<Language> = OnceLock::new();
        *CURRENT.get_or_init(|| {
            let test = crate::startup::test_instance().is_some()
                && std::env::var("BACKTRACKING_TEST_LANGUAGE")
                    .is_ok_and(|value| value.eq_ignore_ascii_case("zh-Hant"));
            if test {
                Self::TraditionalChinese
            } else {
                Self::English
            }
        })
    }

    fn pick<'a>(self, english: &'a str, chinese: &'a str) -> &'a str {
        match self {
            Self::English => english,
            Self::TraditionalChinese => chinese,
        }
    }

    /// The tooltip of a channel's label.
    pub(crate) fn channel(self, number: usize, count: usize, label: &str) -> String {
        match self {
            Self::English => format!("Channel {number} of {count}: {label}"),
            Self::TraditionalChinese => format!("第 {number} 聲道（共 {count} 個）：{label}"),
        }
    }

    pub(crate) fn scroll_hint(self) -> &'static str {
        self.pick(
            "Scroll here or drag the bar on the right to see the other channels.",
            "在這裡捲動，或拖曳右側的捲軸，查看其他聲道。",
        )
    }

    /// The tooltip of a channel group's button.
    pub(crate) fn show_group(self, label: &str) -> String {
        match self {
            Self::English => format!("Show {label}"),
            Self::TraditionalChinese => format!("顯示{label}"),
        }
    }

    pub(crate) fn group_shown(self, label: &str) -> String {
        match self {
            Self::English => format!("{label} is on show and is what you drag out"),
            Self::TraditionalChinese => format!("目前顯示{label}，拖出的也是它"),
        }
    }

    pub(crate) fn no_loudness(self) -> &'static str {
        self.pick(
            "Loudness is measured for mono and stereo sources only.",
            "響度只量測單聲道與立體聲來源。",
        )
    }

    /// Under the history setting, when wide lanes leave room for less than was asked for.
    pub(crate) fn shorter_history(self, channels: usize, seconds: f64) -> String {
        let length = self.duration(seconds);
        match self {
            Self::English => {
                format!("With {channels} channels, memory holds the latest {length}.")
            }
            Self::TraditionalChinese => format!("{channels} 個聲道時，記憶體可保留最近 {length}。"),
        }
    }

    pub(crate) fn duration(self, seconds: f64) -> String {
        let total = seconds.max(0.0).floor() as u64;
        let (minutes, seconds) = (total / 60, total % 60);
        match self {
            Self::English => {
                let part = |value: u64, unit: &str| {
                    format!("{value} {unit}{}", if value == 1 { "" } else { "s" })
                };
                match (minutes, seconds) {
                    (0, seconds) => part(seconds, "second"),
                    (minutes, 0) => part(minutes, "minute"),
                    (minutes, seconds) => {
                        format!("{} {}", part(minutes, "minute"), part(seconds, "second"))
                    }
                }
            }
            Self::TraditionalChinese => match (minutes, seconds) {
                (0, seconds) => format!("{seconds} 秒"),
                (minutes, 0) => format!("{minutes} 分鐘"),
                (minutes, seconds) => format!("{minutes} 分 {seconds} 秒"),
            },
        }
    }

    pub(crate) fn wide_export_field(self) -> &'static str {
        self.pick("Sources with more than two channels", "超過兩個聲道的來源")
    }

    pub(crate) fn wide_export_mode(self, multichannel: bool) -> &'static str {
        if multichannel {
            self.pick("One multichannel WAV", "一個多聲道 WAV")
        } else {
            self.pick("One mono WAV per channel", "每個聲道一個單聲道 WAV")
        }
    }

    pub(crate) fn wide_export_note(self) -> &'static str {
        self.pick(
            "Only the channel group on show is exported.",
            "只匯出目前顯示的聲道群組。",
        )
    }

    pub(crate) fn test_copy_startup(self) -> &'static str {
        self.pick(
            "A test copy does not change the Windows startup entry.",
            "測試用的程式不會變更 Windows 開機啟動設定。",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::Language::{English, TraditionalChinese};

    #[test]
    fn durations_read_as_plain_words_in_both_languages() {
        assert_eq!(English.duration(1.0), "1 second");
        assert_eq!(English.duration(59.9), "59 seconds");
        assert_eq!(English.duration(60.0), "1 minute");
        assert_eq!(English.duration(279.4), "4 minutes 39 seconds");
        assert_eq!(English.duration(61.0), "1 minute 1 second");
        assert_eq!(TraditionalChinese.duration(279.4), "4 分 39 秒");
        assert_eq!(TraditionalChinese.duration(120.0), "2 分鐘");
        assert_eq!(TraditionalChinese.duration(42.0), "42 秒");
    }

    #[test]
    fn every_new_text_exists_in_both_languages() {
        for language in [English, TraditionalChinese] {
            assert!(!language.scroll_hint().is_empty());
            assert!(!language.no_loudness().is_empty());
            assert!(!language.wide_export_field().is_empty());
            assert!(!language.wide_export_note().is_empty());
            assert!(!language.test_copy_startup().is_empty());
            assert_ne!(
                language.wide_export_mode(true),
                language.wide_export_mode(false)
            );
        }
        assert_eq!(
            English.shorter_history(40, 279.0),
            "With 40 channels, memory holds the latest 4 minutes 39 seconds."
        );
        assert_eq!(
            TraditionalChinese.shorter_history(40, 279.0),
            "40 個聲道時，記憶體可保留最近 4 分 39 秒。"
        );
        assert_eq!(English.channel(3, 12, "C"), "Channel 3 of 12: C");
        assert_eq!(TraditionalChinese.show_group("Objects"), "顯示Objects");
        assert_ne!(English.scroll_hint(), TraditionalChinese.scroll_hint());
    }
}
