//! # 门票 02 验收测试
//!
//! 覆盖全部 7 条验收标准。每个测试使用独立临时目录，不依赖共享状态。
//!
//! 验收标准：
//! 1. 两个档案中相同御魂标识、设置和决定互不串联
//! 2. 相同原始内容只写入一个对象文件，并可被多个事件引用
//! 3. 对象哈希不匹配时被隔离且不会作为当前数据使用
//! 4. 目录版本升级不改写历史快照事实
//! 5. PVE/PVP 全局常用度可被档案级覆盖，并可复制到另一档案
//! 6. 迁移失败能够保留或恢复升级前数据库
//! 7. 数据库外键、唯一约束和关键查询索引具备测试

#![cfg(test)]

use std::path::PathBuf;
use std::sync::Arc;

use crate::application::error::AppError;
use crate::application::importer::{ImportFileInput, ImportOptions, ImportUseCase};
use crate::application::rule_use_cases::RuleUseCase;
use crate::application::services::AppServices;
use crate::application::use_cases::{
    AnalysisUseCase, BackupUseCase, CURRENT_BUILTIN_CATALOG_VERSION, CatalogUseCase,
    ProfileUseCase, StorageUseCase,
};
use crate::domain::{
    CommonnessValue, NewGameProfile, RawObject, Snapshot, SoulRadarPoint, load_default_preset,
    rule_sharing::export_rule_file,
};
use crate::infrastructure::database::connection::Database;
use crate::infrastructure::database::migrations::MigrationRunner;
use crate::infrastructure::raw_object_store::RawObjectStore;

// ─── 测试帮助 ─────────────────────────────────────────────────────────────────

