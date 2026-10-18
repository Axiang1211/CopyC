//! macOS：NSPasteboard changeCount 監聽、Finder 檔案 URL、密碼管理器的
//! org.nspasteboard.ConcealedType 標記、CGWindowList 全螢幕偵測與 NSWorkspace 檔案圖示。

use super::{AppInfo, NativeClip};
use core_foundation::{
    array::{CFArray, CFArrayRef},
    base::{CFType, TCFType},
    dictionary::{CFDictionary, CFDictionaryRef},
    number::CFNumber,
    string::CFString,
};
use objc2::{rc::autoreleasepool, AnyThread};
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSDeviceRGBColorSpace, NSGraphicsContext, NSPasteboard,
    NSWindow, NSWindowCollectionBehavior, NSWorkspace,
};
use objc2_foundation::{NSActivityOptions, NSDictionary, NSPoint, NSProcessInfo, NSRect, NSSize, NSString, NSURL};
use std::{
    path::{Path, PathBuf},
    sync::mpsc::Sender,
    thread,
    time::Duration,
};

const POLL_INTERVAL: Duration = Duration::from_millis(150);
/// kCGStatusWindowLevel：高於一般視窗與全螢幕 App。
const STATUS_WINDOW_LEVEL: isize = 25;

/// 關閉 App Nap：常駐背景的工具若被系統節流，輪詢會延遲到數秒才觸發。
pub fn platform_setup() {
    let reason = NSString::from_str("CopyC 正在監聽剪貼簿變更");
    let token = NSProcessInfo::processInfo()
        .beginActivityWithOptions_reason(NSActivityOptions::UserInitiatedAllowingIdleSystemSleep, &reason);
    std::mem::forget(token);
}

pub fn init_worker_thread() {}

/// macOS 沒有剪貼簿變更通知，以 changeCount 判斷；每次寫入（含重複複製同一內容）都會遞增。
pub fn start_clipboard_listener(tx: Sender<()>) {
    thread::spawn(move || {
        let mut last = autoreleasepool(|_| NSPasteboard::generalPasteboard().changeCount());
        loop {
            thread::sleep(POLL_INTERVAL);
            let now = autoreleasepool(|_| NSPasteboard::generalPasteboard().changeCount());
            if now != last {
                last = now;
                if tx.send(()).is_err() {
                    return;
                }
            }
        }
    });
}

pub fn read_native() -> NativeClip {
    autoreleasepool(|_| {
        let pb = NSPasteboard::generalPasteboard();
        let mut out = NativeClip::default();
        let types: Vec<String> = pb
            .types()
            .map(|arr| arr.iter().map(|t| t.to_string()).collect())
            .unwrap_or_default();
        out.format_count = types.len();
        for t in &types {
            match t.as_str() {
                // nspasteboard.org 慣例：1Password、Bitwarden、KeePassXC 等用來標記密碼
                "org.nspasteboard.ConcealedType" | "com.agilebits.onepassword" => out.sensitive_marker = true,
                "public.png" => {
                    out.has_image = true;
                    out.image_format = Some("PNG".into());
                }
                "public.jpeg" => {
                    out.has_image = true;
                    out.image_format.get_or_insert_with(|| "JPEG".into());
                }
                "com.compuserve.gif" => {
                    out.has_image = true;
                    out.image_format.get_or_insert_with(|| "GIF".into());
                }
                "public.tiff" | "NeXT TIFF v4.0 pasteboard type" => {
                    out.has_image = true;
                    out.image_format.get_or_insert_with(|| "TIFF".into());
                }
                _ => {}
            }
        }
        if types.iter().any(|t| t == "public.file-url") {
            out.files = file_urls(&pb);
        }
        out.source = foreground_app();
        out
    })
}

fn file_urls(pb: &NSPasteboard) -> Vec<PathBuf> {
    let key = NSString::from_str("public.file-url");
    let Some(items) = pb.pasteboardItems() else { return Vec::new() };
    items
        .iter()
        .filter_map(|item| {
            let s = item.stringForType(&key)?;
            let url = NSURL::URLWithString(&s)?;
            // 解析 /.file/id= 形式的檔案參照 URL
            let url = url.filePathURL().unwrap_or(url);
            Some(PathBuf::from(url.path()?.to_string()))
        })
        .collect()
}

fn frontmost() -> Option<(AppInfo, i32)> {
    let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
    let name = app.localizedName().map(|s| s.to_string()).unwrap_or_default();
    let id = app.bundleIdentifier().map(|s| s.to_string()).unwrap_or_default();
    Some((AppInfo { name, id }, app.processIdentifier()))
}

