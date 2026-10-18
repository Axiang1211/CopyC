//! Ctrl-C：常駐背景的複製即時回饋工具。

mod classify;
mod commands;
mod hud;
mod monitor;
mod platform;
mod settings;
mod settings_window;
mod state;
mod tray;

use settings::Settings;
use state::AppState;
use tauri::{Manager, RunEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

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
            commands::set_autostart,
            commands::preview_hud,
            commands::hud_finished,
            commands::hud_rect,
            commands::open_settings,
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
                // 規格：安裝即用、開機自動啟動
                let _ = app.autolaunch().enable();
            }
            app.manage(AppState::new(settings, settings_path));

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
        .expect("無法啟動 Ctrl-C");

    app.run(|_app, event| {
        // 關閉設定視窗不結束程式；只有選單的「結束」（帶 exit code）才會真的退出
        if let RunEvent::ExitRequested { api, code: None, .. } = event {
            api.prevent_exit();
        }
    });
}