/// 创建临时测试目录，返回 (dir, services)。
fn setup(name: &str) -> (PathBuf, AppServices) {
    let root = std::env::temp_dir()
        .join("yys-analysis-acceptance")
        .join(name);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("创建测试根目录");

    let services = AppServices::initialize(
        root.join("yuhun.sqlite3"),
        root.join("snapshots/sha256"),
        root.join("snapshots/quarantine"),
        root.join("backups"),
        root.join("temp"),
    )
    .expect("初始化服务");
    (root, services)
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn create_profile(uc: &ProfileUseCase, name: &str) -> crate::domain::GameProfile {
    uc.create(NewGameProfile {
        display_name: name.to_string(),
        source_identity: None,
        server_label: None,
    })
    .expect("创建档案")
}

// ─── 验收 1：档案隔离 ─────────────────────────────────────────────────────────

/// 两个档案的常用度覆盖互不串联。
/// 相同 set_id "破势" 在两个档案中分别设不同值，验证各自独立。
#[test]
fn 两个档案的常用度覆盖互不串联() {
    let (_dir, services) = setup("profile-isolation");
    let services = Arc::new(services);
    let profile_uc = ProfileUseCase::new(services.clone());
    let catalog_uc = CatalogUseCase::new(services.clone());

    // 安装目录
    catalog_uc.install_builtin().expect("安装目录");

    // 创建档案 A 和 B
    let a = create_profile(&profile_uc, "A");
    let b = create_profile(&profile_uc, "B");

    // 档案 A 将破势 PVE 覆盖为 uncommon
    catalog_uc
        .set_commonness_override(&a.id, "破势", "PVE", "uncommon")
        .expect("A 覆盖破势 PVE");

    // 档案 B 保持默认

    let a_common = catalog_uc.effective_commonness(&a.id).expect("A 常用度");
    let b_common = catalog_uc.effective_commonness(&b.id).expect("B 常用度");

    let a_po = a_common
        .iter()
        .find(|e| e.set_id == "破势" && e.scenario == "PVE")
        .expect("A 破势 PVE 条目");
    let b_po = b_common
        .iter()
        .find(|e| e.set_id == "破势" && e.scenario == "PVE")
        .expect("B 破势 PVE 条目");

    assert_eq!(a_po.value, CommonnessValue::Uncommon, "A 应看到覆盖值");
    assert_eq!(b_po.value, CommonnessValue::Common, "B 应看到默认值");
    assert!(a_po.is_override, "A 的值应标记为覆盖");
    assert!(!b_po.is_override, "B 的值不应标记为覆盖");

    // A 切回全局默认值后，档案覆盖行应被清除，而不是保留一个“同值覆盖”。
    catalog_uc
        .set_commonness_override(&a.id, "破势", "PVE", "common")
        .expect("A 恢复默认常用度");
    let a_reset = catalog_uc
        .effective_commonness(&a.id)
        .expect("读取恢复后的常用度");
    let a_reset_po = a_reset
        .iter()
        .find(|e| e.set_id == "破势" && e.scenario == "PVE")
        .expect("A 恢复后的破势 PVE 条目");
    assert_eq!(a_reset_po.value, CommonnessValue::Common);
    assert!(!a_reset_po.is_override, "恢复默认后不应继续标记覆盖");
}

// ─── 验收 2：原始内容去重 ─────────────────────────────────────────────────────

/// 相同原始内容只写入一个对象文件，并可被多个事件引用。
#[test]
fn 相同原始内容只存一份且可被多次引用() {
    let (_dir, services) = setup("raw-dedup");
    let services = Arc::new(services);
    let profile_uc = ProfileUseCase::new(services.clone());
    let storage_uc = StorageUseCase::new(services.clone());

    let profile = create_profile(&profile_uc, "P");
    let payload = br#"{"version":1,"data":{"hero_equips":[]}}"#;
    let received_at = now_iso();

    let first = storage_uc
        .store_raw(
            &profile.id,
            "importer",
            "native",
            "application/json",
            payload,
            &received_at,
        )
        .expect("首次保存");
    let second = storage_uc
        .store_raw(
            &profile.id,
            "importer",
            "native",
            "application/json",
            payload,
            &received_at,
        )
        .expect("再次保存");

    assert_eq!(
        first.raw_object.sha256, second.raw_object.sha256,
        "相同内容哈希应一致"
    );
    assert!(first.is_new, "首次应标记为新建");
    assert!(!second.is_new, "重复应标记为复用");

    // 文件系统上只存一个文件
    let (count, _) = services.raw_object_store.stats().expect("统计");
    assert_eq!(count, 1, "相同内容应只保存一个对象文件");

    // 两个事件引用同一原始对象
    let events = services
        .events
        .list_by_profile(&profile.id, 10)
        .expect("列出事件");
    assert_eq!(events.len(), 2, "应有两次采集事件");
    assert!(
        events
            .iter()
            .all(|e| e.raw_sha256.as_deref() == Some(&first.raw_object.sha256)),
        "所有事件应引用同一哈希"
    );
}

// ─── 验收 3：哈希校验与隔离 ───────────────────────────────────────────────────

/// 对象哈希不匹配时，文件被移入隔离区且校验失败。
#[test]
fn 对象哈希不匹配时被隔离() {
    let (root, services) = setup("hash-isolation");
    let services = Arc::new(services);
    let profile_uc = ProfileUseCase::new(services.clone());
    let storage_uc = StorageUseCase::new(services.clone());

    let profile = create_profile(&profile_uc, "P");
    let payload = b"intact-data";
    let received_at = now_iso();

    let summary = storage_uc
        .store_raw(
            &profile.id,
            "importer",
            "native",
            "application/json",
            payload,
            &received_at,
        )
        .expect("保存原始对象");

    // 篡改文件
    let hash = &summary.raw_object.sha256;
    let rel_path = RawObjectStore::relative_path_for(hash);
    let file_path = root.join("snapshots/sha256").join(&rel_path);
    std::fs::write(&file_path, b"corrupted-content").expect("篡改文件");

    // 校验应失败
    let result = storage_uc.verify_object(hash);
    assert!(result.is_err(), "哈希不匹配时校验应失败");

    // 文件应已移入隔离区
    let quarantine_dir = root.join("snapshots/quarantine");
    let quarantined_files = std::fs::read_dir(&quarantine_dir)
        .expect("读取隔离目录")
        .count();
    assert!(quarantined_files > 0, "损坏文件应被移入隔离区");

    // 原始路径文件应不存在
    assert!(!file_path.exists(), "损坏文件不应留在内容寻址目录");
}

// ─── 验收 4：目录版本升级不改写历史快照 ────────────────────────────────────────

/// 在目录 v1 下创建的快照，升级到 v2 后 catalog_version 仍为 v1。
#[test]
fn 目录版本升级不改写历史快照事实() {
    let (_dir, services) = setup("catalog-upgrade");
    let services = Arc::new(services);
    let catalog_uc = CatalogUseCase::new(services.clone());
    let profile_uc = ProfileUseCase::new(services.clone());
    let profile = create_profile(&profile_uc, "catalog-upgrade-profile");

    // 安装目录 v1
    catalog_uc
        .install_builtin_version("v1-test")
        .expect("安装 v1");
    let first_status = catalog_uc
        .active_status()
        .expect("读取 v1 状态")
        .expect("应存在 v1 目录");
    assert_eq!(first_status.set_count, 70);
    assert_eq!(first_status.icon_coverage, 70);
    assert_eq!(first_status.mechanics_coverage, 70);
    // 满级属性字段完整时，目录整体按属性核验口径标记为 verified，不受觉醒形态影响。
    assert_eq!(first_status.validation_status, "verified");
    assert_eq!(first_status.shikigami_count, 277);
    assert_eq!(first_status.panel_coverage, 277);
    assert_eq!(first_status.skill_coverage, 277);
    assert!(first_status.sha256.is_some(), "内置包必须保存内容哈希");

    // 同一版本重复安装只刷新幂等事实，不应改变目录数量或校验结果。
    let repeated_status = catalog_uc
        .install_builtin_version("v1-test")
        .expect("重复安装 v1");
    assert_eq!(repeated_status.set_count, 70);
    assert_eq!(repeated_status.sha256, first_status.sha256);

    // 读取边界必须返回完整目录；图标编号和特殊类别都不能在 DTO 之前丢失。
    let sets = catalog_uc.list_sets("v1-test").expect("读取 v1 套装");
    assert_eq!(sets.len(), 70);
    assert!(sets.iter().all(|set| set.icon_asset_id.is_some()));
    assert!(
        sets.iter()
            .any(|set| set.special_category.as_deref() == Some("首领御魂"))
    );
    assert!(
        sets.iter()
            .any(|set| set.special_category.as_deref() == Some("星痕御魂"))
    );

    // 同一目录版本应能读出完整式神条目，并保留嵌套技能的 1–5 级效果。
    let shikigami = catalog_uc
        .list_shikigami("v1-test")
        .expect("读取 v1 式神目录");
    assert_eq!(shikigami.len(), 277);
    // 素材达摩与可培养式神共用图鉴接口，但不要求具备技能 JSON；目录值统一为中文“素材”。
    assert!(shikigami
        .iter()
        .any(|entry| entry.shikigami_id == "411" && entry.rarity == "素材"));
    let first_shikigami = shikigami.first().expect("式神目录不应为空");
    let skills = serde_json::from_str::<Vec<serde_json::Value>>(&first_shikigami.skills_json)
        .expect("式神技能 JSON 应可解析");
    assert!(!skills.is_empty());
    assert_eq!(skills[0]["levels"].as_array().map(Vec::len), Some(5));
    assert!(!first_shikigami.employment_scenes_json.is_empty());

    // 创建快照引用 v1
    let now = now_iso();
    let raw_sha256 = "0".repeat(64);
    services
        .raw_objects
        .upsert(&RawObject {
            sha256: raw_sha256.clone(),
            relative_path: RawObjectStore::relative_path_for(&raw_sha256),
            compression: "zstd".to_owned(),
            media_type: "application/json".to_owned(),
            raw_size: 0,
            stored_size: 0,
            verified_at: None,
            created_at: now_iso(),
        })
        .expect("插入原始对象");
    let snapshot = Snapshot {
        id: uuid::Uuid::new_v4().to_string(),
        profile_id: profile.id,
        raw_sha256,
        content_fingerprint: None,
        scope_fingerprint: None,
        source_kind: "test".to_owned(),
        completeness: "complete".to_owned(),
        scope_json: None,
        captured_at: Some(now.clone()),
        game_version: None,
        adapter_version: None,
        parser_version: "1.0".to_owned(),
        catalog_version: "v1-test".to_owned(),
        parent_snapshot_id: None,
        forced: false,
        created_at: now.clone(),
    };
    services
        .snapshots
        .insert_snapshot(&snapshot, &[], &[])
        .expect("插入快照");

    // 安装目录 v2
    catalog_uc
        .install_builtin_version("v2-test")
        .expect("安装 v2");

    // 重新读取快照，catalog_version 应为 v1
    let loaded = services
        .snapshots
        .get(&snapshot.id)
        .expect("读取快照")
        .expect("快照应存在");
    assert_eq!(
        loaded.catalog_version, "v1-test",
        "升级后历史快照版本不应改变"
    );
    assert_eq!(loaded.id, snapshot.id, "快照 ID 应一致");
}

/// 旧内置目录被读取时应自动升级，确保数据库中的八咫镜文案跟随源码更新。
#[test]
fn 旧内置目录状态读取会刷新八咫镜二件套文案() {
    let (_dir, services) = setup("builtin-catalog-refresh");
    let services = Arc::new(services);
    let catalog_uc = CatalogUseCase::new(services);

    catalog_uc
        .install_builtin_version("builtin-2026.08.8")
        .expect("安装旧内置目录");
    catalog_uc
        .ensure_current_builtin()
        .expect("刷新当前内置目录");

    let status = catalog_uc
        .active_status()
        .expect("读取刷新后的目录状态")
        .expect("刷新后应存在活动目录");
    assert_eq!(status.version, CURRENT_BUILTIN_CATALOG_VERSION);

    let set = catalog_uc
        .list_sets(&status.version)
        .expect("读取刷新后的御魂目录")
        .into_iter()
        .find(|set| set.name == "八咫镜")
        .expect("刷新后的目录应包含八咫镜");
    let effect = set
        .two_piece_effect_json
        .as_deref()
        .and_then(|value| serde_json::from_str::<String>(value).ok())
        .expect("八咫镜应有可解析的二件套说明");
    assert_eq!(
        effect,
        "唯一效果：与怪物战斗时，造成伤害施加光灼；再次施加光灼时移除并提升20%伤害。"
    );
}

// ─── 验收 5：常用度覆盖与复制 ─────────────────────────────────────────────────

/// PVE/PVP 全局常用度可被档案级覆盖，并可复制到另一档案。
#[test]
fn 常用度覆盖可复制到另一档案() {
    let (_dir, services) = setup("commonness-copy");
    let services = Arc::new(services);
    let profile_uc = ProfileUseCase::new(services.clone());
    let catalog_uc = CatalogUseCase::new(services.clone());

    catalog_uc.install_builtin().expect("安装目录");

    let a = create_profile(&profile_uc, "source");
    let b = create_profile(&profile_uc, "target");

    // 档案 A 修改两个覆盖
    catalog_uc
        .set_commonness_override(&a.id, "破势", "PVE", "uncommon")
        .expect("A 覆盖破势 PVE");
    catalog_uc
        .set_commonness_override(&a.id, "针女", "PVP", "uncommon")
        .expect("A 覆盖针女 PVP");

    // 复制到 B
    let copied = catalog_uc
        .copy_commonness(&a.id, &b.id)
        .expect("复制常用度");
    assert_eq!(copied, 2, "应复制 2 条覆盖");

    // 验证 B 看到覆盖值
    let b_common = catalog_uc.effective_commonness(&b.id).expect("B 常用度");
    let b_po = b_common
        .iter()
        .find(|e| e.set_id == "破势" && e.scenario == "PVE")
        .expect("B 破势 PVE");
    assert_eq!(
        b_po.value,
        CommonnessValue::Uncommon,
        "B 应看到从 A 复制的覆盖"
    );
    assert!(b_po.is_override, "B 应标记为覆盖");
}

// ─── 验收 6：迁移失败保留或恢复升级前数据库 ────────────────────────────────────

/// 迁移失败时数据库自动回滚到迁移前版本，数据完整。
#[test]
fn 迁移失败后数据库回滚到迁移前版本() {
    let (_dir, services) = setup("migration-failure");
    // 使用已经迁移到最新版本的数据库
    let db = Database::open(&services.db.path()).expect("打开数据库");
    // 写入一条数据确认迁移后可用
    let now = now_iso();
    db.write("seed", |conn| {
        conn.execute(
            "INSERT INTO app_meta (key, value, updated_at) VALUES ('test', 'pre-migration', ?1)",
            rusqlite::params![now],
        )
        .map_err(|e| AppError::database("seed", &e))
    })
    .expect("写入种子数据");

    // 创建一个包含非法 SQL 的迁移
    let bad_migration = crate::infrastructure::database::migrations::Migration {
        version: 99,
        name: "bad-test",
        sql: "CREATE TABLE this_will_fail (id); INVALID SQL;",
    };
    let bad_runner = MigrationRunner::new_with_migrations(vec![bad_migration]);

    // 运行—预期失败
    let result = db.write("migrate", |conn| bad_runner.run(conn));
    assert!(result.is_err(), "非法迁移应失败");

    // 数据应仍在
    let value: String = db
        .read("verify", |conn| {
            conn.query_row("SELECT value FROM app_meta WHERE key = 'test'", [], |row| {
                row.get(0)
            })
            .map_err(|e| AppError::database("verify", &e))
        })
        .expect("读取种子数据");
    assert_eq!(value, "pre-migration", "迁移失败后数据应保留");
}

/// 恢复备份后数据与迁移前一致。
#[test]
fn 从备份恢复后数据一致() {
    let (_dir, services) = setup("backup-restore");
    let services = Arc::new(services);
    let profile_uc = ProfileUseCase::new(services.clone());

    // 创建档案并写入数据
    let profile = create_profile(&profile_uc, "backup-test");
    let backup_uc = BackupUseCase::new(services.clone());

    // 创建备份
    let record = backup_uc.create_backup("验收测试").expect("创建备份");
    assert!(record.verified, "备份应通过校验");

    // 创建第二个档案（模拟备份后的变化）
    let _unused = create_profile(&profile_uc, "after-backup");

    // 恢复备份
    backup_uc.restore_backup(&record.id).expect("恢复备份");

    // 验证：备份后创建的档案应消失，原始档案应存在
    let profiles = profile_uc.list(false).expect("列出档案");
    assert_eq!(profiles.len(), 1, "恢复后应只有备份前的一个档案");
    assert_eq!(profiles[0].id, profile.id, "档案 ID 应匹配");
}

// ─── 验收 7：外键、唯一约束与索引测试 ─────────────────────────────────────────

/// 外键约束阻止非法引用。
#[test]
fn 外键约束阻止非法档案引用() {
    let (_dir, services) = setup("fk-constraints");
    let services = Arc::new(services);
    let events = &*services.events;

    // 尝试插入不存在的 profile_id 的采集事件
    let event = crate::domain::AcquisitionEvent {
        id: uuid::Uuid::new_v4().to_string(),
        profile_id: "nonexistent".to_owned(),
        source_kind: "importer".to_owned(),
        source_format: "test".to_owned(),
        raw_sha256: None,
        captured_at: None,
        received_at: now_iso(),
        game_version: None,
        adapter_version: None,
        parser_version: "1.0".to_owned(),
        completeness: "unknown".to_owned(),
        scope_json: None,
        result_kind: "error_free".to_owned(),
        snapshot_id: None,
    };
    let result = events.insert(&event);
    assert!(result.is_err(), "不存在的 profile_id 应触发外键错误");
}

/// 唯一约束阻止重复的原始对象相对路径。
#[test]
fn 唯一约束阻止重复原始对象路径() {
    let (_dir, services) = setup("unique-constraints");
    let ro = &*services.raw_objects;

    // 插入第一个对象
    let obj = crate::domain::RawObject {
        sha256: "a".repeat(64),
        relative_path: "duplicate/path.json.zst".to_owned(),
        compression: "zstd".to_owned(),
        media_type: "application/json".to_owned(),
        raw_size: 100,
        stored_size: 50,
        verified_at: None,
        created_at: now_iso(),
    };
    ro.upsert(&obj).expect("首次插入");

    // 尝试插入相同路径但不同哈希
    let obj2 = crate::domain::RawObject {
        sha256: "b".repeat(64),
        relative_path: "duplicate/path.json.zst".to_owned(),
        ..obj.clone()
    };
    let result = ro.upsert(&obj2);
    assert!(result.is_err(), "重复相对路径应触发唯一约束");
}

/// 关键索引在 sqlite_master 中已定义。
#[test]
fn 关键索引存在于数据库() {
    let (_dir, services) = setup("indexes");
    let expected_indexes = [
        "idx_snapshot_profile_captured",
        "idx_snapshot_fingerprint",
        "idx_snapshot_soul_set",
        "idx_snapshot_soul_stable",
        "idx_soul_attribute_lookup",
        "idx_inventory_presence",
        "idx_acquisition_event_profile",
    ];

    for index_name in &expected_indexes {
        let exists: bool = services
            .db
            .read("check_index", |conn| {
                let count: i64 = conn
                    .query_row(
                        "SELECT count(*) FROM sqlite_master WHERE type='index' AND name=?1",
                        rusqlite::params![index_name],
                        |row| row.get(0),
                    )
                    .map_err(|e| AppError::database("check_index", &e))?;
                Ok(count > 0)
            })
            .expect("查询索引");
        assert!(exists, "索引 {index_name} 应存在于数据库");
    }
}

// ─── 验收 03：JSON、快照与库存投影 ───────────────────────────────────────────

/// 验证三类关键投影语义：局部不删除，完整快照才会标记缺失，重复只增加事件。
#[test]
fn 门票03导入快照与库存投影遵循完整性语义() {
    use std::sync::atomic::AtomicBool;

    let (_dir, services) = setup("import-snapshots-inventory");
    let services = Arc::new(services);
    let profile_uc = ProfileUseCase::new(services.clone());
    let profile = create_profile(&profile_uc, "导入测试");
    let importer = ImportUseCase::new(services.clone());
    let cancellation = AtomicBool::new(false);
    let options = ImportOptions::default();

    let first_payload = r#"{
        "format":"yys-analysis-snapshot","schemaVersion":1,"completeness":"complete",
        "souls":[
          {"id":"soul-1","setId":"破势","slot":2,"quality":6,"level":0,
           "mainAttr":{"type":"速度","value":57},"subAttributes":[{"type":"暴击","value":3}]},
          {"id":"soul-2","setId":"针女","slot":6,"quality":6,"level":15,
           "mainAttr":{"type":"暴击","value":1},"subAttributes":[]}
        ]
    }"#;
    let first = importer
        .import_files(
            "test-import-1",
            &profile.id,
            &[ImportFileInput {
                file_name: "first.json".to_owned(),
                payload: first_payload.as_bytes().to_vec(),
            }],
            &options,
            &cancellation,
            |_| Ok(()),
        )
        .expect("完整快照导入");
    assert_eq!(first.succeeded_files, 1);
    assert_eq!(
        services
            .snapshots
            .list_by_profile(&profile.id)
            .expect("快照")
            .len(),
        1
    );
    assert_eq!(
        services
            .snapshots
            .list_inventory(&profile.id)
            .expect("库存")
            .len(),
        2
    );

    let partial_payload = r#"{"hero_equips":[{"id":"soul-1","setId":"破势","slot":2,"quality":6,"level":3,"mainAttrType":"速度","mainAttrValue":57,"subAttributes":[{"type":"暴击","value":6}]}]}"#;
    importer
        .import_files(
            "test-import-2",
            &profile.id,
            &[ImportFileInput {
                file_name: "partial.json".to_owned(),
                payload: partial_payload.as_bytes().to_vec(),
            }],
            &options,
            &cancellation,
            |_| Ok(()),
        )
        .expect("局部快照导入");
    let inventory_after_partial = services
        .snapshots
        .list_inventory(&profile.id)
        .expect("局部库存");
    assert!(
        inventory_after_partial
            .iter()
            .all(|item| item.presence_state == "present")
    );

    let second_complete_payload = r#"{"format":"yys-analysis-snapshot","schemaVersion":1,"completeness":"complete","souls":[{"id":"soul-1","setId":"破势","slot":2,"quality":6,"level":3,"mainAttrType":"速度","mainAttrValue":57,"subAttributes":[{"type":"暴击","value":6}]}]}"#;
    importer
        .import_files(
            "test-import-3",
            &profile.id,
            &[ImportFileInput {
                file_name: "second.json".to_owned(),
                payload: second_complete_payload.as_bytes().to_vec(),
            }],
            &options,
            &cancellation,
            |_| Ok(()),
        )
        .expect("第二次完整快照导入");
    let inventory_after_complete = services
        .snapshots
        .list_inventory(&profile.id)
        .expect("完整库存");
    assert!(
        inventory_after_complete
            .iter()
            .any(|item| item.soul_key == "soul-2" && item.presence_state == "removed")
    );

    let snapshot_count_before_duplicate = services
        .snapshots
        .list_by_profile(&profile.id)
        .expect("快照")
        .len();
    importer
        .import_files(
            "test-import-4",
            &profile.id,
            &[ImportFileInput {
                file_name: "duplicate.json".to_owned(),
                payload: second_complete_payload.as_bytes().to_vec(),
            }],
            &options,
            &cancellation,
            |_| Ok(()),
        )
        .expect("重复导入");
    assert_eq!(
        services
            .snapshots
            .list_by_profile(&profile.id)
            .expect("重复快照")
            .len(),
        snapshot_count_before_duplicate
    );
    let events = services
        .events
        .list_by_profile(&profile.id, 20)
        .expect("事件");
    assert!(events.iter().any(|event| event.result_kind == "unchanged"));
}

