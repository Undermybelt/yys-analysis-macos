//! 数据库连接管理：WAL 模式、外键约束、单写入队列与短读取事务。
//!
//! 采用双连接模型：写入连接由互斥锁串行化，提交全部走 `Immediate` 短事务；
//! 读取连接只执行短只读查询，不与写入互斥，符合 WAL 的并发语义。
//! 整个连接集合放在 `RwLock<Option<...>>` 中，使备份恢复可以原子替换连接。

use crate::application::error::AppError;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};

/// 业务读写共享的数据库句柄；克隆只共享连接集合，不复制连接。
#[derive(Clone)]
pub struct Database {
    path: PathBuf,
    connections: std::sync::Arc<RwLock<Option<DatabaseConnections>>>,
}

/// 一对读写连接；写入连接是全库唯一的写入口。
struct DatabaseConnections {
    writer: Mutex<Connection>,
    reader: Mutex<Connection>,
}

impl Database {
    /// 打开或创建数据库文件，并配置 WAL 与约束。
    pub fn open(path: &Path) -> Result<Self, AppError> {
        let connections = open_connections(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            connections: std::sync::Arc::new(RwLock::new(Some(connections))),
        })
    }

    /// 数据库文件路径，供备份与诊断使用。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 串行写事务：锁写入连接，在 `Immediate` 事务中执行闭包并提交。
    pub fn write<T>(
        &self,
        operation: &str,
        f: impl FnOnce(&mut Connection) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let connections = self.connections_guard()?;
        let connections = connections
            .as_ref()
            .ok_or_else(|| AppError::internal("数据库正在恢复过程中，请稍后重试"))?;
        let mut writer = connections
            .writer
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());

        // 迁移器本身需要跨多个版本持有一笔事务；不能再套一层 BEGIN，
        // 否则 SQLite 会拒绝嵌套事务。普通写入仍由本方法统一包裹事务。
        if operation.starts_with("migrate") {
            return f(&mut writer);
        }

        // rusqlite 0.32 的 Transaction 只实现不可变解引用；使用显式 SQL 事务，
        // 既保留原有闭包的 `&mut Connection` 接口，也允许迁移用例开启后续事务检查。
        writer
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| AppError::database(operation, &error))?;

        match f(&mut writer) {
            Ok(value) => {
                writer
                    .execute_batch("COMMIT")
                    .map_err(|error| AppError::database(operation, &error))?;
                Ok(value)
            }
            Err(error) => {
                let _ = writer.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    /// 短读取：锁读取连接执行只读闭包，不开启事务。
    /// 业务闭包产生的错误原样返回，不重新包装成数据库错误。
    pub fn read<T>(
        &self,
        operation: &str,
        f: impl FnOnce(&Connection) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let connections = self.connections_guard()?;
        let connections = connections
            .as_ref()
            .ok_or_else(|| AppError::internal("数据库正在恢复过程中，请稍后重试"))?;
        let reader = connections
            .reader
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let result = f(&reader);
        if result.is_err() {
            tracing::debug!(operation, "数据库读取闭包返回错误");
        }
        result
    }

    /// 在关闭连接前把 WAL 中的已提交页合并回主数据库文件。
    /// 恢复流程随后会替换主文件，不能遗留旧 WAL 旁路文件污染新数据源。
    pub fn checkpoint(&self) -> Result<(), AppError> {
        let connections = self.connections_guard()?;
        let connections = connections
            .as_ref()
            .ok_or_else(|| AppError::internal("数据库正在恢复过程中，请稍后重试"))?;
        let writer = connections
            .writer
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        writer
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .map_err(|error| AppError::database("checkpoint", &error))
    }

    /// 关闭全部连接；恢复备份时先关闭再替换文件，最后重新打开。
    pub fn close(&self) {
        let mut guard = self
            .connections
            .write()
            .unwrap_or_else(|poison| poison.into_inner());
        *guard = None;
    }

    /// 重新打开指向当前路径的新连接；备份恢复后调用以切换数据源。
    pub fn reopen(&self) -> Result<(), AppError> {
        let fresh = open_connections(&self.path)?;
        let mut guard = self
            .connections
            .write()
            .unwrap_or_else(|poison| poison.into_inner());
        *guard = Some(fresh);
        Ok(())
    }

    /// 获取连接守卫；连接被关闭（恢复中）时返回结构化错误。
    fn connections_guard(
        &self,
    ) -> Result<std::sync::RwLockReadGuard<'_, Option<DatabaseConnections>>, AppError> {
        self.connections
            .read()
            .map_err(|_| AppError::internal("数据库状态锁不可用"))
            .and_then(|guard| {
                if guard.is_none() {
                    Err(AppError::internal("数据库正在恢复过程中，请稍后重试"))
                } else {
                    Ok(guard)
                }
            })
    }
}

