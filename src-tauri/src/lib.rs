//! CopyC：常駐背景的複製即時回饋工具。

mod autostart;
mod classify;
mod commands;
mod history;
mod hud;
mod i18n;
mod monitor;
mod platform;
mod settings;
mod settings_window;
mod state;
mod tray;

use settings::Settings;
use state::AppState;
use tauri::{Manager, RunEvent};
use tauri_plugin_autostart::MacosLauncher;

const AUTOSTART_FLAG: &str = "--autostart";

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // 重複啟動時直接打開偏好設定，而不是再跑一份常駐程式
            settings_window::open(app);
        }))
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec![AUTOSTART_FLAG])))
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::update_settings,
            commands::get_overview,
            commands::clear_history,
            commands::get_autostart,
            commands::set_autostart,
            commands::preview_hud,
            commands::hud_finished,
            commands::hud_rect,
            commands::open_settings,
            commands::update_check,
            commands::update_found,
            commands::open_download,
            commands::open_kopi,
            commands::history_query,
            commands::history_copy,
            commands::history_delete,
            commands::history_clear,
        ])
        .setup(|app| {
            // 只常駐在選單列，不出現在 Dock
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            platform::platform_setup();

            let settings_path = app.path().app_config_dir()?.join("settings.json");
            let first_run = !settings_path.exists();
            let settings = Settings::load(&settings_path);
            if first_run {
                let _ = settings.save(&settings_path);
                // 規格：安裝即用、預設開機自動啟動；使用者可在偏好設定 → 一般關閉
                let handle = app.handle().clone();
                std::thread::spawn(move || autostart::set(&handle, true));
            }
            app.manage(AppState::new(settings, settings_path));
            // 本機複製紀錄：開不起來（例如磁碟唯讀）時只停用歷史功能，不影響通知框
            match history::History::open(&app.path().app_data_dir()?.join("history.sqlite3")) {
                Ok(store) => {
                    let _ = app.state::<AppState>().store.set(store);
                }
                Err(e) => eprintln!("copyc: 無法開啟歷史資料庫：{e}"),
            }

            let handle = app.handle();
            hud::create(handle)?;
            tray::create(handle)?;
            monitor::start(handle.clone());

            let from_autostart = std::env::args().any(|a| a == AUTOSTART_FLAG);
            if first_run && !from_autostart {
                settings_window::open(handle);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("無法啟動 CopyC");

    app.run(|_app, event| {
        // 關閉設定視窗不結束程式；只有選單的「結束」（帶 exit code）才會真的退出
        if let RunEvent::ExitRequested { api, code: None, .. } = event {
            api.prevent_exit();
        }
    });
}
