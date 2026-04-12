use ru_command_protocol::{
    CommandDescriptor, CreateTaskRequest, NodeRegistration, NodeSnapshot, PendingTask,
    SessionEvent, SessionEventKind, SubmitTaskResultRequest, TaskSnapshot, TaskStatus,
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
    NodeNotFound,
    UnsupportedCommand,
    ExtraArgsNotAllowed,
    Created(TaskSnapshot),
}

pub enum ClaimTaskOutcome {
    NodeNotFound,
    NoTask,
    Claimed(PendingTask),
}

pub enum SubmitTaskResultOutcome {
    TaskNotFound,
    NodeMismatch,
    Updated(TaskSnapshot),
}

pub enum CancelTaskOutcome {
    NotFound,
    NotCancelable,
    Canceled(TaskSnapshot),
}

#[derive(Debug, Clone)]
pub struct TaskAudit {
    pub source: String,
    pub actor: Option<String>,
}

impl TaskAudit {
    pub fn new(source: impl Into<String>, actor: Option<String>) -> Self {
        Self {
            source: source.into(),
            actor,
        }
    }
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
        Self::migrate_legacy_schema(&connection)?;
        Self::create_schema(&connection)?;
        Self::ensure_task_column(&connection, "dispatched_at_unix_secs", "INTEGER")?;
        Self::ensure_task_column(&connection, "acked_at_unix_secs", "INTEGER")?;
        Self::ensure_task_column(&connection, "timeout_secs", "INTEGER")?;
        Self::ensure_task_column(&connection, "created_via", "TEXT")?;
        Self::ensure_task_column(&connection, "created_by", "TEXT")?;
        Self::ensure_task_column(&connection, "retry_count", "INTEGER NOT NULL DEFAULT 0")?;
        Self::ensure_task_column(&connection, "retry_reason", "TEXT")?;
        Self::ensure_task_column(&connection, "canceled_at_unix_secs", "INTEGER")?;
        Self::ensure_task_column(&connection, "cancel_reason", "TEXT")?;
        Self::ensure_task_column(&connection, "canceled_via", "TEXT")?;
        Self::ensure_task_column(&connection, "canceled_by", "TEXT")?;

        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn register_node(&self, registration: NodeRegistration) -> Result<NodeSnapshot, DbError> {
        let now = unix_now();
        let connection = self.connection.lock().map_err(|_| DbError::LockPoisoned)?;

        let existing_commands_json: Option<String> = connection
            .query_row(
                "SELECT commands_json FROM nodes WHERE node_id = ?1",
                params![&registration.node_id],
                |row| row.get(0),
            )
            .optional()?;

        let commands_json = if registration.commands.is_empty() {
            existing_commands_json.unwrap_or_else(|| "[]".to_string())
        } else {
            serde_json::to_string(&registration.commands)?
        };

        let registered_at_unix_secs: i64 = connection
            .query_row(
                "SELECT registered_at_unix_secs FROM nodes WHERE node_id = ?1",
                params![&registration.node_id],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(now as i64);

        connection.execute(
            "
            INSERT INTO nodes (
                node_id,
                hostname,
                platform,
                poll_interval_secs,
                registered_at_unix_secs,
                last_seen_unix_secs,
                commands_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(node_id) DO UPDATE SET
                hostname = excluded.hostname,
                platform = excluded.platform,
                poll_interval_secs = excluded.poll_interval_secs,
                last_seen_unix_secs = excluded.last_seen_unix_secs,
                commands_json = excluded.commands_json
            ",
            params![
                &registration.node_id,
                &registration.hostname,
                &registration.platform,
                registration.poll_interval_secs as i64,
                registered_at_unix_secs,
                now as i64,
                commands_json
            ],
        )?;

        self.get_node_inner(&connection, &registration.node_id)?
            .ok_or_else(|| DbError::Data("node disappeared after upsert".to_string()))
    }

    pub fn list_nodes(&self) -> Result<Vec<NodeSnapshot>, DbError> {
        let connection = self.connection.lock().map_err(|_| DbError::LockPoisoned)?;
        let mut statement = connection.prepare(
            "
            SELECT
                node_id,
                hostname,
                platform,
                poll_interval_secs,
                registered_at_unix_secs,
                last_seen_unix_secs,
                commands_json
            FROM nodes
            ORDER BY node_id
            ",
        )?;

        let rows = statement.query_map([], |row| self.read_node(row))?;
        let mut nodes = Vec::new();
        for row in rows {
            nodes.push(row?);
        }
        Ok(nodes)
    }

    pub fn get_node(&self, node_id: &str) -> Result<Option<NodeSnapshot>, DbError> {
        let connection = self.connection.lock().map_err(|_| DbError::LockPoisoned)?;
        self.get_node_inner(&connection, node_id)
    }

    pub fn update_node_commands(
        &self,
        node_id: &str,
        commands: Vec<CommandDescriptor>,
    ) -> Result<Option<NodeSnapshot>, DbError> {
        let now = unix_now();
        let commands_json = serde_json::to_string(&commands)?;
        let connection = self.connection.lock().map_err(|_| DbError::LockPoisoned)?;
        let updated = connection.execute(
            "
            UPDATE nodes
            SET commands_json = ?1,
                last_seen_unix_secs = ?2
            WHERE node_id = ?3
            ",
            params![commands_json, now as i64, node_id],
        )?;

        if updated == 0 {
            return Ok(None);
        }

        self.get_node_inner(&connection, node_id)
    }

    pub fn create_task(
        &self,
        request: CreateTaskRequest,
        audit: TaskAudit,
    ) -> Result<CreateTaskOutcome, DbError> {
        let now = unix_now();
        let connection = self.connection.lock().map_err(|_| DbError::LockPoisoned)?;
        let Some(node) = self.get_node_inner(&connection, &request.node_id)? else {
            return Ok(CreateTaskOutcome::NodeNotFound);
        };

        let Some(command) = node
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
                node_id,
                command_name,
                args_json,
                status,
                created_at_unix_secs,
                created_via,
                created_by,
                timeout_secs,
                retry_count
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0)
            ",
            params![
                &request.node_id,
                &request.command_name,
                args_json,
                task_status_to_str(TaskStatus::Queued),
                now as i64,
                audit.source,
                audit.actor,
                request.timeout_secs.map(|value| value as i64)
            ],
        )?;

        let task_id = connection.last_insert_rowid() as u64;
        let task = self
            .get_task_inner(&connection, task_id)?
            .ok_or_else(|| DbError::Data("task disappeared after insert".to_string()))?;

        Ok(CreateTaskOutcome::Created(task))
    }

