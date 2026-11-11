use std::mem::size_of;
use std::ptr::{null, null_mut};

use windows_sys::Win32::UI::Controls::{
    TASKDIALOG_BUTTON, TASKDIALOGCONFIG, TDCBF_OK_BUTTON, TDF_ALLOW_DIALOG_CANCELLATION,
    TDF_EXPAND_FOOTER_AREA, TaskDialogIndirect,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{IDYES, MB_ICONINFORMATION, MB_OK, MessageBoxW};

use super::Available;

const ENABLE: i32 = 101;
const DEFER: i32 = 102;

pub(super) enum Consent {
    Enable,
    Defer,
    Never,
}

fn wide(value: &str) -> Vec<u16> {
    value
        .encode_utf16()
        .map(|unit| if unit == 0 { 0xfffd } else { unit })
        .chain([0])
        .collect()
}

pub(super) fn ask(update: &Available) -> Consent {
    let title = wide("Backtracking 更新");
    let heading = wide(&format!("有新版本 v{}", update.version));
    let message = wide(&format!(
        "同意後，Backtracking 會在每次啟動時自動檢查並安裝已驗證的更新。\n\n{}",
        preview(&update.notes)
    ));
    let enable = wide("更新並啟用自動更新");
    let defer = wide("下個版本再提示");
    let never = wide("不再顯示更新詢問");
    let details = wide(&update.notes);
    let expand = wide("顯示 GitHub Release 更新內容");
    let collapse = wide("收合 GitHub Release 更新內容");
    let buttons = [
        TASKDIALOG_BUTTON {
            nButtonID: ENABLE,
            pszButtonText: enable.as_ptr(),
        },
        TASKDIALOG_BUTTON {
            nButtonID: DEFER,
            pszButtonText: defer.as_ptr(),
        },
    ];
    let config = TASKDIALOGCONFIG {
        cbSize: size_of::<TASKDIALOGCONFIG>() as u32,
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION | TDF_EXPAND_FOOTER_AREA,
        pszWindowTitle: title.as_ptr(),
        pszMainInstruction: heading.as_ptr(),
        pszContent: message.as_ptr(),
        cButtons: buttons.len() as u32,
        pButtons: buttons.as_ptr(),
        nDefaultButton: DEFER,
        pszVerificationText: never.as_ptr(),
        pszExpandedInformation: details.as_ptr(),
        pszExpandedControlText: expand.as_ptr(),
        pszCollapsedControlText: collapse.as_ptr(),
        cxWidth: 390,
        ..Default::default()
    };
    let mut chosen = 0;
    let mut checked = 0;
    let status = unsafe { TaskDialogIndirect(&config, &mut chosen, null_mut(), &mut checked) };
    if status < 0 {
        let message = wide(&format!(
            "有新版本 v{}。是否更新並啟用自動更新？\n\n{}",
            update.version,
            update.notes.chars().take(1200).collect::<String>()
        ));
        let action = unsafe {
            MessageBoxW(
                null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                windows_sys::Win32::UI::WindowsAndMessaging::MB_YESNO | MB_ICONINFORMATION,
            )
        };
        return if action == IDYES {
            Consent::Enable
        } else {
            Consent::Defer
        };
    }
    if chosen == ENABLE {
        Consent::Enable
    } else if checked != 0 {
        Consent::Never
    } else {
        Consent::Defer
    }
}

pub(super) fn completed(version: &str, notes: &str) {
    let heading = format!("已更新至 Backtracking v{version}");
    let content = format!("Backtracking 已成功啟動新版。\n\n{}", preview(notes));
    notice(&heading, &content, Some(notes));
}

pub(super) fn recovered(message: &str) {
    notice("更新未完成", message, None);
}

fn preview(notes: &str) -> String {
    let mut text = notes.chars();
    let mut preview = text.by_ref().take(360).collect::<String>();
    if text.next().is_some() {
        preview.push('…');
    }
    preview
}

fn notice(heading: &str, content: &str, notes: Option<&str>) {
    let title = wide("Backtracking 更新");
    let heading = wide(heading);
    let content = wide(content);
    let notes = notes.map(wide);
    let expand = wide("顯示 GitHub Release 更新內容");
    let collapse = wide("收合 GitHub Release 更新內容");
    let config = TASKDIALOGCONFIG {
        cbSize: size_of::<TASKDIALOGCONFIG>() as u32,
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION | TDF_EXPAND_FOOTER_AREA,
        dwCommonButtons: TDCBF_OK_BUTTON,
        pszWindowTitle: title.as_ptr(),
        pszMainInstruction: heading.as_ptr(),
        pszContent: content.as_ptr(),
        pszExpandedInformation: notes.as_ref().map_or(null(), |value| value.as_ptr()),
        pszExpandedControlText: expand.as_ptr(),
        pszCollapsedControlText: collapse.as_ptr(),
        cxWidth: 390,
        ..Default::default()
    };
    if unsafe { TaskDialogIndirect(&config, null_mut(), null_mut(), null_mut()) } < 0 {
        unsafe {
            MessageBoxW(
                null_mut(),
                content.as_ptr(),
                heading.as_ptr(),
                MB_OK | MB_ICONINFORMATION,
            )
        };
    }
}
