//! 基础设施层封装本地目录、结构化日志、受控子进程、数据库、原始对象存储与仓库实现。

pub mod character_archives;
pub mod current_data;
pub mod database;
pub mod history_repository;
pub mod logging;
pub mod paths;
pub mod process;
pub mod raw_object_store;
pub mod repositories;
pub mod shikigami_biography_rules;
