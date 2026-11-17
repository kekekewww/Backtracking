//! Words for the stacked channel view and the Add-ons page, in English and Traditional Chinese.
//!
//! The rest of the window is written in English only and has no language setting yet, so the
//! window uses English. A test copy (see `startup::test_instance`) can be started with
//! `BACKTRACKING_TEST_LANGUAGE=zh-Hant` to check the other wording.

use std::sync::OnceLock;

use backtracking_addon::manifest::{Capability, OpenFiles};
use backtracking_addon::protocol::MessageLevel;

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
        self.pick("Multichannel sources", "多聲道來源")
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

    // The Add-ons page and the screen shown before an install.

    pub(crate) fn addons_section(self) -> &'static str {
        self.pick("Add-ons", "附加元件")
    }

    pub(crate) fn addons_empty(self) -> &'static str {
        self.pick("No add-ons are installed.", "尚未安裝任何附加元件。")
    }

    pub(crate) fn addons_explained(self) -> &'static str {
        self.pick(
            "An add-on is a separate program that adds features to Backtracking or changes how \
             it works, for example a new audio source with its own settings and buttons.",
            "附加元件是另外的程式，可以為 Backtracking 加入功能或改變它的運作方式，例如提供新的音訊來源，並附有自己的設定與按鈕。",
        )
    }

    pub(crate) fn addons_how_to_add(self) -> &'static str {
        self.pick(
            "To add one, choose Install from file, or drop a .btaddon file on a Backtracking \
             window.",
            "要加入附加元件，請按「從檔案安裝」，或把 .btaddon 檔案拖到 Backtracking 視窗上。",
        )
    }

    pub(crate) fn install_from_file(self) -> &'static str {
        self.pick("Install from file…", "從檔案安裝…")
    }

    pub(crate) fn drop_hint(self) -> &'static str {
        self.pick(
            "You can also drop a .btaddon file on a Backtracking window.",
            "也可以把 .btaddon 檔案拖到 Backtracking 視窗上。",
        )
    }

    pub(crate) fn file_dialog_title(self) -> &'static str {
        self.pick("Install an add-on", "安裝附加元件")
    }

    pub(crate) fn file_filter(self) -> &'static str {
        self.pick("Backtracking add-on", "Backtracking 附加元件")
    }

    /// Over a window while a `.btaddon` file is dragged across it.
    pub(crate) fn drop_to_review(self) -> &'static str {
        self.pick(
            "Drop to review this add-on before it is installed",
            "放開即可在安裝前查看這個附加元件",
        )
    }

    pub(crate) fn version_by(self, version: &str, author: &str) -> String {
        match self {
            Self::English => format!("Version {version} · by {author}"),
            Self::TraditionalChinese => format!("版本 {version}・作者 {author}"),
        }
    }

    pub(crate) fn enabled(self) -> &'static str {
        self.pick("Enabled", "啟用")
    }

    pub(crate) fn state_running(self) -> &'static str {
        self.pick("Running", "執行中")
    }

    pub(crate) fn state_starting(self) -> &'static str {
        self.pick("Starting…", "啟動中…")
    }

    pub(crate) fn state_stopping(self) -> &'static str {
        self.pick("Stopping…", "停止中…")
    }

    pub(crate) fn state_off(self) -> &'static str {
        self.pick("Off", "已關閉")
    }

    pub(crate) fn state_stopped(self) -> &'static str {
        self.pick("Stopped", "已停止")
    }

    pub(crate) fn state_not_responding(self) -> &'static str {
        self.pick(
            "Not responding: it did not connect within 10 seconds, so it was stopped.",
            "沒有回應：它在 10 秒內沒有連線，因此已被停止。",
        )
    }

    pub(crate) fn state_failed(self, reason: &str) -> String {
        match self {
            Self::English => format!("Stopped because of a problem: {reason}"),
            Self::TraditionalChinese => format!("因發生問題而停止：{reason}"),
        }
    }

    pub(crate) fn state_broken(self, reason: &str) -> String {
        match self {
            Self::English => format!("Broken package: {reason}"),
            Self::TraditionalChinese => format!("套件已損壞：{reason}"),
        }
    }

    pub(crate) fn broken_hint(self) -> &'static str {
        self.pick(
            "Remove it, or install the package again.",
            "請移除它，或重新安裝套件。",
        )
    }

    /// The add-on's own status line, after the word for its level.
    pub(crate) fn addon_message(self, level: MessageLevel, text: &str) -> String {
        let word = match (self, level) {
            (Self::English, MessageLevel::Info) => "Message",
            (Self::English, MessageLevel::Warning) => "Warning",
            (Self::English, MessageLevel::Error) => "Error",
            (Self::TraditionalChinese, MessageLevel::Info) => "訊息",
            (Self::TraditionalChinese, MessageLevel::Warning) => "警告",
            (Self::TraditionalChinese, MessageLevel::Error) => "錯誤",
        };
        match self {
            Self::English => format!("{word}: {text}"),
            Self::TraditionalChinese => format!("{word}：{text}"),
        }
    }

    pub(crate) fn settings_heading(self) -> &'static str {
        self.pick("Settings", "設定")
    }

    pub(crate) fn actions_heading(self) -> &'static str {
        self.pick("Actions", "動作")
    }

    pub(crate) fn not_running_hint(self) -> &'static str {
        self.pick(
            "Its settings and buttons appear here while it is running.",
            "附加元件執行時，這裡會顯示它的設定與按鈕。",
        )
    }

    /// For an add-on that was stopped because it kept failing or stopped answering.
    pub(crate) fn start_again(self) -> &'static str {
        self.pick("Start again", "重新啟動")
    }

    pub(crate) fn could_not_start(self, reason: &str) -> String {
        match self {
            Self::English => format!("Could not start it: {reason}"),
            Self::TraditionalChinese => format!("無法啟動：{reason}"),
        }
    }

    pub(crate) fn could_not_change(self, label: &str, reason: &str) -> String {
        match self {
            Self::English => format!("Could not change {label}: {reason}"),
            Self::TraditionalChinese => format!("無法變更「{label}」：{reason}"),
        }
    }

    pub(crate) fn could_not_run(self, label: &str, reason: &str) -> String {
        match self {
            Self::English => format!("Could not run {label}: {reason}"),
            Self::TraditionalChinese => format!("無法執行「{label}」：{reason}"),
        }
    }

    pub(crate) fn could_not_switch(self, on: bool, reason: &str) -> String {
        match (self, on) {
            (Self::English, true) => format!("Could not turn it on: {reason}"),
            (Self::English, false) => format!("Could not turn it off: {reason}"),
            (Self::TraditionalChinese, true) => format!("無法啟用：{reason}"),
            (Self::TraditionalChinese, false) => format!("無法關閉：{reason}"),
        }
    }

    pub(crate) fn remove_button(self) -> &'static str {
        self.pick("Remove…", "移除…")
    }

    pub(crate) fn remove_question(self, name: &str) -> String {
        match self {
            Self::English => format!("Remove {name}?"),
            Self::TraditionalChinese => format!("要移除「{name}」嗎？"),
        }
    }

    pub(crate) fn remove_explained(self) -> &'static str {
        self.pick(
            "Its program is deleted, and its sources leave the source list.",
            "它的程式會被刪除，它提供的來源也會從來源清單中消失。",
        )
    }

    pub(crate) fn delete_data(self) -> &'static str {
        self.pick("Also delete its saved data", "同時刪除它儲存的資料")
    }

    pub(crate) fn keep_data_note(self) -> &'static str {
        self.pick(
            "Unless you delete it, its saved data stays for a later install.",
            "若不刪除，它儲存的資料會保留，供日後重新安裝時使用。",
        )
    }

    pub(crate) fn remove(self) -> &'static str {
        self.pick("Remove", "移除")
    }

    pub(crate) fn cancel(self) -> &'static str {
        self.pick("Cancel", "取消")
    }

    pub(crate) fn removing(self) -> &'static str {
        self.pick("Removing…", "正在移除…")
    }

    pub(crate) fn could_not_remove(self, reason: &str) -> String {
        match self {
            Self::English => format!("Could not remove it: {reason}"),
            Self::TraditionalChinese => format!("無法移除：{reason}"),
        }
    }

    pub(crate) fn removed(self, name: &str) -> String {
        match self {
            Self::English => format!("Removed {name}."),
            Self::TraditionalChinese => format!("已移除「{name}」。"),
        }
    }

    pub(crate) fn installed(self, name: &str, version: &str) -> String {
        match self {
            Self::English => format!("Installed {name} {version}."),
            Self::TraditionalChinese => format!("已安裝「{name}」{version} 版。"),
        }
    }

    pub(crate) fn review_heading(self) -> &'static str {
        self.pick("Install this add-on?", "要安裝這個附加元件嗎？")
    }

    pub(crate) fn reading(self, file: &str) -> String {
        match self {
            Self::English => format!("Reading {file}…"),
            Self::TraditionalChinese => format!("正在讀取 {file}…"),
        }
    }

    pub(crate) fn homepage(self) -> &'static str {
        self.pick("Homepage", "網站")
    }

    pub(crate) fn license(self) -> &'static str {
        self.pick("License", "授權")
    }

    pub(crate) fn program(self) -> &'static str {
        self.pick("Program it runs", "執行的程式")
    }

    pub(crate) fn unpacked_size(self) -> &'static str {
        self.pick("Unpacked size", "解開後大小")
    }

    pub(crate) fn package_file(self) -> &'static str {
        self.pick("Package file", "套件檔案")
    }

    pub(crate) fn size_in_files(self, bytes: u64, files: usize) -> String {
        let size = byte_size(bytes);
        match (self, files) {
            (Self::English, 1) => format!("{size} in 1 file"),
            (Self::English, files) => format!("{size} in {files} files"),
            (Self::TraditionalChinese, files) => format!("{files} 個檔案，共 {size}"),
        }
    }

    pub(crate) fn asks_heading(self) -> &'static str {
        self.pick("What it asks for", "它要求的功能")
    }

    /// One line per declared capability, in the order of the contract's table.
    pub(crate) fn capability(
        self,
        capability: Capability,
        open_files: Option<&OpenFiles>,
    ) -> String {
        match capability {
            Capability::Source => self
                .pick(
                    "Provide audio sources in the source list",
                    "在來源清單中提供音訊來源",
                )
                .to_owned(),
            Capability::OpenFiles => {
                let (description, extensions) =
                    open_files.map_or((String::new(), String::new()), |files| {
                        let separator = self.pick(", ", "、");
                        let extensions = files
                            .extensions
                            .iter()
                            .map(|extension| format!(".{extension}"))
                            .collect::<Vec<_>>()
                            .join(separator);
                        (files.description.clone(), extensions)
                    });
                match self {
                    Self::English => {
                        format!("Open files of these types: {description} ({extensions})")
                    }
                    Self::TraditionalChinese => {
                        format!("開啟這些類型的檔案：{description}（{extensions}）")
                    }
                }
            }
            Capability::Settings => self
                .pick(
                    "Have settings, which Backtracking shows and keeps",
                    "擁有設定，由 Backtracking 顯示並保存",
                )
                .to_owned(),
            Capability::Actions => self
                .pick(
                    "Add buttons to its entry on the Add-ons page",
                    "在「附加元件」頁面加入它的按鈕",
                )
                .to_owned(),
            Capability::Clips => self
                .pick(
                    "Be told about every clip you export, with its file path",
                    "得知你匯出的每個片段及其檔案路徑",
                )
                .to_owned(),
        }
    }

    pub(crate) fn no_capabilities(self) -> &'static str {
        self.pick(
            "Nothing beyond running its program.",
            "除了執行它的程式之外，沒有其他要求。",
        )
    }

    /// The contract promises that the install screen says this.
    pub(crate) fn trust_warning(self) -> &'static str {
        self.pick(
            "An add-on is a program that runs with your own permissions. It is not sandboxed: \
             it can do anything you can do on this computer, whatever the list above says. \
             Install only add-ons you trust.",
            "附加元件是以你自己的權限執行的程式。它沒有沙箱隔離：不論上面列出什麼，它都能做你在這台電腦上能做的任何事。請只安裝你信任的附加元件。",
        )
    }

    pub(crate) fn starts_note(self) -> &'static str {
        self.pick(
            "Once installed, it starts at once and each time Backtracking starts. You can turn \
             it off on the Add-ons page.",
            "安裝後會立即啟動，之後每次開啟 Backtracking 也會啟動。你可以在「附加元件」頁面關閉它。",
        )
    }

    pub(crate) fn replaces(self, old: &str, new: &str) -> String {
        match self {
            Self::English => format!(
                "This replaces version {old} with version {new}. Its settings and saved data are kept."
            ),
            Self::TraditionalChinese => {
                format!("這會以 {new} 版取代已安裝的 {old} 版，並保留它的設定與儲存的資料。")
            }
        }
    }

    pub(crate) fn replaces_broken(self) -> &'static str {
        self.pick(
            "This replaces a broken installation of the same add-on.",
            "這會取代同一個附加元件已損壞的安裝。",
        )
    }

    pub(crate) fn install(self) -> &'static str {
        self.pick("Install", "安裝")
    }

    pub(crate) fn installing(self) -> &'static str {
        self.pick("Installing…", "正在安裝…")
    }

    pub(crate) fn refused_heading(self) -> &'static str {
        self.pick("This file cannot be installed.", "無法安裝這個檔案。")
    }

    pub(crate) fn close(self) -> &'static str {
        self.pick("Close", "關閉")
    }

    pub(crate) fn install_failed(self, reason: &str) -> String {
        match self {
            Self::English => format!("Could not install it: {reason}"),
            Self::TraditionalChinese => format!("無法安裝：{reason}"),
        }
    }

    pub(crate) fn previous_kept(self, version: &str) -> String {
        match self {
            Self::English => format!("The previous version, {version}, is still installed."),
            Self::TraditionalChinese => format!("先前的 {version} 版仍保持安裝。"),
        }
    }

    pub(crate) fn previous_broken_kept(self) -> &'static str {
        self.pick(
            "The broken installation was left as it was.",
            "已損壞的安裝維持原狀。",
        )
    }

    pub(crate) fn nothing_installed(self) -> &'static str {
        self.pick("Nothing was installed.", "沒有安裝任何東西。")
    }

    pub(crate) fn other_drops_ignored(self, count: usize) -> String {
        match (self, count) {
            (Self::English, 1) => {
                "Only the first add-on is shown; the other add-on file was not opened.".into()
            }
            (Self::English, count) => format!(
                "Only the first add-on is shown; the other {count} add-on files were not opened."
            ),
            (Self::TraditionalChinese, count) => {
                format!("只顯示第一個附加元件；其他 {count} 個附加元件檔案未開啟。")
            }
        }
    }

    pub(crate) fn wait_for_install(self) -> &'static str {
        self.pick(
            "Wait for the current installation to finish.",
            "請等目前的安裝完成。",
        )
    }

    pub(crate) fn addons_unavailable(self) -> &'static str {
        self.pick(
            "Add-ons are not available in this window.",
            "這個視窗無法使用附加元件。",
        )
    }

    pub(crate) fn could_not_open_link(self, reason: &str) -> String {
        match self {
            Self::English => format!("Could not open the link: {reason}"),
            Self::TraditionalChinese => format!("無法開啟連結：{reason}"),
        }
    }

    // Add-on sources in the Sources section: formats, files, transport and problems.

    pub(crate) fn format_field(self) -> &'static str {
        self.pick("Format", "格式")
    }

    pub(crate) fn channel_count(self, count: usize) -> String {
        match self {
            Self::English if count == 1 => "1 channel".to_owned(),
            Self::English => format!("{count} channels"),
            Self::TraditionalChinese => format!("{count} 個聲道"),
        }
    }

    pub(crate) fn routes_only_for_narrow(self) -> &'static str {
        self.pick(
            "Each channel of a wide format has its own row, so there is no route to choose.",
            "多聲道格式的每個聲道各有一列，因此不需選擇聲道路由。",
        )
    }

    pub(crate) fn format_unavailable(self, format: &str) -> String {
        match self {
            Self::English => {
                format!("The add-on reports the format \"{format}\", which it does not list.")
            }
            Self::TraditionalChinese => {
                format!("附加元件回報的格式「{format}」不在它提供的清單中。")
            }
        }
    }

    pub(crate) fn could_not_choose_format(self, label: &str, reason: &str) -> String {
        match self {
            Self::English => format!("Could not choose {label}: {reason}"),
            Self::TraditionalChinese => format!("無法選擇 {label}：{reason}"),
        }
    }

    pub(crate) fn file_field(self) -> &'static str {
        self.pick("File", "檔案")
    }

    pub(crate) fn open_file_button(self) -> &'static str {
        self.pick("Open file…", "開啟檔案…")
    }

    pub(crate) fn open_file_title(self, addon: &str) -> String {
        match self {
            Self::English => format!("Open a file with {addon}"),
            Self::TraditionalChinese => format!("用「{addon}」開啟檔案"),
        }
    }

    pub(crate) fn open_file_help(self, description: &str, extensions: &str) -> String {
        match self {
            Self::English => format!(
                "Opens {description} ({extensions}). You can also drop the file on a window."
            ),
            Self::TraditionalChinese => {
                format!("可開啟 {description}（{extensions}）。也可以把檔案拖放到視窗上。")
            }
        }
    }

    pub(crate) fn opened_file(self, file: &str, addon: &str) -> String {
        match self {
            Self::English => format!("Sent {file} to {addon}."),
            Self::TraditionalChinese => format!("已把 {file} 交給「{addon}」。"),
        }
    }

    pub(crate) fn choose_its_source(self, source: &str) -> String {
        match self {
            Self::English => {
                format!("Choose \"{source}\" as an audio source in Options, Sources, to see it.")
            }
            Self::TraditionalChinese => {
                format!("請在「選項」的「來源」中選擇「{source}」才能看到它。")
            }
        }
    }

    pub(crate) fn could_not_open_file(self, file: &str, reason: &str) -> String {
        match self {
            Self::English => format!("Could not open {file}: {reason}"),
            Self::TraditionalChinese => format!("無法開啟 {file}：{reason}"),
        }
    }

    pub(crate) fn which_addon(self, file: &str) -> String {
        match self {
            Self::English => format!("Which add-on should open {file}?"),
            Self::TraditionalChinese => format!("要用哪個附加元件開啟 {file}？"),
        }
    }

    pub(crate) fn opener_not_running(self, addon: &str) -> String {
        match self {
            Self::English => format!(
                "{addon} opens this kind of file but is not running. Turn it on in Options, Add-ons."
            ),
            Self::TraditionalChinese => {
                format!(
                    "「{addon}」可以開啟這種檔案，但它沒有執行。請在「選項」的「附加元件」中開啟它。"
                )
            }
        }
    }

    pub(crate) fn play(self) -> &'static str {
        self.pick("Play", "播放")
    }

    pub(crate) fn pause(self) -> &'static str {
        self.pick("Pause", "暫停")
    }

    pub(crate) fn stop(self) -> &'static str {
        self.pick("Stop", "停止")
    }

    pub(crate) fn playback_field(self) -> &'static str {
        self.pick("Playback", "播放")
    }

    pub(crate) fn moving_to(self, time: &str) -> String {
        match self {
            Self::English => format!("Moving to {time}. The sound returns in a moment."),
            Self::TraditionalChinese => format!("正在移到 {time}，聲音稍後就會回來。"),
        }
    }

    pub(crate) fn source_not_installed(self, source: &str) -> String {
        match self {
            Self::English => format!("The add-on that provides \"{source}\" is not installed."),
            Self::TraditionalChinese => format!("提供「{source}」的附加元件尚未安裝。"),
        }
    }

    pub(crate) fn source_not_installed_hint(self) -> &'static str {
        self.pick(
            "Install it on the Add-ons page, or choose another source.",
            "請在「附加元件」頁面安裝它，或選擇其他來源。",
        )
    }

    pub(crate) fn source_not_running(self, addon: &str, state: &str) -> String {
        match self {
            Self::English => format!("{addon} is not available. {state}"),
            Self::TraditionalChinese => format!("「{addon}」目前無法使用。{state}"),
        }
    }

    pub(crate) fn source_not_running_hint(self) -> &'static str {
        self.pick(
            "Turn it on or start it again on the Add-ons page, or choose another source.",
            "請在「附加元件」頁面開啟它或重新啟動它，或選擇其他來源。",
        )
    }

    pub(crate) fn source_gone(self, addon: &str, source: &str) -> String {
        match self {
            Self::English => format!("{addon} no longer offers \"{source}\"."),
            Self::TraditionalChinese => format!("「{addon}」不再提供「{source}」。"),
        }
    }

    pub(crate) fn source_gone_hint(self) -> &'static str {
        self.pick(
            "Choose another source. If you expected it, restart the add-on on the Add-ons page.",
            "請選擇其他來源。如果它應該存在，請在「附加元件」頁面重新啟動附加元件。",
        )
    }

    pub(crate) fn addon_error_hint(self) -> &'static str {
        self.pick(
            "Try another file or format, or restart the add-on on the Add-ons page.",
            "請試試其他檔案或格式，或在「附加元件」頁面重新啟動附加元件。",
        )
    }

    pub(crate) fn open_addons_page(self) -> &'static str {
        self.pick("Open the Add-ons page", "開啟附加元件頁面")
    }

    pub(crate) fn transport_tip(self, playing: bool, source: &str) -> String {
        let action = if playing { self.pause() } else { self.play() };
        match self {
            Self::English => format!("{action} {source}"),
            Self::TraditionalChinese => format!("{action}：{source}"),
        }
    }
}

