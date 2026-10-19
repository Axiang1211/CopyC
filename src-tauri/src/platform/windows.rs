//! Windows：AddClipboardFormatListener 事件監聽、CF_HDROP 檔案清單、
//! 密碼管理器隱私格式、SHQueryUserNotificationState 全螢幕偵測與系統檔案圖示。

use super::{AppInfo, NativeClip};
use std::{
    ffi::c_void,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::{mpsc::Sender, OnceLock},
    thread,
    time::Duration,
};
use crate::autostart::Autostart;
use windows::{
    ApplicationModel::{StartupTask, StartupTaskState},
    core::{w, HSTRING, PCWSTR, PWSTR},
    Win32::{
        Foundation::{CloseHandle, HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{
            CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetMonitorInfoW, GetObjectW,
            MonitorFromWindow, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
            HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTONEAREST,
        },
        Storage::{FileSystem::FILE_FLAGS_AND_ATTRIBUTES, Packaging::Appx::GetCurrentPackageFullName},
        System::{
            Com::{CoInitializeEx, COINIT_APARTMENTTHREADED},
            DataExchange::{
                AddClipboardFormatListener, CloseClipboard, EnumClipboardFormats, GetClipboardData,
                GetClipboardFormatNameW, GetClipboardOwner, OpenClipboard,
            },
            LibraryLoader::GetModuleHandleW,
            Memory::{GlobalLock, GlobalSize, GlobalUnlock},
            Threading::{
                OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
        UI::{
            Shell::{
                DragQueryFileW, SHGetFileInfoW, ShellExecuteW, SHQueryUserNotificationState, HDROP,
                QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN, SHFILEINFOW, SHGFI_ICON,
                SHGFI_LARGEICON,
            },
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyIcon, DispatchMessageW, GetClassNameW,
                GetForegroundWindow, GetIconInfo, GetMessageW, GetShellWindow, GetWindowLongPtrW,
                GetWindowRect, GetWindowThreadProcessId, IsZoomed, RegisterClassW, SetWindowLongPtrW,
                SetWindowPos, ShowWindow, TranslateMessage, GWL_EXSTYLE, GWL_STYLE, HICON, HWND_MESSAGE,
                HWND_TOPMOST, ICONINFO, MSG, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
                SWP_SHOWWINDOW, SW_HIDE, SW_SHOWNOACTIVATE, SW_SHOWNORMAL, WINDOW_EX_STYLE, WINDOW_STYLE,
                WM_CLIPBOARDUPDATE, WNDCLASSW, WS_CAPTION, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
            },
        },
    },
};

const CF_DIB: u32 = 8;
const CF_HDROP: u32 = 15;
const CF_DIBV5: u32 = 17;
const CF_BITMAP: u32 = 2;

static TX: OnceLock<Sender<()>> = OnceLock::new();

pub fn platform_setup() {}

/// 背景工作執行緒初始化 COM（SHGetFileInfoW 需要）。
pub fn init_worker_thread() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
}

unsafe extern "system" fn listener_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_CLIPBOARDUPDATE {
        if let Some(tx) = TX.get() {
            let _ = tx.send(());
        }
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

/// 建立 message-only 視窗並註冊 AddClipboardFormatListener：
/// 不論快捷鍵、右鍵選單或網頁「一鍵複製」都會收到 WM_CLIPBOARDUPDATE，零輪詢。
pub fn start_clipboard_listener(tx: Sender<()>) {
    if TX.set(tx).is_err() {
        return;
    }
    thread::spawn(|| unsafe {
        let Ok(module) = GetModuleHandleW(None) else { return };
        let class = w!("CopyCClipboardListener");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(listener_proc),
            hInstance: module.into(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("CopyC"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(module.into()),
            None,
        ) else {
            return;
        };
        if AddClipboardFormatListener(hwnd).is_err() {
            return;
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
}

/// 剪貼簿可能仍被寫入方鎖住，短暫重試。
fn open_clipboard() -> bool {
    for _ in 0..10 {
        if unsafe { OpenClipboard(None) }.is_ok() {
            return true;
        }
        thread::sleep(Duration::from_millis(15));
    }
    false
}

fn format_name(fmt: u32) -> String {
    let mut buf = [0u16; 128];
    let len = unsafe { GetClipboardFormatNameW(fmt, &mut buf) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

/// 讀取一個 DWORD 格式的值（例如 CanIncludeInClipboardHistory）。
unsafe fn read_dword(fmt: u32) -> Option<u32> {
    let handle = GetClipboardData(fmt).ok()?;
    let mem = HGLOBAL(handle.0);
    if GlobalSize(mem) < 4 {
        return None;
    }
    let ptr = GlobalLock(mem) as *const u32;
    if ptr.is_null() {
        return None;
    }
    let value = ptr.read_unaligned();
    let _ = GlobalUnlock(mem);
    Some(value)
}

unsafe fn read_file_list() -> Vec<PathBuf> {
    let Ok(handle) = GetClipboardData(CF_HDROP) else { return Vec::new() };
    let hdrop = HDROP(handle.0);
    let count = DragQueryFileW(hdrop, u32::MAX, None);
    (0..count)
        .filter_map(|i| {
            let len = DragQueryFileW(hdrop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            let got = DragQueryFileW(hdrop, i, Some(&mut buf)) as usize;
            (got > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..got])))
        })
        .collect()
}

pub fn read_native() -> NativeClip {
    let mut out = NativeClip::default();
    if !open_clipboard() {
        return out;
    }
    unsafe {
        let mut fmt = 0u32;
        loop {
            fmt = EnumClipboardFormats(fmt);
            if fmt == 0 {
                break;
            }
            out.format_count += 1;
            match fmt {
                CF_HDROP => out.files = read_file_list(),
                CF_DIB | CF_DIBV5 | CF_BITMAP => {
                    out.has_image = true;
                    out.image_format.get_or_insert_with(|| "BMP".into());
                }
                _ if fmt >= 0xC000 => match format_name(fmt).as_str() {
                    // 1Password、Bitwarden、KeePass 等密碼工具會加上這些格式，要求剪貼簿監控程式忽略
                    "ExcludeClipboardContentFromMonitorProcessing" | "Clipboard Viewer Ignore" => {
                        out.sensitive_marker = true
                    }
                    "CanIncludeInClipboardHistory" | "CanUploadToCloudClipboard" => {
                        if read_dword(fmt) == Some(0) {
                            out.sensitive_marker = true;
                        }
                    }
                    "PNG" | "image/png" => {
                        out.has_image = true;
                        out.image_format = Some("PNG".into());
                    }
                    "JFIF" | "image/jpeg" => {
                        out.has_image = true;
                        out.image_format = Some("JPEG".into());
                    }
                    "GIF" | "image/gif" => {
                        out.has_image = true;
                        out.image_format = Some("GIF".into());
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        let owner = GetClipboardOwner().ok().filter(|h| !h.is_invalid());
        let _ = CloseClipboard();
        out.source = owner.and_then(window_app).or_else(foreground_app);
    }
    out
}

fn window_app(hwnd: HWND) -> Option<AppInfo> {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let process: HANDLE = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(process);
        ok.ok()?;
        let path = PathBuf::from(String::from_utf16_lossy(&buf[..len as usize]));
        let name = path.file_stem()?.to_string_lossy().into_owned();
        Some(AppInfo { name, id: path.to_string_lossy().into_owned() })
    }
}

/// MSIX 封裝版：登錄檔 Run 機碼會被虛擬化而失效，開機啟動改由清單裡的 StartupTask 負責。
pub fn is_packaged() -> bool {
    const APPMODEL_ERROR_NO_PACKAGE: u32 = 15700;
    let mut len = 0u32;
    unsafe { GetCurrentPackageFullName(&mut len, None).0 != APPMODEL_ERROR_NO_PACKAGE }
}

/// 與 packaging/msix/AppxManifest.xml 裡 StartupTask 的 TaskId 一致。
const STARTUP_TASK_ID: &str = "CopyCStartup";

fn startup_task() -> windows::core::Result<StartupTask> {
    StartupTask::GetAsync(&HSTRING::from(STARTUP_TASK_ID))?.join()
}

fn to_autostart(state: StartupTaskState) -> Autostart {
    Autostart {
        enabled: matches!(state, StartupTaskState::Enabled | StartupTaskState::EnabledByPolicy),
        blocked: matches!(
            state,
            StartupTaskState::DisabledByUser | StartupTaskState::DisabledByPolicy | StartupTaskState::EnabledByPolicy
        ),
    }
}

/// MSIX 版的開機啟動狀態。WinRT 的 .join() 會阻塞，只能在背景執行緒呼叫。
pub fn startup_task_state() -> Autostart {
    startup_task().and_then(|t| t.State()).map(to_autostart).unwrap_or_default()
}

/// 使用者在系統設定關閉過（DisabledByUser）時，RequestEnableAsync 不會重新開啟，
/// 回傳的 blocked 讓偏好設定提示改到 Windows 設定裡開啟。
pub fn set_startup_task(enabled: bool) -> Autostart {
    let Ok(task) = startup_task() else { return Autostart::default() };
    let state = if enabled {
        task.RequestEnableAsync().and_then(|op| op.join())
    } else {
        task.Disable().and_then(|_| task.State())
    };
    state.map(to_autostart).unwrap_or_default()
}

pub fn shell_open(url: &str) {
    unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(url), None, None, SW_SHOWNORMAL);
    }
}

pub fn foreground_app() -> Option<AppInfo> {
    let hwnd = unsafe { GetForegroundWindow() };
    (!hwnd.is_invalid()).then_some(hwnd).and_then(window_app)
}

/// 全螢幕獨佔判斷：系統通知狀態（D3D 全螢幕、簡報模式），或前景視窗是「沒有標題列、
/// 也不是最大化」且完整覆蓋所在螢幕的視窗（瀏覽器 F11、影片播放器、無邊框遊戲）。
///
/// 一般最大化視窗會往外多出約 8px 邊框；工作列自動隱藏或副螢幕沒有工作列時，
/// 它也會蓋滿整個螢幕，只看矩形會被誤判成全螢幕，所以要另外排除。
pub fn is_fullscreen() -> bool {
    unsafe {
        if let Ok(state) = SHQueryUserNotificationState() {
            if state == QUNS_RUNNING_D3D_FULL_SCREEN || state == QUNS_PRESENTATION_MODE {
                return true;
            }
        }
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() || hwnd == GetShellWindow() {
            return false;
        }
        let mut class = [0u16; 64];
        let n = GetClassNameW(hwnd, &mut class).max(0) as usize;
        let class = String::from_utf16_lossy(&class[..n]);
        if matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd") {
            return false;
        }
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        if IsZoomed(hwnd).as_bool() || style & WS_CAPTION.0 == WS_CAPTION.0 {
            return false;
        }
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return false;
        }
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return false;
        }
        let m = info.rcMonitor;
        rect.left <= m.left && rect.top <= m.top && rect.right >= m.right && rect.bottom >= m.bottom
    }
}

/// 透過 SHGetFileInfoW 取得系統檔案圖示並轉成 PNG。
pub fn file_icon_png(path: &Path) -> Option<Vec<u8>> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let mut info = SHFILEINFOW::default();
        let ok = SHGetFileInfoW(
            PCWSTR(wide.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        );
        if ok == 0 || info.hIcon.is_invalid() {
            return None;
        }
        let png = icon_to_png(info.hIcon);
        let _ = DestroyIcon(info.hIcon);
        png
    }
}

unsafe fn icon_to_png(icon: HICON) -> Option<Vec<u8>> {
    let mut ii = ICONINFO::default();
    GetIconInfo(icon, &mut ii).ok()?;
    let color: HGDIOBJ = ii.hbmColor.into();
    let mask: HGDIOBJ = ii.hbmMask.into();
    let result = (|| {
        let mut bmp = BITMAP::default();
        if GetObjectW(color, std::mem::size_of::<BITMAP>() as i32, Some(&mut bmp as *mut _ as *mut c_void)) == 0 {
            return None;
        }
        let (w, h) = (bmp.bmWidth, bmp.bmHeight);
        if w <= 0 || h <= 0 {
            return None;
        }
        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        let dc = CreateCompatibleDC(None);
        let lines = GetDIBits(
            dc,
            ii.hbmColor,
            0,
            h as u32,
            Some(pixels.as_mut_ptr() as *mut c_void),
            &mut bi,
            DIB_RGB_COLORS,
        );
        let _ = DeleteDC(dc);
        if lines == 0 {
            return None;
        }
        let has_alpha = pixels.chunks_exact(4).any(|p| p[3] != 0);
        for p in pixels.chunks_exact_mut(4) {
            p.swap(0, 2); // BGRA → RGBA
            if !has_alpha {
                p[3] = 255;
            }
        }
        let img = image::RgbaImage::from_raw(w as u32, h as u32, pixels)?;
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
        Some(png)
    })();
    let _ = DeleteObject(color);
    let _ = DeleteObject(mask);
    result
}

/// 通知框：工具視窗 + WS_EX_NOACTIVATE，永不出現在 Alt-Tab，也永不搶走焦點。
pub fn prepare_hud(window: &tauri::WebviewWindow) {
    // 先讓 tao 設好滑鼠穿透（WS_EX_TRANSPARENT | WS_EX_LAYERED），再補上不啟用與工具視窗樣式
    let _ = window.set_ignore_cursor_events(true);
    let Ok(hwnd) = window.hwnd() else { return };
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | (WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0) as isize);
    }
}

pub fn show_hud(window: &tauri::WebviewWindow) {
    let Ok(hwnd) = window.hwnd() else { return };
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}

pub fn hide_hud(window: &tauri::WebviewWindow) {
    let Ok(hwnd) = window.hwnd() else { return };
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
}