pub fn foreground_app() -> Option<AppInfo> {
    autoreleasepool(|_| frontmost().map(|(a, _)| a))
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGPoint {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGSize {
    width: f64,
    height: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> CFArrayRef;
    fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    fn CGDisplayBounds(display: u32) -> CGRect;
    fn CGRectMakeWithDictionaryRepresentation(dict: CFDictionaryRef, rect: *mut CGRect) -> bool;
}

const ON_SCREEN_ONLY: u32 = 1 << 0;
const EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;

fn displays() -> Vec<CGRect> {
    let mut ids = [0u32; 16];
    let mut count = 0u32;
    unsafe {
        if CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) != 0 {
            return Vec::new();
        }
        ids[..count as usize].iter().map(|&d| CGDisplayBounds(d)).collect()
    }
}

/// 前景 App 是否有一般層級視窗完整覆蓋某個螢幕（原生全螢幕、遊戲、Keynote 放映）。
/// 只讀取視窗外框與擁有者 PID，不需要螢幕錄製權限。
pub fn is_fullscreen() -> bool {
    let Some((_, pid)) = autoreleasepool(|_| frontmost()) else { return false };
    let screens = displays();
    let raw = unsafe { CGWindowListCopyWindowInfo(ON_SCREEN_ONLY | EXCLUDE_DESKTOP_ELEMENTS, 0) };
    if raw.is_null() {
        return false;
    }
    let windows: CFArray<CFDictionary<CFString, CFType>> = unsafe { CFArray::wrap_under_create_rule(raw) };
    let k_pid = CFString::from_static_string("kCGWindowOwnerPID");
    let k_layer = CFString::from_static_string("kCGWindowLayer");
    let k_bounds = CFString::from_static_string("kCGWindowBounds");
    let number = |d: &CFDictionary<CFString, CFType>, k: &CFString| {
        d.find(k).and_then(|v| v.downcast::<CFNumber>()).and_then(|n| n.to_i64())
    };

    windows.iter().any(|w| {
        if number(&w, &k_pid) != Some(pid as i64) || number(&w, &k_layer) != Some(0) {
            return false;
        }
        let Some(bounds) = w.find(&k_bounds).and_then(|v| v.downcast::<CFDictionary>()) else { return false };
        let mut r = CGRect::default();
        if !unsafe { CGRectMakeWithDictionaryRepresentation(bounds.as_concrete_TypeRef(), &mut r) } {
            return false;
        }
        screens.iter().any(|s| {
            (r.origin.x - s.origin.x).abs() < 1.0
                && (r.origin.y - s.origin.y).abs() < 1.0
                && r.size.width >= s.size.width - 1.0
                && r.size.height >= s.size.height - 1.0
        })
    })
}

/// NSWorkspace 系統檔案圖示，繪製成 64×64 PNG。
pub fn file_icon_png(path: &Path) -> Option<Vec<u8>> {
    autoreleasepool(|_| unsafe {
        let icon = NSWorkspace::sharedWorkspace().iconForFile(&NSString::from_str(&path.to_string_lossy()));
        let rep = NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            64,
            64,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )?;
        let ctx = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
        NSGraphicsContext::saveGraphicsState_class();
        NSGraphicsContext::setCurrentContext(Some(&ctx));
        icon.drawInRect(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(64.0, 64.0)));
        ctx.flushGraphics();
        NSGraphicsContext::restoreGraphicsState_class();
        let data = rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())?;
        Some(data.to_vec())
    })
}

fn ns_window(window: &tauri::WebviewWindow) -> Option<&NSWindow> {
    let ptr = window.ns_window().ok()?;
    (!ptr.is_null()).then(|| unsafe { &*(ptr as *const NSWindow) })
}

/// 通知框：狀態列層級、跨所有桌面空間並可浮在全螢幕 App 之上。
pub fn prepare_hud(window: &tauri::WebviewWindow) {
    let _ = window.set_ignore_cursor_events(true);
    let Some(ns) = ns_window(window) else { return };
    ns.setLevel(STATUS_WINDOW_LEVEL);
    ns.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    ns.setHasShadow(false);
}

/// orderFrontRegardless：顯示但不成為 key window、也不啟用本 App，焦點留在使用者的工作視窗。
pub fn show_hud(window: &tauri::WebviewWindow) {
    if let Some(ns) = ns_window(window) {
        ns.orderFrontRegardless();
    }
}

pub fn hide_hud(window: &tauri::WebviewWindow) {
    if let Some(ns) = ns_window(window) {
        ns.orderOut(None);
    }
}
