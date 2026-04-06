use ru_command_protocol::{
    AgentRegistration, AgentSnapshot, CreateTaskRequest, PendingTask, SubmitTaskResultRequest,
    TaskSnapshot, TaskStatus,
};
use rusqlite::{params, Connection, OptionalExtension};
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Database {
    connection: Mutex<Connection>,
}

pub enum CreateTaskOutcome {
    AgentNotFound,
    UnsupportedCommand,
    ExtraArgsNotAllowed,
    Created(TaskSnapshot),
}

pub enum ClaimTaskOutcome {
    AgentNotFound,
    NoTask,
    Claimed(PendingTask),
}

pub enum SubmitTaskResultOutcome {
    TaskNotFound,
    AgentMismatch,
    Updated(TaskSnapshot),
}

pub enum CancelTaskOutcome {
    NotFound,
    NotCancelable,
    Canceled(TaskSnapshot),
}

#[derive(Debug)]
pub enum DbError {
    Sql(rusqlite::Error),
    Json(serde_json::Error),
    LockPoisoned,
    Data(String),
}

impl Display for DbError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sql(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
            Self::LockPoisoned => write!(f, "database lock poisoned"),
            Self::Data(message) => write!(f, "{message}"),
        }
    }
}

impl Error for DbError {}

impl From<rusqlite::Error> for DbError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sql(value)
    }
}

