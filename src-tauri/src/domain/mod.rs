//! 领域层承载御魂分析的业务事实、实体与仓库接口。
//!
//! 领域实体不依赖 Tauri、文件系统或数据库实现。仓库特质定义领域层所需的数据访问
//! 契约，具体实现在基础设施层完成。

pub mod actions;
pub mod embryo_decision;
pub mod growth_quality;
pub mod history;
pub mod miracle_conch;
pub mod models;
pub mod repositories;
pub mod rule_sharing;
pub mod rules;
pub mod simulation;

pub use actions::*;
pub use embryo_decision::*;
pub use growth_quality::*;
pub use history::*;
pub use miracle_conch::*;
pub use models::*;
pub use repositories::*;
pub use rule_sharing::*;
pub use rules::*;
pub use simulation::*;