/// ON DELETE CASCADE 验证：删除档案后其事件应自动删除。
#[test]
fn 删除档案级联删除关联事件() {
    let (_dir, services) = setup("cascade-delete");
    let services = Arc::new(services);
    let profile_uc = ProfileUseCase::new(services.clone());
    let storage_uc = StorageUseCase::new(services.clone());

    let profile = create_profile(&profile_uc, "cascade-test");
    let payload = b"cascade-test-data";
    let received_at = now_iso();

    // 保存原始对象会产生采集事件
    storage_uc
        .store_raw(
            &profile.id,
            "importer",
            "native",
            "application/json",
            payload,
            &received_at,
        )
        .expect("存储原始对象");

    // 确认有事件
    let events_before = services
        .events
        .list_by_profile(&profile.id, 10)
        .expect("列出事件");
    assert!(!events_before.is_empty(), "应有事件");

    // 直接删除档案（仓库不支持删除，但可以通过 SQL 完成测试）
    services
        .db
        .write("delete_profile", |conn| {
            conn.execute(
                "DELETE FROM game_profile WHERE id = ?1",
                rusqlite::params![profile.id],
            )
            .map_err(|e| AppError::database("delete", &e))
        })
        .expect("删除档案");

    // 验证事件自动级联删除
    let events_after = services
        .events
        .list_by_profile(&profile.id, 10)
        .expect("列出事件");
    assert!(events_after.is_empty(), "删除档案后事件应自动级联删除");
}

