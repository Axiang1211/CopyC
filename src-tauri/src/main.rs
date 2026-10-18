// Release 版不顯示主控台視窗
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    ctrl_c_lib::run();
}