/// A size as Windows Explorer shows it, in units of 1024.
pub(crate) fn byte_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
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

    #[test]
    fn every_source_text_exists_in_both_languages_and_differs() {
        let pairs: Vec<(String, String)> = [
            super::Language::format_field,
            super::Language::routes_only_for_narrow,
            super::Language::file_field,
            super::Language::open_file_button,
            super::Language::play,
            super::Language::pause,
            super::Language::stop,
            super::Language::playback_field,
            super::Language::source_not_installed_hint,
            super::Language::source_not_running_hint,
            super::Language::source_gone_hint,
            super::Language::addon_error_hint,
            super::Language::open_addons_page,
        ]
        .into_iter()
        .map(|text| {
            (
                text(English).to_owned(),
                text(TraditionalChinese).to_owned(),
            )
        })
        .chain([
            (
                English.channel_count(12),
                TraditionalChinese.channel_count(12),
            ),
            (
                English.format_unavailable("x"),
                TraditionalChinese.format_unavailable("x"),
            ),
            (
                English.could_not_choose_format("L", "x"),
                TraditionalChinese.could_not_choose_format("L", "x"),
            ),
            (
                English.open_file_title("A"),
                TraditionalChinese.open_file_title("A"),
            ),
            (
                English.open_file_help("Audio", ".ec3"),
                TraditionalChinese.open_file_help("Audio", ".ec3"),
            ),
            (
                English.opened_file("f", "A"),
                TraditionalChinese.opened_file("f", "A"),
            ),
            (
                English.choose_its_source("S"),
                TraditionalChinese.choose_its_source("S"),
            ),
            (
                English.could_not_open_file("f", "x"),
                TraditionalChinese.could_not_open_file("f", "x"),
            ),
            (
                English.which_addon("f"),
                TraditionalChinese.which_addon("f"),
            ),
            (
                English.opener_not_running("A"),
                TraditionalChinese.opener_not_running("A"),
            ),
            (
                English.moving_to("1:00"),
                TraditionalChinese.moving_to("1:00"),
            ),
            (
                English.source_not_installed("S"),
                TraditionalChinese.source_not_installed("S"),
            ),
            (
                English.source_not_running("A", "Stopped"),
                TraditionalChinese.source_not_running("A", "已停止"),
            ),
            (
                English.source_gone("A", "S"),
                TraditionalChinese.source_gone("A", "S"),
            ),
            (
                English.transport_tip(true, "S"),
                TraditionalChinese.transport_tip(true, "S"),
            ),
        ])
        .collect();
        for (english, chinese) in pairs {
            assert!(!english.is_empty() && !chinese.is_empty());
            assert_ne!(english, chinese);
            assert!(chinese.chars().any(|c| c as u32 >= 0x3000), "{chinese}");
        }
        assert_eq!(English.channel_count(1), "1 channel");
        assert_eq!(English.channel_count(12), "12 channels");
    }

    #[test]
    fn sizes_read_as_explorer_shows_them() {
        assert_eq!(super::byte_size(0), "0 B");
        assert_eq!(super::byte_size(1023), "1023 B");
        assert_eq!(super::byte_size(1024), "1.0 KB");
        assert_eq!(super::byte_size(232_125), "227 KB");
        assert_eq!(super::byte_size(5 * 1024 * 1024 + 300 * 1024), "5.3 MB");
        assert_eq!(super::byte_size(512 * 1024 * 1024), "512 MB");
        assert_eq!(English.size_in_files(2048, 1), "2.0 KB in 1 file");
        assert_eq!(English.size_in_files(2048, 3), "2.0 KB in 3 files");
        assert_eq!(
            TraditionalChinese.size_in_files(2048, 3),
            "3 個檔案，共 2.0 KB"
        );
    }

    #[test]
    fn every_add_on_text_exists_in_both_languages_and_differs() {
        use backtracking_addon::protocol::MessageLevel;
        let pairs: Vec<(String, String)> = [
            super::Language::addons_section,
            super::Language::addons_empty,
            super::Language::addons_explained,
            super::Language::addons_how_to_add,
            super::Language::install_from_file,
            super::Language::drop_hint,
            super::Language::file_dialog_title,
            super::Language::file_filter,
            super::Language::drop_to_review,
            super::Language::enabled,
            super::Language::state_running,
            super::Language::state_starting,
            super::Language::state_stopping,
            super::Language::state_off,
            super::Language::state_stopped,
            super::Language::state_not_responding,
            super::Language::broken_hint,
            super::Language::settings_heading,
            super::Language::actions_heading,
            super::Language::not_running_hint,
            super::Language::start_again,
            super::Language::remove_button,
            super::Language::remove_explained,
            super::Language::delete_data,
            super::Language::keep_data_note,
            super::Language::remove,
            super::Language::cancel,
            super::Language::removing,
            super::Language::review_heading,
            super::Language::homepage,
            super::Language::license,
            super::Language::program,
            super::Language::unpacked_size,
            super::Language::package_file,
            super::Language::asks_heading,
            super::Language::no_capabilities,
            super::Language::trust_warning,
            super::Language::starts_note,
            super::Language::replaces_broken,
            super::Language::install,
            super::Language::installing,
            super::Language::refused_heading,
            super::Language::close,
            super::Language::previous_broken_kept,
            super::Language::nothing_installed,
            super::Language::wait_for_install,
            super::Language::addons_unavailable,
        ]
        .into_iter()
        .map(|text| {
            (
                text(English).to_owned(),
                text(TraditionalChinese).to_owned(),
            )
        })
        .chain([
            (
                English.version_by("1", "A"),
                TraditionalChinese.version_by("1", "A"),
            ),
            (
                English.state_failed("x"),
                TraditionalChinese.state_failed("x"),
            ),
            (
                English.state_broken("x"),
                TraditionalChinese.state_broken("x"),
            ),
            (
                English.addon_message(MessageLevel::Warning, "x"),
                TraditionalChinese.addon_message(MessageLevel::Warning, "x"),
            ),
            (
                English.could_not_change("L", "x"),
                TraditionalChinese.could_not_change("L", "x"),
            ),
            (
                English.could_not_run("L", "x"),
                TraditionalChinese.could_not_run("L", "x"),
            ),
            (
                English.could_not_switch(true, "x"),
                TraditionalChinese.could_not_switch(true, "x"),
            ),
            (
                English.could_not_switch(false, "x"),
                TraditionalChinese.could_not_switch(false, "x"),
            ),
            (
                English.could_not_start("x"),
                TraditionalChinese.could_not_start("x"),
            ),
            (
                English.remove_question("N"),
                TraditionalChinese.remove_question("N"),
            ),
            (
                English.could_not_remove("x"),
                TraditionalChinese.could_not_remove("x"),
            ),
            (English.removed("N"), TraditionalChinese.removed("N")),
            (
                English.installed("N", "1"),
                TraditionalChinese.installed("N", "1"),
            ),
            (English.reading("f"), TraditionalChinese.reading("f")),
            (
                English.replaces("1", "2"),
                TraditionalChinese.replaces("1", "2"),
            ),
            (
                English.install_failed("x"),
                TraditionalChinese.install_failed("x"),
            ),
            (
                English.previous_kept("1"),
                TraditionalChinese.previous_kept("1"),
            ),
            (
                English.other_drops_ignored(2),
                TraditionalChinese.other_drops_ignored(2),
            ),
            (
                English.could_not_open_link("x"),
                TraditionalChinese.could_not_open_link("x"),
            ),
        ])
        .collect();
        for (english, chinese) in pairs {
            assert!(!english.is_empty() && !chinese.is_empty());
            assert_ne!(english, chinese);
            // The Chinese text is written in Chinese, not left in English.
            assert!(chinese.chars().any(|c| c as u32 >= 0x3000), "{chinese}");
        }
        assert_eq!(
            English.addon_message(MessageLevel::Error, "No objects."),
            "Error: No objects."
        );
        assert_eq!(
            TraditionalChinese.addon_message(MessageLevel::Info, "好"),
            "訊息：好"
        );
        assert_eq!(
            English.replaces("0.1.0", "0.2.0"),
            "This replaces version 0.1.0 with version 0.2.0. Its settings and saved data are kept."
        );
        assert_eq!(
            English.other_drops_ignored(1),
            "Only the first add-on is shown; the other add-on file was not opened."
        );
    }
}
