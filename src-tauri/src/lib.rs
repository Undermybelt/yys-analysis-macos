//! Tauri 组合根：装配本地数据库与目录命令，前端不直接接触文件系统或 SQLite。

mod application;
mod domain;
mod infrastructure;
mod interface;

#[cfg(test)]
mod acceptance;

use application::tasks::TaskRegistry;
use application::services::AppServices;
use infrastructure::{
    logging,
    paths::AppPaths,
    process::ProcessRegistry,
};
use interface::state::AppState;
use tauri::{Manager, RunEvent};

/// 创建并运行桌面应用；注册御魂与式神目录、库存和分析所需的 Tauri 命令边界。
pub fn run() {
    let app = tauri::Builder::default()
        // 外链统一交给系统默认浏览器，避免 WebView 内的 target=_blank 无法创建窗口。
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let paths = AppPaths::resolve(app.handle())?;
            paths.ensure_required_directories()?;

            // 日志守卫必须与应用同寿命，否则异步日志线程会在初始化后立即停止。
            let logging_guard = logging::initialize(&paths.log_directory)?;
            // 新版本只使用专用分析投影；旧 yuhun.sqlite3 与 snapshots 目录保留在磁盘但不再读取。
            let database_path = paths.data_directory.join("current-analysis.sqlite3");
            let snapshot_root = paths.data_directory.join("current-data/raw");
            let quarantine_directory = paths.data_directory.join("current-data/quarantine");
            let backups_directory = paths.data_directory.join("backups");
            let temp_directory = paths.temp_directory.clone();
            let services = std::sync::Arc::new(AppServices::initialize(
                database_path,
                snapshot_root,
                quarantine_directory,
                backups_directory,
                temp_directory,
            )?);
            let process_registry = ProcessRegistry::default();

            // macOS 版不持有模拟器或桌面进程读取会话。
            app.manage(AppState {
                services,
                _logging_guard: logging_guard,
            });
            app.manage(paths);
            app.manage(TaskRegistry::default());
            app.manage(process_registry);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            interface::commands::cancel_background_task,
            // 签名更新命令必须全部注册到 Tauri；否则更新页和启动检查会收到 Command not found。
            interface::commands::check_updates,
            interface::commands::install_update,
            interface::commands::list_installed_packages,
            interface::commands::rollback_update,
            interface::commands::clear_current_inventory,
            interface::commands::list_character_archives,
            interface::commands::export_character_archive,
            interface::commands::import_character_archive,
            interface::commands::delete_character_archive,
            interface::commands::clear_character_archives,
            interface::commands::set_active_character,
            interface::commands::inspect_cbg_read,
            interface::commands::start_cbg_import_task,
            interface::commands::install_builtin_catalog,
            interface::commands::get_catalog_status,
            interface::commands::list_soul_sets,
            interface::commands::list_shikigami,
            interface::commands::list_my_souls,
            interface::commands::analyze_plus15_growth_quality,
            interface::commands::analyze_four_leg_embryo_decision,
            interface::commands::analyze_embryo_decision,
            interface::commands::list_my_shikigami,
            interface::commands::list_shikigami_bag,
            interface::commands::list_shikigami_shards,
            interface::commands::list_shikigami_story_progress,
            interface::commands::lookup_shikigami_shards_across_archives,
            interface::commands::get_current_items,
            interface::commands::get_current_realm_cards,
            interface::commands::get_current_data_summary,
            interface::commands::get_current_guild,
            interface::commands::list_my_soul_scores,
            interface::commands::get_soul_radar_cache,
            interface::commands::calculate_soul_radar,
            interface::commands::get_head_tail_cache,
            interface::commands::calculate_head_tail,
            interface::commands::simulate_enhancement,
            interface::commands::calculate_miracle_conch,
            interface::commands::get_miracle_conch_configuration,
            interface::commands::set_miracle_conch_configuration,
            // 评分计算先读取当前游戏档案；命令未注册时前端会直接收到 Command not found。
            interface::commands::list_profiles,
            // 规则库命令必须统一注册；缺少任意一个都会在 WebView 中表现为 Command not found。
            interface::commands::list_rule_versions,
            interface::commands::list_score_standards,
            interface::commands::preview_rule_preset,
            interface::commands::import_rule_preset,
            interface::commands::copy_rule_version,
            interface::commands::save_rule_version,
            interface::commands::export_rule_version,
            interface::commands::set_rule_activation,
            interface::commands::set_active_score_standard,
            interface::commands::delete_score_standard,
            interface::commands::preview_rule_impact,
            interface::commands::start_rule_recalculation_task,
        ])
        .build(tauri::generate_context!())
        .expect("无法创建平安志应用");

    app.run(|app_handle, event| {
        // 退出前清理任务、读取会话和受控子进程，避免本地资源留在后台。
        if matches!(event, RunEvent::ExitRequested { .. } | RunEvent::Exit) {
            let task_count = app_handle.state::<TaskRegistry>().cancel_all();
            let cleanup = app_handle.state::<ProcessRegistry>().cleanup_all();
            tracing::info!(
                cancelled_tasks = task_count,
                cleaned_processes = cleanup.len(),
                "平安志应用退出清理完成"
            );
        }
    });
}
