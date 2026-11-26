#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent;
mod engine;
mod fsx;
mod gguf;
mod hf;
mod mcp;
mod models;
mod store;
mod toolparse;
mod tools;
mod util;

use std::sync::Arc;
use tauri::{Manager, RunEvent};

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(engine::Engine::default())
        .manage(Arc::new(mcp::Mcp::default()))
        .manage(agent::Agent::default())
        .manage(hf::Hf::default())
        .invoke_handler(tauri::generate_handler![
            store::settings_get,
            store::settings_set,
            store::settings_patch,
            store::chat_list,
            store::chat_load,
            store::chat_save,
            store::chat_delete,
            models::models_scan,
            engine::engine_status,
            engine::engine_log,
            engine::engine_load,
            engine::engine_unload,
            engine::provider_models,
            hf::hf_search,
            hf::hf_files,
            hf::hf_download,
            hf::hf_cancel,
            agent::agent_run,
            agent::agent_cancel,
            agent::permission_reply,
            mcp::mcp_status,
            mcp::mcp_get_config,
            mcp::mcp_save_config,
            mcp::mcp_reload,
            fsx::fs_list,
            fsx::fs_read,
            fsx::fs_reveal,
            fsx::open_url,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let mcp = app.state::<Arc<mcp::Mcp>>().inner().clone();
            tauri::async_runtime::spawn(async move {
                mcp.reload(handle).await;
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building harness");

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            handle.state::<engine::Engine>().kill_blocking();
            handle.state::<Arc<mcp::Mcp>>().kill_blocking();
        }
    });
}