impl From<serde_json::Error> for DbError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl Database {
    pub fn open(path: &str) -> Result<Self, DbError> {
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS agents (
                agent_id TEXT PRIMARY KEY,
                hostname TEXT NOT NULL,
                platform TEXT NOT NULL,
                poll_interval_secs INTEGER NOT NULL,
                registered_at_unix_secs INTEGER NOT NULL,
                last_seen_unix_secs INTEGER NOT NULL,
                commands_json TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS tasks (
                task_id INTEGER PRIMARY KEY AUTOINCREMENT,
                agent_id TEXT NOT NULL,
                command_name TEXT NOT NULL,
                args_json TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at_unix_secs INTEGER NOT NULL,
                dispatched_at_unix_secs INTEGER,
                acked_at_unix_secs INTEGER,
                started_at_unix_secs INTEGER,
                finished_at_unix_secs INTEGER,
                timeout_secs INTEGER,
                retry_count INTEGER NOT NULL DEFAULT 0,
                retry_reason TEXT,
                canceled_at_unix_secs INTEGER,
                cancel_reason TEXT,
                result_json TEXT,
                FOREIGN KEY(agent_id) REFERENCES agents(agent_id)
            );

            CREATE INDEX IF NOT EXISTS idx_tasks_agent_status_task_id
            ON tasks (agent_id, status, task_id);
            ",
        )?;
        Self::ensure_task_column(&connection, "dispatched_at_unix_secs", "INTEGER")?;
        Self::ensure_task_column(&connection, "acked_at_unix_secs", "INTEGER")?;
        Self::ensure_task_column(&connection, "timeout_secs", "INTEGER")?;
        Self::ensure_task_column(&connection, "retry_count", "INTEGER NOT NULL DEFAULT 0")?;
        Self::ensure_task_column(&connection, "retry_reason", "TEXT")?;
        Self::ensure_task_column(&connection, "canceled_at_unix_secs", "INTEGER")?;
        Self::ensure_task_column(&connection, "cancel_reason", "TEXT")?;

        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn register_agent(
        &self,
        registration: AgentRegistration,
    ) -> Result<AgentSnapshot, DbError> {
        let now = unix_now();
        let commands_json = serde_json::to_string(&registration.commands)?;
        let connection = self.connection.lock().map_err(|_| DbError::LockPoisoned)?;

        let registered_at_unix_secs: i64 = connection
            .query_row(
                "SELECT registered_at_unix_secs FROM agents WHERE agent_id = ?1",
                params![&registration.agent_id],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(now as i64);

        connection.execute(
            "
            INSERT INTO agents (
                agent_id,
                hostname,
                platform,
                poll_interval_secs,
                registered_at_unix_secs,
                last_seen_unix_secs,
                commands_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(agent_id) DO UPDATE SET
                hostname = excluded.hostname,
                platform = excluded.platform,
                poll_interval_secs = excluded.poll_interval_secs,
                last_seen_unix_secs = excluded.last_seen_unix_secs,
                commands_json = excluded.commands_json
            ",
            params![
                &registration.agent_id,
                &registration.hostname,
                &registration.platform,
                registration.poll_interval_secs as i64,
                registered_at_unix_secs,
                now as i64,
                commands_json
            ],
        )?;

        self.get_agent_inner(&connection, &registration.agent_id)?
            .ok_or_else(|| DbError::Data("agent disappeared after upsert".to_string()))
    }

    pub fn list_agents(&self) -> Result<Vec<AgentSnapshot>, DbError> {
        let connection = self.connection.lock().map_err(|_| DbError::LockPoisoned)?;
        let mut statement = connection.prepare(
            "
            SELECT
                agent_id,
                hostname,
                platform,
                poll_interval_secs,
                registered_at_unix_secs,
                last_seen_unix_secs,
                commands_json
            FROM agents
            ORDER BY agent_id
            ",
        )?;

        let rows = statement.query_map([], |row| self.read_agent(row))?;
        let mut agents = Vec::new();
        for row in rows {
            agents.push(row?);
        }
        Ok(agents)
    }

    pub fn get_agent(&self, agent_id: &str) -> Result<Option<AgentSnapshot>, DbError> {
        let connection = self.connection.lock().map_err(|_| DbError::LockPoisoned)?;
        self.get_agent_inner(&connection, agent_id)
    }

    pub fn create_task(&self, request: CreateTaskRequest) -> Result<CreateTaskOutcome, DbError> {
        let now = unix_now();
        let connection = self.connection.lock().map_err(|_| DbError::LockPoisoned)?;
        let Some(agent) = self.get_agent_inner(&connection, &request.agent_id)? else {
            return Ok(CreateTaskOutcome::AgentNotFound);
        };

        let Some(command) = agent
            .commands
            .iter()
            .find(|item| item.name == request.command_name)
        else {
            return Ok(CreateTaskOutcome::UnsupportedCommand);
        };

        if !command.allow_extra_args && !request.args.is_empty() {
            return Ok(CreateTaskOutcome::ExtraArgsNotAllowed);
        }

        let args_json = serde_json::to_string(&request.args)?;
        connection.execute(
            "
            INSERT INTO tasks (
                agent_id,
                command_name,
                args_json,
                status,
                created_at_unix_secs,
                timeout_secs,
                retry_count
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)
            ",
            params![
                &request.agent_id,
                &request.command_name,
                args_json,
                task_status_to_str(TaskStatus::Queued),
                now as i64,
                request.timeout_secs.map(|value| value as i64)
            ],
        )?;

        let task_id = connection.last_insert_rowid() as u64;
        let task = self
            .get_task_inner(&connection, task_id)?
            .ok_or_else(|| DbError::Data("task disappeared after insert".to_string()))?;

        Ok(CreateTaskOutcome::Created(task))
    }

    pub fn list_tasks(&self) -> Result<Vec<TaskSnapshot>, DbError> {
        let connection = self.connection.lock().map_err(|_| DbError::LockPoisoned)?;
        let mut statement = connection.prepare(
            "
            SELECT
                task_id,
                agent_id,
                command_name,
                args_json,
                status,
                created_at_unix_secs,
                dispatched_at_unix_secs,
                acked_at_unix_secs,
                started_at_unix_secs,
                finished_at_unix_secs,
                timeout_secs,
                retry_count,
                retry_reason,
                canceled_at_unix_secs,
                cancel_reason,
                result_json
            FROM tasks
            ORDER BY task_id
            ",
        )?;

        let rows = statement.query_map([], |row| self.read_task(row))?;
        let mut tasks = Vec::new();
        for row in rows {
            tasks.push(row?);
        }
        Ok(tasks)
    }

    pub fn list_tasks_for_agent(
        &self,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<TaskSnapshot>, DbError> {
        let connection = self.connection.lock().map_err(|_| DbError::LockPoisoned)?;
        let mut statement = connection.prepare(
            "
            SELECT
                task_id,
                agent_id,
                command_name,
                args_json,
                status,
                created_at_unix_secs,
                dispatched_at_unix_secs,
                acked_at_unix_secs,
                started_at_unix_secs,
                finished_at_unix_secs,
                timeout_secs,
                retry_count,
                retry_reason,
                canceled_at_unix_secs,
                cancel_reason,
                result_json
            FROM tasks
            WHERE agent_id = ?1
            ORDER BY task_id DESC
            LIMIT ?2
            ",
        )?;

        let rows =
            statement.query_map(params![agent_id, limit as i64], |row| self.read_task(row))?;
        let mut tasks = Vec::new();
        for row in rows {
            tasks.push(row?);
        }
        Ok(tasks)
    }

    pub fn get_task(&self, task_id: u64) -> Result<Option<TaskSnapshot>, DbError> {
        let connection = self.connection.lock().map_err(|_| DbError::LockPoisoned)?;
        self.get_task_inner(&connection, task_id)
    }

    pub fn requeue_incomplete_tasks(&self, agent_id: &str, reason: &str) -> Result<(), DbError> {
        let connection = self.connection.lock().map_err(|_| DbError::LockPoisoned)?;
        connection.execute(
            "
            UPDATE tasks
            SET status = ?1,
                dispatched_at_unix_secs = NULL,
                acked_at_unix_secs = NULL,
                started_at_unix_secs = NULL
                ,retry_count = retry_count + 1
                ,retry_reason = ?2
            WHERE agent_id = ?3
              AND status IN (?4, ?5)
              AND finished_at_unix_secs IS NULL
            ",
            params![
                task_status_to_str(TaskStatus::Queued),
                reason,
                agent_id,
                task_status_to_str(TaskStatus::Dispatched),
                task_status_to_str(TaskStatus::Running)
            ],
        )?;
        Ok(())
    }

    pub fn claim_task(&self, agent_id: &str) -> Result<ClaimTaskOutcome, DbError> {
        let now = unix_now();
        let mut connection = self.connection.lock().map_err(|_| DbError::LockPoisoned)?;
        let transaction = connection.transaction()?;

        let updated = transaction.execute(
            "UPDATE agents SET last_seen_unix_secs = ?1 WHERE agent_id = ?2",
            params![now as i64, agent_id],
        )?;
        if updated == 0 {
            transaction.rollback()?;
            return Ok(ClaimTaskOutcome::AgentNotFound);
        }

        let pending = {
            let mut statement = transaction.prepare(
                "
                SELECT
                    task_id,
                    agent_id,
                    command_name,
                    args_json,
                    created_at_unix_secs,
                    timeout_secs,
                    retry_count
                FROM tasks
                WHERE agent_id = ?1 AND status = ?2
                ORDER BY task_id
                LIMIT 1
                ",
            )?;

            statement
                .query_row(
                    params![agent_id, task_status_to_str(TaskStatus::Queued)],
                    |row| {
                        let args_json: String = row.get(3)?;
                        let args: Vec<String> =
                            serde_json::from_str(&args_json).map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    3,
                                    rusqlite::types::Type::Text,
                                    Box::new(error),
                                )
                            })?;

                        Ok(PendingTask {
                            task_id: row.get::<_, i64>(0)? as u64,
                            agent_id: row.get(1)?,
                            command_name: row.get(2)?,
                            args,
                            created_at_unix_secs: row.get::<_, i64>(4)? as u64,
                            timeout_secs: row.get::<_, Option<i64>>(5)?.map(|value| value as u64),
                            retry_count: row.get::<_, i64>(6)? as u32,
                        })
                    },
                )
                .optional()?
        };

        let Some(task) = pending else {
            transaction.commit()?;
            return Ok(ClaimTaskOutcome::NoTask);
        };

        transaction.execute(
            "
            UPDATE tasks
            SET status = ?1,
                dispatched_at_unix_secs = ?2,
                acked_at_unix_secs = NULL,
                started_at_unix_secs = NULL,
                finished_at_unix_secs = NULL,
                canceled_at_unix_secs = NULL,
                cancel_reason = NULL,
                result_json = NULL
            WHERE task_id = ?3
            ",
            params![
                task_status_to_str(TaskStatus::Dispatched),
                now as i64,
                task.task_id as i64
            ],
        )?;
        transaction.commit()?;

        Ok(ClaimTaskOutcome::Claimed(task))
    }

    pub fn acknowledge_task(
        &self,
        task_id: u64,
        agent_id: &str,
    ) -> Result<Option<TaskSnapshot>, DbError> {
        let now = unix_now();
        let connection = self.connection.lock().map_err(|_| DbError::LockPoisoned)?;
        let updated = connection.execute(
            "
            UPDATE tasks
            SET status = ?1,
                acked_at_unix_secs = ?2,
                started_at_unix_secs = CASE
                    WHEN started_at_unix_secs IS NULL THEN ?2
                    ELSE started_at_unix_secs
                END
            WHERE task_id = ?3
              AND agent_id = ?4
              AND status = ?5
            ",
            params![
                task_status_to_str(TaskStatus::Running),
                now as i64,
                task_id as i64,
                agent_id,
                task_status_to_str(TaskStatus::Dispatched)
            ],
        )?;

        if updated == 0 {
            return Ok(None);
        }

        self.get_task_inner(&connection, task_id)
    }

    pub fn retry_task(
        &self,
        task_id: u64,
        agent_id: &str,
        reason: &str,
    ) -> Result<Option<TaskSnapshot>, DbError> {
        let connection = self.connection.lock().map_err(|_| DbError::LockPoisoned)?;
        let updated = connection.execute(
            "
            UPDATE tasks
            SET status = ?1,
                dispatched_at_unix_secs = NULL,
                acked_at_unix_secs = NULL,
                started_at_unix_secs = NULL,
                finished_at_unix_secs = NULL,
                canceled_at_unix_secs = NULL,
                cancel_reason = NULL,
                result_json = NULL,
                retry_count = retry_count + 1,
                retry_reason = ?2
            WHERE task_id = ?3
              AND agent_id = ?4
              AND status IN (?5, ?6)
            ",
            params![
                task_status_to_str(TaskStatus::Queued),
                reason,
                task_id as i64,
                agent_id,
                task_status_to_str(TaskStatus::Dispatched),
                task_status_to_str(TaskStatus::Running)
            ],
        )?;

        if updated == 0 {
            return Ok(None);
        }

        self.get_task_inner(&connection, task_id)
    }

    pub fn submit_task_result(
        &self,
        task_id: u64,
        request: SubmitTaskResultRequest,
    ) -> Result<SubmitTaskResultOutcome, DbError> {
        let finished_at = unix_now();
        let result_json = serde_json::to_string(&request.result)?;
        let mut connection = self.connection.lock().map_err(|_| DbError::LockPoisoned)?;
        let transaction = connection.transaction()?;

        let task_row: Option<(String, String)> = transaction
            .query_row(
                "SELECT agent_id, status FROM tasks WHERE task_id = ?1",
                params![task_id as i64],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        let Some((task_agent_id, task_status)) = task_row else {
            transaction.rollback()?;
            return Ok(SubmitTaskResultOutcome::TaskNotFound);
        };

        if task_agent_id != request.agent_id {
            transaction.rollback()?;
            return Ok(SubmitTaskResultOutcome::AgentMismatch);
        }

        if !matches!(
            task_status_from_str(&task_status)?,
            TaskStatus::Dispatched | TaskStatus::Running
        ) {
            transaction.rollback()?;
            let task = self
                .get_task_inner(&connection, task_id)?
                .ok_or_else(|| DbError::Data("task disappeared after status check".to_string()))?;
            return Ok(SubmitTaskResultOutcome::Updated(task));
        }

        transaction.execute(
            "
            UPDATE tasks
            SET status = ?1,
                acked_at_unix_secs = COALESCE(acked_at_unix_secs, ?2),
                started_at_unix_secs = COALESCE(started_at_unix_secs, ?2),
                finished_at_unix_secs = ?2,
                result_json = ?3
            WHERE task_id = ?4
            ",
            params![
                task_status_to_str(if request.result.success {
                    TaskStatus::Succeeded
                } else {
                    TaskStatus::Failed
                }),
                finished_at as i64,
                result_json,
                task_id as i64
            ],
        )?;
        transaction.commit()?;

        let task = self
            .get_task_inner(&connection, task_id)?
            .ok_or_else(|| DbError::Data("task disappeared after update".to_string()))?;

        Ok(SubmitTaskResultOutcome::Updated(task))
    }

    pub fn cancel_task(
        &self,
        task_id: u64,
        reason: Option<String>,
    ) -> Result<CancelTaskOutcome, DbError> {
        let canceled_at = unix_now();
        let reason = reason.unwrap_or_else(|| "canceled by server".to_string());
        let connection = self.connection.lock().map_err(|_| DbError::LockPoisoned)?;

        let task_status: Option<String> = connection
            .query_row(
                "SELECT status FROM tasks WHERE task_id = ?1",
                params![task_id as i64],
                |row| row.get(0),
            )
            .optional()?;

        let Some(task_status) = task_status else {
            return Ok(CancelTaskOutcome::NotFound);
        };

        let task_status = task_status_from_str(&task_status)?;
        if !matches!(
            task_status,
            TaskStatus::Queued | TaskStatus::Dispatched | TaskStatus::Running
        ) {
            return Ok(CancelTaskOutcome::NotCancelable);
        }

        connection.execute(
            "
            UPDATE tasks
            SET status = ?1,
                finished_at_unix_secs = ?2,
                canceled_at_unix_secs = ?2,
                cancel_reason = ?3
            WHERE task_id = ?4
            ",
            params![
                task_status_to_str(TaskStatus::Canceled),
                canceled_at as i64,
                &reason,
                task_id as i64
            ],
        )?;

        let task = self
            .get_task_inner(&connection, task_id)?
            .ok_or_else(|| DbError::Data("task disappeared after cancel".to_string()))?;

        Ok(CancelTaskOutcome::Canceled(task))
    }

    fn get_agent_inner(
        &self,
        connection: &Connection,
        agent_id: &str,
    ) -> Result<Option<AgentSnapshot>, DbError> {
        let mut statement = connection.prepare(
            "
            SELECT
                agent_id,
                hostname,
                platform,
                poll_interval_secs,
                registered_at_unix_secs,
                last_seen_unix_secs,
                commands_json
            FROM agents
            WHERE agent_id = ?1
            ",
        )?;

        let agent = statement
            .query_row(params![agent_id], |row| self.read_agent(row))
            .optional()?;
        Ok(agent)
    }

    fn get_task_inner(
        &self,
        connection: &Connection,
        task_id: u64,
    ) -> Result<Option<TaskSnapshot>, DbError> {
        let mut statement = connection.prepare(
            "
            SELECT
                task_id,
                agent_id,
                command_name,
                args_json,
                status,
                created_at_unix_secs,
                dispatched_at_unix_secs,
                acked_at_unix_secs,
                started_at_unix_secs,
                finished_at_unix_secs,
                timeout_secs,
                retry_count,
                retry_reason,
                canceled_at_unix_secs,
                cancel_reason,
                result_json
            FROM tasks
            WHERE task_id = ?1
            ",
        )?;

        let task = statement
            .query_row(params![task_id as i64], |row| self.read_task(row))
            .optional()?;
        Ok(task)
    }

    fn read_agent(&self, row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentSnapshot> {
        let commands_json: String = row.get(6)?;
        let commands = serde_json::from_str(&commands_json).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                6,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;

        Ok(AgentSnapshot {
            agent_id: row.get(0)?,
            hostname: row.get(1)?,
            platform: row.get(2)?,
            poll_interval_secs: row.get::<_, i64>(3)? as u64,
            registered_at_unix_secs: row.get::<_, i64>(4)? as u64,
            last_seen_unix_secs: row.get::<_, i64>(5)? as u64,
            commands,
        })
    }

    fn read_task(&self, row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskSnapshot> {
        let args_json: String = row.get(3)?;
        let status: String = row.get(4)?;
        let result_json: Option<String> = row.get(15)?;

        let args = serde_json::from_str(&args_json).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
        let result = match result_json {
            Some(value) => Some(serde_json::from_str(&value).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    15,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?),
            None => None,
        };

        Ok(TaskSnapshot {
            task_id: row.get::<_, i64>(0)? as u64,
            agent_id: row.get(1)?,
            command_name: row.get(2)?,
            args,
            status: task_status_from_str(&status).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    4,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?,
            created_at_unix_secs: row.get::<_, i64>(5)? as u64,
            dispatched_at_unix_secs: row.get::<_, Option<i64>>(6)?.map(|value| value as u64),
            acked_at_unix_secs: row.get::<_, Option<i64>>(7)?.map(|value| value as u64),
            started_at_unix_secs: row.get::<_, Option<i64>>(8)?.map(|value| value as u64),
            finished_at_unix_secs: row.get::<_, Option<i64>>(9)?.map(|value| value as u64),
            timeout_secs: row.get::<_, Option<i64>>(10)?.map(|value| value as u64),
            retry_count: row.get::<_, i64>(11)? as u32,
            retry_reason: row.get(12)?,
            canceled_at_unix_secs: row.get::<_, Option<i64>>(13)?.map(|value| value as u64),
            cancel_reason: row.get(14)?,
            result,
        })
    }

    fn ensure_task_column(
        connection: &Connection,
        column_name: &str,
        column_definition: &str,
    ) -> Result<(), DbError> {
        let mut statement = connection.prepare("PRAGMA table_info(tasks)")?;
        let existing_columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;

        if existing_columns.iter().any(|column| column == column_name) {
            return Ok(());
        }

        connection.execute(
            &format!("ALTER TABLE tasks ADD COLUMN {column_name} {column_definition}"),
            [],
        )?;
        Ok(())
    }
}

fn task_status_to_str(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Queued => "queued",
        TaskStatus::Dispatched => "dispatched",
        TaskStatus::Running => "running",
        TaskStatus::Succeeded => "succeeded",
        TaskStatus::Failed => "failed",
        TaskStatus::Canceled => "canceled",
    }
}

fn task_status_from_str(value: &str) -> Result<TaskStatus, DbError> {
    match value {
        "queued" => Ok(TaskStatus::Queued),
        "dispatched" => Ok(TaskStatus::Dispatched),
        "running" => Ok(TaskStatus::Running),
        "succeeded" => Ok(TaskStatus::Succeeded),
        "failed" => Ok(TaskStatus::Failed),
        "canceled" => Ok(TaskStatus::Canceled),
        other => Err(DbError::Data(format!("unknown task status: {other}"))),
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{
        CancelTaskOutcome, ClaimTaskOutcome, CreateTaskOutcome, Database, SubmitTaskResultOutcome,
    };
    use ru_command_protocol::{
        AgentRegistration, CommandDescriptor, ExecutionResult, SubmitTaskResultRequest, TaskStatus,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn persists_agents_and_tasks_across_reopen() {
        let db_path = unique_db_path("persist");

        {
            let database = Database::open(db_path.to_str().unwrap()).unwrap();
            let agent = database
                .register_agent(sample_registration("node-persist"))
                .unwrap();
            assert_eq!(agent.agent_id, "node-persist");

            let task = match database
                .create_task(ru_command_protocol::CreateTaskRequest {
                    agent_id: "node-persist".to_string(),
                    command_name: "echo".to_string(),
                    args: vec!["hello".to_string()],
                    timeout_secs: Some(30),
                })
                .unwrap()
            {
                CreateTaskOutcome::Created(task) => task,
                _ => panic!("expected created task"),
            };

            assert_eq!(task.command_name, "echo");
        }

        {
            let database = Database::open(db_path.to_str().unwrap()).unwrap();
            let agents = database.list_agents().unwrap();
            let tasks = database.list_tasks().unwrap();

            assert_eq!(agents.len(), 1);
            assert_eq!(agents[0].agent_id, "node-persist");
            assert_eq!(tasks.len(), 1);
            assert_eq!(tasks[0].agent_id, "node-persist");
            assert_eq!(tasks[0].args, vec!["hello".to_string()]);
            assert_eq!(tasks[0].timeout_secs, Some(30));
        }

        cleanup_db_files(&db_path);
    }

    #[test]
    fn claims_and_completes_task() {
        let db_path = unique_db_path("task-flow");
        let database = Database::open(db_path.to_str().unwrap()).unwrap();
        database
            .register_agent(sample_registration("node-flow"))
            .unwrap();

        let created_task = match database
            .create_task(ru_command_protocol::CreateTaskRequest {
                agent_id: "node-flow".to_string(),
                command_name: "echo".to_string(),
                args: vec!["hello".to_string(), "sqlite".to_string()],
                timeout_secs: Some(10),
            })
            .unwrap()
        {
            CreateTaskOutcome::Created(task) => task,
            _ => panic!("expected created task"),
        };

        let claimed = match database.claim_task("node-flow").unwrap() {
            ClaimTaskOutcome::Claimed(task) => task,
            _ => panic!("expected claimed task"),
        };
        assert_eq!(
            database.get_task(claimed.task_id).unwrap().unwrap().status,
            TaskStatus::Dispatched
        );
        database
            .acknowledge_task(claimed.task_id, "node-flow")
            .unwrap()
            .expect("acknowledged task");
        assert_eq!(claimed.task_id, created_task.task_id);
        assert_eq!(
            claimed.args,
            vec!["hello".to_string(), "sqlite".to_string()]
        );

        let updated = match database
            .submit_task_result(
                claimed.task_id,
                SubmitTaskResultRequest {
                    agent_id: "node-flow".to_string(),
                    result: ExecutionResult {
                        success: true,
                        exit_code: Some(0),
                        stdout: "hello sqlite\n".to_string(),
                        stderr: String::new(),
                        duration_ms: 12,
                        error: None,
                    },
                },
            )
            .unwrap()
        {
            SubmitTaskResultOutcome::Updated(task) => task,
            _ => panic!("expected updated task"),
        };

        assert_eq!(super::task_status_to_str(updated.status), "succeeded");
        assert!(updated.result.is_some());
        assert_eq!(updated.result.unwrap().stdout, "hello sqlite\n");

        cleanup_db_files(&db_path);
    }

    #[test]
    fn retries_and_cancels_task() {
        let db_path = unique_db_path("retry-cancel");
        let database = Database::open(db_path.to_str().unwrap()).unwrap();
        database
            .register_agent(sample_registration("node-retry"))
            .unwrap();

        let created_task = match database
            .create_task(ru_command_protocol::CreateTaskRequest {
                agent_id: "node-retry".to_string(),
                command_name: "echo".to_string(),
                args: vec![],
                timeout_secs: Some(5),
            })
            .unwrap()
        {
            CreateTaskOutcome::Created(task) => task,
            _ => panic!("expected created task"),
        };

        let claimed = match database.claim_task("node-retry").unwrap() {
            ClaimTaskOutcome::Claimed(task) => task,
            _ => panic!("expected claimed task"),
        };
        let retried = database
            .retry_task(claimed.task_id, "node-retry", "task timeout expired")
            .unwrap()
            .expect("retried task");
        assert_eq!(retried.status, TaskStatus::Queued);
        assert_eq!(retried.retry_count, 1);
        assert_eq!(
            retried.retry_reason.as_deref(),
            Some("task timeout expired")
        );

        let canceled = match database
            .cancel_task(created_task.task_id, Some("operator canceled".to_string()))
            .unwrap()
        {
            CancelTaskOutcome::Canceled(task) => task,
            _ => panic!("expected canceled task"),
        };
        assert_eq!(canceled.status, TaskStatus::Canceled);
        assert_eq!(canceled.cancel_reason.as_deref(), Some("operator canceled"));

        cleanup_db_files(&db_path);
    }

    fn sample_registration(agent_id: &str) -> AgentRegistration {
        AgentRegistration {
            agent_id: agent_id.to_string(),
            hostname: "test-host".to_string(),
            platform: "test-platform".to_string(),
            poll_interval_secs: 3,
            commands: vec![CommandDescriptor {
                name: "echo".to_string(),
                description: "echo text".to_string(),
                default_args: Vec::new(),
                allow_extra_args: true,
            }],
        }
    }

    fn unique_db_path(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("ruchat-{prefix}-{unique}.db"))
    }

    fn cleanup_db_files(db_path: &PathBuf) {
        let _ = fs::remove_file(db_path);
        let _ = fs::remove_file(format!("{}-wal", db_path.display()));
        let _ = fs::remove_file(format!("{}-shm", db_path.display()));
    }
}