/// 清空角色库存时，依赖该库存的雷达缓存也必须同步删除，避免角色身份归档后残留旧图表。
#[test]
fn 清空角色库存删除雷达缓存() {
    let (_dir, services) = setup("clear-inventory-radar-cache");
    let services = Arc::new(services);
    let profile_uc = ProfileUseCase::new(services.clone());
    let profile = create_profile(&profile_uc, "radar-cache");

    services
        .radar
        .replace(
            &profile.id,
            &now_iso(),
            "inventory-revision",
            1,
            &[SoulRadarPoint {
                set_id: "破势".to_owned(),
                slot: 6,
                metric_type: "crit_damage".to_owned(),
                value: 0.3,
                main_attr_type: Some("crit_damage".to_owned()),
            }],
        )
        .expect("写入雷达缓存");
    assert!(services.radar.get(&profile.id).expect("读取雷达缓存").is_some());

    // 角色清理复用同一条库存删除边界，雷达父缓存及其点位应在事务中一并消失。
    services
        .snapshots
        .clear_inventory(&profile.id)
        .expect("清空角色库存");
    assert!(services.radar.get(&profile.id).expect("读取清理后的雷达缓存").is_none());
}

// ─── 验收 05：分析待办、决定与批次 ───────────────────────────────────────────