/// 打开两个连接并统一配置 PRAGMA。
fn open_connections(path: &Path) -> Result<DatabaseConnections, AppError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| AppError::io("创建数据库目录", &error))?;
    }
    let writer =
        Connection::open(path).map_err(|error| AppError::database("打开数据库", &error))?;
    configure_connection(&writer)?;
    let reader =
        Connection::open(path).map_err(|error| AppError::database("打开数据库", &error))?;
    configure_connection(&reader)?;
    Ok(DatabaseConnections {
        writer: Mutex::new(writer),
        reader: Mutex::new(reader),
    })
}

/// 为单个连接配置 WAL、外键、busy 超时与同步级别。
fn configure_connection(connection: &Connection) -> Result<(), AppError> {
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(|error| AppError::database("启用 WAL 模式", &error))?;
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(|error| AppError::database("启用外键约束", &error))?;
    connection
        .pragma_update(None, "busy_timeout", 5_000)
        .map_err(|error| AppError::database("设置 busy_timeout", &error))?;
    connection
        .pragma_update(None, "synchronous", "NORMAL")
        .map_err(|error| AppError::database("设置 synchronous", &error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Database;
    use crate::application::error::AppError;

    fn temp_db(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("yys-analysis-test-db").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("创建测试目录");
        dir.join("test.sqlite3")
    }

    #[test]
    fn 写入与读取共用同一数据源() {
        let path = temp_db("connection-basic");
        let db = Database::open(&path).expect("打开数据库");

        db.write("test", |conn| {
            conn.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)", [])
                .map_err(|e| AppError::database("create", &e))?;
            conn.execute("INSERT INTO t (v) VALUES ('hello')", [])
                .map_err(|e| AppError::database("insert", &e))?;
            Ok(())
        })
        .expect("写入成功");

        let value = db
            .read("test", |conn| {
                conn.query_row("SELECT v FROM t WHERE id = 1", [], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|e| AppError::database("select", &e))
            })
            .expect("读取成功");
        assert_eq!(value, "hello");
    }

    #[test]
    fn 写事务失败时自动回滚() {
        let path = temp_db("connection-rollback");
        let db = Database::open(&path).expect("打开数据库");

        let result: Result<(), AppError> = db.write("test", |conn| {
            conn.execute("CREATE TABLE t (id INTEGER PRIMARY KEY)", [])
                .map_err(|e| AppError::database("create", &e))?;
            conn.execute("INSERT INTO t (id) VALUES (1)", [])
                .map_err(|e| AppError::database("insert", &e))?;
            Err(AppError::internal("测试失败"))
        });
        assert!(result.is_err());

        let count: i64 = db
            .read("test", |conn| {
                conn.query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name='t'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|e| AppError::database("count", &e))
            })
            .expect("读取成功");
        assert_eq!(count, 0, "回滚后不应残留部分表");
    }

    #[test]
    fn 关闭后重新打开可以恢复使用() {
        let path = temp_db("connection-reopen");
        let db = Database::open(&path).expect("打开数据库");

        db.write("test", |conn| {
            conn.execute("CREATE TABLE t (id INTEGER PRIMARY KEY)", [])
                .map_err(|e| AppError::database("create", &e))?;
            Ok(())
        })
        .expect("写入成功");

        db.close();
        db.reopen().expect("重新打开成功");

        let count: i64 = db
            .read("test", |conn| {
                conn.query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name='t'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|e| AppError::database("count", &e))
            })
            .expect("读取成功");
        assert_eq!(count, 1, "关闭后数据应持久化");
    }
}