    pub fn record_session_event(
        &self,
        node_id: &str,
        kind: SessionEventKind,
        message: impl Into<String>,
    ) -> Result<SessionEvent, DbError> {
        let created_at_unix_secs = unix_now();
        let message = message.into();
        let connection = self.connection.lock().map_err(|_| DbError::LockPoisoned)?;
        connection.execute(
            "
            INSERT INTO session_events (
                node_id,
                event_kind,
                message,
                created_at_unix_secs
            ) VALUES (?1, ?2, ?3, ?4)
            ",
            params![
                node_id,
                session_event_kind_to_str(kind),
                &message,
                created_at_unix_secs as i64
            ],
        )?;

        Ok(SessionEvent {
            event_id: connection.last_insert_rowid() as u64,
            node_id: node_id.to_string(),
            kind,
            message,
            created_at_unix_secs,
        })
    }

    pub fn list_session_events_for_node(
        &self,
        node_id: &str,
        limit: usize,
    ) -> Result<Vec<SessionEvent>, DbError> {
        let connection = self.connection.lock().map_err(|_| DbError::LockPoisoned)?;
        let mut statement = connection.prepare(
            "
            SELECT
                event_id,
                node_id,
                event_kind,
                message,
                created_at_unix_secs
            FROM session_events
            WHERE node_id = ?1
            ORDER BY event_id DESC
            LIMIT ?2
            ",
        )?;

        let rows = statement.query_map(params![node_id, limit as i64], |row| {
            self.read_session_event(row)
        })?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row?);
        }
        Ok(events)
    }

    pub fn list_session_events(
        &self,
        node_id: Option<&str>,
        kind: Option<SessionEventKind>,
        limit: usize,
    ) -> Result<Vec<SessionEvent>, DbError> {
        let connection = self.connection.lock().map_err(|_| DbError::LockPoisoned)?;
        let mut statement = connection.prepare(
            "
            SELECT
                event_id,
                node_id,
                event_kind,
                message,
                created_at_unix_secs
            FROM session_events
            WHERE (?1 IS NULL OR node_id = ?1)
              AND (?2 IS NULL OR event_kind = ?2)
            ORDER BY event_id DESC
            LIMIT ?3
            ",
        )?;

        let kind = kind.map(session_event_kind_to_str);
        let rows = statement.query_map(params![node_id, kind, limit as i64], |row| {
            self.read_session_event(row)
        })?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row?);
        }
        Ok(events)
    }

    fn create_schema(connection: &Connection) -> Result<(), DbError> {
        connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS nodes (
                node_id TEXT PRIMARY KEY,
                hostname TEXT NOT NULL,
                platform TEXT NOT NULL,
                poll_interval_secs INTEGER NOT NULL,
                registered_at_unix_secs INTEGER NOT NULL,
                last_seen_unix_secs INTEGER NOT NULL,
                commands_json TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS tasks (
                task_id INTEGER PRIMARY KEY AUTOINCREMENT,
                node_id TEXT NOT NULL,
                command_name TEXT NOT NULL,
                args_json TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at_unix_secs INTEGER NOT NULL,
                created_via TEXT,
                created_by TEXT,
                dispatched_at_unix_secs INTEGER,
                acked_at_unix_secs INTEGER,
                started_at_unix_secs INTEGER,
                finished_at_unix_secs INTEGER,
                timeout_secs INTEGER,
                retry_count INTEGER NOT NULL DEFAULT 0,
                retry_reason TEXT,
                canceled_at_unix_secs INTEGER,
                cancel_reason TEXT,
                canceled_via TEXT,
                canceled_by TEXT,
                result_json TEXT,
                FOREIGN KEY(node_id) REFERENCES nodes(node_id)
            );

            CREATE INDEX IF NOT EXISTS idx_tasks_node_status_task_id
            ON tasks (node_id, status, task_id);
            DROP INDEX IF EXISTS idx_tasks_agent_status_task_id;

            CREATE TABLE IF NOT EXISTS session_events (
                event_id INTEGER PRIMARY KEY AUTOINCREMENT,
                node_id TEXT NOT NULL,
                event_kind TEXT NOT NULL,
                message TEXT NOT NULL,
                created_at_unix_secs INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_session_events_node_event_id
            ON session_events (node_id, event_id DESC);
            ",
        )?;
        Ok(())
    }

    fn migrate_legacy_schema(connection: &Connection) -> Result<(), DbError> {
        let has_agents_table = Self::table_exists(connection, "agents")?;
        let has_tasks_table = Self::table_exists(connection, "tasks")?;
        let tasks_has_legacy_agent_id =
            has_tasks_table && Self::table_has_column(connection, "tasks", "agent_id")?;

        if !has_agents_table && !tasks_has_legacy_agent_id {
            return Ok(());
        }

        connection.pragma_update(None, "foreign_keys", "OFF")?;
        connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS nodes (
                node_id TEXT PRIMARY KEY,
                hostname TEXT NOT NULL,
                platform TEXT NOT NULL,
                poll_interval_secs INTEGER NOT NULL,
                registered_at_unix_secs INTEGER NOT NULL,
                last_seen_unix_secs INTEGER NOT NULL,
                commands_json TEXT NOT NULL
            );
            ",
        )?;

        if has_agents_table {
            connection.execute(
                "
                INSERT OR REPLACE INTO nodes (
                    node_id,
                    hostname,
                    platform,
                    poll_interval_secs,
                    registered_at_unix_secs,
                    last_seen_unix_secs,
                    commands_json
                )
                SELECT
                    agent_id,
                    hostname,
                    platform,
                    poll_interval_secs,
                    registered_at_unix_secs,
                    last_seen_unix_secs,
                    commands_json
                FROM agents
                ",
                [],
            )?;
        }

        if tasks_has_legacy_agent_id {
            connection.execute("ALTER TABLE tasks RENAME TO tasks_legacy", [])?;
            Self::create_schema(connection)?;
            connection.execute(
                "
                INSERT INTO tasks (
                    task_id,
                    node_id,
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
                )
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
                FROM tasks_legacy
                ",
                [],
            )?;
            connection.execute("DROP TABLE tasks_legacy", [])?;
        }

        if has_agents_table {
            connection.execute("DROP TABLE agents", [])?;
        }

        connection.pragma_update(None, "foreign_keys", "ON")?;
        Ok(())
    }

    pub fn list_tasks(&self) -> Result<Vec<TaskSnapshot>, DbError> {
        let connection = self.connection.lock().map_err(|_| DbError::LockPoisoned)?;
        let mut statement = connection.prepare(
            "
            SELECT
                task_id,
                node_id,
                command_name,
                args_json,
                status,
                created_at_unix_secs,
                created_via,
                created_by,
                dispatched_at_unix_secs,
                acked_at_unix_secs,
                started_at_unix_secs,
                finished_at_unix_secs,
                timeout_secs,
                retry_count,
                retry_reason,
                canceled_at_unix_secs,
                cancel_reason,
                canceled_via,
                canceled_by,
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

    pub fn list_recent_tasks(&self, limit: usize) -> Result<Vec<TaskSnapshot>, DbError> {
        let connection = self.connection.lock().map_err(|_| DbError::LockPoisoned)?;
        let mut statement = connection.prepare(
            "
            SELECT
                task_id,
                node_id,
                command_name,
                args_json,
                status,
                created_at_unix_secs,
                created_via,
                created_by,
                dispatched_at_unix_secs,
                acked_at_unix_secs,
                started_at_unix_secs,
                finished_at_unix_secs,
                timeout_secs,
                retry_count,
                retry_reason,
                canceled_at_unix_secs,
                cancel_reason,
                canceled_via,
                canceled_by,
                result_json
            FROM tasks
            ORDER BY task_id DESC
            LIMIT ?1
            ",
        )?;

        let rows = statement.query_map(params![limit as i64], |row| self.read_task(row))?;
        let mut tasks = Vec::new();
        for row in rows {
            tasks.push(row?);
        }
        Ok(tasks)
    }

    pub fn list_tasks_for_node(
        &self,
        node_id: &str,
        limit: usize,
    ) -> Result<Vec<TaskSnapshot>, DbError> {
        let connection = self.connection.lock().map_err(|_| DbError::LockPoisoned)?;
        let mut statement = connection.prepare(
            "
            SELECT
                task_id,
                node_id,
                command_name,
                args_json,
                status,
                created_at_unix_secs,
                created_via,
                created_by,
                dispatched_at_unix_secs,
                acked_at_unix_secs,
                started_at_unix_secs,
                finished_at_unix_secs,
                timeout_secs,
                retry_count,
                retry_reason,
                canceled_at_unix_secs,
                cancel_reason,
                canceled_via,
                canceled_by,
                result_json
            FROM tasks
            WHERE node_id = ?1
            ORDER BY task_id DESC
            LIMIT ?2
            ",
        )?;

        let rows =
            statement.query_map(params![node_id, limit as i64], |row| self.read_task(row))?;
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

    pub fn requeue_incomplete_tasks(&self, node_id: &str, reason: &str) -> Result<(), DbError> {
        let connection = self.connection.lock().map_err(|_| DbError::LockPoisoned)?;
        connection.execute(
            "
            UPDATE tasks
            SET status = ?1,
                dispatched_at_unix_secs = NULL,
                acked_at_unix_secs = NULL,
                started_at_unix_secs = NULL,
                canceled_at_unix_secs = NULL,
                cancel_reason = NULL,
                canceled_via = NULL,
                canceled_by = NULL,
                retry_count = retry_count + 1,
                retry_reason = ?2
            WHERE node_id = ?3
              AND status IN (?4, ?5)
              AND finished_at_unix_secs IS NULL
            ",
            params![
                task_status_to_str(TaskStatus::Queued),
                reason,
                node_id,
                task_status_to_str(TaskStatus::Dispatched),
                task_status_to_str(TaskStatus::Running)
            ],
        )?;
        Ok(())
    }

    pub fn claim_task(&self, node_id: &str) -> Result<ClaimTaskOutcome, DbError> {
        let now = unix_now();
        let mut connection = self.connection.lock().map_err(|_| DbError::LockPoisoned)?;
        let transaction = connection.transaction()?;

        let updated = transaction.execute(
            "UPDATE nodes SET last_seen_unix_secs = ?1 WHERE node_id = ?2",
            params![now as i64, node_id],
        )?;
        if updated == 0 {
            transaction.rollback()?;
            return Ok(ClaimTaskOutcome::NodeNotFound);
        }

        let pending = {
            let mut statement = transaction.prepare(
                "
                SELECT
                    task_id,
                    node_id,
                    command_name,
                    args_json,
                    created_at_unix_secs,
                    timeout_secs,
                    retry_count
                FROM tasks
                WHERE node_id = ?1 AND status = ?2
                ORDER BY task_id
                LIMIT 1
                ",
            )?;

            statement
                .query_row(
                    params![node_id, task_status_to_str(TaskStatus::Queued)],
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
                            node_id: row.get(1)?,
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
                canceled_via = NULL,
                canceled_by = NULL,
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
        node_id: &str,
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
              AND node_id = ?4
              AND status = ?5
            ",
            params![
                task_status_to_str(TaskStatus::Running),
                now as i64,
                task_id as i64,
                node_id,
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
        node_id: &str,
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
                canceled_via = NULL,
                canceled_by = NULL,
                result_json = NULL,
                retry_count = retry_count + 1,
                retry_reason = ?2
            WHERE task_id = ?3
              AND node_id = ?4
              AND status IN (?5, ?6)
            ",
            params![
                task_status_to_str(TaskStatus::Queued),
                reason,
                task_id as i64,
                node_id,
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
                "SELECT node_id, status FROM tasks WHERE task_id = ?1",
                params![task_id as i64],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        let Some((task_node_id, task_status)) = task_row else {
            transaction.rollback()?;
            return Ok(SubmitTaskResultOutcome::TaskNotFound);
        };

        if task_node_id != request.node_id {
            transaction.rollback()?;
            return Ok(SubmitTaskResultOutcome::NodeMismatch);
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
        audit: TaskAudit,
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
                cancel_reason = ?3,
                canceled_via = ?4,
                canceled_by = ?5
            WHERE task_id = ?6
            ",
            params![
                task_status_to_str(TaskStatus::Canceled),
                canceled_at as i64,
                &reason,
                audit.source,
                audit.actor,
                task_id as i64
            ],
        )?;

        let task = self
            .get_task_inner(&connection, task_id)?
            .ok_or_else(|| DbError::Data("task disappeared after cancel".to_string()))?;

        Ok(CancelTaskOutcome::Canceled(task))
    }

    fn get_node_inner(
        &self,
        connection: &Connection,
        node_id: &str,
    ) -> Result<Option<NodeSnapshot>, DbError> {
        let mut statement = connection.prepare(
            "
            SELECT
                node_id,
                hostname,
                platform,
                poll_interval_secs,
                registered_at_unix_secs,
                last_seen_unix_secs,
                commands_json
            FROM nodes
            WHERE node_id = ?1
            ",
        )?;

        let node = statement
            .query_row(params![node_id], |row| self.read_node(row))
            .optional()?;
        Ok(node)
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
                node_id,
                command_name,
                args_json,
                status,
                created_at_unix_secs,
                created_via,
                created_by,
                dispatched_at_unix_secs,
                acked_at_unix_secs,
                started_at_unix_secs,
                finished_at_unix_secs,
                timeout_secs,
                retry_count,
                retry_reason,
                canceled_at_unix_secs,
                cancel_reason,
                canceled_via,
                canceled_by,
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

    fn read_node(&self, row: &rusqlite::Row<'_>) -> rusqlite::Result<NodeSnapshot> {
        let commands_json: String = row.get(6)?;
        let commands = serde_json::from_str(&commands_json).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                6,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;

        Ok(NodeSnapshot {
            node_id: row.get(0)?,
            hostname: row.get(1)?,
            platform: row.get(2)?,
            poll_interval_secs: row.get::<_, i64>(3)? as u64,
            registered_at_unix_secs: row.get::<_, i64>(4)? as u64,
            last_seen_unix_secs: row.get::<_, i64>(5)? as u64,
            online: false,
            session_protocol_version: None,
            session_transport_stack: None,
            session_heartbeat_interval_secs: None,
            session_capabilities: Vec::new(),
            commands,
        })
    }

    fn read_task(&self, row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskSnapshot> {
        let args_json: String = row.get(3)?;
        let status: String = row.get(4)?;
        let result_json: Option<String> = row.get(19)?;

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
                    19,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?),
            None => None,
        };

        Ok(TaskSnapshot {
            task_id: row.get::<_, i64>(0)? as u64,
            node_id: row.get(1)?,
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
            created_via: row.get(6)?,
            created_by: row.get(7)?,
            dispatched_at_unix_secs: row.get::<_, Option<i64>>(8)?.map(|value| value as u64),
            acked_at_unix_secs: row.get::<_, Option<i64>>(9)?.map(|value| value as u64),
            started_at_unix_secs: row.get::<_, Option<i64>>(10)?.map(|value| value as u64),
            finished_at_unix_secs: row.get::<_, Option<i64>>(11)?.map(|value| value as u64),
            timeout_secs: row.get::<_, Option<i64>>(12)?.map(|value| value as u64),
            retry_count: row.get::<_, i64>(13)? as u32,
            retry_reason: row.get(14)?,
            canceled_at_unix_secs: row.get::<_, Option<i64>>(15)?.map(|value| value as u64),
            cancel_reason: row.get(16)?,
            canceled_via: row.get(17)?,
            canceled_by: row.get(18)?,
            result,
        })
    }

    fn read_session_event(&self, row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionEvent> {
        let kind: String = row.get(2)?;
        Ok(SessionEvent {
            event_id: row.get::<_, i64>(0)? as u64,
            node_id: row.get(1)?,
            kind: session_event_kind_from_str(&kind).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?,
            message: row.get(3)?,
            created_at_unix_secs: row.get::<_, i64>(4)? as u64,
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

    fn table_exists(connection: &Connection, table_name: &str) -> Result<bool, DbError> {
        Ok(connection
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 LIMIT 1",
                params![table_name],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    fn table_has_column(
        connection: &Connection,
        table_name: &str,
        column_name: &str,
    ) -> Result<bool, DbError> {
        let mut statement = connection.prepare(&format!("PRAGMA table_info({table_name})"))?;
        let existing_columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(existing_columns.iter().any(|column| column == column_name))
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

fn session_event_kind_to_str(value: SessionEventKind) -> &'static str {
    match value {
        SessionEventKind::ConnectRejected => "connect_rejected",
        SessionEventKind::AuthFailed => "auth_failed",
        SessionEventKind::SessionOpened => "session_opened",
        SessionEventKind::NodeRegistered => "node_registered",
        SessionEventKind::SessionClosed => "session_closed",
    }
}

fn session_event_kind_from_str(value: &str) -> Result<SessionEventKind, DbError> {
    match value {
        "connect_rejected" => Ok(SessionEventKind::ConnectRejected),
        "auth_failed" => Ok(SessionEventKind::AuthFailed),
        "session_opened" => Ok(SessionEventKind::SessionOpened),
        "node_registered" => Ok(SessionEventKind::NodeRegistered),
        "session_closed" => Ok(SessionEventKind::SessionClosed),
        other => Err(DbError::Data(format!(
            "unknown session event kind: {other}"
        ))),
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
        TaskAudit,
    };
    use ru_command_protocol::{
        CommandDescriptor, ExecutionResult, NodeRegistration, SessionEventKind,
        SubmitTaskResultRequest, TaskStatus,
    };
    use rusqlite::{params, Connection};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn persists_nodes_and_tasks_across_reopen() {
        let db_path = unique_db_path("persist");

        {
            let database = Database::open(db_path.to_str().unwrap()).unwrap();
            let node = database
                .register_node(sample_registration("node-persist"))
                .unwrap();
            assert_eq!(node.node_id, "node-persist");

            let task = match database
                .create_task(
                    ru_command_protocol::CreateTaskRequest {
                        node_id: "node-persist".to_string(),
                        command_name: "echo".to_string(),
                        args: vec!["hello".to_string()],
                        timeout_secs: Some(30),
                    },
                    sample_audit("console", "persist-test"),
                )
                .unwrap()
            {
                CreateTaskOutcome::Created(task) => task,
                _ => panic!("expected created task"),
            };

            assert_eq!(task.command_name, "echo");
            assert_eq!(task.created_via.as_deref(), Some("console"));
            assert_eq!(task.created_by.as_deref(), Some("persist-test"));
        }

        {
            let database = Database::open(db_path.to_str().unwrap()).unwrap();
            let nodes = database.list_nodes().unwrap();
            let tasks = database.list_tasks().unwrap();

            assert_eq!(nodes.len(), 1);
            assert_eq!(nodes[0].node_id, "node-persist");
            assert_eq!(tasks.len(), 1);
            assert_eq!(tasks[0].node_id, "node-persist");
            assert_eq!(tasks[0].args, vec!["hello".to_string()]);
            assert_eq!(tasks[0].timeout_secs, Some(30));
            assert_eq!(tasks[0].created_via.as_deref(), Some("console"));
            assert_eq!(tasks[0].created_by.as_deref(), Some("persist-test"));
        }

        cleanup_db_files(&db_path);
    }

    #[test]
    fn claims_and_completes_task() {
        let db_path = unique_db_path("task-flow");
        let database = Database::open(db_path.to_str().unwrap()).unwrap();
        database
            .register_node(sample_registration("node-flow"))
            .unwrap();

        let created_task = match database
            .create_task(
                ru_command_protocol::CreateTaskRequest {
                    node_id: "node-flow".to_string(),
                    command_name: "echo".to_string(),
                    args: vec!["hello".to_string(), "sqlite".to_string()],
                    timeout_secs: Some(10),
                },
                sample_audit("api", "flow-test"),
            )
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
                    node_id: "node-flow".to_string(),
                    result: ExecutionResult {
                        success: true,
                        exit_code: Some(0),
                        stdout: "hello sqlite\n".to_string(),
                        stderr: String::new(),
                        stdout_truncated: false,
                        stderr_truncated: false,
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
            .register_node(sample_registration("node-retry"))
            .unwrap();

        let created_task = match database
            .create_task(
                ru_command_protocol::CreateTaskRequest {
                    node_id: "node-retry".to_string(),
                    command_name: "echo".to_string(),
                    args: vec![],
                    timeout_secs: Some(5),
                },
                sample_audit("console", "retry-test"),
            )
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
            .cancel_task(
                created_task.task_id,
                Some("operator canceled".to_string()),
                sample_audit("console", "retry-test"),
            )
            .unwrap()
        {
            CancelTaskOutcome::Canceled(task) => task,
            _ => panic!("expected canceled task"),
        };
        assert_eq!(canceled.status, TaskStatus::Canceled);
        assert_eq!(canceled.cancel_reason.as_deref(), Some("operator canceled"));
        assert_eq!(canceled.canceled_via.as_deref(), Some("console"));
        assert_eq!(canceled.canceled_by.as_deref(), Some("retry-test"));

        cleanup_db_files(&db_path);
    }

    #[test]
    fn preserves_existing_commands_when_hello_registration_has_none() {
        let db_path = unique_db_path("preserve-commands");
        let database = Database::open(db_path.to_str().unwrap()).unwrap();

        let original = database
            .register_node(sample_registration("node-preserve"))
            .unwrap();
        assert_eq!(original.commands.len(), 1);

        let updated = database
            .register_node(NodeRegistration {
                node_id: "node-preserve".to_string(),
                hostname: "new-host".to_string(),
                platform: "new-platform".to_string(),
                poll_interval_secs: 5,
                commands: Vec::new(),
            })
            .unwrap();

        assert_eq!(updated.hostname, "new-host");
        assert_eq!(updated.platform, "new-platform");
        assert_eq!(updated.commands.len(), 1);
        assert_eq!(updated.commands[0].name, "echo");

        cleanup_db_files(&db_path);
    }

    #[test]
    fn migrates_legacy_agents_table_to_nodes() {
        let db_path = unique_db_path("legacy-schema");
        {
            let connection = Connection::open(db_path.to_str().unwrap()).unwrap();
            connection
                .execute_batch(
                    "
                    CREATE TABLE agents (
                        agent_id TEXT PRIMARY KEY,
                        hostname TEXT NOT NULL,
                        platform TEXT NOT NULL,
                        poll_interval_secs INTEGER NOT NULL,
                        registered_at_unix_secs INTEGER NOT NULL,
                        last_seen_unix_secs INTEGER NOT NULL,
                        commands_json TEXT NOT NULL
                    );

                    CREATE TABLE tasks (
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
                        result_json TEXT
                    );
                    ",
                )
                .unwrap();
            connection
                .execute(
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
                    ",
                    params![
                        "node-legacy",
                        "legacy-host",
                        "darwin-arm64",
                        3_i64,
                        10_i64,
                        20_i64,
                        serde_json::to_string(&sample_registration("node-legacy").commands)
                            .unwrap()
                    ],
                )
                .unwrap();
            connection
                .execute(
                    "
                    INSERT INTO tasks (
                        task_id,
                        agent_id,
                        command_name,
                        args_json,
                        status,
                        created_at_unix_secs,
                        retry_count
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                    ",
                    params![
                        1_i64,
                        "node-legacy",
                        "echo",
                        "[\"hello\"]",
                        "queued",
                        11_i64,
                        0_i64
                    ],
                )
                .unwrap();
        }

        let database = Database::open(db_path.to_str().unwrap()).unwrap();
        let nodes = database.list_nodes().unwrap();
        let tasks = database.list_tasks().unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node_id, "node-legacy");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].node_id, "node-legacy");

        cleanup_db_files(&db_path);
    }

    #[test]
    fn records_and_lists_session_events() {
        let db_path = unique_db_path("session-events");
        let database = Database::open(db_path.to_str().unwrap()).unwrap();
        database
            .register_node(sample_registration("node-events"))
            .unwrap();

        database
            .record_session_event(
                "node-events",
                SessionEventKind::SessionOpened,
                "mqtt connected",
            )
            .unwrap();
        database
            .record_session_event(
                "node-events",
                SessionEventKind::NodeRegistered,
                "client hello accepted",
            )
            .unwrap();

        let events = database
            .list_session_events_for_node("node-events", 10)
            .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, SessionEventKind::NodeRegistered);
        assert_eq!(events[0].message, "client hello accepted");
        assert_eq!(events[1].kind, SessionEventKind::SessionOpened);
        assert_eq!(events[1].message, "mqtt connected");

        let all_events = database.list_session_events(None, None, 10).unwrap();
        assert_eq!(all_events.len(), 2);
        assert_eq!(all_events[0].node_id, "node-events");
        assert_eq!(all_events[0].kind, SessionEventKind::NodeRegistered);

        let opened_events = database
            .list_session_events(None, Some(SessionEventKind::SessionOpened), 10)
            .unwrap();
        assert_eq!(opened_events.len(), 1);
        assert_eq!(opened_events[0].kind, SessionEventKind::SessionOpened);

        let scoped_events = database
            .list_session_events(
                Some("node-events"),
                Some(SessionEventKind::NodeRegistered),
                10,
            )
            .unwrap();
        assert_eq!(scoped_events.len(), 1);
        assert_eq!(scoped_events[0].message, "client hello accepted");

        cleanup_db_files(&db_path);
    }

    fn sample_registration(node_id: &str) -> NodeRegistration {
        NodeRegistration {
            node_id: node_id.to_string(),
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

    fn sample_audit(source: &str, actor: &str) -> TaskAudit {
        TaskAudit::new(source, Some(actor.to_string()))
    }

    fn unique_db_path(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("command-plane-{prefix}-{unique}.db"))
    }

    fn cleanup_db_files(db_path: &PathBuf) {
        let _ = fs::remove_file(db_path);
        let _ = fs::remove_file(format!("{}-wal", db_path.display()));
        let _ = fs::remove_file(format!("{}-shm", db_path.display()));
    }
}