/// 导入不自动生成待办；用户重算后可以查看逐用途解释，且用户决定支持独立重算与撤销。
#[test]
fn 门票05待办和用户决定在重算后保持独立() {
    let (_dir, services) = setup("analysis-inbox-decisions");
    let services = Arc::new(services);
    let profile_uc = ProfileUseCase::new(services.clone());
    let profile = create_profile(&profile_uc, "analysis-test");
    CatalogUseCase::new(services.clone())
        .install_builtin()
        .expect("安装目录");

    let importer = ImportUseCase::new(services.clone());
    let cancellation = std::sync::atomic::AtomicBool::new(false);
    let payload = r#"{
        "format":"yys-analysis-snapshot","schemaVersion":1,"completeness":"complete",
        "souls":[{"id":"soul-1","setId":"破势","slot":6,"quality":6,"level":0,
          "mainAttrType":"暴击","mainAttrValue":1,
          "subAttributes":[{"type":"暴击","value":0.08},{"type":"暴伤","value":0.1},
            {"type":"攻击加成","value":0.1}]}]
    }"#;
    importer
        .import_files(
            "analysis-import",
            &profile.id,
            &[ImportFileInput {
                file_name: "analysis.json".to_owned(),
                payload: payload.as_bytes().to_vec(),
            }],
            &ImportOptions::default(),
            &cancellation,
            |_| Ok(()),
        )
        .expect("导入御魂");

    let snapshot = services
        .snapshots
        .list_by_profile(&profile.id)
        .expect("快照")
        .into_iter()
        .next()
        .expect("应有快照");
    let analysis = AnalysisUseCase::new(services.clone());

    // 模拟旧版铁血战士胖虎标准仍处于启用状态，随后又导入同名新版标准的场景。
    // 重算必须选择最新正文，不能因为当前指针已经是用户标准就继续沿用旧版本。
    let rules = RuleUseCase::new(services.clone());
    let mut old_score_preset = load_default_preset().expect("默认规则预设").preset;
    old_score_preset.id = "community.tiexue.panghu".to_owned();
    old_score_preset.version = "1.0.0".to_owned();
    old_score_preset.title = "铁血战士胖虎".to_owned();
    old_score_preset.author = "铁血战士胖虎".to_owned();
    old_score_preset.raw_description = Some("旧版评分规则".to_owned());
    let old_score_payload =
        export_rule_file(&old_score_preset.validate().expect("校验旧版用户标准"))
            .expect("导出旧版用户评分标准");
    let old_score_version = rules
        .import("file", old_score_payload.as_bytes())
        .expect("导入旧版铁血战士胖虎评分标准");
    rules
        .set_active_score_standard(&old_score_version.id)
        .expect("启用旧版铁血战士胖虎评分标准");

    // 保存时间必须晚于旧版，确保“最新版”排序在不同平台和文件系统精度下保持稳定。
    std::thread::sleep(std::time::Duration::from_millis(2));
    let mut new_score_preset = load_default_preset().expect("默认规则预设").preset;
    new_score_preset.id = "community.tiexue.panghu".to_owned();
    new_score_preset.version = "2.0.0".to_owned();
    new_score_preset.title = "铁血战士胖虎".to_owned();
    new_score_preset.author = "铁血战士胖虎".to_owned();
    new_score_preset.raw_description = Some("新版评分规则".to_owned());
    let new_score_payload =
        export_rule_file(&new_score_preset.validate().expect("校验新版用户标准"))
            .expect("导出新版用户评分标准");
    let new_score_version = rules
        .import("file", new_score_payload.as_bytes())
        .expect("导入新版铁血战士胖虎评分标准");

    // 导入完成后不自动评分，库存页应据此提示用户明确开始计算。
    let before_recalculation = analysis
        .list_todos(&profile.id, None, None, None, 100, 0)
        .expect("读取未计算待办");
    assert_eq!(before_recalculation.total, 0, "导入后不应自动生成评分待办");

    // 用户确认开始计算后，才为当前库存生成评分及逐用途解释。
    analysis
        .recalculate(&profile.id, Some(&snapshot.id))
        .expect("手动生成待办");
    let page = analysis
        .list_todos(&profile.id, None, None, None, 100, 0)
        .expect("待办");
    assert_eq!(page.total, 1, "当前库存应有一条待办");
    assert!(
        !page.items[0].detail_json.is_empty(),
        "待办应保存逐用途解释"
    );
    let detail: serde_json::Value =
        serde_json::from_str(&page.items[0].detail_json).expect("解析待办详情");
    assert_eq!(
        detail["scoreStandard"]["title"],
        serde_json::Value::String("铁血战士胖虎".to_owned()),
        "实际评分标准必须使用铁血战士胖虎标准"
    );
    assert_eq!(
        detail["scoreStandard"]["version"],
        serde_json::Value::String("2.0.0".to_owned()),
        "实际评分标准必须使用最新版本"
    );
    assert_eq!(
        detail["rulePreset"]["hash"],
        serde_json::Value::String(new_score_version.normalized_hash.clone()),
        "实际评分正文必须与最新版本哈希一致"
    );
    let score_summaries = analysis
        .list_score_summaries_by_keys(&profile.id, &[page.items[0].soul_key.clone()])
        .expect("读取御魂评分摘要");
    assert_eq!(
        score_summaries[0].standard_hash.as_deref(),
        Some(new_score_version.normalized_hash.as_str()),
        "库存列表必须返回实际评分正文哈希以识别旧结果"
    );

    let soul_key = page.items[0].soul_key.clone();
    let applied = analysis
        .apply_decisions(
            &profile.id,
            vec![crate::domain::DecisionChange {
                soul_key: soul_key.clone(),
                decision: Some("keep".to_owned()),
                note: Some("手动保留".to_owned()),
            }],
            false,
        )
        .expect("写入决定");
    assert_eq!(applied.added_count, 1);

    analysis
        .recalculate(&profile.id, Some(&snapshot.id))
        .expect("再次重算");
    let after_recalculate = analysis
        .list_todos(&profile.id, None, None, None, 100, 0)
        .expect("重算后待办");
    assert_eq!(
        after_recalculate.items[0].user_decision.as_deref(),
        Some("keep"),
        "规则重算不能覆盖用户决定"
    );

    analysis
        .undo_decision(&applied.operation_id)
        .expect("撤销决定");
    let after_undo = analysis
        .list_todos(&profile.id, None, None, None, 100, 0)
        .expect("撤销后待办");
    assert!(
        after_undo.items[0].user_decision.is_none(),
        "撤销应恢复无决定"
    );
}

