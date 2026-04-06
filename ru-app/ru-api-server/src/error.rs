use crate::db::DbError;
use axum::http::StatusCode;
use axum::Json;
use std::io;

pub(crate) type ApiResult<T> = Result<Json<T>, (StatusCode, String)>;

pub(crate) fn internal_error(error: DbError) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

pub(crate) fn to_io_error<E>(error: E) -> io::Error
where
    E: std::fmt::Display,
{
    io::Error::new(io::ErrorKind::Other, error.to_string())
}