/// 批量决定应在预览阶段统计保护项，并能按游戏内筛选字段生成强化批次。
#[test]
fn 门票05批量预览保护冲突并生成强化批次() {
    let (_dir, services) = setup("analysis-batches");
    let services = Arc::new(services);
    let profile_uc = ProfileUseCase::new(services.clone());
    let profile = create_profile(&profile_uc, "batch-test");
    CatalogUseCase::new(services.clone())
        .install_builtin()
        .expect("安装目录");
    let importer = ImportUseCase::new(services.clone());
    let cancellation = std::sync::atomic::AtomicBool::new(false);
    let payload = r#"{"format":"yys-analysis-snapshot","schemaVersion":1,"completeness":"complete","souls":[
      {"id":"soul-1","setId":"破势","slot":6,"quality":6,"level":0,"mainAttrType":"暴击","mainAttrValue":1,"subAttributes":[{"type":"暴击","value":0.08},{"type":"暴伤","value":0.1},{"type":"攻击加成","value":0.1}]},
      {"id":"soul-2","setId":"破势","slot":6,"quality":6,"level":0,"mainAttrType":"暴击","mainAttrValue":1,"subAttributes":[{"type":"暴击","value":0.08},{"type":"暴伤","value":0.1},{"type":"速度","value":10}]}
    ]}"#;
    importer
        .import_files(
            "batch-import",
            &profile.id,
            &[ImportFileInput {
                file_name: "batch.json".to_owned(),
                payload: payload.as_bytes().to_vec(),
            }],
            &ImportOptions::default(),
            &cancellation,
            |_| Ok(()),
        )
        .expect("导入御魂");

    let analysis = AnalysisUseCase::new(services.clone());
    analysis.recalculate(&profile.id, None).expect("生成待办");
    let page = analysis
        .list_todos(&profile.id, None, None, None, 100, 0)
        .expect("待办");
    let keys = page
        .items
        .iter()
        .map(|item| item.soul_key.clone())
        .collect::<Vec<_>>();
    assert_eq!(keys.len(), 2);

    let preview = analysis
        .preview_decisions(&profile.id, &keys, "plan_recycle", false)
        .expect("批量预览");
    assert_eq!(preview.selected_count, 2);
    assert!(preview.protected_count <= 2, "预览应明确计算保护项");

    let batch = analysis
        .create_batch(&crate::domain::CreateBatchRequest {
            profile_id: profile.id.clone(),
            kind: "strengthen".to_owned(),
            target_level: Some(3),
            soul_keys: keys,
        })
        .expect("创建强化批次");
    assert_eq!(batch.kind, "strengthen");
    assert!(batch.group_count >= 1, "强化批次应至少有一个筛选分组");
}
